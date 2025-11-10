pub mod command;
pub mod config;
pub mod proto;
pub mod prover;
pub mod server;
#[cfg(test)]
pub mod tests;

use alloy_consensus::{Block as ConsensusBlock, EthereumTxEnvelope, TxEip4844};
use alloy_genesis::Genesis as AlloyGenesis;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::{Block, BlockNumberOrTag, Transaction};
use anyhow::{anyhow, Context, Result};
use ev_types::v1::{
    get_block_request::Identifier, store_service_client::StoreServiceClient, GetBlockRequest, GetMetadataRequest,
};
use reth_chainspec::ChainSpec;
use rsp_client_executor::io::EthClientExecutorInput;
use rsp_host_executor::EthHostExecutor;
use rsp_primitives::genesis::Genesis;
use rsp_rpc_db::RpcDb;
use std::{fs, sync::Arc};
use tracing::debug;
use zeth_core::EthEvmConfig;
use zeth_rpc_proxy::execution_witness;

/// Generates the client executor input (STF) for an EVM block.
pub async fn generate_client_executor_input_sp1(
    rpc_url: &str,
    block_number: u64,
    chain_spec: Arc<ChainSpec>,
    genesis: Genesis,
) -> Result<EthClientExecutorInput> {
    let host_executor = EthHostExecutor::eth(chain_spec.clone(), None);

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let rpc_db = RpcDb::new(provider.clone(), block_number - 1);

    let client_input = host_executor
        .execute(block_number, &rpc_db, &provider, genesis, None, false)
        .await
        .with_context(|| format!("Failed to execute block {block_number}"))?;

    Ok(client_input)
}

pub async fn generate_client_executor_input_zeth(rpc_url: &str, block_id: u64) -> Result<zeth_core::Input> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let eth_evm_config = EthEvmConfig::mainnet();
    let witness = execution_witness(
        eth_evm_config.clone(),
        &provider,
        alloy_rpc_types::BlockNumberOrTag::Number(block_id),
    )
    .await?;

    let zeth_input = zeth_core::Input {
        block: fetch_block_for_zeth(rpc_url, block_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch block: {e}"))?,
        signers: vec![],
        witness: witness,
    };
    Ok(zeth_input)
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
    debug!("Connected to sequencer url: {}", sequencer_url);
    let block_req = GetBlockRequest {
        identifier: Some(Identifier::Height(1)),
    };
    debug!("Getting block from sequencer url: {}", sequencer_url);
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

pub async fn fetch_block_for_zeth(
    rpc_url: &str,
    block_id: u64,
) -> Result<ConsensusBlock<EthereumTxEnvelope<TxEip4844>>> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let block: Option<Block<Transaction>> = provider
        .get_block_by_number(BlockNumberOrTag::Number(block_id.into()))
        .await?;

    let rpc_block = block.ok_or_else(|| anyhow!("Block {} not found", block_id))?;

    // The RPC header has an 'inner' field that contains the consensus header
    let consensus_header = rpc_block.header.inner;

    // Convert RPC transactions to consensus transaction envelopes
    let transactions: Vec<EthereumTxEnvelope<TxEip4844>> = match rpc_block.transactions {
        alloy_rpc_types::BlockTransactions::Full(txs) => {
            txs.into_iter()
                .map(|tx| {
                    // Convert RPC transaction to consensus envelope
                    tx.try_into()
                        .map_err(|e| anyhow!("Failed to convert transaction: {}", e))
                })
                .collect::<Result<Vec<_>>>()?
        }
        alloy_rpc_types::BlockTransactions::Hashes(_) | alloy_rpc_types::BlockTransactions::Uncle => {
            return Err(anyhow!("Block must have full transaction details"));
        }
    };

    // Convert withdrawals if present - collect into Vec and let Into trait handle conversion
    let withdrawals = rpc_block.withdrawals.map(|w| {
        let vec: Vec<_> = w.into_iter().map(|wd| wd.into()).collect();
        vec.into()
    });

    Ok(ConsensusBlock {
        header: consensus_header,
        body: alloy_consensus::BlockBody {
            transactions,
            ommers: vec![],
            withdrawals,
        },
    })
}
