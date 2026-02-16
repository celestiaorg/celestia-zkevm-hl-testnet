pub mod command;
pub mod config;
pub mod proto;
pub mod prover;
pub mod server;
#[cfg(test)]
pub mod tests;

use alloy_genesis::Genesis as AlloyGenesis;
use alloy_provider::ProviderBuilder;
use anyhow::{Context, Result};
use ev_types::v1::{
    get_block_request::Identifier, store_service_client::StoreServiceClient, GetBlockRequest, GetMetadataRequest,
};
use reth_chainspec::ChainSpec;
use rsp_client_executor::io::EvolveClientExecutorInput;
use rsp_host_executor::EvolveHostExecutor;
use rsp_primitives::genesis::Genesis;
use std::{fs, sync::Arc};
use tracing::debug;

/// Generates the client executor input (STF) for an EVM block.
pub async fn generate_client_executor_input(
    rpc_url: &str,
    block_number: u64,
    chain_spec: Arc<ChainSpec>,
    genesis: Genesis,
) -> Result<EvolveClientExecutorInput> {
    let host_executor = EvolveHostExecutor::evolve(chain_spec.clone(), None, &genesis);

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let client_input = host_executor
        .execute(block_number, &provider, genesis, None, false)
        .await
        .with_context(|| format!("Failed to execute block {block_number}"))?;

    Ok(client_input)
}

/// Loads the genesis file from disk and converts it into a ChainSpec
pub fn load_chain_spec_from_genesis(path: &str) -> Result<(Genesis, Arc<ChainSpec>)> {
    let genesis_json = fs::read_to_string(path).with_context(|| format!("Failed to read genesis file at {path}"))?;
    let alloy_genesis: AlloyGenesis = serde_json::from_str(&genesis_json)?;

    let genesis = Genesis::Custom(alloy_genesis.config);
    let chain_spec: Arc<ChainSpec> = Arc::new((&genesis).try_into()?);

    Ok((genesis, chain_spec))
}

pub async fn get_sequencer_pubkey(sequencer_url: String) -> Result<Vec<u8>> {
    debug!("Connecting to sequencer url: {}", sequencer_url);
    let mut sequencer_client = StoreServiceClient::connect(sequencer_url.clone()).await?;
    let block_req = GetBlockRequest {
        identifier: Some(Identifier::Height(1)),
    };
    let resp = sequencer_client.get_block(block_req).await?;
    debug!("Got block from sequencer url: {}", sequencer_url);
    let pub_key = resp
        .into_inner()
        .block
        .ok_or_else(|| anyhow::anyhow!("Block not found"))?
        .header
        .ok_or_else(|| anyhow::anyhow!("Header not found"))?
        .signer
        .ok_or_else(|| anyhow::anyhow!("Signer not found"))?
        .pub_key;

    Ok(pub_key[4..].to_vec())
}

// Get the Celestia inclusion height for a given Evolve block number
pub async fn inclusion_height(block_number: u64, sequencer_rpc_url: String) -> anyhow::Result<u64> {
    let mut client = StoreServiceClient::connect(sequencer_rpc_url).await?;
    let req = GetMetadataRequest {
        key: format!("rhb/{block_number}/d"),
    };

    let resp = client.get_metadata(req).await?;
    let height = u64::from_le_bytes(resp.into_inner().value[..8].try_into()?);
    Ok(height)
}
