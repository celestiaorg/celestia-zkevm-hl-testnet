use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_rpc_types::{BlockId, BlockNumberOrTag};
use anyhow::Result;
use celestia_grpc_client::proto::celestia::zkism::v1::MsgCreateInterchainSecurityModule;
use celestia_grpc_client::proto::hyperlane::warp::v1::MsgSetToken;
use celestia_grpc_client::types::ClientConfig;
use celestia_grpc_client::CelestiaIsmClient;
use celestia_rpc::HeaderClient;
use sp1_sdk::{HashableKey, Prover, ProverClient};
use tracing::info;

use crate::command::cli::{QueryCommands, VERSION};
use crate::config::Config;
use crate::get_sequencer_pubkey;
use crate::prover::chain::ChainContext;
use crate::prover::programs::batch::BATCH_ELF;
use crate::prover::programs::message::EV_HYPERLANE_ELF;
use crate::server::start_server;
use ev_zkevm_types::programs::block::State;
use storage::proofs::{ProofStorage, RocksDbProofStorage};

pub mod cli;
pub use cli::{Cli, Commands};

pub fn init() -> Result<()> {
    Config::init()?;

    Ok(())
}

pub async fn start() -> Result<()> {
    let config = Config::load()?;
    info!("Starting HTTP server");
    start_server(config).await?;

    Ok(())
}

pub fn unsafe_reset_db() -> Result<()> {
    let storage_path = Config::storage_path();
    info!("Resetting db state at {}", storage_path.display());

    let mut storage = RocksDbProofStorage::new(storage_path)?;
    storage.unsafe_reset()?;
    Ok(())
}

pub async fn create_ism() -> Result<()> {
    let config = Config::load()?;
    let ism_client = Arc::new(CelestiaIsmClient::new(ClientConfig::from_env()?).await?);
    let chain_ctx = ChainContext::from_config(config.clone(), ism_client.clone()).await?;

    let celestia_client = chain_ctx.celestia_client();
    let namespace = chain_ctx.namespace();

    // Find the most recent Celestia height with a blob and retrieve the associated EVM block height.
    let mut search_height: u64 = celestia_client.header_local_head().await?.height().value();
    let (header, ev_block_height) = loop {
        let header = celestia_client.header_get_by_height(search_height).await?;
        if let Some(block_height) = chain_ctx.latest_block_for_height(search_height).await? {
            break (header, block_height);
        }

        if search_height == 0 {
            return Err(anyhow::anyhow!("No SignedData blobs found in chain"));
        }
        search_height -= 1;
    };

    let height: u64 = header.height().value();
    let block_hash = header.hash().as_bytes().to_vec();

    let block = chain_ctx
        .evm_provider()
        .get_block(BlockId::Number(BlockNumberOrTag::Number(ev_block_height)))
        .await?
        .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

    let ev_state_root = block.header.state_root;

    // todo: deploy the ISM and Update
    let pub_key = get_sequencer_pubkey(config.rpc.evnode_rpc).await?;

    let groth16_vkey = Config::groth16_vkey();
    let (state_transition_vkey, state_membership_vkey) = setup_state_vkeys();

    let initial_state = State {
        state_root: ev_state_root.0,
        celestia_header_hash: block_hash.try_into().unwrap(),
        celestia_height: height,
        height: ev_block_height,
        namespace: namespace.as_bytes().try_into().unwrap(),
        public_key: pub_key.try_into().unwrap(),
    };

    // pad merkle tree address to 32 bytes
    let merkle_tree_address = *Address::from_str(&config.hyperlane.evm.merkle_tree_address)
        .unwrap()
        .into_word();
    let create_message = MsgCreateInterchainSecurityModule {
        creator: ism_client.signer_address().to_string(),
        state: bincode::serialize(&initial_state)?,
        merkle_tree_address: merkle_tree_address.to_vec(),
        groth16_vkey,
        state_transition_vkey,
        state_membership_vkey,
    };

    let response = ism_client.send_tx(create_message).await?;
    if !response.success {
        let tx_hash = response.tx_hash;
        let error_msg = response.error_message.unwrap_or("unknown error".to_string());
        return Err(anyhow::anyhow!("Tx {tx_hash} failed to create ism: {error_msg}",));
    }

    info!("ISM created successfully");
    Ok(())
}

fn setup_state_vkeys() -> (Vec<u8>, Vec<u8>) {
    info!("Setting up ELF for state proofs");
    let prover = ProverClient::builder().cpu().build();
    let (_, state_transition_vkey) = prover.setup(BATCH_ELF);

    info!("Setting up ELF for membership proofs");
    let (_, state_membership_vkey) = prover.setup(EV_HYPERLANE_ELF);

    (
        state_transition_vkey.bytes32_raw().to_vec(),
        state_membership_vkey.bytes32_raw().to_vec(),
    )
}

pub async fn set_token_ism(ism_id: String, token_id: String) -> Result<()> {
    let config = ClientConfig::from_env()?;
    let ism_client = CelestiaIsmClient::new(config).await?;

    let message = MsgSetToken {
        owner: ism_client.signer_address().to_string(),
        token_id,
        new_owner: ism_client.signer_address().to_string(),
        ism_id,
        renounce_ownership: false,
    };

    let response = ism_client.send_tx(message).await?;
    if !response.success {
        let tx_hash = response.tx_hash;
        let error_msg = response.error_message.unwrap_or("unknown error".to_string());
        return Err(anyhow::anyhow!("Tx {tx_hash} failed to set token ism: {error_msg}",));
    }

    info!("ISM updated successfully");
    Ok(())
}

pub fn version() {
    info!("Version: {VERSION}");
}

// HTTP client types for proof queries
use serde::Deserialize;

#[derive(Deserialize)]
struct BlockProofResponse {
    celestia_height: u64,
    proof_data: String,
    public_values: String,
    created_at: u64,
}

#[derive(Deserialize)]
struct MembershipProofResponse {
    proof_data: String,
    public_values: String,
    created_at: u64,
}

#[derive(Deserialize)]
struct RangeProofResponse {
    start_height: u64,
    end_height: u64,
    proof_data: String,
    public_values: String,
    created_at: u64,
}

pub async fn query(query_cmd: QueryCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match query_cmd {
        QueryCommands::LatestBlock { server } => {
            let url = format!("{server}/proofs/block/latest");
            let response: BlockProofResponse = client.get(&url).send().await?.json().await?;

            info!("Latest block proof:");
            info!("  Height: {}", response.celestia_height);
            info!("  Proof size: {} bytes", hex::decode(&response.proof_data)?.len());
            info!(
                "  Public values size: {} bytes",
                hex::decode(&response.public_values)?.len()
            );
            info!("  Created at (Unix): {}", response.created_at);
        }
        QueryCommands::Block { height, server } => {
            let url = format!("{server}/proofs/block/{height}");
            let response: BlockProofResponse = client.get(&url).send().await?.json().await?;

            info!("Block proof for height {height}:");
            info!("  Height: {}", response.celestia_height);
            info!("  Proof size: {} bytes", hex::decode(&response.proof_data)?.len());
            info!(
                "  Public values size: {} bytes",
                hex::decode(&response.public_values)?.len()
            );
            info!("  Created at (Unix): {}", response.created_at);
        }
        QueryCommands::BlockRange {
            start_height,
            end_height,
            server,
        } => {
            let url = format!("{server}/proofs/block/range?start={start_height}&end={end_height}");
            let response: Vec<BlockProofResponse> = client.get(&url).send().await?.json().await?;

            info!("Found {} block proof(s):\n", response.len());

            for (i, proof) in response.iter().enumerate() {
                info!("Proof {} of {}:", i + 1, response.len());
                info!("  Height: {}", proof.celestia_height);
                info!("  Proof size: {} bytes", hex::decode(&proof.proof_data)?.len());
                info!(
                    "  Public values size: {} bytes",
                    hex::decode(&proof.public_values)?.len()
                );
                info!("  Created at (Unix): {}", proof.created_at);
                info!("");
            }
        }
        QueryCommands::LatestMembership { server } => {
            let url = format!("{server}/proofs/membership/latest");
            let response: MembershipProofResponse = client.get(&url).send().await?.json().await?;

            info!("Latest membership proof:");
            info!("  Proof size: {} bytes", hex::decode(&response.proof_data)?.len());
            info!(
                "  Public values size: {} bytes",
                hex::decode(&response.public_values)?.len()
            );
            info!("  Created at (Unix): {}", response.created_at);
        }
        QueryCommands::Membership { height, server } => {
            let url = format!("{server}/proofs/membership/{height}");
            let response: MembershipProofResponse = client.get(&url).send().await?.json().await?;

            info!("Membership proof for height {height}:");
            info!("  Proof size: {} bytes", hex::decode(&response.proof_data)?.len());
            info!(
                "  Public values size: {} bytes",
                hex::decode(&response.public_values)?.len()
            );
            info!("  Created at (Unix): {}", response.created_at);
        }
        QueryCommands::RangeProofs {
            start_height,
            end_height,
            server,
        } => {
            let url = format!("{server}/proofs/range?start={start_height}&end={end_height}");
            let response: Vec<RangeProofResponse> = client.get(&url).send().await?.json().await?;

            info!("Found {} range proof(s):\n", response.len());

            for (i, proof) in response.iter().enumerate() {
                info!("Range Proof {} of {}:", i + 1, response.len());
                info!("  Range: {} - {}", proof.start_height, proof.end_height);
                info!("  Proof size: {} bytes", hex::decode(&proof.proof_data)?.len());
                info!(
                    "  Public values size: {} bytes",
                    hex::decode(&proof.public_values)?.len()
                );
                info!("  Created at (Unix): {}", proof.created_at);
                info!("");
            }
        }
    }

    Ok(())
}
