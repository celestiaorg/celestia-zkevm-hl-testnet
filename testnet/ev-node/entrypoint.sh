#!/bin/sh
set -e

cd /usr/bin

sleep 5

# Create default evnode config if missing
# TODO: The --evnode.signer.path flag is not respected: https://github.com/evstack/ev-node/issues/2603
if [ ! -f "$HOME/.evm-single/config/genesis.json" ]; then
  # Run init to create directory structure and initial config
  ./evm-single init --evnode.node.aggregator=true --evnode.signer.passphrase_file "/config/passphrase.txt"

  # Replace the randomly generated signer.json with the deterministic one
  cp /config/signer.json "$HOME/.evm-single/config/signer.json"

  # Update genesis.json with the correct proposer address
  # The deterministic signer's public key (hex): 3964a68700cf76e215626e076e76d23bd1f4c3b31184b5822fd7b4df15d5ce9a
  # SHA256 hash of pubkey (full 32 bytes): acef3b0d46e3c7ed6d37c8d5ae926203a9d514634da115dd3417763de0523189
  # In base64 (Go JSON []byte encoding): rO87DUbjx+1tN8jVrpJiA6nVFGNNoRXdNBd2PeBSMYk=
  CORRECT_PROPOSER="rO87DUbjx+1tN8jVrpJiA6nVFGNNoRXdNBd2PeBSMYk="

  # Update the genesis.json file
  sed -i "s/\"proposer_address\": *\"[^\"]*\"/\"proposer_address\": \"$CORRECT_PROPOSER\"/" "$HOME/.evm-single/config/genesis.json"
fi

# Conditionally add --evnode.da.address if DA_ADDRESS is set
da_flag=""
if [ -n "$DA_ADDRESS" ]; then
  da_flag="--evnode.da.address $DA_ADDRESS"
fi

# Conditionally add --evnode.da.auth_token if DA_AUTH_TOKEN is set
da_auth_token_flag=""
if [ -n "$DA_AUTH_TOKEN" ]; then
  da_auth_token_flag="--evnode.da.auth_token $DA_AUTH_TOKEN"
fi

# Conditionally add --evnode.da.header_namespace and --evnode.da.data_namespace if set
da_header_namespace_flag=""
if [ -n "$DA_HEADER_NAMESPACE" ]; then
  da_header_namespace_flag="--evnode.da.namespace $DA_HEADER_NAMESPACE"
fi

da_data_namespace_flag=""
if [ -n "$DA_DATA_NAMESPACE" ]; then
  da_data_namespace_flag="--evnode.da.data_namespace $DA_DATA_NAMESPACE"
fi

exec ./evm-single start \
  --evm.jwt-secret-file "/config/jwt.hex" \
  --evm.genesis-hash $EVM_GENESIS_HASH \
  --evm.engine-url $EVM_ENGINE_URL \
  --evm.eth-url $EVM_ETH_URL \
  --evnode.node.block_time $EVM_BLOCK_TIME \
  --evnode.node.aggregator=true \
  --evnode.rpc.address "0.0.0.0:7331" \
  --evnode.signer.passphrase_file "/config/passphrase.txt" \
  --evnode.signer.signer_path "$HOME/.evm-single/config" \
  $da_flag \
  $da_auth_token_flag \
  $da_header_namespace_flag \
  $da_data_namespace_flag