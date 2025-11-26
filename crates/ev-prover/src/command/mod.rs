use std::sync::Arc;

use alloy_provider::Provider;
use alloy_rpc_types::{BlockId, BlockNumberOrTag};
use anyhow::Result;
use celestia_grpc_client::proto::celestia::zkism::v1::MsgCreateZkExecutionIsm;
use celestia_grpc_client::proto::hyperlane::warp::v1::MsgSetToken;
use celestia_grpc_client::types::ClientConfig;
use celestia_grpc_client::CelestiaIsmClient;
use celestia_rpc::HeaderClient;
use sp1_sdk::{HashableKey, Prover, ProverClient};
use tracing::info;

use crate::command::cli::{QueryCommands, VERSION};
use crate::config::Config;
use crate::get_sequencer_pubkey;
use crate::proto::celestia::prover::v1::prover_client::ProverClient as GrpcProverClient;
use crate::proto::celestia::prover::v1::{
    GetBlockProofRequest, GetBlockProofsInRangeRequest, GetLatestBlockProofRequest, GetLatestMembershipProofRequest,
    GetMembershipProofRequest, GetRangeProofsRequest,
};
use crate::prover::chain::ChainContext;
use crate::prover::programs::batch::BATCH_ELF;
use crate::prover::programs::message::EV_HYPERLANE_ELF;
use crate::server::start_server;
use storage::proofs::{ProofStorage, RocksDbProofStorage};

pub mod cli;
pub use cli::{Cli, Commands};

pub fn init() -> Result<()> {
    Config::init()?;

    Ok(())
}

pub async fn start() -> Result<()> {
    let config = Config::load()?;
    info!("Starting gRPC server at {}", config.grpc_address);
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

    let create_message = MsgCreateZkExecutionIsm {
        creator: ism_client.signer_address().to_string(),
        state_root: ev_state_root.to_vec(),
        height: ev_block_height,
        celestia_header_hash: block_hash,
        celestia_height: height,
        namespace: namespace.as_bytes().to_vec(),
        sequencer_public_key: pub_key,
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

pub async fn query(query_cmd: QueryCommands) -> Result<()> {
    match query_cmd {
        QueryCommands::LatestBlock { server } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client.get_latest_block_proof(GetLatestBlockProofRequest {}).await?;
            let inner = response.into_inner();

            if let Some(proof) = inner.proof {
                info!("Latest block proof:");
                info!("  Height: {}", proof.celestia_height);
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
            } else {
                info!("No proof data returned");
            }
        }
        QueryCommands::Block { height, server } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client
                .get_block_proof(GetBlockProofRequest {
                    celestia_height: height,
                })
                .await?;

            if let Some(proof) = response.into_inner().proof {
                info!("Block proof for height {height}:");
                info!("  Height: {}", proof.celestia_height);
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
            } else {
                info!("No proof data returned");
            }
        }
        QueryCommands::BlockRange {
            start_height,
            end_height,
            server,
        } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client
                .get_block_proofs_in_range(GetBlockProofsInRangeRequest {
                    start_height,
                    end_height,
                })
                .await?;

            let proofs = response.into_inner().proofs;
            info!("Found {} block proof(s):\n", proofs.len());

            for (i, proof) in proofs.iter().enumerate() {
                info!("Proof {} of {}:", i + 1, proofs.len());
                info!("  Height: {}", proof.celestia_height);
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
                info!("");
            }
        }
        QueryCommands::LatestMembership { server } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client
                .get_latest_membership_proof(GetLatestMembershipProofRequest {})
                .await?;

            if let Some(proof) = response.into_inner().proof {
                info!("Latest membership proof:");
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
            } else {
                info!("No proof data returned");
            }
        }
        QueryCommands::Membership { height, server } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client
                .get_membership_proof(GetMembershipProofRequest { height })
                .await?;

            if let Some(proof) = response.into_inner().proof {
                info!("Membership proof for height {height}:");
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
            } else {
                info!("No proof data returned");
            }
        }
        QueryCommands::RangeProofs {
            start_height,
            end_height,
            server,
        } => {
            let mut client = GrpcProverClient::connect(server).await?;
            let response = client
                .get_range_proofs(GetRangeProofsRequest {
                    start_height,
                    end_height,
                })
                .await?;

            let proofs = response.into_inner().proofs;
            info!("Found {} range proof(s):\n", proofs.len());

            for (i, proof) in proofs.iter().enumerate() {
                info!("Range Proof {} of {}:", i + 1, proofs.len());
                info!("  Range: {} - {}", proof.start_height, proof.end_height);
                info!("  Proof size: {} bytes", proof.proof_data.len());
                info!("  Public values size: {} bytes", proof.public_values.len());
                info!("  Created at (Unix): {}", proof.created_at);
                info!("");
            }
        }
    }

    Ok(())
}
