use std::fmt::{Display, Formatter, Result as FmtResult};

use alloy_primitives::FixedBytes;
use celestia_types::{
    DataAvailabilityHeader,
    nmt::{Namespace, NamespaceProof},
};
use hex::encode;
use rsp_client_executor::io::EthClientExecutorInput;
use serde::{Deserialize, Serialize};

/// BlockExecInput is the input for the BlockExec circuit.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockExecInput {
    pub header_raw: Vec<u8>,
    pub dah: DataAvailabilityHeader,
    pub blobs_raw: Vec<u8>,
    pub pub_key: Vec<u8>,
    pub namespace: Namespace,
    pub proofs: Vec<NamespaceProof>,
    pub executor_inputs: Vec<EthClientExecutorInput>,
    pub trusted_height: u64,
    pub trusted_root: FixedBytes<32>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BlockExecOutput {
    // celestia_header_hash is the merkle hash of the Celestia block header.
    pub celestia_header_hash: [u8; 32],
    // prev_celestia_height is the height of the previous Celestia block.
    pub prev_celestia_height: u64,
    // prev_celestia_header_hash is the merkle hash of the previous Celestia block header.
    pub prev_celestia_header_hash: [u8; 32],
    // new_height is the block number after the state transition function has been applied.
    pub new_height: u64,
    // new_state_root is the EVM application state root after the state transition function has been applied.
    pub new_state_root: [u8; 32],
    // prev_height is the block number before the state transition function has been applied.
    pub prev_height: u64,
    // prev_state_root is the EVM application state root before the state transition function has been applied.
    pub prev_state_root: [u8; 32],
    // namespace is the Celestia namespace that contains the blob data.
    pub namespace: Namespace,
    // public_key is the sequencer's public key used to verify the signatures of the signed data.
    pub public_key: [u8; 32],
}

/// Display trait implementation to format hashes as hex encoded output.
impl Display for BlockExecOutput {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "BlockExecOutput {{")?;
        writeln!(f, "  celestia_header_hash: {}", encode(self.celestia_header_hash))?;
        writeln!(f, "  prev_celestia_height: {}", self.prev_celestia_height)?;
        writeln!(
            f,
            "  prev_celestia_header_hash: {}",
            encode(self.prev_celestia_header_hash)
        )?;
        writeln!(f, "  new_height: {}", self.new_height)?;
        writeln!(f, "  new_state_root: {}", encode(self.new_state_root))?;
        writeln!(f, "  prev_height: {}", self.prev_height)?;
        writeln!(f, "  prev_state_root: {}", encode(self.prev_state_root))?;
        writeln!(f, "  namespace: {}", encode(self.namespace.0))?;
        writeln!(f, "  public_key: {}", encode(self.public_key))?;
        writeln!(f, "}}")
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BatchExecInput {
    pub blocks: Vec<BlockExecInput>,
    /// CBOR-serialized trusted LightBlock (bincode doesn't work with tendermint's serde attrs)
    pub trusted_light_block_raw: Vec<u8>,
    /// CBOR-serialized new LightBlock (bincode doesn't work with tendermint's serde attrs)
    pub new_light_block_raw: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct BatchExecOutput {
    // the length prefix of the state, little-endian encoded bytes of the u64 length of the serialized state
    pub state_len_bytes: [u8; 8],
    // the starting point of the state transition
    pub state: State,
    // the length prefix of the new state, little-endian encoded bytes of the u64 length of the serialized new state
    pub new_state_len_bytes: [u8; 8],
    // the result of the state transition
    pub new_state: State,
}

impl Display for BatchExecOutput {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "BatchExecOutput {{")?;
        writeln!(f, "  state_len: {}", u64::from_le_bytes(self.state_len_bytes))?;
        writeln!(f, "  state: {}", self.state)?;
        writeln!(f, "  new_state_len: {}", u64::from_le_bytes(self.new_state_len_bytes))?;
        writeln!(f, "  new_state: {}", self.new_state)?;
        writeln!(f, "}}")
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub state_root: [u8; 32],
    pub height: u64,
    pub celestia_header_hash: [u8; 32],
    pub celestia_height: u64,
    pub namespace: [u8; 29],
    pub public_key: [u8; 32],
}

impl State {
    pub fn length(&self) -> u64 {
        bincode::serialize(self).unwrap().len() as u64
    }
}

/// Display trait implementation to format hashes as hex encoded output.
impl Display for State {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "State {{")?;
        writeln!(f, "  state_root: {}", encode(self.state_root))?;
        writeln!(f, "  height: {}", self.height)?;
        writeln!(f, "  celestia_header_hash: {}", encode(self.celestia_header_hash))?;
        writeln!(f, "  celestia_height: {}", self.celestia_height)?;
        writeln!(f, "  namespace: {}", encode(self.namespace))?;
        writeln!(f, "  public_key: {}", encode(self.public_key))?;
        writeln!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_exec_output_serialization_roundtrip() {
        let state = State {
            state_root: [1u8; 32],
            height: 50,
            celestia_header_hash: [2u8; 32],
            celestia_height: 100,
            namespace: [3u8; 29],
            public_key: [4u8; 32],
        };

        let new_state = State {
            state_root: [5u8; 32],
            height: 51,
            celestia_header_hash: [6u8; 32],
            celestia_height: 101,
            namespace: [3u8; 29],
            public_key: [4u8; 32],
        };

        let state_len = bincode::serialize(&state).unwrap().len() as u64;
        let new_state_len = bincode::serialize(&new_state).unwrap().len() as u64;
        let output = BatchExecOutput {
            state_len_bytes: state_len.to_le_bytes(),
            state: state.clone(),
            new_state_len_bytes: new_state_len.to_le_bytes(),
            new_state: new_state.clone(),
        };

        let serialized = bincode::serialize(&output).unwrap();

        assert_eq!(
            serialized.len(),
            298,
            "Serialized output should be exactly 298 bytes with no length prefix"
        );

        assert_eq!(&serialized[..8], state_len.to_le_bytes());

        let state_bytes = bincode::serialize(&state).unwrap();
        let new_state_bytes = bincode::serialize(&new_state).unwrap();
        assert_eq!(state_bytes.len(), 141, "State should serialize to 141 bytes");
        assert_eq!(new_state_bytes.len(), 141, "State should serialize to 141 bytes");

        assert_eq!(&serialized[8..149], state_bytes.as_slice());
        assert_eq!(&serialized[149..157], new_state_len.to_le_bytes());
        assert_eq!(&serialized[157..298], new_state_bytes.as_slice());

        let deserialized: BatchExecOutput = bincode::deserialize(&serialized).unwrap();

        assert_eq!(output, deserialized);
    }
}
