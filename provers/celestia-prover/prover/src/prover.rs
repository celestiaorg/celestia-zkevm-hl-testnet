//! Prover for SP1 ICS07 Tendermint programs.

use std::env;

use crate::programs::{MembershipProgram, SP1Program, UpdateClientProgram};
use alloy_sol_types::SolValue;
use ibc_client_tendermint_types::Header;
use ibc_core_commitment_types::merkle::MerkleProof;
use ibc_eureka_solidity_types::sp1_ics07::IICS07TendermintMsgs::{
    ClientState as SolClientState, ConsensusState as SolConsensusState,
};
use ibc_eureka_solidity_types::sp1_ics07::IMembershipMsgs::KVPair;
use ibc_proto::{ibc::lightclients::tendermint::v1::Header as RawHeader, Protobuf};
use sp1_sdk::{
    EnvProver, ProverClient, SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey,
};

/// A prover for [`SP1Program`] programs.
#[allow(clippy::module_name_repetitions)]
pub struct SP1ICS07TendermintProver<T: SP1Program> {
    /// [`sp1_sdk::ProverClient`] for generating proofs.
    pub prover_client: EnvProver,
    /// The proving key.
    pub pkey: SP1ProvingKey,
    /// The verifying key.
    pub vkey: SP1VerifyingKey,
    /// The proof type.
    pub proof_type: SupportedProofType,
    _phantom: std::marker::PhantomData<T>,
}

/// The supported proof types.
#[derive(Clone, Debug, Copy)]
pub enum SupportedProofType {
    /// Groth16 proof.
    Groth16,
}

impl<T: SP1Program> SP1ICS07TendermintProver<T> {
    /// Create a new prover.
    #[must_use]
    #[tracing::instrument(skip_all)]
    pub fn new(proof_type: SupportedProofType) -> Self {
        tracing::info!("Initializing SP1 ProverClient...");
        if let Ok(mode) = env::var("SP1_PROVER") {
            println!("SP1_Prover mode: {mode}");
        };
        let prover_client = ProverClient::from_env();
        let (pkey, vkey) = prover_client.setup(T::ELF);
        tracing::info!("SP1 ProverClient initialized");
        Self {
            prover_client,
            pkey,
            vkey,
            proof_type,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Prove the given input.
    /// # Panics
    /// If the proof cannot be generated or validated.
    #[must_use]
    pub fn prove(&self, stdin: &SP1Stdin) -> SP1ProofWithPublicValues {
        // Generate the proof. Depending on SP1_PROVER env variable, this may be a mock, local or
        // network proof.
        let proof: SP1ProofWithPublicValues = match self.proof_type {
            SupportedProofType::Groth16 => self
                .prover_client
                .prove(&self.pkey, stdin)
                .groth16()
                .run()
                .expect("proving failed"),
        };

        self.prover_client
            .verify(&proof, &self.vkey)
            .expect("verification failed");

        proof
    }
}

impl SP1ICS07TendermintProver<UpdateClientProgram> {
    /// Generate a proof of an update from `trusted_consensus_state` to a proposed header.
    ///
    /// # Panics
    /// Panics if the inputs cannot be encoded, the proof cannot be generated or the proof is
    /// invalid.
    #[must_use]
    pub fn generate_proof(
        &self,
        client_state: &SolClientState,
        trusted_consensus_state: &SolConsensusState,
        proposed_header: &Header,
        time: u64,
    ) -> SP1ProofWithPublicValues {
        // Encode the inputs into our program.
        let encoded_1 = client_state.abi_encode();
        let encoded_2 = trusted_consensus_state.abi_encode();
        let mut encoded_3 = vec![];
        <Header as Protobuf<RawHeader>>::encode(proposed_header.clone(), &mut encoded_3)
            .expect("Failed to encode header");
        let encoded_4 = time.to_le_bytes().into();

        // Write the encoded inputs to stdin.
        let mut stdin = SP1Stdin::new();
        stdin.write_vec(encoded_1);
        stdin.write_vec(encoded_2);
        stdin.write_vec(encoded_3);
        stdin.write_vec(encoded_4);

        self.prove(&stdin)
    }
}

impl SP1ICS07TendermintProver<MembershipProgram> {
    /// Generate a proof of verify (non)membership for multiple key-value pairs.
    ///
    /// # Panics
    /// Panics if the proof cannot be generated or the proof is invalid.
    #[must_use]
    pub fn generate_proof(
        &self,
        commitment_root: &[u8],
        kv_proofs: Vec<(KVPair, MerkleProof)>,
    ) -> SP1ProofWithPublicValues {
        assert!(!kv_proofs.is_empty(), "No key-value pairs to prove");
        let len = u16::try_from(kv_proofs.len()).expect("too many key-value pairs");

        let mut stdin = SP1Stdin::new();
        stdin.write_slice(commitment_root);
        stdin.write_slice(&len.to_le_bytes());
        for (kv_pair, proof) in kv_proofs {
            stdin.write_vec(kv_pair.abi_encode());
            stdin.write_vec(proof.encode_vec());
        }

        self.prove(&stdin)
    }
}

impl TryFrom<u8> for SupportedProofType {
    type Error = String;

    fn try_from(n: u8) -> Result<Self, Self::Error> {
        match n {
            0 => Ok(Self::Groth16),
            n => Err(format!("Unsupported proof type: {n}")),
        }
    }
}
