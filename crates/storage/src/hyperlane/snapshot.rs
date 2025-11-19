// This module contains the HyperlaneShapshotStore, which is a wrapper around the RocksDB database.
// It is used to store and retrieve Hyperlane snapshots.
// The snapshots are stored in a column family called "snapshots".

use anyhow::{Context, Result};
use ev_zkevm_types::programs::hyperlane::tree::{MerkleTree, ZERO_BYTES};
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, RwLock};

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

pub struct HyperlaneSnapshotStore {
    pub db: Arc<RwLock<DB>>,
}

impl HyperlaneSnapshotStore {
    pub fn new<P: AsRef<Path>>(base_path: P, trusted_snapshot: Option<MerkleTree>) -> Result<Self> {
        let db_path = base_path.as_ref().join("snapshots.db");

        let opts = Self::get_opts()?;
        let cfs = Self::get_cfs()?;
        let db = DB::open_cf_descriptors(&opts, db_path, cfs)?;
        let snapshot_store = Self {
            db: Arc::new(RwLock::new(db)),
        };
        if let Some(trusted_snapshot) = trusted_snapshot {
            snapshot_store
                .insert_snapshot(0, HyperlaneSnapshot::new(0, trusted_snapshot))
                .context("Failed to insert trusted snapshot")?;
        } else {
            snapshot_store.insert_snapshot(0, HyperlaneSnapshot::new(0, MerkleTree::default()))?;
        }
        Ok(snapshot_store)
    }

    pub fn get_opts() -> Result<Options> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        Ok(opts)
    }

    pub fn get_cfs() -> Result<Vec<ColumnFamilyDescriptor>> {
        Ok(vec![ColumnFamilyDescriptor::new("snapshots", Options::default())])
    }

    /// Insert a Hyperlane Snapshot into the database
    pub fn insert_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) -> Result<()> {
        // Serialize outside the lock to minimize lock duration
        let serialized = bincode::serialize(&snapshot).context("Failed to serialize snapshot")?;

        let write_lock = self
            .db
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to acquire write lock: {e}"))?;
        let cf = write_lock
            .cf_handle("snapshots")
            .context("Missing snapshots column family")?;
        write_lock
            .put_cf(cf, index.to_be_bytes(), serialized)
            .context("Failed to insert snapshot into database")?;
        Ok(())
    }

    /// Get a Hyperlane Snapshot by index
    pub fn get_snapshot(&self, index: u64) -> Result<HyperlaneSnapshot> {
        let read_lock = self.db.read().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let cf = read_lock.cf_handle("snapshots").context("Missing CF")?;
        let snapshot_bytes = read_lock
            .get_cf(cf, index.to_be_bytes())?
            .context("Failed to get snapshot")?;
        let mut snapshot: HyperlaneSnapshot = bincode::deserialize(&snapshot_bytes)?;

        // normalize: replace "" with ZERO_BYTES
        for h in snapshot.tree.branch.iter_mut() {
            if h.is_empty() {
                *h = ZERO_BYTES.to_string();
            }
        }

        Ok(snapshot)
    }

    /// Get the latest pending snapshot, we expect only the most recent snapshot to be unfinalized
    pub fn get_pending_snapshot(&self) -> Result<Option<(u64, HyperlaneSnapshot)>> {
        let read_lock = self
            .db
            .read()
            .map_err(|e| anyhow::anyhow!("Failed to acquire read lock: {e}"))?;
        let cf = read_lock.cf_handle("snapshots").context("Missing CF")?;
        let mut iter = read_lock.iterator_cf(cf, IteratorMode::End);
        while let Some(Ok((k, v))) = iter.next() {
            if k.len() != 8 {
                continue;
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            let index = u64::from_be_bytes(buf);
            let mut snapshot: HyperlaneSnapshot = bincode::deserialize(&v).context("Failed to deserialize snapshot")?;
            for h in snapshot.tree.branch.iter_mut() {
                if h.is_empty() {
                    *h = ZERO_BYTES.to_string();
                }
            }
            if !snapshot.finalized {
                return Ok(Some((index, snapshot)));
            }
        }
        Ok(None)
    }

    /// Finalize a Hyperlane Snapshot after successful proof submission
    pub fn finalize_snapshot(&self, index: u64) -> Result<()> {
        let mut snapshot = self
            .get_snapshot(index)
            .with_context(|| format!("Snapshot at index {index} not found"))?;
        if snapshot.finalized {
            return Err(anyhow::anyhow!(
                "Tried to finalize a finalized snapshot at index {index}"
            ));
        }
        snapshot.finalized = true;
        self.insert_snapshot(index, snapshot)
    }

    /// Get the next insert index for the Hyperlane Snapshot store
    pub fn current_index(&self) -> Result<u64> {
        let read_lock = self
            .db
            .read()
            .map_err(|e| anyhow::anyhow!("Failed to acquire read lock: {e}"))?;
        let cf = read_lock.cf_handle("snapshots").context("Missing CF")?;
        let mut iter = read_lock.iterator_cf(cf, IteratorMode::End);
        if let Some(Ok((k, _))) = iter.next() {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            Ok(u64::from_be_bytes(buf))
        } else {
            Ok(0)
        }
    }

    /// Reset the database by dropping the snapshots column family and creating a new one
    pub fn reset_db(&self) -> Result<()> {
        let mut write_lock = self
            .db
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to acquire write lock: {e}"))?;
        write_lock.drop_cf("snapshots")?;
        let opts = Options::default();
        write_lock.create_cf("snapshots", &opts)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None).unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot).unwrap();
    }
    #[test]
    fn test_get_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None).unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot.clone()).unwrap();
        let retrieved_snapshot = store.get_snapshot(0).unwrap();
        assert_eq!(retrieved_snapshot, snapshot);
    }
    #[test]
    fn test_get_pending_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None).unwrap();
        let first_snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        let second_snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        let third_snapshot = HyperlaneSnapshot::new(2, MerkleTree::default());
        store.insert_snapshot(0, first_snapshot.clone()).unwrap();
        store.insert_snapshot(1, second_snapshot.clone()).unwrap();
        store.insert_snapshot(2, third_snapshot.clone()).unwrap();
        store.finalize_snapshot(0).unwrap();
        store.finalize_snapshot(1).unwrap();
        let retrieved_snapshot = store.get_pending_snapshot().unwrap();
        assert_eq!(retrieved_snapshot, Some((2, third_snapshot)));
    }
    #[test]
    fn test_finalize_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None).unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot.clone()).unwrap();
        store.finalize_snapshot(0).unwrap();
        let retrieved_snapshot = store.get_snapshot(0).unwrap();
        assert!(retrieved_snapshot.finalized);
    }
}
