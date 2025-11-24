// This module contains the HyperlaneSnapshotStore, which is a wrapper around the RocksDB database.
// It is used to store and retrieve Hyperlane snapshots.
// The snapshots are stored in a column family called "snapshots".

use ev_zkevm_types::programs::hyperlane::tree::{MerkleTree, ZERO_BYTES};
use rocksdb::{DB, IteratorMode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

use crate::db::CF_SNAPSHOTS;

#[derive(Debug, Error)]
pub enum SnapshotStorageError {
    #[error("Database error: {0}")]
    Database(#[from] rocksdb::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("Column family not found: {0}")]
    ColumnFamilyNotFound(String),
    #[error("Snapshot not found at index: {0}")]
    SnapshotNotFound(u64),
    #[error("Attempted to finalize already finalized snapshot at index: {0}")]
    AlreadyFinalized(u64),
    #[error("General error: {0}")]
    General(String),
}

pub type SnapshotStorageResult<T> = Result<T, SnapshotStorageError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HyperlaneSnapshot {
    // the trusted EV height in the ZKISM
    pub height: u64,
    // the Hyperlane Message Tree e.g. Snapshot
    pub tree: MerkleTree,
    // whether this Snapshot has been finalized
    pub finalized: bool,
}

impl HyperlaneSnapshot {
    pub fn new(height: u64, tree: MerkleTree) -> HyperlaneSnapshot {
        HyperlaneSnapshot {
            height,
            tree,
            finalized: false,
        }
    }
    pub fn finalize(&mut self) {
        self.finalized = true;
    }
}

/// Trait for Hyperlane snapshot storage operations.
///
/// This trait abstracts snapshot storage to enable testing and different implementations.
pub trait HyperlaneSnapshotStorage: Send + Sync {
    /// Insert a Hyperlane snapshot at the given index.
    fn insert_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) -> SnapshotStorageResult<()>;

    /// Get a Hyperlane snapshot by index.
    fn get_snapshot(&self, index: u64) -> SnapshotStorageResult<HyperlaneSnapshot>;

    /// Get the latest pending (unfinalized) snapshot.
    fn get_pending_snapshot(&self) -> SnapshotStorageResult<Option<(u64, HyperlaneSnapshot)>>;

    /// Finalize a snapshot after successful proof submission.
    fn finalize_snapshot(&self, index: u64) -> SnapshotStorageResult<()>;

    /// Get the current index (highest index in use).
    fn current_index(&self) -> SnapshotStorageResult<u64>;

    /// Reset the database, removing all snapshots.
    ///
    /// WARNING: This destroys all data. Only use for testing.
    fn reset_db(&self) -> SnapshotStorageResult<()>;
}

/// RocksDB-backed implementation of Hyperlane snapshot storage.
///
/// Uses a shared database instance with the CF_SNAPSHOTS column family.
pub struct HyperlaneSnapshotStore {
    db: Arc<DB>,
}

impl HyperlaneSnapshotStore {
    /// Creates a new snapshot store using a shared database instance.
    ///
    /// If a trusted_snapshot is provided, it will be inserted at index 0.
    /// Otherwise, an empty merkle tree is inserted at index 0.
    pub fn new(db: Arc<DB>, trusted_snapshot: Option<MerkleTree>) -> SnapshotStorageResult<Self> {
        let snapshot_store = Self { db };

        if let Some(trusted_snapshot) = trusted_snapshot {
            snapshot_store.insert_snapshot(0, HyperlaneSnapshot::new(0, trusted_snapshot))?;
        } else {
            snapshot_store.insert_snapshot(0, HyperlaneSnapshot::new(0, MerkleTree::default()))?;
        }

        Ok(snapshot_store)
    }

    fn get_cf(&self) -> SnapshotStorageResult<&rocksdb::ColumnFamily> {
        self.db
            .cf_handle(CF_SNAPSHOTS)
            .ok_or_else(|| SnapshotStorageError::ColumnFamilyNotFound(CF_SNAPSHOTS.to_string()))
    }

    fn normalize_snapshot(&self, mut snapshot: HyperlaneSnapshot) -> HyperlaneSnapshot {
        // Replace empty strings with ZERO_BYTES in the merkle tree branch
        for h in snapshot.tree.branch.iter_mut() {
            if h.is_empty() {
                *h = ZERO_BYTES.to_string();
            }
        }
        snapshot
    }

}

impl HyperlaneSnapshotStorage for HyperlaneSnapshotStore {
    fn insert_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) -> SnapshotStorageResult<()> {
        let serialized = bincode::serialize(&snapshot)?;
        let cf = self.get_cf()?;
        self.db.put_cf(cf, index.to_be_bytes(), serialized)?;
        Ok(())
    }

    fn get_snapshot(&self, index: u64) -> SnapshotStorageResult<HyperlaneSnapshot> {
        let cf = self.get_cf()?;
        let snapshot_bytes = self
            .db
            .get_cf(cf, index.to_be_bytes())?
            .ok_or(SnapshotStorageError::SnapshotNotFound(index))?;

        let snapshot: HyperlaneSnapshot = bincode::deserialize(&snapshot_bytes)?;
        Ok(self.normalize_snapshot(snapshot))
    }

    fn get_pending_snapshot(&self) -> SnapshotStorageResult<Option<(u64, HyperlaneSnapshot)>> {
        let cf = self.get_cf()?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);

        while let Some(Ok((k, v))) = iter.next() {
            if k.len() != 8 {
                continue;
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            let index = u64::from_be_bytes(buf);
            let snapshot: HyperlaneSnapshot = bincode::deserialize(&v)?;
            let snapshot = self.normalize_snapshot(snapshot);

            if !snapshot.finalized {
                return Ok(Some((index, snapshot)));
            }
        }
        Ok(None)
    }

    fn finalize_snapshot(&self, index: u64) -> SnapshotStorageResult<()> {
        let mut snapshot = self.get_snapshot(index)?;

        if snapshot.finalized {
            return Err(SnapshotStorageError::AlreadyFinalized(index));
        }

        snapshot.finalized = true;
        self.insert_snapshot(index, snapshot)
    }

    fn current_index(&self) -> SnapshotStorageResult<u64> {
        let cf = self.get_cf()?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);

        if let Some(Ok((k, _))) = iter.next() {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            Ok(u64::from_be_bytes(buf))
        } else {
            Ok(0)
        }
    }

    fn reset_db(&self) -> SnapshotStorageResult<()> {
        let cf = self.get_cf()?;
        // Delete all keys in the column family
        let mut batch = rocksdb::WriteBatch::default();
        let iter = self.db.iterator_cf(cf, IteratorMode::Start);

        for item in iter {
            let (key, _) = item?;
            batch.delete_cf(cf, key);
        }

        self.db.write(batch)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::UnifiedDB;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, impl HyperlaneSnapshotStorage) {
        let temp_dir = TempDir::new().unwrap();
        let db = UnifiedDB::new(temp_dir.path()).unwrap();
        let store = HyperlaneSnapshotStore::new(db.inner().clone(), None).unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_insert_snapshot() {
        let (_temp_dir, store) = create_test_store();
        let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        store.insert_snapshot(1, snapshot).unwrap();
    }

    #[test]
    fn test_get_snapshot() {
        let (_temp_dir, store) = create_test_store();
        let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        store.insert_snapshot(1, snapshot.clone()).unwrap();
        let retrieved_snapshot = store.get_snapshot(1).unwrap();
        assert_eq!(retrieved_snapshot, snapshot);
    }

    #[test]
    fn test_get_pending_snapshot() {
        let (_temp_dir, store) = create_test_store();
        let first_snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        let second_snapshot = HyperlaneSnapshot::new(2, MerkleTree::default());
        let third_snapshot = HyperlaneSnapshot::new(3, MerkleTree::default());
        store.insert_snapshot(1, first_snapshot.clone()).unwrap();
        store.insert_snapshot(2, second_snapshot.clone()).unwrap();
        store.insert_snapshot(3, third_snapshot.clone()).unwrap();
        store.finalize_snapshot(0).unwrap(); // Finalize the default one
        store.finalize_snapshot(1).unwrap();
        store.finalize_snapshot(2).unwrap();
        let retrieved_snapshot = store.get_pending_snapshot().unwrap();
        assert_eq!(retrieved_snapshot, Some((3, third_snapshot)));
    }

    #[test]
    fn test_finalize_snapshot() {
        let (_temp_dir, store) = create_test_store();
        let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        store.insert_snapshot(1, snapshot.clone()).unwrap();
        store.finalize_snapshot(1).unwrap();
        let retrieved_snapshot = store.get_snapshot(1).unwrap();
        assert!(retrieved_snapshot.finalized);
    }
}

/// Testing utilities for HyperlaneSnapshotStorage implementations.
///
/// This module provides an in-memory mock implementation of HyperlaneSnapshotStorage
/// that can be used in tests across all crates that depend on the storage trait.
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory implementation of HyperlaneSnapshotStorage for testing.
    ///
    /// Uses HashMaps and Mutex for thread-safe, deterministic testing
    /// without requiring a real database.
    pub struct MockSnapshotStorage {
        snapshots: Mutex<HashMap<u64, HyperlaneSnapshot>>,
    }

    impl MockSnapshotStorage {
        pub fn new() -> Self {
            let mock = Self {
                snapshots: Mutex::new(HashMap::new()),
            };
            // Insert default snapshot at index 0
            mock.snapshots.lock().unwrap().insert(
                0,
                HyperlaneSnapshot::new(0, MerkleTree::default()),
            );
            mock
        }

        /// Insert a snapshot directly for testing
        pub fn insert_test_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) {
            self.snapshots.lock().unwrap().insert(index, snapshot);
        }
    }

    impl Default for MockSnapshotStorage {
        fn default() -> Self {
            Self::new()
        }
    }

    impl HyperlaneSnapshotStorage for MockSnapshotStorage {
        fn insert_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) -> SnapshotStorageResult<()> {
            self.snapshots.lock().unwrap().insert(index, snapshot);
            Ok(())
        }

        fn get_snapshot(&self, index: u64) -> SnapshotStorageResult<HyperlaneSnapshot> {
            self.snapshots
                .lock()
                .unwrap()
                .get(&index)
                .cloned()
                .ok_or(SnapshotStorageError::SnapshotNotFound(index))
        }

        fn get_pending_snapshot(&self) -> SnapshotStorageResult<Option<(u64, HyperlaneSnapshot)>> {
            let snapshots = self.snapshots.lock().unwrap();
            let mut pending: Vec<_> = snapshots
                .iter()
                .filter(|(_, snap)| !snap.finalized)
                .map(|(idx, snap)| (*idx, snap.clone()))
                .collect();

            // Return the highest index pending snapshot
            pending.sort_by_key(|(idx, _)| *idx);
            Ok(pending.last().cloned())
        }

        fn finalize_snapshot(&self, index: u64) -> SnapshotStorageResult<()> {
            let mut snapshots = self.snapshots.lock().unwrap();
            let snapshot = snapshots
                .get_mut(&index)
                .ok_or(SnapshotStorageError::SnapshotNotFound(index))?;

            if snapshot.finalized {
                return Err(SnapshotStorageError::AlreadyFinalized(index));
            }

            snapshot.finalized = true;
            Ok(())
        }

        fn current_index(&self) -> SnapshotStorageResult<u64> {
            let snapshots = self.snapshots.lock().unwrap();
            Ok(snapshots.keys().max().copied().unwrap_or(0))
        }

        fn reset_db(&self) -> SnapshotStorageResult<()> {
            let mut snapshots = self.snapshots.lock().unwrap();
            snapshots.clear();
            snapshots.insert(0, HyperlaneSnapshot::new(0, MerkleTree::default()));
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_mock_insert_and_retrieve() {
            let mock = MockSnapshotStorage::new();
            let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());

            mock.insert_snapshot(1, snapshot.clone()).unwrap();

            let retrieved = mock.get_snapshot(1).unwrap();
            assert_eq!(retrieved.height, 1);
            assert!(!retrieved.finalized);
        }

        #[test]
        fn test_mock_finalize() {
            let mock = MockSnapshotStorage::new();
            let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
            mock.insert_snapshot(1, snapshot).unwrap();

            mock.finalize_snapshot(1).unwrap();

            let retrieved = mock.get_snapshot(1).unwrap();
            assert!(retrieved.finalized);
        }

        #[test]
        fn test_mock_already_finalized_error() {
            let mock = MockSnapshotStorage::new();
            let snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
            mock.insert_snapshot(1, snapshot).unwrap();
            mock.finalize_snapshot(1).unwrap();

            let result = mock.finalize_snapshot(1);
            assert!(matches!(result, Err(SnapshotStorageError::AlreadyFinalized(1))));
        }

        #[test]
        fn test_mock_get_pending() {
            let mock = MockSnapshotStorage::new();
            let snap1 = HyperlaneSnapshot::new(1, MerkleTree::default());
            let mut snap2 = HyperlaneSnapshot::new(2, MerkleTree::default());
            snap2.finalized = true;
            let snap3 = HyperlaneSnapshot::new(3, MerkleTree::default());

            mock.insert_snapshot(1, snap1).unwrap();
            mock.insert_snapshot(2, snap2).unwrap();
            mock.insert_snapshot(3, snap3.clone()).unwrap();

            let pending = mock.get_pending_snapshot().unwrap();
            assert!(pending.is_some());
            let (idx, snapshot) = pending.unwrap();
            assert_eq!(idx, 3);
            assert_eq!(snapshot.height, 3);
            assert!(!snapshot.finalized);
        }

        #[test]
        fn test_mock_current_index() {
            let mock = MockSnapshotStorage::new();
            assert_eq!(mock.current_index().unwrap(), 0);

            mock.insert_snapshot(5, HyperlaneSnapshot::new(5, MerkleTree::default()))
                .unwrap();
            assert_eq!(mock.current_index().unwrap(), 5);
        }

        #[test]
        fn test_mock_reset() {
            let mock = MockSnapshotStorage::new();
            mock.insert_snapshot(5, HyperlaneSnapshot::new(5, MerkleTree::default()))
                .unwrap();

            mock.reset_db().unwrap();

            assert_eq!(mock.current_index().unwrap(), 0);
            assert!(mock.get_snapshot(5).is_err());
            assert!(mock.get_snapshot(0).is_ok());
        }
    }
}
