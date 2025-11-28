//! An SP1 program that verifies a sequence of N `ev-exec` proofs.
//!
//! It accepts:
//! - N verification keys
//! - N serialized public values (each from a `EvmBlockExecOutput`)
//!
//! It performs:
//! 1. Proof verification for each input
//! 2. Sequential header verification (i.e., block continuity)
//! 3. Aggregation of metadata into a `EvmRangeExecOutput`
//!
//! It commits:
//! - The trusted block height and state root
//! - The new block height and state root
//! - The latest Celestia header hash from the sequence

#![no_main]
sp1_zkvm::entrypoint!(main);

use ev_zkevm_types::programs::block::{BlockExecOutput, BlockRangeExecInput, BlockRangeExecOutput, State};
use sha2::{Digest, Sha256};
use sp1_primitives::types::Buffer;

pub fn main() {
    let inputs: BlockRangeExecInput = sp1_zkvm::io::read::<BlockRangeExecInput>();

    assert_eq!(
        inputs.vkeys.len(),
        inputs.public_values.len(),
        "mismatch between number of verification keys and public value blobs"
    );

    let proof_count = inputs.vkeys.len();

    for i in 0..proof_count {
        let digest = Sha256::digest(&inputs.public_values[i]);
        sp1_zkvm::lib::verify::verify_sp1_proof(&inputs.vkeys[i], &digest.into());
    }

    let outputs: Vec<BlockExecOutput> = inputs
        .public_values
        .iter()
        .map(|bytes| {
            let mut buffer = Buffer::from(bytes);
            buffer.read::<BlockExecOutput>()
        })
        .collect();

    for window in outputs.windows(2).enumerate() {
        let (i, pair) = window;
        let (prev, curr) = (&pair[0], &pair[1]);
        assert_eq!(
            curr.prev_height,
            prev.new_height,
            "verify sequential EVM headers failed at index {}: expected {:?}, got {:?}",
            i + 1,
            prev.new_height,
            curr.prev_height
        );

        assert_eq!(
            curr.prev_state_root,
            prev.new_state_root,
            "verify sequential EVM state roots failed at index {}: expected {:?}, got {:?}",
            i + 1,
            prev.new_state_root,
            curr.prev_state_root
        );

        assert_eq!(
            curr.prev_celestia_header_hash,
            prev.celestia_header_hash,
            "verify sequential Celestia headers failed at index {}: expected {:?}, got {:?}",
            i + 1,
            prev.celestia_header_hash,
            curr.prev_celestia_header_hash
        );

        assert_eq!(
            curr.namespace, prev.namespace,
            "unexpected namespace: expected {:?}, got {:?}",
            prev.namespace, curr.namespace
        );

        assert_eq!(
            curr.public_key, prev.public_key,
            "unexpected public key: expected {:?}, got {:?}",
            prev.public_key, curr.public_key
        );
    }

    let first = outputs.first().expect("No outputs provided");
    let last = outputs.last().expect("No outputs provided");

    let state = State {
        state_root: first.prev_state_root,
        height: first.prev_height,
        celestia_header_hash: first.prev_celestia_header_hash,
        celestia_height: first.prev_celestia_height,
        namespace: first
            .namespace
            .as_bytes()
            .try_into()
            .expect("namespace must be 29 bytes"),
        public_key: first.public_key,
    };

    let new_state = State {
        state_root: last.new_state_root,
        height: last.new_height,
        celestia_header_hash: last.celestia_header_hash,
        celestia_height: last.prev_celestia_height + inputs.public_values.len() as u64,
        namespace: last
            .namespace
            .as_bytes()
            .try_into()
            .expect("namespace must be 29 bytes"),
        public_key: last.public_key,
    };

    let state_length_prefix = bincode::serialize(&state).expect("failed to serialize state").len() as u64;
    let new_state_length_prefix = bincode::serialize(&new_state)
        .expect("failed to serialize new_state")
        .len() as u64;

    let output = BlockRangeExecOutput {
        state_len_bytes: state_length_prefix.to_le_bytes(),
        state,
        new_state_len_bytes: new_state_length_prefix.to_le_bytes(),
        new_state,
    };

    sp1_zkvm::io::commit(&output);
}
