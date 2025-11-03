use std::env;
use std::sync::Arc;

use alloy_primitives::Address;
use alloy_primitives::FixedBytes;
use alloy_provider::ProviderBuilder;
use anyhow::Result;
use celestia_grpc_client::types::ClientConfig;
use celestia_grpc_client::{CelestiaIsmClient, QueryIsmRequest};
use ev_state_queries::{DefaultProvider, MockStateQueryProvider};
use ev_types::v1::get_block_request::Identifier;
use ev_types::v1::store_service_client::StoreServiceClient;
use ev_types::v1::GetBlockRequest;
use reqwest::Url;
use std::str::FromStr;
use storage::hyperlane::message::HyperlaneMessageStore;
use storage::hyperlane::snapshot::HyperlaneSnapshotStore;
use storage::proofs::RocksDbProofStorage;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::try_join;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server as TonicServer;
use tonic_reflection::server::Builder as ReflectionBuilder;
use tracing::{debug, error};

use crate::config::Config;
use crate::proto::celestia::prover::v1::prover_server::ProverServer;
use crate::prover::programs::block::TrustedState;
use crate::prover::programs::message::HyperlaneMessageProver;
use crate::prover::service::ProverService;
use crate::prover::{MessageProofRequest, MessageProofSync};

#[cfg(not(feature = "combined"))]
use crate::prover::{
    programs::{
        block::{AppContext, BlockExecProver},
        range::{BlockRangeExecProver, BlockRangeExecService},
    },
    BlockProofCommitted,
};

#[cfg(feature = "combined")]
use crate::prover::programs::combined::{AppContext as CombinedAppContext, EvCombinedProver};
use crate::prover::programs::message::AppContext as MessageAppContext;
use storage::proofs::ProofStorage;

struct Server {
    pub message_prover: Arc<HyperlaneMessageProver>,
    #[cfg(not(feature = "combined"))]
    pub block_prover: Arc<BlockExecProver>,
    #[cfg(not(feature = "combined"))]
    pub block_range_prover: Arc<BlockRangeExecProver>,
    #[cfg(feature = "combined")]
    pub combined_prover: Arc<EvCombinedProver>,
}

impl Server {
    pub fn new(
        message_prover: Arc<HyperlaneMessageProver>,
        #[cfg(not(feature = "combined"))] block_prover: Arc<BlockExecProver>,
        #[cfg(not(feature = "combined"))] block_range_prover: Arc<BlockRangeExecProver>,
        #[cfg(feature = "combined")] combined_prover: Arc<EvCombinedProver>,
    ) -> Self {
        Self {
            message_prover,
            #[cfg(not(feature = "combined"))]
            block_prover,
            #[cfg(not(feature = "combined"))]
            block_range_prover,
            #[cfg(feature = "combined")]
            combined_prover,
        }
    }
    pub async fn start_message_prover(
        &self,
        rx_range: mpsc::Receiver<MessageProofRequest>,
        ism_client: Arc<CelestiaIsmClient>,
        message_sync: Arc<MessageProofSync>,
    ) -> Result<JoinHandle<()>> {
        let message_prover = Arc::clone(&self.message_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = message_prover.run(rx_range, ism_client, message_sync).await {
                error!("Message prover task failed: {e:?}");
            }
        }))
    }
    #[cfg(not(feature = "combined"))]
    pub async fn start_block_prover(&self) -> Result<JoinHandle<()>> {
        let block_prover = Arc::clone(&self.block_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = block_prover.run().await {
                error!("Block prover task failed: {e:?}");
            }
        }))
    }
    #[cfg(not(feature = "combined"))]
    pub async fn start_block_range_prover(
        self,
        client: CelestiaIsmClient,
        storage: Arc<dyn ProofStorage>,
        rx_block: mpsc::Receiver<BlockProofCommitted>,
        tx_range: mpsc::Sender<MessageProofRequest>,
        batch_size: usize,
    ) -> Result<JoinHandle<()>> {
        Ok(tokio::spawn(async move {
            match BlockRangeExecService::new(
                client,
                self.block_range_prover.clone(),
                storage.clone(),
                rx_block,
                tx_range,
                batch_size,
                16,
            )
            .await
            {
                Ok(service) => {
                    if let Err(e) = service.run().await {
                        error!("Block range prover task failed: {e:?}");
                    }
                }
                Err(e) => {
                    error!("Failed to create BlockRangeExecService: {e:?}");
                }
            }
        }))
    }
    #[cfg(feature = "combined")]
    pub async fn start_combined_prover(&self, message_sync: Arc<MessageProofSync>) -> Result<JoinHandle<()>> {
        let combined_prover = Arc::clone(&self.combined_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = combined_prover.run(message_sync).await {
                error!("Combined prover task failed: {e:?}");
            }
        }))
    }
}

pub async fn start_server(config: Config) -> Result<()> {
    let listener = TcpListener::bind(config.grpc_address.clone()).await?;
    let sequencer_rpc_url = std::env::var("SEQUENCER_RPC_URL").expect("SEQUENCER_RPC_URL must be set");
    let reth_rpc_url = std::env::var("RETH_RPC_URL").expect("RETH_RPC_URL must be set");
    let reth_ws_url = std::env::var("RETH_WS_URL").expect("RETH_WS_URL must be set");
    let descriptor_bytes = include_bytes!("../../src/proto/descriptor.bin");
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(descriptor_bytes)
        .build()
        .unwrap();
    // TODO: Remove this config cloning when we can rely on the public key from config
    // https://github.com/evstack/ev-node/issues/2603
    let mut config_clone = config.clone();
    config_clone.pub_key = public_key(sequencer_rpc_url).await?;
    debug!("Successfully got pubkey from evnode: {}", config_clone.pub_key);
    let client_config = ClientConfig::from_env()?;
    let client = CelestiaIsmClient::new(client_config).await?;
    let trusted_state = get_trusted_state(&client).await?;
    debug!("Successfully got trusted state from ism: {}", trusted_state);
    // Initialize RocksDB storage in the default data directory
    let storage_path = Config::storage_path().join("proofs.db");
    let storage = Arc::new(RocksDbProofStorage::new(storage_path)?);
    // shared resources
    let config = ClientConfig::from_env()?;
    let ism_client = Arc::new(CelestiaIsmClient::new(config).await?);
    let (tx_range, rx_range) = mpsc::channel::<MessageProofRequest>(256);
    let message_sync = MessageProofSync::shared();
    #[cfg(not(feature = "combined"))]
    {
        let batch_size = config_clone.batch_size;
        let concurrency = config_clone.concurrency;
        let queue_capacity = config_clone.queue_capacity;
        let (tx_block, rx_block) = mpsc::channel::<BlockProofCommitted>(256);
        let block_prover = BlockExecProver::new(
            AppContext::new(config_clone.clone(), trusted_state)?,
            tx_block,
            storage.clone(),
            queue_capacity,
            concurrency,
        );
        let block_range_prover = BlockRangeExecProver::new()?;
        let message_prover = prepare_message_prover(reth_rpc_url, reth_ws_url, storage.clone())?;
        let server = Server::new(
            Arc::new(message_prover),
            Arc::new(block_prover),
            Arc::new(block_range_prover),
        );
        let block_handle = server.start_block_prover().await?;
        let message_handle = server.start_message_prover(rx_range, ism_client, message_sync).await?;
        let block_range_handle = server
            .start_block_range_prover(client, storage.clone(), rx_block, tx_range, batch_size)
            .await?;
        let result = try_join!(block_handle, message_handle, block_range_handle);
        result?;
    }

    #[cfg(feature = "combined")]
    {
        let combined_context = CombinedAppContext::from_config(&config_clone, Arc::clone(&ism_client)).await?;
        let combined_prover = EvCombinedProver::new(combined_context, tx_range.clone())?;
        let message_prover = prepare_message_prover(reth_rpc_url, reth_ws_url, storage.clone())?;
        let server = Server::new(Arc::new(message_prover), Arc::new(combined_prover));
        let combined_handle = server.start_combined_prover(message_sync.clone()).await?;
        let message_handle = server.start_message_prover(rx_range, ism_client, message_sync).await?;
        let result = try_join!(combined_handle, message_handle);
        result?;
    }

    let prover_service = ProverService::new(storage)?;

    TonicServer::builder()
        .add_service(reflection_service)
        .add_service(ProverServer::new(prover_service))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await?;

    Ok(())
}

fn prepare_message_prover(
    reth_rpc_url: String,
    reth_ws_url: String,
    storage: Arc<dyn ProofStorage>,
) -> Result<HyperlaneMessageProver> {
    let ism_id = env::var("CELESTIA_ISM_ID").expect("CELESTIA_ISM_ID must be set");
    let mailbox_address = env::var("MAILBOX_ADDRESS").expect("MAILBOX_ADDRESS must be set");
    let celestia_mailbox_address = env::var("CELESTIA_MAILBOX_ADDRESS").expect("CELESTIA_MAILBOX_ADDRESS must be set");
    let merkle_tree_address = env::var("MERKLE_TREE_ADDRESS").expect("MERKLE_TREE_ADDRESS must be set");
    let message_storage_path = Config::storage_path().join("messages.db");
    let snapshot_storage_path = Config::storage_path().join("snapshots.db");
    let hyperlane_message_store = Arc::new(HyperlaneMessageStore::new(message_storage_path).unwrap());
    let hyperlane_snapshot_store = Arc::new(HyperlaneSnapshotStore::new(snapshot_storage_path, None).unwrap());
    let ctx = MessageAppContext {
        evm_rpc: reth_rpc_url.clone(),
        evm_ws: reth_ws_url,
        mailbox_address: Address::from_str(&mailbox_address).unwrap(),
        celestia_mailbox_address,
        merkle_tree_address: Address::from_str(&merkle_tree_address).unwrap(),
        ism_id,
    };
    let evm_provider: DefaultProvider = ProviderBuilder::new().connect_http(Url::from_str(&reth_rpc_url).unwrap());
    Ok(HyperlaneMessageProver::new(
        ctx,
        hyperlane_message_store,
        hyperlane_snapshot_store,
        storage.clone(),
        Arc::new(MockStateQueryProvider::new(evm_provider)),
    )?)
}

// TODO: Use from config file when we can have a reproducible key in docker-compose.
// For now query the pubkey on startup from evnode.
// https://github.com/evstack/ev-node/issues/2603
pub async fn public_key(sequencer_rpc_url: String) -> Result<String> {
    let mut sequencer_client = StoreServiceClient::connect(sequencer_rpc_url).await?;
    let block_req = GetBlockRequest {
        identifier: Some(Identifier::Height(1)),
    };
    let resp = sequencer_client.get_block(block_req).await?;
    let pub_key = resp.into_inner().block.unwrap().header.unwrap().signer.unwrap().pub_key;
    Ok(hex::encode(&pub_key[4..]))
}

pub async fn get_trusted_state(client: &CelestiaIsmClient) -> Result<TrustedState> {
    let resp = client
        .ism(QueryIsmRequest {
            id: client.ism_id().to_string(),
        })
        .await?;

    let ism = resp.ism.unwrap();

    Ok(TrustedState::new(ism.height, FixedBytes::from_slice(&ism.state_root)))
}
