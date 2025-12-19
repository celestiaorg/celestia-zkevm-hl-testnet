//! An SP1 program that verifies the execution and inclusion of ev-reth blocks in
//! the Celestia data availability network.
//!
//! This program combines the functionality contained within `ev-exec` and `ev-range-exec`.
//!
//! It accepts N BlockExecInputs which include:
//! - Celestia block header and associated data availability header (DAH).
//! - Namespace
//! - Blobs
//! - Sequencer Public Key
//! - NamespaceProofs
//! - EthClientExecutorInputs (RSP - state transition function)
//! - Trusted Height
//! - Trusted State Root
//!
//! For each Celestia block it performs the following steps:
//! 1. Deserializes the program inputs.
//! 2. Verifies completeness of the namespace using the provided blobs.
//! 3. Executes the EVM blocks via the state transition function.
//! 4. Filters blobs to SignedData and verifies the sequencer signature.
//! 5. Verifies equivalency between the EVM block data and blob data via SignedData.
//!
//! The program then verifies sequentiality of each Celestia block output, ensuring a contiguous series.
//!
//! It commits:
//! - The trusted Celestia block height and header hash.
//! - The new Celestia block height and header hash.
//! - The trusted ev-reth block height and state root
//! - The new ev-reth block height and state root
//! - The Namespace and public key of the sequencer.

#![no_main]
sp1_zkvm::entrypoint!(main);

use ev_zkevm_types::programs::block::{BatchExecInput, BlockVerifier};
use tendermint_light_client_verifier::types::LightBlock;

pub fn main() {
    let input: BatchExecInput = sp1_zkvm::io::read::<BatchExecInput>();

    // Deserialize light blocks from CBOR (bincode doesn't work with tendermint's serde attrs)
    let trusted_light_block: LightBlock =
        serde_cbor::from_slice(&input.trusted_light_block_raw).expect("failed to deserialize trusted light block");
    let new_light_block: LightBlock =
        serde_cbor::from_slice(&input.new_light_block_raw).expect("failed to deserialize new light block");

    let block_verifier = BlockVerifier {};
    let output = block_verifier
        .verify_range(input.blocks, trusted_light_block, new_light_block)
        .expect("failed to verify range");
    sp1_zkvm::io::commit(&output);
}
