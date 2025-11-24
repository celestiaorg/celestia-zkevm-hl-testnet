//! Application storage wrapper that combines all storage abstractions.
//!
//! This module provides `AppStorage`, a unified interface to all storage subsystems
//! (proofs, messages, snapshots) backed by a single shared database instance.

use anyhow::Result;
use ev_zkevm_types::programs::hyperlane::tree::MerkleTree;
use std::path::Path;
use std::sync::Arc;

use crate::db::UnifiedDB;
use crate::hyperlane::message::{HyperlaneMessageStorage, HyperlaneMessageStore};
use crate::hyperlane::snapshot::{HyperlaneSnapshotStorage, HyperlaneSnapshotStore};
use crate::proofs::{ProofStorage, RocksDbProofStorage};

pub struct AppStorage {
    /// Unified database instance
    db: UnifiedDB,
    /// Proof storage abstraction
    proofs: Arc<dyn ProofStorage>,
    /// Hyperlane message storage abstraction
    messages: Arc<dyn HyperlaneMessageStorage>,
    /// Hyperlane snapshot storage abstraction
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

    /// Returns a reference to the proof storage.
    pub fn proofs(&self) -> &Arc<dyn ProofStorage> {
        &self.proofs
    }

    /// Returns a reference to the message storage.
    pub fn messages(&self) -> &Arc<dyn HyperlaneMessageStorage> {
        &self.messages
    }

    /// Returns a reference to the snapshot storage.
    pub fn snapshots(&self) -> &Arc<dyn HyperlaneSnapshotStorage> {
        &self.snapshots
    }

    /// Returns a reference to the underlying unified database.
    ///
    /// This can be used for advanced operations like atomic cross-store transactions.
    pub fn db(&self) -> &UnifiedDB {
        &self.db
    }

    /// Resets all storage by deleting all keys in all column families.
    ///
    /// WARNING: This destroys all data. Only use for testing.
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

        // Verify all stores are accessible
        assert!(storage.proofs() as *const _ as *const () != std::ptr::null());
        assert!(storage.messages() as *const _ as *const () != std::ptr::null());
        assert!(storage.snapshots() as *const _ as *const () != std::ptr::null());
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
