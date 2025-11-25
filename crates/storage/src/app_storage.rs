use anyhow::Result;
use ev_zkevm_types::programs::hyperlane::tree::MerkleTree;
use std::path::Path;
use std::sync::Arc;

use crate::db::UnifiedDB;
use crate::hyperlane::message::{HyperlaneMessageStorage, HyperlaneMessageStore};
use crate::hyperlane::snapshot::{HyperlaneSnapshotStorage, HyperlaneSnapshotStore};
use crate::proofs::{ProofStorage, RocksDbProofStorage};

pub struct AppStorage {
    db: UnifiedDB,
    proofs: Arc<dyn ProofStorage>,
    messages: Arc<dyn HyperlaneMessageStorage>,
    snapshots: Arc<dyn HyperlaneSnapshotStorage>,
}

impl AppStorage {
    pub fn new<P: AsRef<Path>>(base_path: P, trusted_snapshot: Option<MerkleTree>) -> Result<Self> {
        let db = UnifiedDB::new(base_path)?;
        let db_arc = db.inner().clone();

        let proofs = Arc::new(RocksDbProofStorage::new(db_arc.clone()));
        let messages = Arc::new(HyperlaneMessageStore::new(db_arc.clone()));
        let snapshots = Arc::new(HyperlaneSnapshotStore::new(db_arc, trusted_snapshot)?);

        Ok(Self {
            db,
            proofs,
            messages,
            snapshots,
        })
    }

    pub fn proofs(&self) -> &Arc<dyn ProofStorage> {
        &self.proofs
    }

    pub fn messages(&self) -> &Arc<dyn HyperlaneMessageStorage> {
        &self.messages
    }

    pub fn snapshots(&self) -> &Arc<dyn HyperlaneSnapshotStorage> {
        &self.snapshots
    }

    pub fn db(&self) -> &UnifiedDB {
        &self.db
    }

    pub fn unsafe_reset_all(&self) -> Result<()> {
        self.db.unsafe_reset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_app_storage_creation() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path(), None).unwrap();

        // Verify all stores are accessible (they are all references so they can't be null)
        let _ = storage.proofs();
        let _ = storage.messages();
        let _ = storage.snapshots();
    }

    #[test]
    fn test_app_storage_with_trusted_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let trusted_tree = MerkleTree::default();
        let storage = AppStorage::new(temp_dir.path(), Some(trusted_tree)).unwrap();

        // Verify snapshot was initialized
        let snapshot = storage.snapshots().get_snapshot(0).unwrap();
        assert_eq!(snapshot.height, 0);
    }

    #[test]
    fn test_unsafe_reset_all() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path(), None).unwrap();

        // Verify reset works without errors
        storage.unsafe_reset_all().unwrap();
    }
}
