use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::prover::chain::ChainContext;
use crate::prover::config::{MAX_BATCH_SIZE, MAX_INDEXING_RANGE};
use crate::prover::{
    config::{BATCH_SIZE, MIN_BATCH_SIZE, WARN_DISTANCE},
    MessageProofRequest, MessageProofSync, ProverConfig, RangeProofCommitted,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use celestia_grpc_client::{MsgUpdateInterchainSecurityModule, QueryIsmRequest};
use celestia_rpc::HeaderClient;
use ev_zkevm_types::programs::block::{BlockRangeExecOutput, State};
use sp1_sdk::{SP1ProofMode, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey};
use storage::hyperlane::message::HyperlaneMessageStore;
use tee_attestation_types::{AttestationResponse, Inputs as TeeAttestationInput};

use tokio::{sync::mpsc, time::interval};
use tracing::{debug, error, info, warn};

use crate::prover::ProgramProver;
use crate::prover::{prover_from_env, SP1Prover};

struct ProverStatus {
    trusted_height: u64,
    trusted_celestia_height: u64,
    celestia_head: u64,
}

impl ProverStatus {
    fn is_batch_ready(&self, batch_size: u64) -> bool {
        self.trusted_celestia_height + batch_size <= self.celestia_head
    }

    fn blocks_remaining(&self, batch_size: u64) -> u64 {
        (self.trusted_celestia_height + batch_size).saturating_sub(self.celestia_head)
    }

    fn distance(&self) -> u64 {
        self.celestia_head.saturating_sub(self.trusted_celestia_height)
    }
}

#[derive(Clone)]
pub struct TeeProverConfig {
    pub pk: Arc<SP1ProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
}

impl TeeProverConfig {
    pub fn new(pk: SP1ProvingKey, vk: SP1VerifyingKey, mode: SP1ProofMode) -> Self {
        TeeProverConfig {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
        }
    }
}

impl ProverConfig for TeeProverConfig {
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

pub struct TeeExecProver {
    ctx: Arc<ChainContext>,
    range_tx: mpsc::Sender<MessageProofRequest>,
    config: TeeProverConfig,
    prover: Arc<SP1Prover>,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
}

#[async_trait]
impl ProgramProver for TeeExecProver {
    type Config = TeeProverConfig;
    type Input = TeeAttestationInput;
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

impl TeeExecProver {
    /// Creates a new prover instance.
    pub fn new(
        ctx: Arc<ChainContext>,
        range_tx: mpsc::Sender<MessageProofRequest>,
        hyperlane_message_store: Arc<HyperlaneMessageStore>,
    ) -> Result<Self> {
        let prover = prover_from_env();
        let config = TeeExecProver::default_config(prover.as_ref());

        Ok(Self {
            ctx,
            config,
            prover,
            range_tx,
            hyperlane_message_store,
        })
    }

    /// Returns the prover config.
    pub fn default_config(prover: &SP1Prover) -> TeeProverConfig {
        let elf_bytes = std::fs::read("elfs/tee-attestation-elf").expect("Failed to read tee-attestation-elf");
        let (pk, vk) = prover.setup(&elf_bytes);
        TeeProverConfig::new(pk, vk, SP1ProofMode::Groth16)
    }

    /// Loads the ProverStatus by querying the trusted state from the on-chain ism and the
    /// the latest header from Celestia.
    async fn load_prover_status(&self) -> Result<ProverStatus> {
        let resp = self
            .ctx
            .ism_client()
            .ism(QueryIsmRequest {
                id: self.ctx.ism_id().to_string(),
            })
            .await?;
        let ism = resp.ism.ok_or_else(|| anyhow!("ZKISM not found"))?;
        let state: State = bincode::deserialize(&ism.state).unwrap();
        let celestia_head = self.ctx.celestia_client().header_local_head().await?.height().value();

        Ok(ProverStatus {
            trusted_height: state.height,
            trusted_celestia_height: state.celestia_height,
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
        mailbox_nonce: &mut u32,
    ) -> Result<u64> {
        if scan_start >= latest_head {
            return Ok(current_batch);
        }

        for height in scan_start..=latest_head {
            let Some(block_number) = self.ctx.latest_block_for_height(height).await? else {
                continue;
            };

            let nonce = self.ctx.mailbox_nonce_at(block_number).await?;

            if nonce > *mailbox_nonce {
                // Ensure batch size meets minimum requirement
                let blocks_elapsed = height.saturating_sub(trusted_celestia_height);
                let batch_size = blocks_elapsed.clamp(MIN_BATCH_SIZE, MAX_BATCH_SIZE);
                *mailbox_nonce = nonce;
                debug!("Found non-empty block at height {height}, adjusting batch size to {batch_size}");
                return Ok(batch_size);
            }
        }

        Ok(BATCH_SIZE)
    }

    /// Queries and stores Hyperlane mailbox events from the provided block range (inclusive),
    /// chunking requests to respect `MAX_INDEXING_RANGE`.
    /// The `MAX_INDEXING_RANGE` const is set to align with the default value of 100,000 blocks.
    /// This setting can be configured via the EVM execution client using `max_blocks_per_filter: u64` and `max_logs_per_response: usize`.
    async fn index_messages(&self, start_block: u64, end_block: u64) -> Result<()> {
        if start_block > end_block {
            return Ok(());
        }

        let indexer = self.ctx.hyperlane_indexer();
        let mut from_block = start_block;
        while from_block <= end_block {
            let to_block = std::cmp::min(from_block + MAX_INDEXING_RANGE - 1, end_block);
            debug!("Indexing mailbox events from block {from_block} to {to_block}");

            let filter = indexer.filter_with_range(from_block, to_block);
            indexer
                .process(filter, self.ctx.evm_provider(), self.hyperlane_message_store.clone())
                .await?;
            from_block = to_block + 1;
        }

        Ok(())
    }

    /// Submits a state transition proof msg to the zk verifier on-chain.
    async fn submit_proof_msg(&self, proof: &SP1ProofWithPublicValues) -> Result<()> {
        let id = self.ctx.ism_id().to_string();
        let public_values = proof.public_values.as_slice().to_vec();
        let signer = self.ctx.ism_client().signer_address().to_string();

        let msg = MsgUpdateInterchainSecurityModule::new(id, proof.bytes(), public_values, signer);

        info!("Updating ZKISM on Celestia...");
        let response = self.ctx.ism_client().send_tx(msg).await?;
        if !response.success {
            error!("Failed to submit state transition proof to ZKISM: {:?}", response);
            return Err(anyhow::anyhow!("Failed to submit state transition proof to ZKISM"));
        }

        info!("Proof tx submitted to ism with hash: {}", response.tx_hash);

        Ok(())
    }

    /// Starts the batched prover loop.
    pub async fn run(self: Arc<Self>, message_sync: Arc<MessageProofSync>) -> Result<()> {
        let mut batch_size = BATCH_SIZE;
        let mut mailbox_nonce = self.ctx.mailbox_nonce().await?;
        let mut scan_head: Option<u64> = None;
        let mut poll = interval(Duration::from_secs(6)); // BlockTime=6s
        loop {
            message_sync.wait_for_idle().await;
            poll.tick().await;
            let status = self.load_prover_status().await?;
            if scan_head.is_none() {
                scan_head = Some(status.trusted_celestia_height + 1);
            }

            let scan_start = scan_head.ok_or_else(|| anyhow!("Scan head is not set"))?;
            if scan_start < status.celestia_head {
                // only check if batch size can be reduced if a new mailbox event was emitted
                batch_size = self
                    .calculate_batch_size(
                        scan_start,
                        status.celestia_head,
                        status.trusted_celestia_height,
                        batch_size,
                        &mut mailbox_nonce,
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

            let tee_app_url = std::env::var("TEE_APP_URL").expect("TEE_APP_URL environment variable is not set");

            // Fetch attestation from the TEE app
            let client = reqwest::Client::new();
            let response = client
                .get(format!("{}/attestation", tee_app_url))
                .send()
                .await
                .expect("Failed to connect to TEE app");

            let attestation: AttestationResponse = response.json().await.expect("Failed to parse attestation response");

            if !attestation.success {
                panic!(
                    "Attestation failed at step {:?}: {:?}",
                    attestation.step, attestation.error
                );
            }

            let quote = hex::decode(attestation.quote.ok_or_else(|| anyhow!("Missing quote"))?)?;

            let collateral =
                dcap_qvl::collateral::get_collateral("https://pccs.phala.network/sgx/certification/v4/", &quote)
                    .await?;

            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("Failed to get current time")
                .as_secs();

            let input = TeeAttestationInput {
                quote,
                event_log: hex::decode(attestation.event_log.ok_or_else(|| anyhow!("Missing event log"))?)?,
                report_data: Vec::new(), // not used in circuit, extracted from quote
                output: hex::decode(attestation.output.ok_or_else(|| anyhow!("Missing output"))?)?,
                collateral,
                now,
            };

            // Generate the proof
            let start_time = Instant::now();
            let (proof, output) = self.prove(input).await?;
            // generate proof here
            info!("Proof generation time: {}", start_time.elapsed().as_millis());

            // Index if new ev blocks were included.
            self.index_messages(status.trusted_height + 1, output.new_state.height)
                .await?;

            if let Err(e) = self.submit_proof_msg(&proof).await {
                error!(?e, "Failed to submit tx to ism");
            }

            // reset batch size and fast forward checkpoints
            batch_size = BATCH_SIZE;
            scan_head = Some(status.celestia_head + 1);

            let permit = message_sync.begin().await;
            let commit = RangeProofCommitted::new(output.new_state.height, output.new_state.state_root);
            let request = MessageProofRequest::with_permit(commit, permit);
            self.range_tx.send(request).await?;
        }
    }
}
