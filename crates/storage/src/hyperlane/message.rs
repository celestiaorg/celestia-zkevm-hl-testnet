/// This module contains the implementation of the HyperlaneMessageStore, which is a wrapper around the RocksDB database.
/// It is used to store and retrieve Hyperlane messages.
/// The messages are stored in a column family called "messages".
use anyhow::{Context, Result};
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options, SliceTransform};
use std::path::Path;
use std::sync::Arc;

use crate::hyperlane::StoredHyperlaneMessage;

pub struct HyperlaneMessageStore {
    pub db: Arc<DB>,
}

impl HyperlaneMessageStore {
    pub async fn from_path<P: AsRef<Path>>(base_path: P) -> Result<Self> {
        let db_path = base_path.as_ref().join("messages.db");

        let db_opts = Self::get_opts()?;
        let cfs = Self::get_cfs()?;
        let db = DB::open_cf_descriptors(&db_opts, db_path, cfs)?;
        Ok(Self { db: Arc::new(db) })
    }

    fn get_opts() -> Result<Options> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        Ok(opts)
    }

    fn get_cfs() -> Result<Vec<ColumnFamilyDescriptor>> {
        let mut cf_opts = Options::default();
        cf_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
        Ok(vec![ColumnFamilyDescriptor::new("messages", cf_opts)])
    }

    /// Insert a serialized hyperlane message into the database
    pub async fn insert_message(&self, index: u64, message: StoredHyperlaneMessage) -> Result<()> {
        let serialized = bincode::serialize(&message)?;

        if let Some(block) = message.block_number {
            let cf_blk = self.db.cf_handle("messages").expect("Missing messages CF");
            let mut key = block.to_be_bytes().to_vec();
            key.extend_from_slice(&index.to_be_bytes());
            self.db.put_cf(cf_blk, key, &serialized)?;
        }

        Ok(())
    }

    /// Get all stored Hyperlane messages for a given block height
    pub async fn get_by_block(&self, block: u64) -> Result<Vec<StoredHyperlaneMessage>> {
        let cf_blk = self.db.cf_handle("messages").context("Missing CF")?;
        let mut result = Vec::new();
        let prefix = block.to_be_bytes();
        let iter = self.db.prefix_iterator_cf(cf_blk, prefix);
        for kv in iter {
            let (_k, v) = kv?;
            result.push(bincode::deserialize(&v)?);
        }
        Ok(result)
    }

    /// Get the next index to use for insertion.
    pub async fn current_index(&self) -> Result<u64> {
        let cf = self.db.cf_handle("messages").context("Missing messages CF")?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);
        if let Some(Ok((k, _))) = iter.next() {
            if k.len() != 16 {
                anyhow::bail!("messages CF key length != 16 (got {})", k.len());
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k[8..16]);
            return Ok(u64::from_be_bytes(buf) + 1);
        }
        Ok(0)
    }

    /// Prune all Hyperlane messages from the database
    pub async fn reset_db(&mut self) -> Result<()> {
        let db = Arc::get_mut(&mut self.db)
            .ok_or_else(|| anyhow::anyhow!("Cannot get mutable reference to DB - multiple references exist"))?;
        db.drop_cf("messages")?;
        let mut cf_opts = Options::default();
        cf_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
        db.create_cf("messages", &cf_opts)?;
        Ok(())
    }
}
