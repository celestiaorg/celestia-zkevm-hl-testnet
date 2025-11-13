use std::{str::FromStr, sync::Arc};

use alloy_primitives::Address;
use alloy_provider::ProviderBuilder;
use celestia_grpc_client::{types::ClientConfig, CelestiaIsmClient};
use ev_prover::prover::{
    programs::message::{AppContext, HyperlaneMessageProver},
    MessageProofSync,
};
use ev_state_queries::{DefaultProvider, MockStateQueryProvider};
use reqwest::Url;
use storage::{
    hyperlane::{message::HyperlaneMessageStore, snapshot::HyperlaneSnapshotStore},
    proofs::RocksDbProofStorage,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[tokio::test]
async fn test_run_message_prover() {
    dotenvy::dotenv().ok();
    let ism_id = std::env::var("CELESTIA_ISM_ID").expect("CELESTIA_ISM_ID must be set");
    let mailbox_address = std::env::var("MAILBOX_ADDRESS").expect("MAILBOX_ADDRESS must be set");
    let celestia_mailbox_address =
        std::env::var("CELESTIA_MAILBOX_ADDRESS").expect("CELESTIA_MAILBOX_ADDRESS must be set");
    let merkle_tree_address = std::env::var("MERKLE_TREE_ADDRESS").expect("MERKLE_TREE_ADDRESS must be set");
    let reth_rpc_url = std::env::var("RETH_RPC_URL").expect("RETH_RPC_URL must be set");
    let reth_ws_url = std::env::var("RETH_WS_URL").expect("RETH_WS_URL must be set");
    let config = ClientConfig::from_env().unwrap();
    let ism_client = Arc::new(CelestiaIsmClient::new(config).await.unwrap());
    // Configure logging for ev-prover
    let filter = EnvFilter::new("ev-prover=debug,sp1_core=warn,sp1_runtime=warn,sp1_sdk=warn,sp1_vm=warn");
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let tmp = TempDir::new().expect("cannot create temp directory");
    let snapshot_storage_path = dirs::home_dir()
        .expect("cannot find home directory")
        .join(&tmp)
        .join("data")
        .join("snapshots.db");
    let message_storage_path = dirs::home_dir()
        .expect("cannot find home directory")
        .join(&tmp)
        .join("data")
        .join("messages.db");
    let proof_storage_path = dirs::home_dir()
        .expect("cannot find home directory")
        .join(&tmp)
        .join("data")
        .join("proofs.db");
    let hyperlane_message_store = Arc::new(HyperlaneMessageStore::new(message_storage_path).unwrap());
    let hyperlane_snapshot_store = Arc::new(HyperlaneSnapshotStore::new(snapshot_storage_path, None).unwrap());
    let proof_store = Arc::new(RocksDbProofStorage::new(proof_storage_path).unwrap());

    hyperlane_message_store.reset_db().unwrap();
    hyperlane_snapshot_store.reset_db().unwrap();

    let app = AppContext {
        evm_rpc: reth_rpc_url.clone(),
        evm_ws: reth_ws_url,
        mailbox_address: Address::from_str(&mailbox_address).unwrap(),
        celestia_mailbox_address,
        merkle_tree_address: Address::from_str(&merkle_tree_address).unwrap(),
        ism_id,
    };

    let evm_provider: DefaultProvider = ProviderBuilder::new().connect_http(Url::from_str(&reth_rpc_url).unwrap());

    let (_tx, rx) = mpsc::channel(256);
    let prover = Arc::new(
        HyperlaneMessageProver::new(
            app,
            hyperlane_message_store,
            hyperlane_snapshot_store,
            proof_store,
            Arc::new(MockStateQueryProvider::new(evm_provider)),
        )
        .unwrap(),
    );
    prover.run(rx, ism_client, MessageProofSync::shared()).await.unwrap();
}
