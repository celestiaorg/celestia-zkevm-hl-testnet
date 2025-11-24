//! Atomic transaction utilities for cross-store operations.
//!
//! This module provides helpers for performing atomic operations
//! across multiple storage abstractions.

use anyhow::Result;
use rocksdb::WriteBatch;

use crate::db::{CF_MEMBERSHIP_PROOFS, CF_SNAPSHOTS};
use crate::hyperlane::snapshot::HyperlaneSnapshot;
use crate::proofs::StoredMembershipProof;

pub struct AtomicSnapshotProofOps<'a> {
    db: &'a rocksdb::DB,
    batch: WriteBatch,
}

impl<'a> AtomicSnapshotProofOps<'a> {
    /// Creates a new atomic operation builder.
    pub fn new(db: &'a rocksdb::DB) -> Self {
        Self {
            db,
            batch: WriteBatch::default(),
        }
    }

    pub fn insert_snapshot(&mut self, index: u64, snapshot: &HyperlaneSnapshot) -> Result<()> {
        let cf = self
            .db
            .cf_handle(CF_SNAPSHOTS)
            .ok_or_else(|| anyhow::anyhow!("CF_SNAPSHOTS not found"))?;

        let serialized = bincode::serialize(snapshot)?;
        self.batch.put_cf(cf, index.to_be_bytes(), serialized);
        Ok(())
    }

    pub fn store_membership_proof(&mut self, height: u64, proof: &StoredMembershipProof) -> Result<()> {
        let cf = self
            .db
            .cf_handle(CF_MEMBERSHIP_PROOFS)
            .ok_or_else(|| anyhow::anyhow!("CF_MEMBERSHIP_PROOFS not found"))?;

        let serialized = bincode::serialize(proof)?;
        self.batch.put_cf(cf, height.to_be_bytes(), serialized);
        Ok(())
    }

    pub fn finalize_snapshot(&mut self, index: u64, snapshot: &HyperlaneSnapshot) -> Result<()> {
        let cf = self
            .db
            .cf_handle(CF_SNAPSHOTS)
            .ok_or_else(|| anyhow::anyhow!("CF_SNAPSHOTS not found"))?;

        let mut finalized_snapshot = snapshot.clone();
        finalized_snapshot.finalized = true;

        let serialized = bincode::serialize(&finalized_snapshot)?;
        self.batch.put_cf(cf, index.to_be_bytes(), serialized);
        Ok(())
    }

    pub fn commit(self) -> Result<()> {
        self.db.write(self.batch)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::UnifiedDB;
    use ev_zkevm_types::programs::hyperlane::tree::MerkleTree;
    use tempfile::TempDir;

    #[test]
    fn test_atomic_snapshot_proof_ops() {
        let temp_dir = TempDir::new().unwrap();
        let db = UnifiedDB::new(temp_dir.path()).unwrap();

        let mut ops = AtomicSnapshotProofOps::new(db.inner());

        let snapshot = HyperlaneSnapshot::new(100, MerkleTree::default());
        ops.insert_snapshot(1, &snapshot).unwrap();

        let proof = StoredMembershipProof {
            proof_data: vec![1, 2, 3],
            public_values: vec![4, 5, 6],
            created_at: 1234567890,
        };
        ops.store_membership_proof(100, &proof).unwrap();

        let prev_snapshot = HyperlaneSnapshot::new(0, MerkleTree::default());
        ops.finalize_snapshot(0, &prev_snapshot).unwrap();

        // Commit atomically
        ops.commit().unwrap();

        // Verify all operations succeeded
        let cf_snapshots = db.inner().cf_handle(CF_SNAPSHOTS).unwrap();
        let cf_proofs = db.inner().cf_handle(CF_MEMBERSHIP_PROOFS).unwrap();

        assert!(db.inner().get_cf(cf_snapshots, 1u64.to_be_bytes()).unwrap().is_some());
        assert!(db.inner().get_cf(cf_proofs, 100u64.to_be_bytes()).unwrap().is_some());
        assert!(db.inner().get_cf(cf_snapshots, 0u64.to_be_bytes()).unwrap().is_some());

        // Verify finalized snapshot is actually finalized
        let finalized_bytes = db.inner().get_cf(cf_snapshots, 0u64.to_be_bytes()).unwrap().unwrap();
        let finalized: HyperlaneSnapshot = bincode::deserialize(&finalized_bytes).unwrap();
        assert!(finalized.finalized);
    }
}
