use std::sync::Arc;
use std::{fmt::Display, result::Result::Ok};

use anyhow::Result;
use async_trait::async_trait;
use sp1_sdk::env::EnvProver;
use sp1_sdk::{ProveRequest, Prover, ProverClient, SP1ProofWithPublicValues, SP1Stdin};
use tracing::debug;

pub mod abi;
pub mod chain;
pub mod config;
pub mod programs;
pub mod service;
pub mod sync;

pub use config::{ProverConfig, ProverMode};
pub use sync::{MessageProofPermit, MessageProofRequest, MessageProofSync};

pub type SP1Prover = EnvProver;

/// ProgramProver is a trait implemented per SP1 program*.
///
/// Associated types let each program pick its own Input and Output context.
#[async_trait]
pub trait ProgramProver {
    /// Config implements the BaseProverConfig trait while allowing per implementation extensions.
    type Config: ProverConfig + Send + Sync + 'static;
    /// Context needed to build the stdin for this program.
    type Input: Send + 'static;
    /// Output data to return alongside the proof.
    type Output: Send + 'static;

    /// Returns the program configuration containing the ELF and proof mode.
    fn cfg(&self) -> &Self::Config;

    /// Build the program stdin from the prover inputs.
    fn build_stdin(&self, input: Self::Input) -> Result<SP1Stdin>;

    /// Prove produces a proof and parsed outputs.
    /// The default implementation matches the configured proof mode and program elf from the prover config.
    async fn prove(&self, input: Self::Input) -> Result<(SP1ProofWithPublicValues, Self::Output)> {
        let cfg = self.cfg();
        let stdin = self.build_stdin(input)?;
        let prover = self.prover();

        // The single-GPU CUDA pipeline must be re-initialized with a fresh `setup()`
        // before every proof; reusing a proving key from a prior setup confuses it.
        // For all other backends we reuse the proving key computed once at construction.
        let proof = if ProverMode::is_cuda() {
            let pk = prover.setup(cfg.elf()).await?;
            prover.prove(&pk, stdin).mode(cfg.proof_mode()).await?
        } else {
            let pk = cfg.pk();
            prover.prove(pk.as_ref(), stdin).mode(cfg.proof_mode()).await?
        };

        let output = self.post_process(proof.clone())?;
        Ok((proof, output))
    }

    /// Returns the SP1 Prover.
    fn prover(&self) -> Arc<SP1Prover>;

    /// Parse or convert program outputs.
    fn post_process(&self, proof: SP1ProofWithPublicValues) -> Result<Self::Output>;
}

/// Construct a prover based on the SP1_PROVER environment variable.
///
/// In SP1 v6 the [`EnvProver`] returned by [`ProverClient::from_env`] internally
/// dispatches to the mock/cpu/cuda/network backend based on the `SP1_PROVER`
/// environment variable (network mode is configured via `NETWORK_RPC_URL` and
/// `NETWORK_PRIVATE_KEY`), so a single concrete type replaces the old
/// `Arc<dyn Prover>` trait object.
pub async fn prover_from_env() -> Arc<SP1Prover> {
    let mode: ProverMode = ProverMode::from_env();
    debug!("Using {mode:?} prover backend");

    Arc::new(ProverClient::from_env().await)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeProofCommitted {
    pub trusted_height: u64,
    pub trusted_root: [u8; 32],
}

impl RangeProofCommitted {
    pub fn new(trusted_height: u64, trusted_root: [u8; 32]) -> Self {
        Self {
            trusted_height,
            trusted_root,
        }
    }

    pub fn trusted_height(&self) -> u64 {
        self.trusted_height
    }

    pub fn trusted_root(&self) -> [u8; 32] {
        self.trusted_root
    }
}

impl Display for RangeProofCommitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "height={} root=0x{}",
            self.trusted_height(),
            hex::encode(self.trusted_root())
        )
    }
}
