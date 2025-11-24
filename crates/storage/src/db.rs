use anyhow::Result;
use rocksdb::{ColumnFamilyDescriptor, DB, Options, SliceTransform};
use std::path::Path;
use std::sync::Arc;

pub const CF_BLOCK_PROOFS: &str = "block_proofs";
pub const CF_RANGE_PROOFS: &str = "range_proofs";
pub const CF_MEMBERSHIP_PROOFS: &str = "membership_proofs";
pub const CF_METADATA: &str = "metadata";
pub const CF_MESSAGES: &str = "messages";
pub const CF_SNAPSHOTS: &str = "snapshots";

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
            ColumnFamilyDescriptor::new(CF_MESSAGES, Self::message_cf_opts()),
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

    pub fn begin_transaction(&self) -> rocksdb::WriteBatch {
        rocksdb::WriteBatch::default()
    }

    pub fn commit_transaction(&self, batch: rocksdb::WriteBatch) -> Result<()> {
        self.db.write(batch)?;
        Ok(())
    }
}

pub struct AtomicTransaction<'a> {
    db: &'a UnifiedDB,
    batch: rocksdb::WriteBatch,
}

impl<'a> AtomicTransaction<'a> {
    pub fn new(db: &'a UnifiedDB) -> Self {
        Self {
            db,
            batch: rocksdb::WriteBatch::default(),
        }
    }

    pub fn put_cf<K, V>(&mut self, cf: &rocksdb::ColumnFamily, key: K, value: V)
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        self.batch.put_cf(cf, key, value);
    }

    pub fn delete_cf<K>(&mut self, cf: &rocksdb::ColumnFamily, key: K)
    where
        K: AsRef<[u8]>,
    {
        self.batch.delete_cf(cf, key);
    }

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

        let cf = db.inner().cf_handle(CF_METADATA).unwrap();
        db.inner().put_cf(cf, b"test_key", b"test_value").unwrap();

        assert!(db.inner().get_cf(cf, b"test_key").unwrap().is_some());

        db.unsafe_reset().unwrap();

        let cf = db.inner().cf_handle(CF_METADATA).unwrap();
        assert!(db.inner().get_cf(cf, b"test_key").unwrap().is_none());
    }
}
