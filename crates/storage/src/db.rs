//! Unified database module for consolidated RocksDB storage.
//!
//! This module provides a single physical RocksDB instance with multiple
//! column families for different logical stores (proofs, messages, snapshots).
//! This design reduces resource overhead while maintaining logical separation.

use anyhow::Result;
use rocksdb::{ColumnFamilyDescriptor, DB, Options, SliceTransform};
use std::path::Path;
use std::sync::Arc;

/// Column family names for different storage types
pub const CF_BLOCK_PROOFS: &str = "block_proofs";
pub const CF_RANGE_PROOFS: &str = "range_proofs";
pub const CF_MEMBERSHIP_PROOFS: &str = "membership_proofs";
pub const CF_METADATA: &str = "metadata";
pub const CF_MESSAGES: &str = "messages";
pub const CF_SNAPSHOTS: &str = "snapshots";

/// Unified database instance shared across all storage abstractions.
///
/// This provides a single RocksDB instance with multiple column families,
/// reducing operational overhead while maintaining logical separation.
#[derive(Clone)]
pub struct UnifiedDB {
    db: Arc<DB>,
}

impl UnifiedDB {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Result<Self> {
        let db_path = base_path.as_ref().join("storage.db");

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);

        let cfs = vec![
            ColumnFamilyDescriptor::new(CF_BLOCK_PROOFS, Self::proof_cf_opts()),
            ColumnFamilyDescriptor::new(CF_RANGE_PROOFS, Self::proof_cf_opts()),
            ColumnFamilyDescriptor::new(CF_MEMBERSHIP_PROOFS, Self::proof_cf_opts()),
            ColumnFamilyDescriptor::new(CF_METADATA, Options::default()),
            // Message storage with prefix extractor for efficient block-based queries
            ColumnFamilyDescriptor::new(CF_MESSAGES, Self::message_cf_opts()),
            // Snapshot storage
            ColumnFamilyDescriptor::new(CF_SNAPSHOTS, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&db_opts, db_path, cfs)?;
        Ok(Self { db: Arc::new(db) })
    }

    pub fn inner(&self) -> &Arc<DB> {
        &self.db
    }

    fn proof_cf_opts() -> Options {
        Options::default()
    }

    fn message_cf_opts() -> Options {
        let mut opts = Options::default();
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
        opts
    }

    /// Resets all column families by deleting all keys.
    ///
    /// WARNING: This destroys all data. Only use for testing.
    pub fn unsafe_reset(&self) -> Result<()> {
        let mut batch = rocksdb::WriteBatch::default();

        let cfs = [
            CF_BLOCK_PROOFS,
            CF_RANGE_PROOFS,
            CF_MEMBERSHIP_PROOFS,
            CF_METADATA,
            CF_MESSAGES,
            CF_SNAPSHOTS,
        ];

        for cf_name in cfs {
            if let Some(cf) = self.db.cf_handle(cf_name) {
                let iter = self.db.iterator_cf(cf, rocksdb::IteratorMode::Start);
                for item in iter {
                    let (key, _) = item?;
                    batch.delete_cf(cf, key);
                }
            }
        }

        self.db.write(batch)?;
        Ok(())
    }

    /// Creates an atomic transaction builder for cross-store operations.
    ///
    /// This allows multiple operations across different stores to be committed atomically.
    ///
    /// # Example
    /// ```no_run
    /// use storage::db::UnifiedDB;
    /// use std::path::Path;
    ///
    /// # fn main() -> anyhow::Result<()> {
    /// let db = UnifiedDB::new(Path::new("./data"))?;
    /// let tx = db.begin_transaction();
    ///
    /// // Add operations from different stores
    /// // tx.put_cf(...);
    /// // tx.delete_cf(...);
    ///
    /// // Commit atomically
    /// db.commit_transaction(tx)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn begin_transaction(&self) -> rocksdb::WriteBatch {
        rocksdb::WriteBatch::default()
    }

    /// Commits an atomic transaction.
    ///
    /// All operations in the batch will be applied atomically, or none will be applied.
    pub fn commit_transaction(&self, batch: rocksdb::WriteBatch) -> Result<()> {
        self.db.write(batch)?;
        Ok(())
    }
}

/// Transaction builder for atomic cross-store operations.
///
/// This provides a higher-level API for building atomic transactions
/// that span multiple storage abstractions.
///
/// # Example
/// ```no_run
/// use storage::db::{UnifiedDB, AtomicTransaction};
/// use storage::hyperlane::snapshot::HyperlaneSnapshot;
/// use ev_zkevm_types::programs::hyperlane::tree::MerkleTree;
/// use std::path::Path;
///
/// # fn main() -> anyhow::Result<()> {
/// let db = UnifiedDB::new(Path::new("./data"))?;
/// let mut tx = AtomicTransaction::new(&db);
///
/// // Example: Store snapshot and update metadata atomically
/// let snapshot = HyperlaneSnapshot::new(100, MerkleTree::default());
/// let serialized = bincode::serialize(&snapshot)?;
///
/// let cf_snapshots = db.inner().cf_handle("snapshots").unwrap();
/// tx.put_cf(cf_snapshots, 1u64.to_be_bytes(), &serialized);
///
/// // Commit all operations atomically
/// tx.commit()?;
/// # Ok(())
/// # }
/// ```
pub struct AtomicTransaction<'a> {
    db: &'a UnifiedDB,
    batch: rocksdb::WriteBatch,
}

impl<'a> AtomicTransaction<'a> {
    /// Creates a new atomic transaction.
    pub fn new(db: &'a UnifiedDB) -> Self {
        Self {
            db,
            batch: rocksdb::WriteBatch::default(),
        }
    }

    /// Adds a put operation to the transaction.
    pub fn put_cf<K, V>(&mut self, cf: &rocksdb::ColumnFamily, key: K, value: V)
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        self.batch.put_cf(cf, key, value);
    }

    /// Adds a delete operation to the transaction.
    pub fn delete_cf<K>(&mut self, cf: &rocksdb::ColumnFamily, key: K)
    where
        K: AsRef<[u8]>,
    {
        self.batch.delete_cf(cf, key);
    }

    /// Commits the transaction atomically.
    ///
    /// All operations will be applied atomically, or none will be applied if an error occurs.
    pub fn commit(self) -> Result<()> {
        self.db.commit_transaction(self.batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_unified_db_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db = UnifiedDB::new(temp_dir.path()).unwrap();

        // Verify all column families exist
        let cf_names = [
            CF_BLOCK_PROOFS,
            CF_RANGE_PROOFS,
            CF_MEMBERSHIP_PROOFS,
            CF_METADATA,
            CF_MESSAGES,
            CF_SNAPSHOTS,
        ];

        for cf_name in cf_names {
            assert!(
                db.inner().cf_handle(cf_name).is_some(),
                "Column family {} should exist",
                cf_name
            );
        }
    }

    #[test]
    fn test_unsafe_reset() {
        let temp_dir = TempDir::new().unwrap();
        let db = UnifiedDB::new(temp_dir.path()).unwrap();

        // Write some data
        let cf = db.inner().cf_handle(CF_METADATA).unwrap();
        db.inner().put_cf(cf, b"test_key", b"test_value").unwrap();

        // Verify data exists
        assert!(db.inner().get_cf(cf, b"test_key").unwrap().is_some());

        // Reset
        db.unsafe_reset().unwrap();

        // Verify data is gone
        let cf = db.inner().cf_handle(CF_METADATA).unwrap();
        assert!(db.inner().get_cf(cf, b"test_key").unwrap().is_none());
    }
}
