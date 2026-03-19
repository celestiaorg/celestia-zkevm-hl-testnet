use std::sync::Arc;

use alloy_primitives::FixedBytes;
use alloy_provider::Provider;
use anyhow::{anyhow, Result};
use celestia_grpc_client::{MsgUpdateInterchainSecurityModule, QueryIsmRequest};
use celestia_rpc::{BlobClient, HeaderClient, ShareClient};
use celestia_types::{
    nmt::{Namespace, NamespaceProof},
    Blob,
};
use ev_types::v1::SignedData;
use ev_zkevm_types::programs::block::{BlockExecInput, State};
use prost::Message;
use rsp_client_executor::io::EthClientExecutorInput;
use sp1_sdk::SP1ProofWithPublicValues;
use storage::hyperlane::message::HyperlaneMessageStore;
use tracing::{debug, error, info};

use crate::prover::chain::ChainContext;
use crate::prover::config::{BATCH_SIZE, MAX_BATCH_SIZE, MAX_INDEXING_RANGE, MIN_BATCH_SIZE};

/// ProverStatus of the latest Celestia state relevant to the prover loop.
///
/// The methods on this type encapsulate small pieces of batching logic so
/// the main control flow stays readable.
pub struct ProverStatus {
    pub trusted_height: u64,
    pub trusted_root: FixedBytes<32>,
    pub trusted_celestia_height: u64,
    pub celestia_head: u64,
}

impl ProverStatus {
    /// Loads the ProverStatus by querying the trusted state from the on-chain ISM and
    /// the latest header from Celestia.
    pub async fn load(ctx: &ChainContext) -> Result<ProverStatus> {
        let resp = ctx
            .ism_client()
            .ism(QueryIsmRequest {
                id: ctx.ism_id().to_string(),
            })
            .await?;
        let ism = resp.ism.ok_or_else(|| anyhow!("ZKISM not found"))?;
        let state: State = bincode::deserialize(&ism.state).unwrap();
        let trusted_root = FixedBytes::from_slice(&state.state_root);
        let celestia_head = ctx.celestia_client().header_local_head().await?.height().value();

        Ok(ProverStatus {
            trusted_height: state.height,
            trusted_root,
            trusted_celestia_height: state.celestia_height,
            celestia_head,
        })
    }

    /// Returns true if enough new blocks have been produced to start proving a batch.
    pub fn is_batch_ready(&self, batch_size: u64) -> bool {
        self.trusted_celestia_height + batch_size <= self.celestia_head
    }

    /// Returns how many more blocks are needed to reach a full batch.
    pub fn blocks_remaining(&self, batch_size: u64) -> u64 {
        (self.trusted_celestia_height + batch_size).saturating_sub(self.celestia_head)
    }

    /// Returns how far ahead the Celestia head is from the trusted height.
    pub fn distance(&self) -> u64 {
        self.celestia_head.saturating_sub(self.trusted_celestia_height)
    }
}

/// Calculates the block prover batch size given the starting height, latest height and trusted height.
/// If a non-empty block is found then the batch is reduced.
pub async fn calculate_batch_size(
    ctx: &ChainContext,
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
        let Some(block_number) = ctx.latest_block_for_height(height).await? else {
            continue;
        };

        let nonce = ctx.mailbox_nonce_at(block_number).await?;

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
pub async fn index_messages(
    ctx: &ChainContext,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
    start_block: u64,
    end_block: u64,
) -> Result<()> {
    if start_block > end_block {
        return Ok(());
    }

    let indexer = ctx.hyperlane_indexer();
    let mut from_block = start_block;
    while from_block <= end_block {
        let to_block = std::cmp::min(from_block + MAX_INDEXING_RANGE - 1, end_block);
        debug!("Indexing mailbox events from block {from_block} to {to_block}");

        let filter = indexer.filter_with_range(from_block, to_block);
        indexer
            .process(filter, ctx.evm_provider(), hyperlane_message_store.clone())
            .await?;
        from_block = to_block + 1;
    }

    Ok(())
}

/// Submits a state transition proof msg to the zk verifier on-chain.
pub async fn submit_proof_msg(ctx: &ChainContext, proof: &SP1ProofWithPublicValues) -> Result<()> {
    let id = ctx.ism_id().to_string();
    let public_values = proof.public_values.as_slice().to_vec();
    let signer = ctx.ism_client().signer_address().to_string();

    let msg = MsgUpdateInterchainSecurityModule::new(id, proof.bytes(), public_values, signer);

    info!("Updating ZKISM on Celestia...");
    let response = ctx.ism_client().send_tx(msg).await?;
    if !response.success {
        error!("Failed to submit state transition proof to ZKISM: {:?}", response);
        return Err(anyhow::anyhow!("Failed to submit state transition proof to ZKISM"));
    }

    info!("Proof tx submitted to ism with hash: {}", response.tx_hash);

    Ok(())
}

/// Builds a single block prover input for the given Celestia height.
pub async fn build_block_input(
    ctx: &ChainContext,
    height: u64,
    namespace: Namespace,
    trusted_height: &mut u64,
    trusted_root: &mut FixedBytes<32>,
) -> Result<BlockExecInput> {
    let blobs: Vec<Blob> = ctx
        .celestia_client()
        .blob_get_all(height, &[namespace])
        .await?
        .unwrap_or_default();
    debug!("Got {} blobs for block: {}", blobs.len(), height);

    let extended_header = ctx.celestia_client().header_get_by_height(height).await?;
    let namespace_data = ctx
        .celestia_client()
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
            pub_key: ctx.pub_key_bytes(),
            namespace,
            proofs,
            executor_inputs: vec![],
            trusted_height: *trusted_height,
            trusted_root: *trusted_root,
        });
    }

    // Process blobs to extract executor inputs
    let mut last_height = 0;
    for blob in blobs.as_slice() {
        let signed_data = match SignedData::decode(blob.data.as_slice()) {
            Ok(data) => data,
            Err(_) => continue,
        };
        let data = signed_data.data.ok_or_else(|| anyhow!("Data not found"))?;
        let height = data.metadata.ok_or_else(|| anyhow!("Metadata not found"))?.height;
        last_height = height;
        debug!("Got SignedData for ev block {height}");

        let client_executor_input = ctx.generate_executor_input(height).await?;
        executor_inputs.push(client_executor_input);
    }

    // Construct the block execution input
    let input = BlockExecInput {
        header_raw: serde_cbor::to_vec(&extended_header.header)?,
        dah: extended_header.dah,
        blobs_raw: serde_cbor::to_vec(&blobs)?,
        pub_key: ctx.pub_key_bytes(),
        namespace,
        proofs,
        executor_inputs: executor_inputs.clone(),
        trusted_height: *trusted_height,
        trusted_root: *trusted_root,
    };

    // Update trusted state based on the last EVM block processed
    let block = ctx
        .evm_provider()
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
