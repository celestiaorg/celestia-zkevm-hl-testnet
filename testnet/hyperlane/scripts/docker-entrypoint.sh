#!/bin/bash

# The following docker-entrypoint script performs deployment of Hyperlane infrastructure
# on both ev-reth and celestia.
# To minimise proving time in the docker env in this repository we first deploy
# a noop ism stack on celestia and finally overwrite this with a new zk ism deployment.
# This ensures that the initial trusted root used in the zk ism is the same as the
# latest block's state root in ev-reth.

set -euo pipefail

# Wait for ev-node sequencer to be ready
echo "Waiting for ev-node sequencer to be ready..."
sleep 10

# HYP_KEY is the priv key of the EVM account used for Hyperlane contract deployment
export HYP_KEY=0x82bfcfadbf1712f6550d8d2c00a39f05b33ec78939d0167be2a737d691f33a6a
export HYP_KEY_COSMOSNATIVE=0x6e30efb1d3ebd30d1ba08c8d5fc9b190e08394009dc1dd787a69e60c33288a8c

echo "Using Hyperlane registry:"
hyperlane registry list --registry ./registry

echo "Deploying Hyperlane core on Evolve..."
hyperlane core deploy --chain evolve --config ./configs/evolve-core.yaml --registry ./registry --yes
hyperlane core read --chain evolve --config configs/evolve-core.yaml --registry ./registry

echo "Deploying Hyperlane core on Celestia..."
hyperlane core deploy --chain celestiadev --config ./configs/celestia-core.yaml --registry ./registry --yes
hyperlane core read --chain celestiadev --config configs/celestia-core.yaml --registry ./registry

echo "Deploying TIA warp route..."
hyperlane warp deploy --warp-route-id TIA --registry ./registry --yes
