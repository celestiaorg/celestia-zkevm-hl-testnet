mod queries;
use alloy_primitives::FixedBytes;
use anyhow::Result;
use celestia_grpc_client::types::ClientConfig;
use celestia_grpc_client::{CelestiaIsmClient, QueryIsmRequest};
use ev_state_queries::MockStateQueryProvider;
use ev_types::v1::get_block_request::Identifier;
use ev_types::v1::store_service_client::StoreServiceClient;
use ev_types::v1::GetBlockRequest;
use ev_zkevm_types::programs::block::State;
use std::sync::Arc;
use storage::hyperlane::message::HyperlaneMessageStore;
use storage::hyperlane::snapshot::HyperlaneSnapshotStore;
use storage::proofs::RocksDbProofStorage;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::prover::chain::ChainContext;
use crate::prover::programs::block::TrustedState;
use crate::prover::programs::message::HyperlaneMessageProver;
use crate::prover::{MessageProofRequest, MessageProofSync};

#[cfg(not(feature = "batch_mode"))]
use crate::prover::{
    programs::{
        block::BlockExecProver,
        range::{BlockRangeExecProver, BlockRangeExecService},
    },
    BlockProofCommitted,
};
use crate::server::queries::{get_router, HttpServerState};

use storage::proofs::ProofStorage;
#[cfg(feature = "batch_mode")]
use {crate::prover::programs::batch::BatchExecProver, std::time::Duration};

struct Server {
    pub message_prover: Arc<HyperlaneMessageProver>,
    #[cfg(not(feature = "batch_mode"))]
    pub block_prover: Arc<BlockExecProver>,
    #[cfg(not(feature = "batch_mode"))]
    pub block_range_prover: Arc<BlockRangeExecProver>,
    #[cfg(feature = "batch_mode")]
    pub batch_prover: Arc<BatchExecProver>,
}

impl Server {
    pub fn new(
        message_prover: Arc<HyperlaneMessageProver>,
        #[cfg(not(feature = "batch_mode"))] block_prover: Arc<BlockExecProver>,
        #[cfg(not(feature = "batch_mode"))] block_range_prover: Arc<BlockRangeExecProver>,
        #[cfg(feature = "batch_mode")] batch_prover: Arc<BatchExecProver>,
    ) -> Self {
        Self {
            message_prover,
            #[cfg(not(feature = "batch_mode"))]
            block_prover,
            #[cfg(not(feature = "batch_mode"))]
            block_range_prover,
            #[cfg(feature = "batch_mode")]
            batch_prover: batch_prover,
        }
    }
    pub async fn start_message_prover(
        &self,
        rx_range: mpsc::Receiver<MessageProofRequest>,
        message_sync: Arc<MessageProofSync>,
    ) -> Result<JoinHandle<()>> {
        let message_prover = Arc::clone(&self.message_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = message_prover.run(rx_range, message_sync).await {
                error!("Message prover task failed: {e:?}");
            }
        }))
    }
    #[cfg(not(feature = "batch_mode"))]
    pub async fn start_block_prover(&self) -> Result<JoinHandle<()>> {
        let block_prover = Arc::clone(&self.block_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = block_prover.run().await {
                error!("Block prover task failed: {e:?}");
            }
        }))
    }
    #[cfg(not(feature = "batch_mode"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn start_block_range_prover(
        self,
        ctx: Arc<ChainContext>,
        client: CelestiaIsmClient,
        proof_store: Arc<dyn ProofStorage>,
        hyperlane_message_store: Arc<HyperlaneMessageStore>,
        rx_block: mpsc::Receiver<BlockProofCommitted>,
        tx_range: mpsc::Sender<MessageProofRequest>,
        batch_size: usize,
    ) -> Result<JoinHandle<()>> {
        Ok(tokio::spawn(async move {
            match BlockRangeExecService::new(
                ctx.clone(),
                client,
                self.block_range_prover.clone(),
                proof_store.clone(),
                hyperlane_message_store.clone(),
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
    #[cfg(feature = "batch_mode")]
    pub async fn start_batch_prover(&self, message_sync: Arc<MessageProofSync>) -> Result<JoinHandle<()>> {
        let batch_prover = Arc::clone(&self.batch_prover);
        Ok(tokio::spawn(async move {
            if let Err(e) = batch_prover.run(message_sync).await {
                error!("Batch prover task failed: {e:?}");
            }
        }))
    }
}

pub async fn start_server(config: Config) -> Result<()> {
    let sequencer_rpc_url = std::env::var("SEQUENCER_RPC_URL").expect("SEQUENCER_RPC_URL must be set");
    // TODO: Remove this config cloning when we can rely on the public key from config
    // querys://github.com/evstack/ev-node/issues/2603
    let mut config_clone = config.clone();
    config_clone.pub_key = public_key(sequencer_rpc_url).await?;
    debug!("Successfully got pubkey from evnode: {}", config_clone.pub_key);
    // Initialize RocksDB storage in the default data directory
    let storage_path = Config::storage_path();
    let proof_store = Arc::new(RocksDbProofStorage::new(&storage_path)?);
    let hyperlane_message_store = Arc::new(HyperlaneMessageStore::from_path(&storage_path).await.unwrap());
    let hyperlane_snapshot_store = Arc::new(HyperlaneSnapshotStore::from_path(&storage_path).await?);
    // shared resources
    let config = ClientConfig::from_env()?;
    let ism_client = Arc::new(CelestiaIsmClient::new(config).await?);
    let ctx = ChainContext::from_config(config_clone.clone(), ism_client.clone()).await?;

    #[cfg(not(feature = "batch_mode"))]
    let wrapper_task = Some({
        let client_config = ClientConfig::from_env()?;
        let client = CelestiaIsmClient::new(client_config).await?;
        let proof_store = proof_store.clone();
        let hyperlane_snapshot_store = hyperlane_snapshot_store.clone();
        tokio::spawn(async move {
            loop {
                let trusted_state = match get_trusted_state(&client).await {
                    Ok(state) => state,
                    Err(e) => {
                        error!("Failed to get trusted state: {e:?}");
                        continue;
                    }
                };
                debug!("Successfully got trusted state from ism: {}", trusted_state);
                let message_sync = MessageProofSync::shared();
                let batch_size = config_clone.batch_size;
                let concurrency = config_clone.concurrency;
                let queue_capacity = config_clone.queue_capacity;
                let (tx_range, rx_range) = mpsc::channel::<MessageProofRequest>(256);
                let (tx_block, rx_block) = mpsc::channel::<BlockProofCommitted>(256);

                let block_prover = BlockExecProver::new(
                    ctx.clone(),
                    trusted_state,
                    tx_block,
                    proof_store.clone(),
                    queue_capacity,
                    concurrency,
                );
                let block_range_prover = match BlockRangeExecProver::new(ctx.clone()) {
                    Ok(prover) => prover,
                    Err(e) => {
                        error!("Failed to create block range prover: {e:?}");
                        continue;
                    }
                };
                let message_prover = match prepare_message_prover(
                    ctx.clone(),
                    hyperlane_message_store.clone(),
                    hyperlane_snapshot_store.clone(),
                    proof_store.clone(),
                )
                .await
                {
                    Ok(prover) => prover,
                    Err(e) => {
                        error!("Failed to create message prover: {e:?}");
                        continue;
                    }
                };
                let server = Server::new(
                    Arc::new(message_prover),
                    Arc::new(block_prover),
                    Arc::new(block_range_prover),
                );
                let mut block_handle = match server.start_block_prover().await {
                    Ok(handle) => handle,
                    Err(e) => {
                        error!("Failed to start block prover: {e:?}");
                        continue;
                    }
                };
                let mut message_handle = match server.start_message_prover(rx_range, message_sync).await {
                    Ok(handle) => handle,
                    Err(e) => {
                        error!("Failed to start message prover: {e:?}");
                        continue;
                    }
                };
                let mut block_range_handle = match server
                    .start_block_range_prover(
                        ctx.clone(),
                        client.clone(),
                        proof_store.clone(),
                        hyperlane_message_store.clone(),
                        rx_block,
                        tx_range,
                        batch_size,
                    )
                    .await
                {
                    Ok(handle) => handle,
                    Err(e) => {
                        error!("Failed to start block range prover: {e:?}");
                        continue;
                    }
                };
                tokio::select! {
                    r = &mut block_handle => {
                        error!("Block prover stopped: {:?}", r);
                        message_handle.abort();
                        block_range_handle.abort();
                    }
                    r = &mut message_handle => {
                        error!("Message prover stopped: {:?}", r);
                        block_handle.abort();
                        block_range_handle.abort();
                    }
                    r = &mut block_range_handle => {
                        error!("Block range prover stopped: {:?}", r);
                        block_handle.abort();
                        message_handle.abort();
                    }
                }
            }
        })
    });

    #[cfg(feature = "batch_mode")]
    let wrapper_task = Some({
        let proof_store_clone: Arc<dyn ProofStorage> = proof_store.clone();
        let message_sync = MessageProofSync::shared();
        let hyperlane_snapshot_store = hyperlane_snapshot_store.clone();

        tokio::spawn(async move {
            loop {
                let (tx_range, rx_range) = mpsc::channel::<MessageProofRequest>(256);
                let batch_prover = match BatchExecProver::new(ctx.clone(), tx_range, hyperlane_message_store.clone()) {
                    Ok(prover) => prover,
                    Err(e) => {
                        error!("Failed to create batch prover: {e:?}");
                        continue;
                    }
                };
                let message_prover = match prepare_message_prover(
                    ctx.clone(),
                    hyperlane_message_store.clone(),
                    hyperlane_snapshot_store.clone(),
                    proof_store_clone.clone(),
                )
                .await
                {
                    Ok(prover) => prover,
                    Err(e) => {
                        error!("Failed to create message prover: {e:?}");
                        continue;
                    }
                };
                let server = Arc::new(Server::new(Arc::new(message_prover), Arc::new(batch_prover)));

                let mut batch_handle = match server.start_batch_prover(Arc::clone(&message_sync)).await {
                    Ok(handle) => handle,
                    Err(e) => {
                        error!("Failed to start batch prover: {e:?}");
                        continue;
                    }
                };
                let mut message_handle = match server.start_message_prover(rx_range, Arc::clone(&message_sync)).await {
                    Ok(handle) => handle,
                    Err(e) => {
                        error!("Failed to start message prover: {e:?}");
                        continue;
                    }
                };

                tokio::select! {
                    r = &mut batch_handle => {
                        error!("Batch prover stopped: {:?}", r);
                        message_handle.abort();
                    }
                    r = &mut message_handle => {
                        error!("Message prover stopped: {:?}", r);
                        batch_handle.abort();
                    }
                }
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
        })
    });

    // Create HTTP server for queries
    let http_listener = TcpListener::bind("0.0.0.0:9222").await?;
    let http_addr = http_listener.local_addr()?;
    info!("Starting HTTP server at {}", http_addr);

    let http_state = HttpServerState {
        snapshot_store: hyperlane_snapshot_store.clone(),
        proof_store: proof_store.clone(),
    };

    let app = get_router(http_state).await;

    let http_server_task = tokio::spawn(async move {
        axum::serve(http_listener, app)
            .await
            .map_err(|e| anyhow::anyhow!("HTTP server error: {e}"))
    });

    if let Some(mut wrapper_task) = wrapper_task {
        let mut http_server_task = http_server_task;
        tokio::select! {
            r = &mut wrapper_task => {
                error!("Prover wrapper task stopped: {:?}", r);
                http_server_task.abort();
            }
            r = &mut http_server_task => {
                match r {
                    Ok(Ok(())) => debug!("HTTP server stopped gracefully"),
                    Ok(Err(e)) => error!("HTTP server failed: {e:?}"),
                    Err(e) => error!("HTTP server task panicked: {e:?}"),
                }
                wrapper_task.abort();
            }
        }
    } else {
        panic!("Prover service did not start as expected, no wrapper task found");
    }

    Ok(())
}

async fn prepare_message_prover(
    ctx: Arc<ChainContext>,
    hyperlane_message_store: Arc<HyperlaneMessageStore>,
    hyperlane_snapshot_store: Arc<HyperlaneSnapshotStore>,
    proof_store: Arc<dyn ProofStorage>,
) -> Result<HyperlaneMessageProver> {
    // Run the blocking prover setup in a separate thread to avoid blocking the async runtime
    tokio::task::spawn_blocking(move || {
        HyperlaneMessageProver::new(
            ctx.clone(),
            hyperlane_message_store,
            hyperlane_snapshot_store,
            proof_store.clone(),
            Arc::new(MockStateQueryProvider::new(ctx.evm_provider())),
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task join error: {e}"))?
}

// TODO: Use from config file when we can have a reproducible key in docker-compose.
// For now query the pubkey on startup from evnode.
// querys://github.com/evstack/ev-node/issues/2603
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

    let state: State = bincode::deserialize(&ism.state).unwrap();
    Ok(TrustedState::new(
        state.height,
        FixedBytes::from_slice(&state.state_root),
    ))
}
