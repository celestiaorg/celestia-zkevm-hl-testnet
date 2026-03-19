use chrono::DateTime;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use alloy_provider::Provider;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ev_zkevm_types::programs::block::{BlockExecInput, BlockRangeExecOutput};
use serde::{Deserialize, Serialize};
use sp1_sdk::{SP1ProofMode, SP1ProofWithPublicValues, SP1Stdin};
use storage::hyperlane::message::HyperlaneMessageStore;
use tee_attestation_types::{AttestationResponse, Inputs as TeeAttestationInput};
use tokio::{sync::mpsc, time::interval};
use tracing::{debug, error, info, warn};

use crate::prover::chain::ChainContext;
use crate::prover::config::{StandardProverConfig, BATCH_SIZE, WARN_DISTANCE};
use crate::prover::programs::common::{self, ProverStatus};
use crate::prover::{
    prover_from_env, MessageProofRequest, MessageProofSync, ProgramProver, RangeProofCommitted, SP1Prover,
};

#[derive(Deserialize, Serialize)]
struct AttestationRequest {
    block_inputs: Vec<String>,
    trusted_light_block_raw: String,
    new_light_block_raw: String,
}

pub struct TeeExecProver {
    ctx: Arc<ChainContext>,
    range_tx: mpsc::Sender<MessageProofRequest>,
    config: StandardProverConfig,
    prover: Arc<SP1Prover>,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
}

#[async_trait]
impl ProgramProver for TeeExecProver {
    type Config = StandardProverConfig;
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
    pub fn default_config(prover: &SP1Prover) -> StandardProverConfig {
        let elf_bytes = include_bytes!("../../../../../elfs/tee-attestation-elf");
        let (pk, vk) = prover.setup(elf_bytes);
        StandardProverConfig::new(pk, vk, SP1ProofMode::Groth16)
    }

    /// Starts the TEE prover loop.
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

            let tee_app_url = std::env::var("TEE_APP_URL").expect("TEE_APP_URL environment variable is not set");

            // Build block inputs for the batch
            let mut trusted_height = status.trusted_height;
            let mut trusted_state_root = {
                let block = self
                    .ctx
                    .evm_provider()
                    .get_block_by_number(trusted_height.into())
                    .await?
                    .ok_or_else(|| anyhow!("Block {trusted_height} not found"))?;
                block.header.state_root
            };
            let namespace = self.ctx.namespace();
            let mut block_inputs: Vec<BlockExecInput> = Vec::new();

            for celestia_height in status.trusted_celestia_height + 1..=status.trusted_celestia_height + batch_size {
                let input = common::build_block_input(
                    &self.ctx,
                    celestia_height,
                    namespace,
                    &mut trusted_height,
                    &mut trusted_state_root,
                )
                .await?;
                block_inputs.push(input);
            }

            // Serialize block inputs to hex strings
            let serialized_inputs: Vec<String> = block_inputs
                .iter()
                .map(|input| {
                    let bytes = bincode::serialize(&input).expect("failed to serialize input");
                    hex::encode(bytes)
                })
                .collect();

            // Fetch light blocks for Tendermint light client verification
            let trusted_light_block = self.ctx.get_light_block(status.trusted_celestia_height).await?;
            let new_light_block = self
                .ctx
                .get_light_block(status.trusted_celestia_height + batch_size)
                .await?;

            // Serialize light blocks using CBOR (bincode doesn't work with tendermint's serde attrs)
            let trusted_light_block_raw = hex::encode(serde_cbor::to_vec(&trusted_light_block)?);
            let new_light_block_raw = hex::encode(serde_cbor::to_vec(&new_light_block)?);

            // Fetch attestation from the TEE app via POST with block inputs and light blocks
            let client = reqwest::Client::new();
            let request_body = AttestationRequest {
                block_inputs: serialized_inputs,
                trusted_light_block_raw,
                new_light_block_raw,
            };
            let response = client
                .post(format!("{tee_app_url}/attestation"))
                .json(&request_body)
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

            // Add buffer to account for potential clock drift with PCCS server
            const CLOCK_DRIFT_BUFFER_SECS: u64 = 300; // 5 minutes
            let now_with_buffer = now + CLOCK_DRIFT_BUFFER_SECS;

            // Debug: Print the timestamp we're passing to the circuit
            let now_dt = DateTime::from_timestamp(now_with_buffer as i64, 0).unwrap();
            info!(
                "Current timestamp being passed to circuit: {} (Unix: {})",
                now_dt, now_with_buffer
            );

            let input = TeeAttestationInput {
                quote,
                event_log: attestation
                    .event_log
                    .ok_or_else(|| anyhow!("Missing event log"))?
                    .as_bytes()
                    .to_vec(),
                report_data: Vec::new(), // not used in circuit, extracted from quote
                output: hex::decode(attestation.output.ok_or_else(|| anyhow!("Missing output"))?)?,
                collateral,
                now: now_with_buffer,
            };

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
