use std::sync::Arc;

use celestia_grpc_client::{types::ClientConfig, CelestiaIsmClient};
use ev_prover::{
    config::Config,
    prover::{chain::ChainContext, programs::message::HyperlaneMessageProver, MessageProofSync},
};
use ev_state_queries::MockStateQueryProvider;
use storage::{
    hyperlane::{message::HyperlaneMessageStore, snapshot::HyperlaneSnapshotStore},
    proofs::RocksDbProofStorage,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

// TODO: This test needs to be revisted as it seems to have been broken prior to this PR.
#[tokio::test]
#[ignore]
async fn test_run_message_prover() {
    dotenvy::dotenv().ok();

    let config = Config::default();
    let ism_client = Arc::new(CelestiaIsmClient::new(ClientConfig::from_env().unwrap()).await.unwrap());

    // Configure logging for ev-prover
    let filter = EnvFilter::new("ev_prover=debug,sp1_core=warn,sp1_runtime=warn,sp1_sdk=warn,sp1_vm=warn");
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let tmp = TempDir::new().expect("cannot create temp directory");
    let storage_path = tmp.path();

    let hyperlane_message_store = Arc::new(HyperlaneMessageStore::from_path(storage_path).unwrap());
    let hyperlane_snapshot_store = Arc::new(HyperlaneSnapshotStore::new(storage_path, None).unwrap());
    let proof_store = Arc::new(RocksDbProofStorage::new(storage_path).unwrap());

    hyperlane_message_store.reset_db().unwrap();
    hyperlane_snapshot_store.reset_db().unwrap();

    let ctx = ChainContext::from_config(config, ism_client.clone()).await.unwrap();

    let (_tx, rx) = mpsc::channel(256);
    let prover = Arc::new(
        HyperlaneMessageProver::new(
            ctx.clone(),
            hyperlane_message_store,
            hyperlane_snapshot_store,
            proof_store,
            Arc::new(MockStateQueryProvider::new(ctx.evm_provider())),
        )
        .unwrap(),
    );
    prover.run(rx, MessageProofSync::shared()).await.unwrap();
}
