use std::fs::write;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub static EV_BATCH_ELF: &[u8] = include_bytes!("../../../../../elfs/ev-batch-elf");

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ev_zkevm_types::block::{BatchExecInput, BatchExecOutput, BlockExecInput};
use sp1_sdk::{Elf, Prover, ProvingKey, SP1ProofMode, SP1ProofWithPublicValues, SP1Stdin};
use storage::hyperlane::message::HyperlaneMessageStore;
use tokio::{sync::mpsc, time::interval};
use tracing::{debug, error, info, warn};

use crate::prover::chain::ChainContext;
use crate::prover::config::{StandardProverConfig, BATCH_SIZE, WARN_DISTANCE};
use crate::prover::programs::common::{self, ProverStatus};
use crate::prover::{
    prover_from_env, MessageProofRequest, MessageProofSync, ProgramProver, RangeProofCommitted, SP1Prover,
};

pub struct BatchExecProver {
    ctx: Arc<ChainContext>,
    range_tx: mpsc::Sender<MessageProofRequest>,
    config: StandardProverConfig,
    prover: Arc<SP1Prover>,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
}

#[async_trait]
impl ProgramProver for BatchExecProver {
    type Config = StandardProverConfig;
    type Input = BatchExecInput;
    type Output = BatchExecOutput;

    fn cfg(&self) -> &Self::Config {
        &self.config
    }

    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin> {
        let mut stdin = SP1Stdin::new();
        stdin.write(&input);
        Ok(stdin)
    }

    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output> {
        Ok(bincode::deserialize::<BatchExecOutput>(proof.public_values.as_slice())?)
    }

    fn prover(&self) -> Arc<SP1Prover> {
        Arc::clone(&self.prover)
    }
}

impl BatchExecProver {
    /// Creates a new prover instance.
    pub async fn new(
        ctx: Arc<ChainContext>,
        range_tx: mpsc::Sender<MessageProofRequest>,
        hyperlane_message_store: Arc<HyperlaneMessageStore>,
    ) -> Result<Self> {
        let prover = prover_from_env().await;
        let config = BatchExecProver::default_config(prover.as_ref()).await?;

        Ok(Self {
            ctx,
            config,
            prover,
            range_tx,
            hyperlane_message_store,
        })
    }

    /// Returns the prover config.
    pub async fn default_config(prover: &SP1Prover) -> Result<StandardProverConfig> {
        let elf = Elf::Static(EV_BATCH_ELF);
        let pk = prover.setup(elf.clone()).await?;
        let vk = pk.verifying_key().clone();
        Ok(StandardProverConfig::new(pk, vk, elf, SP1ProofMode::Groth16))
    }

    /// Builds the proof input structure for the given batch size starting from the provided height.
    async fn build_proof_inputs(
        &self,
        start_height: u64,
        status: &common::ProverStatus,
        batch_size: u64,
    ) -> Result<BatchExecInput> {
        let mut current_height = status.trusted_height;
        let mut current_root = status.trusted_root;
        let namespace = self.ctx.namespace();
        let mut block_inputs: Vec<BlockExecInput> = Vec::new();

        for block_number in start_height..=start_height + batch_size {
            let input = common::build_block_input(
                &self.ctx,
                block_number,
                namespace,
                &mut current_height,
                &mut current_root,
            )
            .await?;

            block_inputs.push(input);
        }

        // Fetch light blocks for Tendermint light client verification
        // The trusted light block is at the height before the first block in the batch
        let trusted_light_block = self.ctx.get_light_block(status.trusted_celestia_height).await?;

        // The new light block is at the end of the batch
        let new_light_block = self.ctx.get_light_block(start_height + batch_size).await?;

        // Serialize light blocks using CBOR (bincode doesn't work with tendermint's serde attrs)
        let trusted_light_block_raw = serde_cbor::to_vec(&trusted_light_block)?;
        let new_light_block_raw = serde_cbor::to_vec(&new_light_block)?;

        Ok(BatchExecInput {
            blocks: block_inputs,
            trusted_light_block_raw,
            new_light_block_raw,
        })
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
            let status = ProverStatus::load(&self.ctx).await?;
            if scan_head.is_none() {
                scan_head = Some(status.trusted_celestia_height + 1);
            }

            let scan_start = scan_head.ok_or_else(|| anyhow!("Scan head is not set"))?;
            if scan_start < status.celestia_head {
                // only check if batch size can be reduced if a new mailbox event was emitted
                batch_size = common::calculate_batch_size(
                    &self.ctx,
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

            let start_height = status.trusted_celestia_height + 1;
            let input = self.build_proof_inputs(start_height, &status, batch_size).await?;

            // Generate the proof
            let start_time = Instant::now();
            let (proof, output) = self.prove(input).await?;
            info!("Proof generation time: {}", start_time.elapsed().as_millis());

            // Index if new ev blocks were included.
            common::index_messages(
                &self.ctx,
                self.hyperlane_message_store.clone(),
                status.trusted_height + 1,
                output.new_state.height,
            )
            .await?;

            if let Err(e) = common::submit_proof_msg(&self.ctx, &proof).await {
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
