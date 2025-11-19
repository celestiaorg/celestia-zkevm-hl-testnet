#![allow(dead_code)]
use celestia_types::ExtendedHeader;
use std::collections::BTreeMap;
use std::env;
use std::fmt::Display;
use std::result::Result::{Err, Ok};
use std::sync::Arc;

use alloy_primitives::FixedBytes;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use celestia_rpc::blob::BlobsAtHeight;
use celestia_rpc::{client::Client, BlobClient, HeaderClient, ShareClient};
use celestia_types::nmt::NamespaceProof;
use celestia_types::Blob;
use ev_types::v1::SignedData;
use ev_zkevm_types::programs::block::{BlockExecInput, BlockExecOutput};
use jsonrpsee_core::client::Subscription;
use prost::Message;
use rsp_client_executor::io::EthClientExecutorInput;
use sp1_sdk::{include_elf, SP1ProofMode, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey};
use tokio::{
    sync::{mpsc, mpsc::Sender, RwLock, Semaphore},
    task::JoinSet,
};
use tracing::{debug, error, info};

use crate::prover::chain::ChainContext;
use crate::prover::prover_from_env;
use crate::prover::SP1Prover;
use crate::prover::{BlockProofCommitted, ProgramProver, ProverConfig};
use storage::proofs::ProofStorage;

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const EV_EXEC_ELF: &[u8] = include_elf!("ev-exec-program");

#[derive(Clone)]
pub struct BlockExecConfig {
    pub pk: Arc<SP1ProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
}

impl BlockExecConfig {
    pub fn new(pk: SP1ProvingKey, vk: SP1VerifyingKey, mode: SP1ProofMode) -> Self {
        BlockExecConfig {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
        }
    }
}

impl ProverConfig for BlockExecConfig {
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

/// TrustedState tracks the trusted height and state root which is provided to the proof system as inputs.
/// Updates are made optimisticly using the EthClientExecutorInputs queried from the configured EVM full node.
pub struct TrustedState {
    height: u64,
    root: FixedBytes<32>,
}

impl TrustedState {
    pub fn new(height: u64, root: FixedBytes<32>) -> Self {
        Self { height, root }
    }
}

impl Display for TrustedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "height={}, root={}", self.height, self.root)
    }
}

/// A prover for generating SP1 proofs for EVM block execution and data availability in Celestia.
///
/// This struct is responsible for preparing the standard input (`SP1Stdin`)
/// for a zkVM program that takes a blob inclusion proof, data root proof, Celestia Header and
/// EVM state transition function.
pub struct BlockExecProver {
    pub ctx: Arc<ChainContext>,
    pub config: BlockExecConfig,
    pub prover: Arc<SP1Prover>,
    pub trusted_state: RwLock<TrustedState>,
    pub tx: Sender<BlockProofCommitted>,
    pub storage: Arc<dyn ProofStorage>,
    pub queue_capacity: usize,
    pub concurrency: usize,
}

#[async_trait]
impl ProgramProver for BlockExecProver {
    type Config = BlockExecConfig;
    type Input = BlockExecInput;
    type Output = BlockExecOutput;

    /// Returns the program configuration containing the ELF and proof mode.
    fn cfg(&self) -> &Self::Config {
        &self.config
    }

    /// Constructs the `SP1Stdin` input required for proving.
    ///
    /// This function serializes and writes structured input data into the
    /// stdin buffer in the expected format for the SP1 program.
    ///
    /// # Errors
    /// Returns an error if serialization of any input component fails.
    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin> {
        let mut stdin = SP1Stdin::new();
        stdin.write(&input);
        Ok(stdin)
    }

    /// Parses the `SP1PublicValues` from the proof and converts it into the
    /// program's custom output type.
    ///
    /// # Errors
    /// - Returns an error if deserialization fails.
    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output> {
        Ok(bincode::deserialize::<BlockExecOutput>(proof.public_values.as_slice())?)
    }

    /// Returns the SP1 Prover.
    fn prover(&self) -> Arc<SP1Prover> {
        Arc::clone(&self.prover)
    }
}

struct BlockEvent {
    height: u64,
    blobs: Vec<Blob>,
}

impl BlockEvent {
    fn new(height: u64, blobs: Vec<Blob>) -> Self {
        Self { height, blobs }
    }
}

struct ProofJob {
    height: u64,
    extended_header: ExtendedHeader,
    proofs: Vec<NamespaceProof>,
    blobs: Vec<Blob>,
    executor_inputs: Vec<EthClientExecutorInput>,
}

struct ScheduledProofJob {
    job: Arc<ProofJob>,
    trusted_height: u64,
    trusted_root: FixedBytes<32>,
}

impl BlockExecProver {
    /// Creates a new instance of [`BlockExecProver`] for the provided [`ChainContext`] using default configuration
    /// and prover environment settings.
    pub fn new(
        ctx: Arc<ChainContext>,
        trusted_state: TrustedState,
        tx: Sender<BlockProofCommitted>,
        storage: Arc<dyn ProofStorage>,
        queue_capacity: usize,
        concurrency: usize,
    ) -> Self {
        let prover = prover_from_env();
        let config = BlockExecProver::default_config(prover.as_ref());
        let trusted_state = RwLock::new(trusted_state);

        Self {
            ctx,
            config,
            prover,
            trusted_state,
            tx,
            storage,
            queue_capacity,
            concurrency,
        }
    }

    /// Returns the default prover configuration for the block execution program.
    pub fn default_config(prover: &SP1Prover) -> BlockExecConfig {
        let (pk, vk) = prover.setup(EV_EXEC_ELF);
        BlockExecConfig::new(pk, vk, SP1ProofMode::Compressed)
    }

    async fn connect_and_subscribe(&self) -> Result<(Arc<Client>, Subscription<BlobsAtHeight>)> {
        let client = self.ctx.celestia_ws_client().await?;
        let subscription = client
            .blob_subscribe(self.ctx.namespace())
            .await
            .context("Blob subscription failed")?;

        Ok((self.ctx.celestia_client(), subscription))
    }

    /// Runs the block prover loop with a 3-stage pipeline:
    ///
    /// 1. **Prepare**: For each [`BlockEvent`] received from the Celestia subscription,
    ///    fetch and build the proof inputs in parallel (bounded by `CONCURRENCY`).
    /// 2. **Schedule**: In height order, attach the current trusted snapshot and
    ///    optimistically advance the shared [`TrustedState`] for subsequent jobs.
    /// 3. **Prove**: Spawn proof workers (also concurrency-limited) that generate
    ///    proofs using the assigned inputs.
    ///
    /// The Celestia node produces a new [`BlobsAtHeight`] event for each block. An
    /// event may or may not contain any blobs in the configured namespace at a given
    /// height. Events are fed into the pipeline via the WebSocket subscription, and
    /// proofs are generated concurrently while ensuring the trusted state is updated
    /// monotonically in block-height order.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let (client, mut subscription) = self.connect_and_subscribe().await?;

        // Queues for the 3-stage pipeline
        let (event_tx, mut event_rx) = mpsc::channel::<BlockEvent>(self.queue_capacity);
        let (job_tx, mut job_rx) = mpsc::channel::<ProofJob>(self.queue_capacity);
        let (sched_tx, mut sched_rx) = mpsc::channel::<ScheduledProofJob>(self.queue_capacity);

        // ========== Stage 1: Prepare proof inputs (parallel, IO-bound) ==========
        let sem = Arc::new(Semaphore::new(self.concurrency));
        tokio::spawn({
            let client = client.clone();
            let prover = self.clone();

            let job_tx = job_tx.clone();
            let sem = sem.clone();
            async move {
                let mut tasks = JoinSet::new();

                while let Some(event) = event_rx.recv().await {
                    debug!("New block event height={}, blobs={}", event.height, event.blobs.len());
                    let client = client.clone();
                    let prover = prover.clone();
                    let job_tx = job_tx.clone();
                    let permit = sem.clone().acquire_owned().await.unwrap();

                    tasks.spawn(async move {
                        let _permit = permit; // limit concurrent prepares
                        match prover.prepare_inputs(client, event).await {
                            Ok(job) => {
                                let _ = job_tx.send(job).await;
                            }
                            Err(e) => error!("Failed to retrieve proof inputs: {e:#}"),
                        }
                    });
                }

                while tasks.join_next().await.is_some() {}
                error!("Prepare stage shutting down");
            }
        });

        // ========== Stage 2: Assign trusted height and root (single writer, in height order) ==========
        tokio::spawn({
            let prover = self.clone();
            let sched_tx = sched_tx.clone();
            async move {
                let mut buf: BTreeMap<u64, ProofJob> = BTreeMap::new();
                let mut next_height: Option<u64> = None;

                while let Some(job) = job_rx.recv().await {
                    buf.insert(job.height, job);

                    // Process jobs in height order
                    loop {
                        let height = match next_height {
                            Some(h) => h,
                            None => {
                                if let Some((&min_height, _)) = buf.iter().next() {
                                    next_height = Some(min_height);
                                    min_height
                                } else {
                                    break;
                                }
                            }
                        };

                        let Some(job) = buf.remove(&height) else { break };

                        // Snapshot current trusted state for this proof
                        let (trusted_height, trusted_root) = {
                            let s = prover.trusted_state.read().await;
                            (s.height, s.root)
                        };

                        // Optimistically advance global trusted_state for future jobs
                        if let Some(next) = job.executor_inputs.last() {
                            let mut s = prover.trusted_state.write().await;
                            if next.current_block.number > s.height {
                                s.height = next.current_block.number;
                                s.root = next.current_block.state_root;
                            }
                        }

                        let scheduled = ScheduledProofJob {
                            job: Arc::new(job),
                            trusted_height,
                            trusted_root,
                        };

                        if sched_tx.send(scheduled).await.is_err() {
                            break;
                        }

                        next_height = Some(height + 1);
                    }
                }

                error!("Schedule stage shutting down");
            }
        });

        // ========== Stage 3: Prove (parallel, CPU/IO-bound for remote prover network) ==========
        let prove_sem = Arc::new(Semaphore::new(self.concurrency));
        tokio::spawn({
            let prover = self.clone();
            let prove_sem = prove_sem.clone();

            async move {
                let mut tasks = JoinSet::new();

                while let Some(scheduled) = sched_rx.recv().await {
                    let prover = prover.clone();
                    let permit = prove_sem.clone().acquire_owned().await.unwrap();

                    tasks.spawn(async move {
                        let _permit = permit; // limit concurrent proofs

                        if let Err(e) = prover.prove_and_store(scheduled).await {
                            error!("Prove failed: {e:#}");
                        }
                    });
                }

                while tasks.join_next().await.is_some() {}
                error!("Prove stage shutting down");
            }
        });

        // Main subscription loop: feed events into the pipeline
        while let Some(result) = subscription.next().await {
            match result {
                Ok(event) => {
                    let blobs = event.blobs.unwrap_or_default();
                    event_tx
                        .send(BlockEvent::new(event.height, blobs))
                        .await
                        .map_err(|_| anyhow::anyhow!("worker queue closed"))?;
                }
                Err(e) => {
                    error!("Subscription error: {e}");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Retrieves the proof inputs required via RPC calls to the configured celestia and evm nodes.
    async fn prepare_inputs(self: Arc<Self>, client: Arc<Client>, event: BlockEvent) -> Result<ProofJob> {
        // Fetch Celestia header and namespace data
        let extended_header = client.header_get_by_height(event.height).await?;
        let namespace_data = client
            .share_get_namespace_data(&extended_header, self.ctx.namespace())
            .await?;

        let proofs: Vec<NamespaceProof> = namespace_data.rows.iter().map(|row| row.proof.clone()).collect();

        // Decode blob data to extract block heights
        let signed_data: Vec<SignedData> = event
            .blobs
            .iter()
            .filter_map(|blob| SignedData::decode(Bytes::from(blob.data.clone())).ok())
            .collect();

        // Generate executor inputs for each EVM block
        let mut executor_inputs = Vec::with_capacity(signed_data.len());
        for data in signed_data {
            let block_number = data
                .data
                .as_ref()
                .and_then(|d| d.metadata.as_ref())
                .map(|m| m.height)
                .ok_or_else(|| anyhow!("missing height for SignedData"))?;

            executor_inputs.push(self.ctx.generate_executor_input(block_number).await?);
        }

        debug!("Got {} evm inputs at height {}", executor_inputs.len(), event.height);

        Ok(ProofJob {
            height: event.height,
            extended_header,
            proofs,
            blobs: event.blobs,
            executor_inputs,
        })
    }

    async fn prove_and_store(self: Arc<Self>, scheduled: ScheduledProofJob) -> Result<()> {
        let extended_header = &scheduled.job.extended_header;

        // Construct the proof inputs
        let inputs = BlockExecInput {
            header_raw: serde_cbor::to_vec(&extended_header.header)?,
            dah: extended_header.dah.clone(),
            blobs_raw: serde_cbor::to_vec(&scheduled.job.blobs)?,
            pub_key: self.ctx.pub_key_bytes(),
            namespace: self.ctx.namespace(),
            proofs: scheduled.job.proofs.clone(),
            executor_inputs: scheduled.job.executor_inputs.clone(),
            trusted_height: scheduled.trusted_height,
            trusted_root: scheduled.trusted_root,
        };

        // Generate the proof
        let (proof, outputs) = self.prove(inputs).await?;

        // Store the proof (non-blocking failure to avoid breaking the pipeline)
        if let Err(e) = self
            .storage
            .store_block_proof(scheduled.job.height, &proof, &outputs)
            .await
        {
            error!(
                "Failed to store proof for block {}: {} - error: {e:#}",
                scheduled.job.height, outputs,
            );
            // Note: We continue execution even if storage fails to avoid breaking the proving pipeline
        }

        info!(
            "Successfully created and stored proof for block {}. Outputs: {}",
            scheduled.job.height, outputs,
        );

        // Notify that this block proof is committed
        self.tx.send(BlockProofCommitted(scheduled.job.height)).await?;

        Ok(())
    }
}
