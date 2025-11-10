use ev_zkevm_types::programs::block::{BlockVerifier, EvCombinedInput};
use risc0_zkvm::guest::env;

fn main() {
    let input: EvCombinedInput = env::read::<EvCombinedInput>();
    let output = BlockVerifier::verify_range(input.blocks).expect("failed to verify range");
    env::commit(&output);
}
