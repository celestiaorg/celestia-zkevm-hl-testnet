use axum::{
    extract::{Path, Query, State as AxumState},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use ev_zkevm_types::programs::hyperlane::tree::MerkleTree;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use storage::hyperlane::snapshot::HyperlaneSnapshotStore;
use storage::proofs::ProofStorage;
use tracing::error;

mod hex_serde {
    use serde::Serializer;

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }
}

#[derive(Clone)]
pub(crate) struct HttpServerState {
    pub(crate) snapshot_store: Arc<HyperlaneSnapshotStore>,
    pub(crate) proof_store: Arc<dyn ProofStorage>,
}

#[derive(Serialize)]
pub struct SnapshotInfo {
    pub index: u64,
    pub height: u64,
    pub finalized: bool,
    pub tree_count: usize,
    pub tree_root: String,
}

#[derive(Serialize)]
pub struct BlockProofResponse {
    pub celestia_height: u64,
    #[serde(with = "hex_serde")]
    pub proof_data: Vec<u8>,
    #[serde(with = "hex_serde")]
    pub public_values: Vec<u8>,
    pub created_at: u64,
}

#[derive(Serialize)]
pub struct MembershipProofResponse {
    #[serde(with = "hex_serde")]
    pub proof_data: Vec<u8>,
    #[serde(with = "hex_serde")]
    pub public_values: Vec<u8>,
    pub created_at: u64,
}

#[derive(Serialize)]
pub struct RangeProofResponse {
    pub start_height: u64,
    pub end_height: u64,
    #[serde(with = "hex_serde")]
    pub proof_data: Vec<u8>,
    #[serde(with = "hex_serde")]
    pub public_values: Vec<u8>,
    pub created_at: u64,
}

#[derive(Deserialize)]
pub struct RangeQuery {
    pub start: u64,
    pub end: u64,
}

pub(crate) async fn list_snapshots_handler(
    AxumState(state): AxumState<HttpServerState>,
) -> Result<Json<Vec<SnapshotInfo>>, String> {
    match state.snapshot_store.list_all_snapshots().await {
        Ok(snapshots) => {
            let zero_hashes = MerkleTree::zero_hashes();
            let snapshot_infos: Vec<SnapshotInfo> = snapshots
                .into_iter()
                .map(|(index, snapshot)| {
                    let tree_root = snapshot
                        .tree
                        .root_with_ctx(&zero_hashes)
                        .unwrap_or_else(|_| "error".to_string());
                    SnapshotInfo {
                        index,
                        height: snapshot.height,
                        finalized: snapshot.finalized,
                        tree_count: snapshot.tree.count as usize,
                        tree_root,
                    }
                })
                .collect();
            Ok(Json(snapshot_infos))
        }
        Err(e) => {
            error!("Failed to list snapshots: {e:?}");
            Err(format!("Failed to list snapshots: {e}"))
        }
    }
}

pub(crate) async fn get_latest_block_proof_handler(
    AxumState(state): AxumState<HttpServerState>,
) -> Result<Json<BlockProofResponse>, (StatusCode, String)> {
    match state.proof_store.get_latest_block_proof().await {
        Ok(Some(proof)) => Ok(Json(BlockProofResponse {
            celestia_height: proof.celestia_height,
            proof_data: proof.proof_data,
            public_values: proof.public_values,
            created_at: proof.created_at,
        })),
        Ok(None) => Err((StatusCode::NOT_FOUND, "No block proofs found in storage".to_string())),
        Err(e) => {
            error!("Failed to get latest block proof: {e:?}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get latest block proof: {e}"),
            ))
        }
    }
}

pub(crate) async fn get_block_proof_handler(
    AxumState(state): AxumState<HttpServerState>,
    Path(height): Path<u64>,
) -> Result<Json<BlockProofResponse>, (StatusCode, String)> {
    match state.proof_store.get_block_proof(height).await {
        Ok(proof) => Ok(Json(BlockProofResponse {
            celestia_height: proof.celestia_height,
            proof_data: proof.proof_data,
            public_values: proof.public_values,
            created_at: proof.created_at,
        })),
        Err(e) => {
            error!("Failed to get block proof for height {height}: {e:?}");
            Err((StatusCode::NOT_FOUND, format!("Block proof not found: {e}")))
        }
    }
}

pub(crate) async fn get_block_proofs_in_range_handler(
    AxumState(state): AxumState<HttpServerState>,
    Query(range): Query<RangeQuery>,
) -> Result<Json<Vec<BlockProofResponse>>, (StatusCode, String)> {
    match state
        .proof_store
        .get_block_proofs_in_range(range.start, range.end)
        .await
    {
        Ok(proofs) => {
            let responses = proofs
                .into_iter()
                .map(|p| BlockProofResponse {
                    celestia_height: p.celestia_height,
                    proof_data: p.proof_data,
                    public_values: p.public_values,
                    created_at: p.created_at,
                })
                .collect();
            Ok(Json(responses))
        }
        Err(e) => {
            error!("Failed to get block proofs in range: {e:?}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get block proofs: {e}"),
            ))
        }
    }
}

pub(crate) async fn get_latest_membership_proof_handler(
    AxumState(state): AxumState<HttpServerState>,
) -> Result<Json<MembershipProofResponse>, (StatusCode, String)> {
    match state.proof_store.get_latest_membership_proof().await {
        Ok(Some(proof)) => Ok(Json(MembershipProofResponse {
            proof_data: proof.proof_data,
            public_values: proof.public_values,
            created_at: proof.created_at,
        })),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "No membership proofs found in storage".to_string(),
        )),
        Err(e) => {
            error!("Failed to get latest membership proof: {e:?}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get latest membership proof: {e}"),
            ))
        }
    }
}

pub(crate) async fn get_membership_proof_handler(
    AxumState(state): AxumState<HttpServerState>,
    Path(height): Path<u64>,
) -> Result<Json<MembershipProofResponse>, (StatusCode, String)> {
    match state.proof_store.get_membership_proof(height).await {
        Ok(proof) => Ok(Json(MembershipProofResponse {
            proof_data: proof.proof_data,
            public_values: proof.public_values,
            created_at: proof.created_at,
        })),
        Err(e) => {
            error!("Failed to get membership proof for height {height}: {e:?}");
            Err((StatusCode::NOT_FOUND, format!("Membership proof not found: {e}")))
        }
    }
}

pub(crate) async fn get_range_proofs_handler(
    AxumState(state): AxumState<HttpServerState>,
    Query(range): Query<RangeQuery>,
) -> Result<Json<Vec<RangeProofResponse>>, (StatusCode, String)> {
    match state.proof_store.get_range_proofs(range.start, range.end).await {
        Ok(proofs) => {
            let responses = proofs
                .into_iter()
                .map(|p| RangeProofResponse {
                    start_height: p.start_height,
                    end_height: p.end_height,
                    proof_data: p.proof_data,
                    public_values: p.public_values,
                    created_at: p.created_at,
                })
                .collect();
            Ok(Json(responses))
        }
        Err(e) => {
            error!("Failed to get range proofs: {e:?}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get range proofs: {e}"),
            ))
        }
    }
}

pub(crate) async fn get_router(http_state: HttpServerState) -> Router {
    Router::new()
        // Snapshot routes
        .route("/snapshots", get(list_snapshots_handler))
        // Block proof routes
        .route("/proofs/block/latest", get(get_latest_block_proof_handler))
        .route("/proofs/block/:height", get(get_block_proof_handler))
        .route("/proofs/block/range", get(get_block_proofs_in_range_handler))
        // Membership proof routes
        .route("/proofs/membership/latest", get(get_latest_membership_proof_handler))
        .route("/proofs/membership/:height", get(get_membership_proof_handler))
        // Range proof routes
        .route("/proofs/range", get(get_range_proofs_handler))
        .with_state(http_state)
}
