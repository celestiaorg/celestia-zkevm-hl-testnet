use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use alloy_consensus::{BlockHeader, proofs};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use bytes::Bytes;
use celestia_types::Blob;
use celestia_types::nmt::{EMPTY_LEAVES, NamespacedHash};
use ed25519_dalek::{Signature, Verifier as TendermintVerifier, VerifyingKey};
use ev_types::v1::{Data, SignedData};
use nmt_rs::NamespacedSha2Hasher;
use prost::Message;
use reth_primitives::TransactionSigned;
use rsp_client_executor::{executor::EthClientExecutor, io::WitnessInput};
use tendermint::{Time, block::Header};
use tendermint_light_client_verifier::{
    ProdVerifier, Verdict, Verifier,
    options::Options,
    types::{LightBlock, TrustThreshold},
};

use super::io::{BatchExecOutput, BlockExecInput, BlockExecOutput, State};

pub struct BlockVerifier;

impl BlockVerifier {
    fn verify_tendermint(
        &self,
        trusted_light_block: LightBlock,
        new_light_block: LightBlock,
        now: Time,
    ) -> Result<(), Box<dyn Error>> {
        let vp = ProdVerifier::default();
        let opt = Options {
            trust_threshold: TrustThreshold::TWO_THIRDS,
            trusting_period: Duration::from_secs(14 * 24 * 60 * 60),
            clock_drift: Default::default(),
        };
        let verdict = vp.verify_update_header(
            new_light_block.as_untrusted_state(),
            trusted_light_block.as_trusted_state(),
            &opt,
            now,
        );
        match verdict {
            Verdict::Success => {
                println!(
                    "Verified light client update from height {} to height {}!",
                    trusted_light_block.signed_header.header.height.value(),
                    new_light_block.signed_header.header.height.value()
                );
            }
            Verdict::NotEnoughTrust(voting_power_tally) => {
                panic!("Not enough trust in the trusted header, voting power tally: {voting_power_tally:?}");
            }
            Verdict::Invalid(err) => {
                panic!("Could not verify updating to target_block, error: {err:?}")
            }
        };
        Ok(())
    }

    pub fn verify_block(input: BlockExecInput) -> Result<BlockExecOutput, Box<dyn Error>> {
        let celestia_header: Header =
            serde_cbor::from_slice(&input.header_raw).expect("failed to deserialize celestia header");
        let blobs: Vec<Blob> = serde_cbor::from_slice(&input.blobs_raw).expect("failed to deserialize blob data");

        assert_eq!(
            celestia_header.data_hash.unwrap(),
            input.dah.hash(),
            "DataHash mismatch for DataAvailabilityHeader"
        );

        let mut roots = Vec::<&NamespacedHash>::new();
        for row_root in input.dah.row_roots() {
            if row_root.contains::<NamespacedSha2Hasher<29>>(input.namespace.into()) {
                roots.push(row_root);
            }
        }

        assert_eq!(
            roots.len(),
            input.proofs.len(),
            "Number of proofs must equal the number of row roots"
        );

        if roots.is_empty() {
            assert!(blobs.is_empty(), "Blobs must be empty if no roots contain namespace");
        }

        let blob_data: Vec<[u8; 512]> = blobs
            .iter()
            .flat_map(|blob| {
                blob.to_shares()
                    .unwrap()
                    .into_iter()
                    .map(|share| share.as_ref().try_into().unwrap())
            })
            .collect();

        let mut cursor = 0;
        for (proof, root) in input.proofs.iter().zip(roots) {
            if proof.is_of_absence() {
                proof
                    .verify_complete_namespace(root, EMPTY_LEAVES, input.namespace.into())
                    .expect("Failed to verify proof");
                break;
            }
            let share_count = (proof.end_idx() - proof.start_idx()) as usize;
            let end = cursor + share_count;

            let raw_leaves = &blob_data[cursor..end];

            proof
                .verify_complete_namespace(root, raw_leaves, input.namespace.into())
                .expect("Failed to verify proof");

            cursor = end;
        }

        let mut headers = Vec::with_capacity(input.executor_inputs.len());
        if headers.capacity() != 0 {
            let first_input = input.executor_inputs.first().unwrap();

            assert_eq!(
                input.trusted_root,
                first_input.state_anchor(),
                "State anchor must be equal to trusted root"
            );

            assert!(
                input.trusted_height <= first_input.parent_header().number(),
                "Trusted height must be less than or equal to parent header height",
            );

            let executor = EthClientExecutor::eth(
                Arc::new((&first_input.genesis).try_into().expect("invalid genesis block")),
                first_input.custom_beneficiary,
            );

            for input in &input.executor_inputs {
                let header = executor.execute(input.clone()).expect("EVM block execution failed");
                headers.push(header);
            }
        }

        let signed_data: Vec<SignedData> = blobs
            .into_iter()
            .filter_map(|blob| SignedData::decode(Bytes::from(blob.data)).ok())
            .collect();

        let mut tx_data: Vec<Data> = Vec::new();
        for sd in signed_data {
            let signer = sd.signer.as_ref().expect("SignedData must contain signer");

            if signer.pub_key[4..] != input.pub_key {
                continue;
            }

            let data_bytes = sd.data.as_ref().expect("SignedData must contain data").encode_to_vec();
            Self::verify_ed25519(&input.pub_key, &data_bytes, &sd.signature)
                .expect("Sequencer signature verification failed");

            tx_data.push(sd.data.unwrap());
        }

        if tx_data.len() != headers.len() {
            let mut seen = HashSet::<u64>::new();
            tx_data.retain(|data| get_height(data).map(|h| seen.insert(h)).unwrap_or(false));
        }
        tx_data.sort_by_key(|data| get_height(data).expect("Data must contain a height"));

        assert_eq!(
            tx_data.len(),
            headers.len(),
            "Headers and SignedData must be of equal length"
        );

        for (header, data) in headers.iter().zip(tx_data) {
            let mut txs = Vec::with_capacity(data.txs.len());
            for tx_bytes in data.txs {
                let tx = TransactionSigned::decode(&mut tx_bytes.as_slice()).expect("Failed decoding transaction");
                txs.push(tx);
            }

            let root = proofs::calculate_transaction_root(&txs);
            assert_eq!(
                root, header.transactions_root,
                "Calculated root must be equal to header transactions root"
            );
        }

        let new_height: u64 = headers.last().map(|h| h.number).unwrap_or(input.trusted_height);
        let new_state_root: B256 = headers.last().map(|h| h.state_root).unwrap_or(input.trusted_root);

        let output = BlockExecOutput {
            celestia_header_hash: celestia_header
                .hash()
                .as_bytes()
                .try_into()
                .expect("celestia_header_hash must be exactly 32 bytes"),
            prev_celestia_height: celestia_header.height.value() - 1,
            prev_celestia_header_hash: celestia_header
                .last_block_id
                .unwrap()
                .hash
                .as_bytes()
                .try_into()
                .expect("prev_celestia_header_hash must be exactly 32 bytes"),
            new_height,
            new_state_root: new_state_root.into(),
            prev_height: input.trusted_height,
            prev_state_root: input.trusted_root.into(),
            namespace: input.namespace,
            public_key: input.pub_key.try_into().expect("public key must be exactly 32 bytes"),
        };
        Ok(output)
    }

    pub fn verify_range(
        &self,
        inputs: Vec<BlockExecInput>,
        trusted_light_block: LightBlock,
        new_light_block: LightBlock,
    ) -> Result<BatchExecOutput, Box<dyn Error>> {
        let mut outputs: Vec<BlockExecOutput> = Vec::new();
        for block in inputs {
            outputs.push(Self::verify_block(block)?);
        }

        let now = (new_light_block.time() + Duration::from_secs(10)).expect("time overflow");

        self.verify_tendermint(trusted_light_block.clone(), new_light_block.clone(), now)?;

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

        let trusted_header_hash: [u8; 32] = trusted_light_block
            .signed_header
            .header
            .hash()
            .as_bytes()
            .try_into()
            .expect("trusted header hash must be 32 bytes");
        assert_eq!(
            first.prev_celestia_header_hash, trusted_header_hash,
            "First block's prev_celestia_header_hash must match trusted light block header hash"
        );

        let new_header_hash: [u8; 32] = new_light_block
            .signed_header
            .header
            .hash()
            .as_bytes()
            .try_into()
            .expect("new header hash must be 32 bytes");
        assert_eq!(
            last.celestia_header_hash, new_header_hash,
            "Last block's celestia_header_hash must match new light block header hash"
        );

        let new_state = State {
            state_root: last.new_state_root,
            height: last.new_height,
            celestia_header_hash: last.celestia_header_hash,
            celestia_height: first.prev_celestia_height + outputs.len() as u64,
            namespace: last
                .namespace
                .as_bytes()
                .try_into()
                .expect("namespace must be 29 bytes"),
            public_key: last.public_key,
        };

        let state_length_prefix = state.length();
        let new_state_length_prefix = new_state.length();

        let output = BatchExecOutput {
            state_len_bytes: state_length_prefix.to_le_bytes(),
            state,
            new_state_len_bytes: new_state_length_prefix.to_le_bytes(),
            new_state,
        };
        Ok(output)
    }

    fn verify_ed25519(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), Box<dyn Error>> {
        let pub_key: [u8; 32] = public_key
            .try_into()
            .map_err(|e| format!("Public key must be 32 bytes for Ed25519: {e}"))?;

        let verifying_key =
            VerifyingKey::from_bytes(&pub_key).map_err(|e| format!("Invalid Ed25519 public key: {e}"))?;
        let signature = Signature::from_slice(signature).map_err(|e| format!("Invalid Ed25519 signature: {e}"))?;

        verifying_key
            .verify(message, &signature)
            .map_err(|e| format!("Signature verification failed: {e}"))?;
        Ok(())
    }
}

fn get_height(data: &Data) -> Option<u64> {
    data.metadata.as_ref().map(|m| m.height)
}
