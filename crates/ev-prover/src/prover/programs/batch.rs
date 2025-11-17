use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    generate_client_executor_input,
    prover::{
        config::{BATCH_SIZE, MIN_BATCH_SIZE, WARN_DISTANCE},
        MessageProofRequest, MessageProofSync, ProverConfig, RangeProofCommitted,
    },
};
use alloy::sol;
use alloy_primitives::{Address, FixedBytes};
use alloy_provider::{Provider, ProviderBuilder};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use celestia_grpc_client::{CelestiaIsmClient, MsgUpdateZkExecutionIsm, QueryIsmRequest};
use celestia_rpc::{BlobClient, Client, HeaderClient, ShareClient};
use celestia_types::{
    nmt::{Namespace, NamespaceProof},
    Blob,
};
use ev_state_queries::DefaultProvider;
use ev_types::v1::SignedData;
use ev_zkevm_types::programs::block::{BatchExecInput, BlockExecInput, BlockRangeExecOutput};
use prost::Message;
use reth_chainspec::ChainSpec;
use rsp_client_executor::io::EthClientExecutorInput;
use rsp_primitives::genesis::Genesis;
use sp1_sdk::{include_elf, SP1ProofMode, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey};
use tokio::{sync::mpsc, time::interval};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::prover::ProgramProver;
use crate::prover::{prover_from_env, SP1Prover};

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const BATCH_ELF: &[u8] = include_elf!("ev-batch-program");

sol! {
    #[sol(rpc)]
    contract MailboxContract {
        function nonce() public view returns (uint32);
    }
}

/// ProverStatus of the latest Celestia state relevant to the prover loop.
///
/// The methods on this type encapsulate small pieces of batching logic so
/// the main control flow stays readable.
struct ProverStatus {
    trusted_height: u64,
    trusted_root: FixedBytes<32>,
    trusted_celestia_height: u64,
    celestia_head: u64,
}

impl ProverStatus {
    /// Returns true if enough new blocks have been produced to start proving a batch.
    fn is_batch_ready(&self, batch_size: u64) -> bool {
        self.trusted_celestia_height + batch_size <= self.celestia_head
    }

    /// Returns how many more blocks are needed to reach a full batch.
    fn blocks_remaining(&self, batch_size: u64) -> u64 {
        (self.trusted_celestia_height + batch_size).saturating_sub(self.celestia_head)
    }

    /// Returns how far ahead the Celestia head is from the trusted height.
    fn distance(&self) -> u64 {
        self.celestia_head.saturating_sub(self.trusted_celestia_height)
    }
}

/// AppContext contains RPC clients and configuration required by the prover.
///
/// This encapsulates the dependencies required to query on-chain state and build proof inputs
/// including chain spec, genesis, namespace, sequencer key and rpc clients.
pub struct AppContext {
    pub celestia_client: Arc<Client>,
    pub evm_rpc: String,
    pub ism_client: Arc<CelestiaIsmClient>,
    pub mailbox_address: Address,
    pub chain_spec: Arc<ChainSpec>,
    pub genesis: Genesis,
    pub namespace: Namespace,
    pub pub_key: Vec<u8>,
}

impl AppContext {
    pub async fn from_config(
        config: &Config,
        ism_client: Arc<CelestiaIsmClient>,
        mailbox_address: Address,
    ) -> Result<Self> {
        let celestia_client = Client::new(&config.rpc.celestia_rpc, None).await?;
        let genesis = Config::load_genesis()?;
        let chain_spec = Self::chain_spec_from_genesis(&genesis)?;
        let pub_key = hex::decode(config.pub_key.clone())?;

        Ok(Self {
            celestia_client: Arc::new(celestia_client),
            evm_rpc: config.rpc.evreth_rpc.clone(),
            ism_client,
            mailbox_address,
            chain_spec,
            genesis,
            namespace: config.namespace,
            pub_key,
        })
    }

    pub fn chain_spec_from_genesis(genesis: &Genesis) -> Result<Arc<ChainSpec>> {
        let chain_spec: ChainSpec = genesis
            .try_into()
            .map_err(|e| anyhow!("Failed to convert genesis to chain spec: {e}"))?;

        Ok(Arc::new(chain_spec))
    }
}

#[derive(Clone)]
pub struct BatchProverConfig {
    pub pk: Arc<SP1ProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
}

impl BatchProverConfig {
    pub fn new(pk: SP1ProvingKey, vk: SP1VerifyingKey, mode: SP1ProofMode) -> Self {
        BatchProverConfig {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
        }
    }
}

impl ProverConfig for BatchProverConfig {
    fn pk(&self) -> Arc<SP1ProvingKey> {
        Arc::clone(&self.pk)
    }

    fn vk(&self) -> Arc<SP1VerifyingKey> {
        Arc::clone(&self.vk)
    }

    fn proof_mode(&self) -> SP1ProofMode {
        self.proof_mode
    }
}

pub struct BatchExecProver {
    ctx: AppContext,
    range_tx: mpsc::Sender<MessageProofRequest>,
    config: BatchProverConfig,
    prover: Arc<SP1Prover>,
}

#[async_trait]
impl ProgramProver for BatchExecProver {
    type Config = BatchProverConfig;
    type Input = BatchExecInput;
    type Output = BlockRangeExecOutput;

    fn cfg(&self) -> &Self::Config {
        &self.config
    }

    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin> {
        let mut stdin = SP1Stdin::new();
        stdin.write(&input);
        Ok(stdin)
    }

    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output> {
        Ok(bincode::deserialize::<BlockRangeExecOutput>(
            proof.public_values.as_slice(),
        )?)
    }

    fn prover(&self) -> Arc<SP1Prover> {
        Arc::clone(&self.prover)
    }
}

impl BatchExecProver {
    /// Creates a new prover instance.
    pub fn new(ctx: AppContext, range_tx: mpsc::Sender<MessageProofRequest>) -> Result<Self> {
        let prover = prover_from_env();
        let config = BatchExecProver::default_config(prover.as_ref());

        Ok(Self {
            ctx,
            config,
            prover,
            range_tx,
        })
    }

    /// Returns the prover config.
    pub fn default_config(prover: &SP1Prover) -> BatchProverConfig {
        let (pk, vk) = prover.setup(BATCH_ELF);
        BatchProverConfig::new(pk, vk, SP1ProofMode::Groth16)
    }

    /// Starts the batched prover loop.
    pub async fn run(self: Arc<Self>, message_sync: Arc<MessageProofSync>) -> Result<()> {
        let mut batch_size = BATCH_SIZE;
        let mut scan_head: Option<u64> = None;
        let mut poll = interval(Duration::from_secs(6)); // BlockTime=6s
        let evm_provider = ProviderBuilder::new().connect_http(self.ctx.evm_rpc.parse()?);
        let mailbox_contract = MailboxContract::new(self.ctx.mailbox_address, &evm_provider);
        let mut mailbox_nonce = mailbox_contract.nonce().call().await?;
        loop {
            message_sync.wait_for_idle().await;
            poll.tick().await;

            let status = self.load_prover_status().await?;
            let end_height = status.celestia_head;
            let current_mailbox_nonce = mailbox_contract
                .nonce()
                .call()
                .block(alloy_rpc_types::BlockId::Number(
                    alloy_rpc_types::BlockNumberOrTag::Number(self.get_last_blob_height(end_height).await?),
                ))
                .await?;

            debug!("Current mailbox nonce: {current_mailbox_nonce}, mailbox nonce: {mailbox_nonce}");

            if scan_head.is_none() {
                scan_head = Some(status.trusted_celestia_height + 1);
            }

            let scan_start = scan_head.ok_or_else(|| anyhow!("Scan head is not set"))?;
            if scan_start < status.celestia_head && current_mailbox_nonce > mailbox_nonce {
                // only check if batch size can be reduced if a new mailbox event was emitted
                batch_size = self
                    .calculate_batch_size(
                        scan_start,
                        status.celestia_head,
                        status.trusted_celestia_height,
                        batch_size,
                    )
                    .await?;
            }

            if !status.is_batch_ready(batch_size) {
                let blocks_needed = status.blocks_remaining(batch_size);
                let current_height = status.celestia_head;
                debug!("Waiting for {blocks_needed} more blocks to reach required batch size. Current height: {current_height}");
                continue;
            }

            let distance = status.distance();
            if distance >= WARN_DISTANCE {
                warn!("Prover is {distance} blocks behind Celestia head");
            } else {
                info!("Prover is {distance} blocks behind Celestia head");
            }

            let start_height = status.trusted_celestia_height + 1;
            let input = self
                .build_proof_inputs(&evm_provider, start_height, &status, end_height)
                .await?;

            let start_time = Instant::now();
            let (proof, output) = self.prove(input).await?;
            info!("Proof generation time: {}", start_time.elapsed().as_millis());

            if let Err(e) = self.submit_proof_msg(&proof).await {
                error!(?e, "failed to submit tx to ism");
            }

            // reset batch size and fast forward checkpoints
            batch_size = BATCH_SIZE;
            scan_head = Some(status.celestia_head + 1);
            mailbox_nonce = current_mailbox_nonce;

            let permit = message_sync.begin().await;
            let commit = RangeProofCommitted::new(output.new_height, output.new_state_root);
            let request = MessageProofRequest::with_permit(commit, permit);
            self.range_tx.send(request).await?;
        }
    }

    /// Loads the ProverStatus by querying the trusted state from the on-chain ism and the
    /// the latest header from Celestia.
    async fn load_prover_status(&self) -> Result<ProverStatus> {
        let resp = self
            .ctx
            .ism_client
            .ism(QueryIsmRequest {
                id: self.ctx.ism_client.ism_id().to_string(),
            })
            .await?;
        let ism = resp.ism.ok_or_else(|| anyhow!("ZKISM not found"))?;
        let trusted_root = FixedBytes::from_slice(&ism.state_root);
        let celestia_head = self.ctx.celestia_client.header_local_head().await?.height().value();

        Ok(ProverStatus {
            trusted_height: ism.height,
            trusted_root,
            trusted_celestia_height: ism.celestia_height,
            celestia_head,
        })
    }

    /// Calculates the block prover batch size given the starting height, latest height and trusted height.
    /// If a non-empty block is found then the batch is reduced.
    async fn calculate_batch_size(
        &self,
        scan_start: u64,
        latest_head: u64,
        trusted_celestia_height: u64,
        current_batch: u64,
    ) -> Result<u64> {
        if scan_start >= latest_head {
            return Ok(current_batch);
        }

        let namespace = self.ctx.namespace;
        for height in scan_start..=latest_head {
            if !self.is_empty_block(height, namespace).await? {
                warn!("Found non-empty block at height {height}");
                // Ensure batch size stays within allowed range
                let blocks_elapsed = height.saturating_sub(trusted_celestia_height);
                let batch_size = blocks_elapsed.clamp(MIN_BATCH_SIZE, BATCH_SIZE);
                debug!("Found non-empty block at height {height}, adjusting batch size to {batch_size}");
                return Ok(batch_size);
            }
        }

        Ok(BATCH_SIZE)
    }

    /// Retruns true if the block contains zero blobs for the given Namespace.
    async fn is_empty_block(&self, height: u64, namespace: Namespace) -> Result<bool> {
        let blobs: Vec<Blob> = self
            .ctx
            .celestia_client
            .blob_get_all(height, &[namespace])
            .await?
            .unwrap_or_default();

        Ok(blobs.is_empty())
    }

    /// Submits a state transition proof msg to the zk verifier on-chain.
    async fn submit_proof_msg(&self, proof: &SP1ProofWithPublicValues) -> Result<()> {
        let id = self.ctx.ism_client.ism_id().to_string();
        let public_values = proof.public_values.as_slice().to_vec();
        let signer = self.ctx.ism_client.signer_address().to_string();

        let msg = MsgUpdateZkExecutionIsm::new(id, proof.bytes(), public_values, signer);

        info!("Updating ZKISM on Celestia...");
        let response = self.ctx.ism_client.send_tx(msg).await?;
        if !response.success {
            error!("Failed to submit state transition proof to ZKISM: {:?}", response);
            return Err(anyhow::anyhow!("Failed to submit state transition proof to ZKISM"));
        }

        info!("[Done] Proof tx submitted to ism with hash: {}", response.tx_hash);

        Ok(())
    }

    async fn get_last_blob_height(&self, height: u64) -> Result<u64> {
        let blobs: Vec<Blob> = self
            .ctx
            .celestia_client
            .blob_get_all(height, &[self.ctx.namespace])
            .await?
            .unwrap_or_default();

        let last_blob = blobs.last().ok_or_else(|| anyhow!("No blobs found"))?;
        let last_blob_signed_data = match SignedData::decode(last_blob.data.as_slice()) {
            Ok(data) => data,
            Err(_) => return Err(anyhow!("Failed to decode last blob signed data")),
        };
        let last_blob_data = last_blob_signed_data.data.ok_or_else(|| anyhow!("Data not found"))?;
        let last_blob_height = last_blob_data
            .metadata
            .ok_or_else(|| anyhow!("Metadata not found"))?
            .height;
        Ok(last_blob_height)
    }

    /// Builds the proof input structure for the given batch size starting from the provided height.
    async fn build_proof_inputs(
        &self,
        evm_provider: &DefaultProvider,
        start_height: u64,
        status: &ProverStatus,
        end_height: u64,
    ) -> Result<BatchExecInput> {
        let mut current_height = status.trusted_height;
        let mut current_root = status.trusted_root;

        let namespace = self.ctx.namespace;

        let mut block_inputs: Vec<BlockExecInput> = Vec::new();
        for block_number in start_height..=end_height {
            let input = self
                .build_block_input(
                    evm_provider,
                    block_number,
                    namespace,
                    &mut current_height,
                    &mut current_root,
                    self.ctx.chain_spec.clone(),
                    self.ctx.genesis.clone(),
                )
                .await?;

            block_inputs.push(input);
        }
        Ok(BatchExecInput { blocks: block_inputs })
    }

    /// Builds a single block prover input for the given height.
    #[allow(clippy::too_many_arguments)]
    async fn build_block_input(
        &self,
        provider: &DefaultProvider,
        height: u64,
        namespace: Namespace,
        trusted_height: &mut u64,
        trusted_root: &mut FixedBytes<32>,
        chain_spec: Arc<ChainSpec>,
        genesis: Genesis,
    ) -> Result<BlockExecInput> {
        let blobs: Vec<Blob> = self
            .ctx
            .celestia_client
            .blob_get_all(height, &[namespace])
            .await?
            .unwrap_or_default();
        debug!("Got {} blobs for block: {}", blobs.len(), height);

        let extended_header = self.ctx.celestia_client.header_get_by_height(height).await?;
        let namespace_data = self
            .ctx
            .celestia_client
            .share_get_namespace_data(&extended_header, namespace)
            .await?;
        let mut proofs: Vec<NamespaceProof> = Vec::new();
        for row in namespace_data.rows {
            proofs.push(row.proof);
        }
        debug!("Got NamespaceProofs, total: {}", proofs.len());

        let mut executor_inputs: Vec<EthClientExecutorInput> = Vec::new();
        if blobs.is_empty() {
            debug!(
                "No blobs for Celestia height {}, keeping trusted_height={} and trusted_root unchanged",
                height, trusted_height
            );
            return Ok(BlockExecInput {
                header_raw: serde_cbor::to_vec(&extended_header.header)?,
                dah: extended_header.dah,
                blobs_raw: serde_cbor::to_vec(&blobs)?,
                pub_key: self.ctx.pub_key.to_vec(),
                namespace,
                proofs,
                executor_inputs: vec![],
                trusted_height: *trusted_height,
                trusted_root: *trusted_root,
            });
        }

        let mut last_height = 0;
        for blob in blobs.as_slice() {
            let signed_data = match SignedData::decode(blob.data.as_slice()) {
                Ok(data) => data,
                Err(_) => continue,
            };
            let data = signed_data.data.ok_or_else(|| anyhow!("Data not found"))?;
            let height = data.metadata.ok_or_else(|| anyhow!("Metadata not found"))?.height;
            last_height = height;
            debug!("Got SignedData for EVM block {height}");

            let client_executor_input =
                generate_client_executor_input(&self.ctx.evm_rpc, height, chain_spec.clone(), genesis.clone()).await?;
            executor_inputs.push(client_executor_input);
        }

        let input = BlockExecInput {
            header_raw: serde_cbor::to_vec(&extended_header.header)?,
            dah: extended_header.dah,
            blobs_raw: serde_cbor::to_vec(&blobs)?,
            pub_key: self.ctx.pub_key.to_vec(),
            namespace,
            proofs,
            executor_inputs: executor_inputs.clone(),
            trusted_height: *trusted_height,
            trusted_root: *trusted_root,
        };

        let block = provider
            .get_block_by_number(last_height.into())
            .await?
            .ok_or_else(|| anyhow!("Block {last_height} not found"))?;

        *trusted_height = last_height;
        *trusted_root = block.header.state_root;
        debug!(
            "Updated trusted_height to {} and trusted_root to {:?}",
            trusted_height, trusted_root
        );

        Ok(input)
    }
}
