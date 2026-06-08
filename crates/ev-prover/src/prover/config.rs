use std::env;
use std::sync::Arc;

use sp1_sdk::env::EnvProvingKey;
use sp1_sdk::{Elf, SP1ProofMode, SP1VerifyingKey};
use tracing::warn;

// TODO: move these values to config.yaml
pub const BATCH_SIZE: u64 = 1000;
pub const MIN_BATCH_SIZE: u64 = 10;
pub const MAX_BATCH_SIZE: u64 = 100000;
pub const WARN_DISTANCE: u64 = 1500;
pub const MAX_INDEXING_RANGE: u64 = 100000;

/// ProverConfig defines a core capability trait for configs used by a ProgramProver.
pub trait ProverConfig {
    fn pk(&self) -> Arc<EnvProvingKey>;
    fn vk(&self) -> Arc<SP1VerifyingKey>;
    fn proof_mode(&self) -> SP1ProofMode;
    /// The program ELF, retained so the prover can re-run `setup()` per proof
    /// (required by the single-GPU CUDA pipeline).
    fn elf(&self) -> Elf;
}

/// StandardProverConfig is the default implementation of ProverConfig shared by most provers.
#[derive(Clone)]
pub struct StandardProverConfig {
    pub pk: Arc<EnvProvingKey>,
    pub vk: Arc<SP1VerifyingKey>,
    pub proof_mode: SP1ProofMode,
    pub elf: Elf,
}

impl StandardProverConfig {
    pub fn new(pk: EnvProvingKey, vk: SP1VerifyingKey, elf: Elf, mode: SP1ProofMode) -> Self {
        Self {
            pk: Arc::new(pk),
            vk: Arc::new(vk),
            proof_mode: mode,
            elf,
        }
    }
}

impl ProverConfig for StandardProverConfig {
    fn pk(&self) -> Arc<EnvProvingKey> {
        Arc::clone(&self.pk)
    }

    fn vk(&self) -> Arc<SP1VerifyingKey> {
        Arc::clone(&self.vk)
    }

    fn proof_mode(&self) -> SP1ProofMode {
        self.proof_mode
    }

    fn elf(&self) -> Elf {
        self.elf.clone()
    }
}

/// ProverMode defines the backend used for proving: [Mock, CPU, Cuda, Network].
#[derive(Debug, Clone, Copy)]
pub enum ProverMode {
    Mock,
    Cpu,
    Cuda,
    Network,
}

impl ProverMode {
    /// Returns the ProverMode by reading the SP1_PROVER environment variable.
    /// If SP1_PROVER is not set, this method provides a fallback of Mock mode.
    pub fn from_env() -> ProverMode {
        let mode_str = env::var("SP1_PROVER").unwrap_or_default();

        match mode_str.trim().to_ascii_lowercase().as_str() {
            "mock" => Self::Mock,
            "cpu" => Self::Cpu,
            "cuda" => Self::Cuda,
            "network" => Self::Network,
            _ => {
                warn!("SP1_PROVER unset or invalid ('{mode_str}'), defaulting to mock mode");
                Self::Mock
            }
        }
    }

    /// Returns true if the CUDA backend is selected via `SP1_PROVER=cuda`.
    ///
    /// Used to force a fresh `setup()` before every proof: the single-GPU CUDA
    /// pipeline gets confused if a proving key from an earlier setup is reused.
    /// Does not emit the unset/invalid warning that [`Self::from_env`] does.
    pub fn is_cuda() -> bool {
        env::var("SP1_PROVER")
            .map(|v| v.trim().eq_ignore_ascii_case("cuda"))
            .unwrap_or(false)
    }
}
