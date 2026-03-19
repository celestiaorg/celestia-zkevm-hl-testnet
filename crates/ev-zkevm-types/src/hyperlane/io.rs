use std::str::FromStr;

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use super::{
    merkle::{MerkleTree, ZERO_BYTES},
    message::HyperlaneMessage,
    proof::{HYPERLANE_MERKLE_TREE_KEYS, HyperlaneBranchProofInputs},
};

#[derive(Serialize, Deserialize, Debug, Clone)]
/// Inputs for the hyperlane message circuit.
pub struct HyperlaneMessageInputs {
    pub state_root: String,
    pub contract: String,
    pub messages: Vec<HyperlaneMessage>,
    pub branch_proof: HyperlaneBranchProofInputs,
    pub snapshot: MerkleTree,
}

/// Implementation of the hyperlane message inputs.
impl HyperlaneMessageInputs {
    pub fn new(
        state_root: String,
        contract: String,
        messages: Vec<HyperlaneMessage>,
        branch_proof: HyperlaneBranchProofInputs,
        snapshot: MerkleTree,
    ) -> Self {
        Self {
            state_root,
            contract,
            messages,
            branch_proof,
            snapshot,
        }
    }

    /// Verify the hyperlane message inputs against the branch proof and snapshot.
    pub fn verify(&mut self) {
        let message_ids: Vec<String> = self.messages.iter().map(|m| m.id()).collect();
        for message_id in message_ids {
            self.snapshot
                .insert(message_id)
                .expect("Failed to insert message id into snapshot");
        }

        if self
            .snapshot
            .branch
            .iter()
            .all(|_| self.snapshot.branch.iter().all(|b| b == ZERO_BYTES))
        {
            println!("Snapshot branch is empty (all zero bytes) before proof verification");
        }

        for idx in 0..HYPERLANE_MERKLE_TREE_KEYS.len() {
            assert_eq!(
                self.snapshot.branch[idx],
                self.branch_proof.get_branch_node(idx),
                "Branch node {idx} does not match"
            );
        }

        let verified = self
            .branch_proof
            .verify(
                &HYPERLANE_MERKLE_TREE_KEYS,
                Address::from_str(&self.contract).unwrap(),
                &self.state_root,
            )
            .expect("Failed to verify branch proof");
        assert!(verified);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HyperlaneMessageOutputs {
    pub state_root: [u8; 32],
    pub merkle_tree_address: [u8; 32],
    pub message_ids: Vec<[u8; 32]>,
}

impl HyperlaneMessageOutputs {
    pub fn new(state_root: [u8; 32], merkle_tree_address: [u8; 32], message_ids: Vec<[u8; 32]>) -> Self {
        Self {
            state_root,
            merkle_tree_address,
            message_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HyperlaneMessageOutputs;

    #[test]
    fn test_hyperlane_message_outputs_serialization_roundtrip() {
        let output = HyperlaneMessageOutputs::new([1u8; 32], [2u8; 32], vec![[3u8; 32], [4u8; 32]]);

        let serialized = bincode::serialize(&output).unwrap();
        let deserialized: HyperlaneMessageOutputs = bincode::deserialize(&serialized).unwrap();

        assert_eq!(output.state_root, deserialized.state_root);
        assert_eq!(output.merkle_tree_address, deserialized.merkle_tree_address);
        assert_eq!(output.message_ids, deserialized.message_ids);
    }

    #[test]
    fn test_branch_proof_input_conversion_preserves_storage_values() {
        use alloy_primitives::{Address, U256};
        use alloy_rpc_types::{EIP1186AccountProofResponse, EIP1186StorageProof};

        use crate::hyperlane::proof::HyperlaneBranchProof;

        let proof = HyperlaneBranchProof::new(EIP1186AccountProofResponse {
            address: Address::ZERO,
            account_proof: vec![vec![0xc0].into()],
            balance: U256::ZERO,
            code_hash: Default::default(),
            nonce: 0,
            storage_hash: Default::default(),
            storage_proof: vec![EIP1186StorageProof {
                key: Default::default(),
                value: U256::from(7),
                proof: vec![],
            }],
        });

        let inputs = super::HyperlaneBranchProofInputs::from(proof);

        assert_eq!(inputs.storage_values.len(), 1);
        assert_eq!(inputs.storage_values[0], U256::from(7).to_be_bytes::<32>().to_vec());
    }
}
