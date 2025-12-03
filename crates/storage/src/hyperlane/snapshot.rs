// This module contains the HyperlaneShapshotStore, which is a wrapper around the RocksDB database.
// It is used to store and retrieve Hyperlane snapshots.
// The snapshots are stored in a column family called "snapshots".

use anyhow::{Context, Result};
use ev_zkevm_types::programs::hyperlane::tree::{MerkleTree, ZERO_BYTES};
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

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
    pub db: Arc<DB>,
}

impl HyperlaneSnapshotStore {
    pub async fn new<P: AsRef<Path>>(base_path: P, trusted_snapshot: Option<MerkleTree>) -> Result<Self> {
        let db_path = base_path.as_ref().join("snapshots.db");

        let opts = Self::get_opts()?;
        let cfs = Self::get_cfs()?;
        let db = DB::open_cf_descriptors(&opts, db_path, cfs)?;
        let snapshot_store = Self { db: Arc::new(db) };
        if let Some(trusted_snapshot) = trusted_snapshot {
            snapshot_store
                .insert_snapshot(0, HyperlaneSnapshot::new(0, trusted_snapshot))
                .await
                .context("Failed to insert trusted snapshot")?;
        } else {
            snapshot_store
                .insert_snapshot(0, HyperlaneSnapshot::new(0, MerkleTree::default()))
                .await?;
        }
        Ok(snapshot_store)
    }

    pub async fn from_path<P: AsRef<Path>>(base_path: P) -> Result<Self> {
        let db_path = base_path.as_ref().join("snapshots.db");
        let opts = Self::get_opts()?;
        let cfs = Self::get_cfs()?;
        let db = DB::open_cf_descriptors(&opts, db_path, cfs)?;
        let snapshot_store = Self { db: Arc::new(db) };

        // Ensure there's an initial snapshot at index 0 if the database is empty
        let current_idx = snapshot_store.current_index().await?;
        if current_idx == 0 {
            // Check if snapshot at index 0 actually exists
            if snapshot_store.get_snapshot(0).await.is_err() {
                // Create initial snapshot at index 0
                snapshot_store
                    .insert_snapshot(0, HyperlaneSnapshot::new(0, MerkleTree::default()))
                    .await?;
            }
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
    pub async fn insert_snapshot(&self, index: u64, snapshot: HyperlaneSnapshot) -> Result<()> {
        let serialized = bincode::serialize(&snapshot).context("Failed to serialize snapshot")?;

        let cf = self
            .db
            .cf_handle("snapshots")
            .context("Missing snapshots column family")?;
        self.db
            .put_cf(cf, index.to_be_bytes(), serialized)
            .context("Failed to insert snapshot into database")?;
        Ok(())
    }

    /// Get a Hyperlane Snapshot by index
    pub async fn get_snapshot(&self, index: u64) -> Result<HyperlaneSnapshot> {
        let cf = self.db.cf_handle("snapshots").context("Missing CF")?;
        let snapshot_bytes = self
            .db
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
    pub async fn get_pending_snapshot(&self) -> Result<Option<(u64, HyperlaneSnapshot)>> {
        let cf = self.db.cf_handle("snapshots").context("Missing CF")?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);
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
    pub async fn finalize_snapshot(&self, index: u64) -> Result<()> {
        let mut snapshot = self
            .get_snapshot(index)
            .await
            .with_context(|| format!("Snapshot at index {index} not found"))?;
        if snapshot.finalized {
            return Err(anyhow::anyhow!(
                "Tried to finalize a finalized snapshot at index {index}"
            ));
        }
        snapshot.finalized = true;
        self.insert_snapshot(index, snapshot).await
    }

    /// Get the next insert index for the Hyperlane Snapshot store
    pub async fn current_index(&self) -> Result<u64> {
        let cf = self.db.cf_handle("snapshots").context("Missing CF")?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);
        if let Some(Ok((k, _))) = iter.next() {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            Ok(u64::from_be_bytes(buf))
        } else {
            Ok(0)
        }
    }

    /// Reset the database by dropping the snapshots column family and creating a new one
    pub async fn reset_db(&mut self) -> Result<()> {
        let db = Arc::get_mut(&mut self.db)
            .ok_or_else(|| anyhow::anyhow!("Cannot get mutable reference to DB - multiple references exist"))?;
        db.drop_cf("snapshots")?;
        let opts = Options::default();
        db.create_cf("snapshots", &opts)?;
        Ok(())
    }

    /// List all snapshots in the database
    pub async fn list_all_snapshots(&self) -> Result<Vec<(u64, HyperlaneSnapshot)>> {
        let cf = self.db.cf_handle("snapshots").context("Missing CF")?;
        let mut snapshots = Vec::new();

        for item in self.db.iterator_cf(cf, IteratorMode::Start) {
            let (k, v) = item.context("Failed to read snapshot from iterator")?;
            if k.len() != 8 {
                continue;
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k);
            let index = u64::from_be_bytes(buf);

            let mut snapshot: HyperlaneSnapshot = bincode::deserialize(&v).context("Failed to deserialize snapshot")?;

            // normalize: replace "" with ZERO_BYTES
            for h in snapshot.tree.branch.iter_mut() {
                if h.is_empty() {
                    *h = ZERO_BYTES.to_string();
                }
            }

            snapshots.push((index, snapshot));
        }

        Ok(snapshots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_insert_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None)
            .await
            .unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot).await.unwrap();
    }
    #[tokio::test]
    async fn test_get_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None)
            .await
            .unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot.clone()).await.unwrap();
        let retrieved_snapshot = store.get_snapshot(0).await.unwrap();
        assert_eq!(retrieved_snapshot, snapshot);
    }
    #[tokio::test]
    async fn test_get_pending_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None)
            .await
            .unwrap();
        let first_snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        let second_snapshot = HyperlaneSnapshot::new(1, MerkleTree::default());
        let third_snapshot = HyperlaneSnapshot::new(2, MerkleTree::default());
        store.insert_snapshot(0, first_snapshot.clone()).await.unwrap();
        store.insert_snapshot(1, second_snapshot.clone()).await.unwrap();
        store.insert_snapshot(2, third_snapshot.clone()).await.unwrap();
        store.finalize_snapshot(0).await.unwrap();
        store.finalize_snapshot(1).await.unwrap();
        let retrieved_snapshot = store.get_pending_snapshot().await.unwrap();
        assert_eq!(retrieved_snapshot, Some((2, third_snapshot)));
    }
    #[tokio::test]
    async fn test_finalize_snapshot() {
        let store = HyperlaneSnapshotStore::new(tempfile::tempdir().unwrap(), None)
            .await
            .unwrap();
        let snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        store.insert_snapshot(0, snapshot.clone()).await.unwrap();
        store.finalize_snapshot(0).await.unwrap();
        let retrieved_snapshot = store.get_snapshot(0).await.unwrap();
        assert!(retrieved_snapshot.finalized);
    }
}
