pub mod events;
pub mod io;
pub mod merkle;
pub mod message;
pub mod proof;

use sha3::{Digest, Keccak256};

pub use message::*;

pub(crate) fn digest_keccak(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}
