//! The prover implementation of the Hyperlane Message circuit proves all messages that have occurred in between
//! two given heights against a given EVM block height.

#![allow(dead_code)]
use crate::prover::chain::ChainContext;
use crate::prover::{prover_from_env, MessageProofRequest, MessageProofSync, RangeProofCommitted, SP1Prover};
use crate::prover::{ProgramProver, ProverConfig};
use alloy::hex::FromHex;
use alloy_primitives::FixedBytes;
use alloy_provider::Provider;
use alloy_rpc_types::EIP1186AccountProofResponse;
use anyhow::Result;
use celestia_grpc_client::{MsgProcessMessage, MsgSubmitMessages};
use ev_state_queries::StateQueryProvider;
use ev_zkevm_types::hyperlane::encode_hyperlane_message;
use ev_zkevm_types::programs::hyperlane::types::{
    HyperlaneBranchProof, HyperlaneBranchProofInputs, HyperlaneMessageInputs, HyperlaneMessageOutputs,
    HYPERLANE_MERKLE_TREE_KEYS,
};
use sp1_sdk::{include_elf, SP1ProofMode, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey};
use std::{env, sync::Arc};
use storage::hyperlane::StoredHyperlaneMessage;
use storage::hyperlane::{message::HyperlaneMessageStore, snapshot::HyperlaneSnapshotStore};
use storage::proofs::ProofStorage;
use tokio::sync::mpsc::Receiver;
use tracing::{debug, error, info};

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const EV_HYPERLANE_ELF: &[u8] = include_elf!("ev-hyperlane-program");

#[derive(Clone)]
pub struct MessageProverConfig {
    pub pk: Arc<SP1ProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
}

impl MessageProverConfig {
    pub fn new(pk: SP1ProvingKey, vk: SP1VerifyingKey, mode: SP1ProofMode) -> Self {
        MessageProverConfig {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
        }
    }
}

impl ProverConfig for MessageProverConfig {
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

/// MerkleTreeState encapsulates the height of the merkle tree in terms of snapshots and blocks
pub struct MerkleTreeState {
    // the index of the snapshot that we will load from the db, initially 0 (empty by default)
    snapshot_index: u64,
    // the index of the last block whose messages were proven, leading up to the snapshot at index snapshot_index
    height: u64,
}

impl MerkleTreeState {
    pub fn new(snapshot_index: u64, height: u64) -> Self {
        Self { snapshot_index, height }
    }
}

/// HyperlaneMessageProver is a prover for generating SP1 proofs for Hyperlane message inclusion in EVM blocks.
pub struct HyperlaneMessageProver {
    pub ctx: Arc<ChainContext>,
    pub config: MessageProverConfig,
    pub prover: Arc<SP1Prover>,
    pub message_store: Arc<HyperlaneMessageStore>,
    pub snapshot_store: Arc<HyperlaneSnapshotStore>,
    pub proof_store: Arc<dyn ProofStorage>,
    pub state_query_provider: Arc<dyn StateQueryProvider>,
}

impl ProgramProver for HyperlaneMessageProver {
    type Config = MessageProverConfig;
    type Input = HyperlaneMessageInputs;
    type Output = HyperlaneMessageOutputs;

    fn cfg(&self) -> &Self::Config {
        &self.config
    }

    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin> {
        let mut stdin = SP1Stdin::new();
        stdin.write(&input);
        Ok(stdin)
    }

    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output> {
        Ok(bincode::deserialize::<HyperlaneMessageOutputs>(
            proof.public_values.as_slice(),
        )?)
    }

    fn prover(&self) -> Arc<SP1Prover> {
        Arc::clone(&self.prover)
    }
}

impl HyperlaneMessageProver {
    pub fn new(
        ctx: Arc<ChainContext>,
        message_store: Arc<HyperlaneMessageStore>,
        snapshot_store: Arc<HyperlaneSnapshotStore>,
        proof_store: Arc<dyn ProofStorage>,
        state_query_provider: Arc<dyn StateQueryProvider>,
    ) -> Result<Self> {
        let prover = prover_from_env();
        let config = HyperlaneMessageProver::default_config(prover.as_ref());

        Ok(Self {
            ctx,
            config,
            prover,
            message_store,
            snapshot_store,
            proof_store,
            state_query_provider,
        })
    }

    /// Returns the default prover configuration for the block execution program.
    pub fn default_config(prover: &SP1Prover) -> MessageProverConfig {
        let (pk, vk) = prover.setup(EV_HYPERLANE_ELF);
        MessageProverConfig::new(pk, vk, SP1ProofMode::Groth16)
    }

    pub async fn run(
        self: Arc<Self>,
        mut range_rx: Receiver<MessageProofRequest>,
        message_sync: Arc<MessageProofSync>,
    ) -> Result<()> {
        let evm_provider = self.ctx.evm_provider();
        while let Some(request) = range_rx.recv().await {
            let commit_message: RangeProofCommitted = request.commit;
            info!("Received commit message: {:?}", commit_message);

            let committed_height = commit_message.trusted_height();
            let committed_state_root = commit_message.trusted_root();

            let _permit = match request.permit {
                Some(permit) => permit,
                None => message_sync.begin().await,
            };

            let keys: Vec<FixedBytes<32>> = HYPERLANE_MERKLE_TREE_KEYS
                .iter()
                .map(|k| FixedBytes::from_hex(k).map_err(|e| anyhow::anyhow!("Failed to parse fixed bytes: {e}")))
                .collect::<Result<Vec<_>>>()?;

            let merkle_proof = evm_provider
                .get_proof(self.ctx.merkle_tree_address(), keys)
                .block_id(committed_height.into())
                .await?;

            // Run the inner proof generation and submission logic
            if let Err(e) = self
                .run_inner(
                    committed_height,
                    merkle_proof.clone(),
                    FixedBytes::from_slice(&committed_state_root),
                )
                .await
            {
                error!(
                    "Failed to generate proof, Stored Value: {}, error: {e:?}",
                    hex::encode(
                        merkle_proof
                            .storage_proof
                            .last()
                            .ok_or(anyhow::anyhow!("No storage proof for Hyperlane Branch"))?
                            .value
                            .to_be_bytes::<32>()
                    )
                );
            }
        }
        Ok(())
    }

    async fn run_inner(
        &self,
        committed_height: u64,
        proof: EIP1186AccountProofResponse,
        state_root: FixedBytes<32>,
    ) -> Result<()> {
        // Load the current snapshot and check if there are new blocks to process
        let trusted_snapshot_index = self.snapshot_store.current_index()?;
        let mut snapshot = self.snapshot_store.get_snapshot(trusted_snapshot_index)?;

        if snapshot.height == committed_height {
            debug!("No new ev blocks so no new messages to prove");
            return Ok(());
        }

        let start_height = snapshot.height + 1;

        // Collect all messages from the new blocks
        let mut messages: Vec<StoredHyperlaneMessage> = Vec::new();
        for block in start_height..=committed_height {
            messages.extend(self.message_store.get_by_block(block)?);
        }

        if messages.is_empty() {
            debug!("No messages found in db");
            return Ok(());
        }

        // Prepare the branch proof for verification
        let branch_proof = HyperlaneBranchProof::new(proof);

        // Construct program inputs from values
        let input = HyperlaneMessageInputs::new(
            state_root.to_string(),
            self.ctx.merkle_tree_address().to_string(),
            messages.clone().into_iter().map(|m| m.message).collect(),
            HyperlaneBranchProofInputs::from(branch_proof),
            snapshot.tree.clone(),
        );

        // Update the snapshot's merkle tree with new messages
        for message in messages.clone() {
            snapshot.tree.insert(message.message.id())?;
        }

        info!(
            "Proving messages with ids: {:?}",
            messages.iter().map(|m| m.message.id()).collect::<Vec<String>>()
        );

        // Generate the message proof
        let ism_client = self.ctx.ism_client();
        let message_proof = self.prove(input).await?;
        info!("Message proof generated successfully");

        // Prepare the proof submission message
        let message_proof_msg = MsgSubmitMessages::new(
            self.ctx.ism_id().to_string(),
            committed_height,
            message_proof.0.bytes(),
            message_proof.0.public_values.as_slice().to_vec(),
            ism_client.signer_address().to_string(),
        );

        // Store the unfinalized snapshot
        snapshot.height = committed_height;
        let snapshot_index = self.snapshot_store.current_index()? + 1;
        self.snapshot_store.insert_snapshot(snapshot_index, snapshot)?;

        // Submit the proof to ZKISM
        info!("Submitting Hyperlane tree proof to ZKISM...");
        let response = ism_client.send_tx(message_proof_msg).await?;

        if !response.success {
            error!("Failed to submit Hyperlane tree proof to ZKISM: {:?}", response);
            return Err(anyhow::anyhow!("Failed to submit Hyperlane tree proof to ZKISM"));
        }

        info!("ZKISM was updated successfully");
        self.proof_store
            .store_membership_proof(committed_height, &message_proof.0, &message_proof.1)
            .await?;

        // TODO: check for unfinalized shapshots and retry
        // this is a necessary mainnet optimization
        self.snapshot_store.finalize_snapshot(trusted_snapshot_index)?;

        // Relay all verified messages to Celestia
        // TODO: add a finality flag to each message and retry
        // this is a necessary mainnet optimization
        info!("Relaying verified Hyperlane messages to Celestia...");

        for message in messages.clone() {
            let message_hex = alloy::hex::encode(encode_hyperlane_message(&message.message)?);
            let msg = MsgProcessMessage::new(
                self.ctx.config().hyperlane.celestia.mailbox_id.clone(),
                ism_client.signer_address().to_string(),
                alloy::hex::encode(vec![]), // empty metadata; messages are pre-authorized before submission
                message_hex,
            );

            let response = ism_client.send_tx(msg).await?;
            if !response.success {
                error!("Failed to relay Hyperlane message to Celestia: {:?}", response);
                return Err(anyhow::anyhow!("Failed to relay Hyperlane message to Celestia"));
            }
            info!(
                "Successfully submitted Hyperlane message with id {} to Celestia",
                message.message.id()
            );
        }
        Ok(())
    }
}
