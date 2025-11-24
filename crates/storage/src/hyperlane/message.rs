/// This module contains the implementation of the HyperlaneMessageStore, which is a wrapper around the RocksDB database.
/// It is used to store and retrieve Hyperlane messages.
/// The messages are stored in a column family called "messages".
use rocksdb::{DB, IteratorMode};
use std::sync::Arc;
use thiserror::Error;

use crate::db::CF_MESSAGES;
use crate::hyperlane::StoredHyperlaneMessage;

#[derive(Debug, Error)]
pub enum MessageStorageError {
    #[error("Database error: {0}")]
    Database(#[from] rocksdb::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("Column family not found: {0}")]
    ColumnFamilyNotFound(String),
    #[error("Invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
}

pub type MessageStorageResult<T> = Result<T, MessageStorageError>;

/// Trait for Hyperlane message storage operations.
///
/// This trait abstracts message storage to enable testing and different implementations.
pub trait HyperlaneMessageStorage: Send + Sync {
    /// Insert a Hyperlane message into storage at the given index.
    fn insert_message(&self, index: u64, message: StoredHyperlaneMessage) -> MessageStorageResult<()>;

    /// Get all Hyperlane messages for a specific block height.
    fn get_by_block(&self, block: u64) -> MessageStorageResult<Vec<StoredHyperlaneMessage>>;

    /// Get the next available index for message insertion.
    fn current_index(&self) -> MessageStorageResult<u64>;

    /// Reset the database, removing all messages.
    ///
    /// WARNING: This destroys all data. Only use for testing.
    fn reset_db(&self) -> MessageStorageResult<()>;
}

/// RocksDB-backed implementation of Hyperlane message storage.
///
/// Uses a shared database instance with the CF_MESSAGES column family.
/// Keys are composite: [block_height (8 bytes) || index (8 bytes)]
pub struct HyperlaneMessageStore {
    db: Arc<DB>,
}

impl HyperlaneMessageStore {
    /// Creates a new message store using a shared database instance.
    pub fn new(db: Arc<DB>) -> Self {
        Self { db }
    }

    fn get_cf(&self) -> MessageStorageResult<&rocksdb::ColumnFamily> {
        self.db
            .cf_handle(CF_MESSAGES)
            .ok_or_else(|| MessageStorageError::ColumnFamilyNotFound(CF_MESSAGES.to_string()))
    }

}

impl HyperlaneMessageStorage for HyperlaneMessageStore {
    fn insert_message(&self, index: u64, message: StoredHyperlaneMessage) -> MessageStorageResult<()> {
        let serialized = bincode::serialize(&message)?;
        let cf = self.get_cf()?;

        if let Some(block) = message.block_number {
            let mut key = block.to_be_bytes().to_vec();
            key.extend_from_slice(&index.to_be_bytes());
            self.db.put_cf(cf, key, &serialized)?;
        }

        Ok(())
    }

    fn get_by_block(&self, block: u64) -> MessageStorageResult<Vec<StoredHyperlaneMessage>> {
        let cf = self.get_cf()?;
        let mut result = Vec::new();
        let prefix = block.to_be_bytes();
        let iter = self.db.prefix_iterator_cf(cf, prefix);

        for kv in iter {
            let (_k, v) = kv?;
            result.push(bincode::deserialize(&v)?);
        }

        Ok(result)
    }

    fn current_index(&self) -> MessageStorageResult<u64> {
        let cf = self.get_cf()?;
        let mut iter = self.db.iterator_cf(cf, IteratorMode::End);

        if let Some(Ok((k, _))) = iter.next() {
            if k.len() != 16 {
                return Err(MessageStorageError::InvalidKeyLength {
                    expected: 16,
                    actual: k.len(),
                });
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&k[8..16]);
            return Ok(u64::from_be_bytes(buf) + 1);
        }

        Ok(0)
    }

    fn reset_db(&self) -> MessageStorageResult<()> {
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

/// Testing utilities for HyperlaneMessageStorage implementations.
///
/// This module provides an in-memory mock implementation of HyperlaneMessageStorage
/// that can be used in tests across all crates that depend on the storage trait.
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory implementation of HyperlaneMessageStorage for testing.
    ///
    /// Uses HashMaps and Mutex for thread-safe, deterministic testing
    /// without requiring a real database.
    pub struct MockMessageStorage {
        messages: Mutex<HashMap<(u64, u64), StoredHyperlaneMessage>>, // (block, index) -> message
        current_index: Mutex<u64>,
    }

    impl MockMessageStorage {
        pub fn new() -> Self {
            Self {
                messages: Mutex::new(HashMap::new()),
                current_index: Mutex::new(0),
            }
        }

        /// Insert a message directly for testing
        pub fn insert_test_message(&self, block: u64, index: u64, message: StoredHyperlaneMessage) {
            self.messages.lock().unwrap().insert((block, index), message);
            let mut current = self.current_index.lock().unwrap();
            *current = (*current).max(index + 1);
        }
    }

    impl Default for MockMessageStorage {
        fn default() -> Self {
            Self::new()
        }
    }

    impl HyperlaneMessageStorage for MockMessageStorage {
        fn insert_message(&self, index: u64, message: StoredHyperlaneMessage) -> MessageStorageResult<()> {
            if let Some(block) = message.block_number {
                self.messages.lock().unwrap().insert((block, index), message);
                let mut current = self.current_index.lock().unwrap();
                *current = (*current).max(index + 1);
            }
            Ok(())
        }

        fn get_by_block(&self, block: u64) -> MessageStorageResult<Vec<StoredHyperlaneMessage>> {
            let messages = self.messages.lock().unwrap();
            let mut result: Vec<StoredHyperlaneMessage> = messages
                .iter()
                .filter(|((b, _), _)| *b == block)
                .map(|(_, msg)| msg.clone())
                .collect();
            result.sort_by_key(|msg| msg.block_number);
            Ok(result)
        }

        fn current_index(&self) -> MessageStorageResult<u64> {
            Ok(*self.current_index.lock().unwrap())
        }

        fn reset_db(&self) -> MessageStorageResult<()> {
            self.messages.lock().unwrap().clear();
            *self.current_index.lock().unwrap() = 0;
            Ok(())
        }
    }

    #[test]
    fn test_mock_insert_and_retrieve() {
        use ev_zkevm_types::hyperlane::HyperlaneMessage;

        let mock = MockMessageStorage::new();
        let test_message = HyperlaneMessage {
            version: 3,
            nonce: 0,
            origin: 1234,
            sender: [0u8; 32],
            destination: 5678,
            recipient: [0u8; 32],
            body: vec![1, 2, 3, 4],
        };
        let message = StoredHyperlaneMessage::new(test_message, Some(100));

        mock.insert_message(0, message.clone()).unwrap();

        let retrieved = mock.get_by_block(100).unwrap();
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].block_number, Some(100));
    }

    #[test]
    fn test_mock_current_index() {
        use ev_zkevm_types::hyperlane::HyperlaneMessage;

        let mock = MockMessageStorage::new();
        assert_eq!(mock.current_index().unwrap(), 0);

        let test_message = HyperlaneMessage {
            version: 3,
            nonce: 0,
            origin: 1234,
            sender: [0u8; 32],
            destination: 5678,
            recipient: [0u8; 32],
            body: vec![1, 2, 3, 4],
        };
        let message = StoredHyperlaneMessage::new(test_message, Some(100));
        mock.insert_message(0, message).unwrap();
        assert_eq!(mock.current_index().unwrap(), 1);
    }

    #[test]
    fn test_mock_reset() {
        use ev_zkevm_types::hyperlane::HyperlaneMessage;

        let mock = MockMessageStorage::new();
        let test_message = HyperlaneMessage {
            version: 3,
            nonce: 0,
            origin: 1234,
            sender: [0u8; 32],
            destination: 5678,
            recipient: [0u8; 32],
            body: vec![1, 2, 3, 4],
        };
        let message = StoredHyperlaneMessage::new(test_message, Some(100));
        mock.insert_message(0, message).unwrap();

        mock.reset_db().unwrap();
        assert_eq!(mock.current_index().unwrap(), 0);
        assert_eq!(mock.get_by_block(100).unwrap().len(), 0);
    }
}
