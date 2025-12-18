//! An end-to-end example of using the SP1 SDK to generate a proof of a program that can be executed
//! or have a core proof generated.
//!
//! You can run this script using the following command:
//! ```shell
//! RUST_LOG=info cargo run -p ev-hyperlane-script --release -- --execute --contract 0xFCb1d485ef46344029D9E8A7925925e146B3430E --start-idx 0 --end-idx 23 --target-height 268 --rpc-url http://127.0.0.1:8545
//! ```
//! or
//! ```shell
//! RUST_LOG=info cargo run -p ev-hyperlane-script --release -- --prove --contract 0xFCb1d485ef46344029D9E8A7925925e146B3430E --start-idx 0 --end-idx 23 --target-height 268 --rpc-url http://127.0.0.1:8545
//! ```

use alloy_primitives::{hex::FromHex, Address, FixedBytes};
use alloy_provider::{Provider, ProviderBuilder};
use anyhow::{Context, Result};
use clap::Parser;
use ev_zkevm_types::programs::hyperlane::{
    tree::MerkleTree,
    types::{HyperlaneBranchProof, HyperlaneBranchProofInputs, HyperlaneMessageInputs, HYPERLANE_MERKLE_TREE_KEYS},
};
use sp1_sdk::{include_elf, ProverClient, SP1Stdin};
use std::{env, str::FromStr, time::Instant};
use storage::hyperlane::message::HyperlaneMessageStore;
use url::Url;
#[cfg(feature = "retry")]
use {
    celestia_grpc_client::{
        types::ClientConfig, CelestiaIsmClient, MsgProcessMessage, MsgSubmitMessages, QueryIsmRequest,
    },
    ev_zkevm_types::{hyperlane::encode_hyperlane_message, programs::block::State},
    storage::hyperlane::snapshot::HyperlaneSnapshotStore,
};

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const EV_HYPERLANE_ELF: &[u8] = include_elf!("ev-hyperlane-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[cfg(not(feature = "retry"))]
struct Args {
    #[arg(long)]
    execute: bool,

    #[arg(long)]
    prove: bool,

    #[arg(long)]
    contract: String,

    #[arg(long)]
    from_height: u64,

    #[arg(long)]
    to_height: u64,

    #[arg(long)]
    rpc_url: String,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[cfg(feature = "retry")]
struct Args {
    #[arg(long)]
    contract: String,
    #[arg(long)]
    snapshot_index: u64,
    #[arg(long)]
    mailbox_id: String,
    #[arg(long)]
    rpc_url: String,
}

#[tokio::main]
async fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to set default crypto provider");

    dotenvy::dotenv().ok();
    sp1_sdk::utils::setup_logger();
    let args = Args::parse();

    let client = ProverClient::from_env();
    let message_storage_path = dirs::home_dir()
        .expect("cannot find home directory")
        .join(".ev-prover")
        .join("data");
    let hyperlane_message_store = HyperlaneMessageStore::from_path(message_storage_path).unwrap();

    #[cfg(not(feature = "retry"))]
    {
        if args.execute == args.prove {
            eprintln!("Error: You must specify either --execute or --prove");
            std::process::exit(1);
        }

        let mut stdin = SP1Stdin::new();
        write_proof_inputs(
            &mut stdin,
            &hyperlane_message_store,
            &args.contract,
            &args.rpc_url,
            args.from_height,
            args.to_height,
        )
        .await
        .expect("failed to write proof inputs");

        if args.execute {
            client
                .execute(EV_HYPERLANE_ELF, &stdin)
                .run()
                .expect("failed to execute program");
            println!("Program executed successfully!");
        } else {
            use ev_zkevm_types::programs::hyperlane::types::HyperlaneMessageOutputs;
            let (pk, vk) = client.setup(EV_HYPERLANE_ELF);
            let start_time = Instant::now();
            let proof = client.prove(&pk, &stdin).run().expect("failed to generate proof");
            println!("Proof generation time: {:?}", Instant::now() - start_time);
            println!("Successfully generated proof!");
            client.verify(&proof, &vk).expect("failed to verify proof");
            println!("Successfully verified proof!");
            let proof_outputs: HyperlaneMessageOutputs =
                bincode::deserialize(proof.public_values.as_slice()).expect("Failed to deserialize proof outputs");
            println!("Proof outputs: {proof_outputs:?}");
        }
    }

    #[cfg(feature = "retry")]
    {
        let snapshot_storage_path = dirs::home_dir()
            .expect("cannot find home directory")
            .join(".ev-prover")
            .join("data");
        let hyperlane_snapshot_store = HyperlaneSnapshotStore::from_path(snapshot_storage_path).unwrap();
        let previous_snapshot = hyperlane_snapshot_store
            .get_snapshot(args.snapshot_index)
            .expect("Fatal, snapshot was lost");

        let config = ClientConfig::from_env().unwrap();
        let ism_client = CelestiaIsmClient::new(config.clone()).await.unwrap();
        let ev_trusted_state = ism_client
            .ism(QueryIsmRequest {
                id: config.ism_id.clone(),
            })
            .await
            .unwrap();
        let state: State = bincode::deserialize(&ev_trusted_state.ism.unwrap().state).unwrap();
        let mut stdin = SP1Stdin::new();

        write_proof_inputs(
            &mut stdin,
            &hyperlane_message_store,
            &args.contract,
            &args.rpc_url,
            previous_snapshot.height,
            state.height,
        )
        .await
        .unwrap();

        let (pk, _) = client.setup(EV_HYPERLANE_ELF);
        let start_time = Instant::now();

        let proof = client
            .prove(&pk, &stdin)
            .groth16()
            .run()
            .expect("failed to generate proof");
        println!("Proof generation time: {:?}", Instant::now() - start_time);

        let proof_msg = MsgSubmitMessages::new(
            config.ism_id,
            state.height,
            proof.bytes(),
            proof.public_values.as_slice().to_vec(),
            config.signer_address.clone(),
        );

        let response = ism_client.send_tx(proof_msg).await.unwrap();
        if !response.success {
            panic!("Failed to submit proof: {:?}", response);
        }

        // Relay verified messages to Celestia
        let mut messages = Vec::new();
        for height in previous_snapshot.height..=state.height {
            let block_messages = hyperlane_message_store
                .get_by_block(height)
                .expect("Failed to get messages");
            for block_message in block_messages {
                messages.push(block_message);
            }
        }

        for message in messages {
            let message_hex = alloy::hex::encode(encode_hyperlane_message(&message.message).unwrap());
            let msg = MsgProcessMessage::new(
                args.mailbox_id.clone(),
                config.signer_address.clone(),
                alloy::hex::encode(vec![]),
                message_hex,
            );
            let response = match ism_client.send_tx(msg).await {
                Ok(response) => response,
                Err(e) => {
                    eprintln!("Failed to relay message: {:?}", e);
                    continue;
                }
            };
            if !response.success {
                eprintln!("Failed to relay message: {:?}", response);
            } else {
                println!("Successfully relayed message: {:?}", message.message.id());
            }
        }
    }
}

async fn write_proof_inputs(
    stdin: &mut SP1Stdin,
    message_store: &HyperlaneMessageStore,
    contract: &str,
    rpc_url: &str,
    from_height: u64,
    to_height: u64,
) -> Result<()> {
    let mut messages = Vec::new();
    for height in from_height..=to_height {
        let block_messages = message_store.get_by_block(height).expect("Failed to get messages");
        for block_message in block_messages {
            messages.push(block_message);
        }
    }
    let provider = ProviderBuilder::new().connect_http(Url::from_str(rpc_url).expect("Failed to create provider"));
    let proof = provider
        .get_proof(
            Address::from_str(contract).expect("Failed to create contract address"),
            HYPERLANE_MERKLE_TREE_KEYS
                .iter()
                .map(|k| FixedBytes::from_hex(k).expect("Failed to create fixed bytes"))
                .collect(),
        )
        .block_id(alloy_eips::BlockId::Number(alloy_eips::BlockNumberOrTag::Number(
            to_height,
        )))
        .await
        .expect("Failed to get proof");
    let block = provider
        .get_block(alloy_eips::BlockId::Number(alloy_eips::BlockNumberOrTag::Number(
            to_height,
        )))
        .await?
        .context("Failed to get block")?;
    let execution_state_root = alloy::hex::encode(block.header.state_root.0);
    let branch_proof = HyperlaneBranchProof::new(proof);
    let inputs = HyperlaneMessageInputs::new(
        execution_state_root,
        contract.to_string(),
        messages.into_iter().map(|m| m.message).collect(),
        HyperlaneBranchProofInputs::from(branch_proof),
        MerkleTree::default(),
    );
    stdin.write(&inputs);
    Ok(())
}
