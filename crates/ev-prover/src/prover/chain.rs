use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::Address;
use alloy_provider::ProviderBuilder;
use alloy_rpc_types::{BlockId, BlockNumberOrTag, Filter};
use anyhow::{anyhow, Context, Result};
use celestia_grpc_client::CelestiaIsmClient;
use celestia_rpc::{client::Client, BlobClient};
use celestia_types::{nmt::Namespace, Blob};
use ev_state_queries::{
    hyperlane::indexer::HyperlaneIndexer, DefaultProvider, MockStateQueryProvider, StateQueryProvider,
};
use ev_types::v1::SignedData;
use ev_zkevm_types::events::Dispatch;
use prost::Message;
use reth_chainspec::ChainSpec;
use rsp_client_executor::io::EthClientExecutorInput;
use rsp_host_executor::EthHostExecutor;
use rsp_primitives::genesis::Genesis;
use rsp_rpc_db::RpcDb;
use url::Url;

use crate::config::Config;
use crate::prover::abi::{MailboxContract, MailboxContract::MailboxContractInstance};

/// Shared chain context constructed from configuration and long-lived clients.
///
/// This contains the common resources required across prover services:
/// - Chain configuration (genesis, chain spec, namespace..etc)
/// - RPC clients
/// - Hyperlane configuration and ISM client
pub struct ChainContext {
    config: Arc<Config>,
    chain_spec: Arc<ChainSpec>,
    genesis: Genesis,
    celestia_client: Arc<Client>,
    evm_provider: DefaultProvider,
    ism_client: Arc<CelestiaIsmClient>,
}

impl ChainContext {
    /// Builds a context from pre-constructed components.
    pub fn new(
        config: Config,
        chain_spec: Arc<ChainSpec>,
        genesis: Genesis,
        celestia_client: Arc<Client>,
        evm_provider: DefaultProvider,
        ism_client: Arc<CelestiaIsmClient>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            config: Arc::new(config),
            chain_spec,
            genesis,
            celestia_client,
            evm_provider,
            ism_client,
        }))
    }

    /// Constructs a context from the given configuration and ISM client.
    pub async fn from_config(config: Config, ism_client: Arc<CelestiaIsmClient>) -> Result<Arc<Self>> {
        let genesis = Config::load_genesis()?;
        let chain_spec = Self::load_chain_spec_from_genesis(&genesis)?;

        let auth_token = config.rpc.celestia_auth_token.as_deref();
        let celestia_client = Arc::new(Client::new(&config.rpc.celestia_rpc, auth_token).await?);
        let evm_provider =
            ProviderBuilder::new().connect_http(Url::parse(&config.rpc.evreth_rpc).context("invalid evm rpc url")?);

        Self::new(config, chain_spec, genesis, celestia_client, evm_provider, ism_client)
    }

    /// Converts the JSON genesis into a chain spec.
    pub fn load_chain_spec_from_genesis(genesis: &Genesis) -> Result<Arc<ChainSpec>> {
        let chain_spec: ChainSpec = genesis
            .try_into()
            .map_err(|e| anyhow!("Failed to convert genesis to chain spec: {e}"))?;

        Ok(Arc::new(chain_spec))
    }

    /// Exposes the underlying configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the namespace configured for Celestia blobs.
    pub fn namespace(&self) -> Namespace {
        self.config.namespace
    }

    /// Returns the sequencer public key bytes.
    pub fn pub_key_bytes(&self) -> Vec<u8> {
        hex::decode(&self.config.pub_key).expect("invalid sequencer pub key hex")
    }

    /// Returns the chain spec.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        Arc::clone(&self.chain_spec)
    }

    /// Returns the genesis configuration.
    pub fn genesis(&self) -> Genesis {
        self.genesis.clone()
    }

    /// Returns the Celestia RPC client (HTTP).
    pub fn celestia_client(&self) -> Arc<Client> {
        Arc::clone(&self.celestia_client)
    }

    /// Creates a new Celestia WebSocket client for subscriptions.
    pub async fn celestia_ws_client(&self) -> Result<Client> {
        let url = self.celestia_ws_url()?;
        let auth_token = self.config.rpc.celestia_auth_token.as_deref();
        Client::new(url.as_str(), auth_token).await.map_err(|e| anyhow!(e))
    }

    /// Returns the ISM client.
    pub fn ism_client(&self) -> Arc<CelestiaIsmClient> {
        Arc::clone(&self.ism_client)
    }

    /// Returns the configured ISM identifier.
    pub fn ism_id(&self) -> &str {
        &self.config.hyperlane.celestia.ism_id
    }

    /// Returns the Hyperlane mailbox contract address.
    pub fn mailbox_address(&self) -> Address {
        Address::from_str(&self.config.hyperlane.evm.mailbox_address).expect("invalid Hyperlane mailbox address")
    }

    /// Returns a new instance of the Hyperlane mailbox contract bound to the default provider.
    pub fn mailbox_contract(&self) -> MailboxContractInstance<DefaultProvider> {
        MailboxContract::new(self.mailbox_address(), self.evm_provider())
    }

    /// Returns the current mailbox nonce at the latest head.
    pub async fn mailbox_nonce(&self) -> Result<u32> {
        Ok(self.mailbox_contract().nonce().call().await?)
    }

    /// Returns the mailbox nonce at the specified block number.
    pub async fn mailbox_nonce_at(&self, block_number: u64) -> Result<u32> {
        Ok(self
            .mailbox_contract()
            .nonce()
            .call()
            .block(BlockId::Number(BlockNumberOrTag::Number(block_number)))
            .await?)
    }

    /// Returns the Hyperlane merkle tree contract address.
    pub fn merkle_tree_address(&self) -> Address {
        Address::from_str(&self.config.hyperlane.evm.merkle_tree_address)
            .expect("invalid Hyperlane merkle tree address")
    }

    /// Returns the configured EVM websocket endpoint.
    pub fn evm_ws_endpoint(&self) -> &str {
        &self.config.rpc.evreth_ws
    }

    /// Returns the configured EVM HTTP endpoint.
    pub fn evm_http_url(&self) -> &str {
        &self.config.rpc.evreth_rpc
    }

    /// Clones the prepared EVM provider.
    pub fn evm_provider(&self) -> DefaultProvider {
        self.evm_provider.clone()
    }

    /// Creates a state query provider backed by the default EVM provider.
    pub fn state_query_provider(&self) -> Arc<dyn StateQueryProvider> {
        Arc::new(MockStateQueryProvider::new(self.evm_provider()))
    }

    /// Creates the Hyperlane message indexer.
    pub fn hyperlane_indexer(&self) -> HyperlaneIndexer {
        let filter = Filter::new().address(self.mailbox_address()).event(&Dispatch::id());
        HyperlaneIndexer::new(filter)
    }

    /// Generates STF inputs for the configured chain at the requested block height.
    pub async fn generate_executor_input(&self, block_number: u64) -> Result<EthClientExecutorInput> {
        let host_executor = EthHostExecutor::eth(self.chain_spec(), None);
        let provider = self.evm_provider();
        let rpc_db = RpcDb::new(provider.clone(), block_number.saturating_sub(1));

        let executor_input = host_executor
            .execute(block_number, &rpc_db, &provider, self.genesis(), None, false)
            .await?;

        Ok(executor_input)
    }

    /// Queries the namespace for all blobs for the provided height.
    /// Iterates blobs in reverse order attempting to decode the payload to a SignedData.
    /// Returns the block height on the associated SignedData metadata.
    pub async fn latest_block_for_height(&self, height: u64) -> Result<Option<u64>> {
        let blobs: Vec<Blob> = self
            .celestia_client()
            .blob_get_all(height, &[self.namespace()])
            .await?
            .unwrap_or_default();

        if blobs.is_empty() {
            return Ok(None);
        }

        for blob in blobs.iter().rev() {
            if let Ok(signed_data) = SignedData::decode(blob.data.as_slice()) {
                if let Some(data) = signed_data.data {
                    if let Some(metadata) = data.metadata {
                        return Ok(Some(metadata.height));
                    }
                }
            }
        }

        Ok(None)
    }

    fn celestia_ws_url(&self) -> Result<Url> {
        let mut url = Url::parse(&self.config.rpc.celestia_rpc).context("invalid celestia rpc url")?;
        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            other => other,
        }
        .to_string();
        url.set_scheme(&scheme)
            .map_err(|_| anyhow!("unsupported Celestia RPC scheme: {}", url.scheme()))?;
        Ok(url)
    }
}
