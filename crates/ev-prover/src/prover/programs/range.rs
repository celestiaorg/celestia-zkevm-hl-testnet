#![allow(dead_code)]
use std::{collections::BTreeSet, env, sync::Arc};

use alloy_rpc_types::Filter;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use celestia_grpc_client::{CelestiaIsmClient, StateTransitionProofMsg};
use ev_zkevm_types::events::Dispatch;
use ev_zkevm_types::programs::block::{BlockRangeExecInput, BlockRangeExecOutput};
use sp1_sdk::{
    include_elf, HashableKey, SP1Proof, SP1ProofMode, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin,
    SP1VerifyingKey,
};
use storage::hyperlane::message::HyperlaneMessageStore;
use storage::proofs::ProofStorage;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Semaphore;
use tracing::{debug, error, info};

use crate::prover::chain::ChainContext;
use crate::prover::{
    programs::block::EV_EXEC_ELF, BlockProofCommitted, MessageProofRequest, ProgramProver, ProverConfig,
    RangeProofCommitted,
};
use crate::prover::{prover_from_env, SP1Prover};

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const EV_RANGE_EXEC_ELF: &[u8] = include_elf!("ev-range-exec-program");

/// ProofInput is a convenience type used for proof aggregation inputs within the BlockRangeExecProver program.
pub struct ProofInput {
    proof: SP1Proof,
    vkey: SP1VerifyingKey,
}

impl ProofInput {
    pub fn new(proof: SP1Proof, vkey: SP1VerifyingKey) -> Self {
        Self { proof, vkey }
    }
}

#[derive(Clone)]
pub struct BlockRangeExecConfig {
    pub pk: Arc<SP1ProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
    pub block_exec: ProgramVerifyingKey,
}

impl BlockRangeExecConfig {
    pub fn new(pk: SP1ProvingKey, vk: SP1VerifyingKey, mode: SP1ProofMode, block_exec: ProgramVerifyingKey) -> Self {
        BlockRangeExecConfig {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
            block_exec,
        }
    }
}

impl ProverConfig for BlockRangeExecConfig {
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

#[derive(Clone)]
pub struct ProgramVerifyingKey {
    pub vk: Arc<SP1VerifyingKey>,
    pub digest: [u32; 8],
}

impl ProgramVerifyingKey {
    pub fn new(vk: Arc<SP1VerifyingKey>) -> Self {
        let digest = vk.vk.hash_u32();
        Self { vk, digest }
    }
}

#[async_trait]
impl ProgramProver for BlockRangeExecProver {
    type Config = BlockRangeExecConfig;
    type Input = (BlockRangeExecInput, Vec<ProofInput>);
    type Output = BlockRangeExecOutput;

    /// Returns the program configuration containing the ELF and proof mode.
    fn cfg(&self) -> &Self::Config {
        &self.config
    }

    /// Constructs the SP1Stdin by serializing the program inputs:
    /// - Verifier key digests (`vkeys`)
    /// - Public inputs for each proof
    /// - The compressed SP1 proofs and their associated verifying keys.
    ///
    /// # Errors
    /// - Returns an error if any proof is not in compressed format.
    /// - Returns an error if the number of `proofs` and `vkeys` do not match.
    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin> {
        let mut stdin = SP1Stdin::new();

        let (inputs, proof_inputs) = input;
        if inputs.vkeys.len() != proof_inputs.len() {
            return Err(anyhow!(
                "mismatched lengths: {} vkeys vs {} proof_inputs",
                inputs.vkeys.len(),
                proof_inputs.len()
            ));
        }

        stdin.write(&inputs);
        for proof_input in proof_inputs.iter() {
            match &proof_input.proof {
                SP1Proof::Compressed(inner) => {
                    stdin.write_proof(*inner.clone(), proof_input.vkey.vk.clone());
                }
                _ => {
                    return Err(anyhow::anyhow!("Expected compressed SP1 proof"));
                }
            }
        }

        Ok(stdin)
    }

    /// Parses the `SP1PublicValues` from the proof and converts it into the
    /// program's custom output type.
    ///
    /// # Errors
    /// - Returns an error if deserialization fails.
    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output> {
        Ok(bincode::deserialize::<BlockRangeExecOutput>(
            proof.public_values.as_slice(),
        )?)
    }

    /// Returns the SP1 Prover.
    fn prover(&self) -> Arc<SP1Prover> {
        Arc::clone(&self.prover)
    }
}

/// A prover for verifying and aggregating SP1 proofs over a range of blocks.
///
/// This struct is responsible for preparing the standard input (`SP1Stdin`)
/// for a zkVM program that takes a sequence of SP1 proofs, their corresponding
/// public inputs, and verifier key digests. The program then verifies them
/// reducing the result to a single groth16 proof.
///
///
/// - All SP1 proofs must be in compressed format (`SP1Proof::Compressed`).
/// - The number of `vkeys` must exactly match the number of `proofs`.
pub struct BlockRangeExecProver {
    ctx: Arc<ChainContext>,
    config: BlockRangeExecConfig,
    prover: Arc<SP1Prover>,
}

impl BlockRangeExecProver {
    pub fn new(ctx: Arc<ChainContext>) -> Result<Self> {
        let prover = prover_from_env();
        let config = BlockRangeExecProver::default_config(prover.as_ref());

        Ok(Self { ctx, config, prover })
    }

    /// Returns the default prover configuration for the block execution program.
    pub fn default_config(prover: &SP1Prover) -> BlockRangeExecConfig {
        let (pk, vk) = prover.setup(EV_RANGE_EXEC_ELF);
        let (_, inner_vk) = prover.setup(EV_EXEC_ELF);

        BlockRangeExecConfig::new(
            pk,
            vk,
            SP1ProofMode::Groth16,
            ProgramVerifyingKey::new(Arc::new(inner_vk)),
        )
    }
}

pub struct BlockRangeExecService {
    ctx: Arc<ChainContext>,
    client: CelestiaIsmClient,
    prover: Arc<BlockRangeExecProver>,
    proof_store: Arc<dyn ProofStorage>,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
    rx_block: Receiver<BlockProofCommitted>,
    tx_range: Sender<MessageProofRequest>,

    batch_size: usize,
    concurrency: Arc<Semaphore>,

    pending: BTreeSet<BlockProofCommitted>,
    next_expected: Option<u64>,
}

impl BlockRangeExecService {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ctx: Arc<ChainContext>,
        client: CelestiaIsmClient,
        prover: Arc<BlockRangeExecProver>,
        proof_store: Arc<dyn ProofStorage>,
        hyperlane_message_store: Arc<HyperlaneMessageStore>,
        rx_block: Receiver<BlockProofCommitted>,
        tx_range: Sender<MessageProofRequest>,
        batch_size: usize,
        concurrency: usize,
    ) -> Result<Self> {
        let next_expected = proof_store.get_range_cursor().await?;
        debug!(?next_expected, "Loaded next expected range cursor from proof_store");

        Ok(Self {
            ctx,
            client,
            prover,
            proof_store,
            hyperlane_message_store,
            rx_block,
            tx_range,
            batch_size,
            concurrency: Arc::new(Semaphore::new(concurrency)),
            pending: BTreeSet::new(),
            next_expected,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let indexer = self.ctx.hyperlane_indexer();

        while let Some(ev) = self.rx_block.recv().await {
            self.pending.insert(ev);
            debug!("Block execution proofs pending: {}", self.pending.len());

            // Process all complete batches that are ready
            while let Some((start, end)) = self.next_provable_range()? {
                // Persist the cursor after determining the next range
                if let Some(cursor) = self.next_expected {
                    let proof_store = self.proof_store.clone();
                    proof_store.set_range_cursor(cursor).await?;
                    debug!(next_expected = cursor, "Persisted next expected range cursor");
                }

                let permit = self.concurrency.clone().acquire_owned().await?;

                // Clone resources for the spawned task
                let client = self.client.clone();
                let prover = self.prover.clone();
                let proof_store = self.proof_store.clone();
                let tx = self.tx_range.clone();
                let mut indexer_clone = indexer.clone();
                let ctx = self.ctx.clone();
                let hyperlane_message_store = self.hyperlane_message_store.clone();

                // Spawn a concurrent task to aggregate and submit the range proof
                info!(?start, ?end, "Spawning new task for aggregate range");
                tokio::spawn(async move {
                    let _permit = permit;

                    match Self::aggregate_range(start, end, prover, proof_store).await {
                        Ok((proof, output)) => {
                            // Submit the range proof to the ISM
                            if let Err(e) = Self::submit_range_proof(&client, &proof).await {
                                error!(?e, "Failed to submit tx to ism");
                            }

                            let event = RangeProofCommitted::new(output.new_height, output.new_state_root);
                            let message = MessageProofRequest::new(event);

                            // Index Hyperlane messages if new EV blocks were included
                            if output.trusted_height < output.new_height {
                                indexer_clone.filter = Filter::new()
                                    .address(ctx.mailbox_address())
                                    .event(&Dispatch::id())
                                    // start indexing from the first ev block after our last checkpoint
                                    .from_block(output.trusted_height + 1)
                                    .to_block(output.new_height);

                                // Run the indexer to get all messages that occurred since the last trusted height
                                if let Err(e) = indexer_clone
                                    .index(hyperlane_message_store.clone(), ctx.evm_provider())
                                    .await
                                {
                                    error!(?e, "Failed to index hyperlane messages");
                                }
                            }

                            // Send the range proof committed event downstream
                            if let Err(e) = tx.send(message).await {
                                error!(?e, "Failed to send RangeProofCommitted event on channel");
                            }
                        }
                        Err(e) => {
                            error!(?e, %start, %end, "Range aggregation failed");
                        }
                    }
                });
            }
        }
        Ok(())
    }

    /// Calculate the next provable range bounded by batch size.
    /// If a complete batch exists then remove those entries from `pending`, advance the cursor, and return the range.
    /// Note: the start and end range indices are inclusive.
    fn next_provable_range(&mut self) -> Result<Option<(u64, u64)>> {
        if self.batch_size == 0 {
            return Ok(None);
        }

        // Initialize cursor from the smallest pending height if not set
        if self.next_expected.is_none() {
            match self.pending.first() {
                Some(h) => self.next_expected = Some(h.height()),
                None => return Ok(None), // nothing pending yet
            }
        }

        let start = self.next_expected.unwrap();
        let end = start + (self.batch_size as u64) - 1;

        // Verify we have exactly batch_size contiguous elements
        let mut cursor = start;
        let iter = self.pending.range(BlockProofCommitted(start)..);
        for proof in iter.take(self.batch_size) {
            if proof.height() != cursor {
                return Ok(None); // missing contiguous element, incomplete batch
            }
            cursor += 1;
        }

        // Ensure batch is complete
        if cursor <= end {
            return Ok(None);
        }

        // Remove completed batch from pending set and advance cursor
        for h in start..=end {
            self.pending.remove(&BlockProofCommitted(h));
        }

        self.next_expected = Some(cursor);
        Ok(Some((start, end)))
    }

    /// Aggregates a range of block proofs, start and end inclusive.
    async fn aggregate_range(
        start: u64,
        end: u64,
        prover: Arc<BlockRangeExecProver>,
        proof_store: Arc<dyn ProofStorage>,
    ) -> Result<(SP1ProofWithPublicValues, BlockRangeExecOutput)> {
        // Load all block proofs in the range from storage
        let block_proofs = proof_store.get_block_proofs_in_range(start, end).await?;
        let inner = &prover.cfg().block_exec;
        let vkeys = vec![inner.digest; block_proofs.len()];

        // Extract public values and reconstruct SP1 proofs
        let mut public_values = Vec::with_capacity(block_proofs.len());
        let mut proofs = Vec::with_capacity(block_proofs.len());
        for stored_proof in block_proofs {
            let proof: SP1Proof = bincode::deserialize(&stored_proof.proof_data)?;

            public_values.push(stored_proof.public_values);
            proofs.push(ProofInput::new(proof, (*inner.vk).clone()));
        }

        // Generate the aggregated proof
        let input = (BlockRangeExecInput { vkeys, public_values }, proofs);
        let (res, output) = prover.prove(input).await?;

        // Store the range proof for future reference
        proof_store.store_range_proof(start, end, &res, &output).await?;

        info!("Successfully created and stored proof for range {start}-{end}. Outputs: {output}");

        Ok((res, output))
    }

    async fn submit_range_proof(client: &CelestiaIsmClient, proof: &SP1ProofWithPublicValues) -> Result<()> {
        let public_values = proof.public_values.to_vec();
        let signer_address = client.signer_address().to_string();
        let ism_id = client.ism_id().to_string();

        // Prepare and send the state transition proof message
        let proof_msg = StateTransitionProofMsg::new(ism_id, proof.bytes(), public_values, signer_address);
        let res = client.send_tx(proof_msg).await?;

        info!("Proof tx submitted to ism with hash: {}", res.tx_hash);

        Ok(())
    }
}
