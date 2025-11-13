#!/bin/bash
set -e

# Configuration
export HYP_KEY="${HYP_KEY:-0x82bfcfadbf1712f6550d8d2c00a39f05b33ec78939d0167be2a737d691f33a6a}"
RPC_URL="http://localhost:8545"
CELESTIA_RPC="http://localhost:26657"
DOMAIN_CELESTIA=69420
DOMAIN_EVM=1234
REGISTRY_PATH="hyperlane/registry"
ISM_ID="0x726f757465725f69736d000000000000000000000000002a0000000000000001"

echo "==> Deploying Native EVM Token Warp Route to Celestia"

# Step 1: Deploy HypNative contract on EVM
echo "Step 1: Deploying HypNative contract on EVM..."
hyperlane warp deploy \
  --registry "$REGISTRY_PATH" \
  --config "$REGISTRY_PATH/deployments/warp_routes/ETH/rethlocal-deploy.yaml" \
  --yes \
  > /tmp/warp-deploy.log 2>&1

# Extract deployed contract address
HYPNATIVE_ADDRESS=$(grep "addressOrDenom:" "$REGISTRY_PATH/deployments/warp_routes/ETH/rethlocal-config.yaml" | awk '{print $3}' | tr -d '"')

if [ -z "$HYPNATIVE_ADDRESS" ]; then
  echo "ERROR: Failed to deploy HypNative contract"
  cat /tmp/warp-deploy.log
  exit 1
fi

echo "  ✓ HypNative deployed: $HYPNATIVE_ADDRESS"

# Step 2: Initialize Merkle Tree Hook on Celestia (required for relayer)
echo "Step 2: Initializing Merkle Tree Hook on Celestia..."
MAILBOX_ID="0x68797065726c616e650000000000000000000000000000000000000000000000"

# Check if merkle tree hook already exists
EXISTING_HOOKS=$(docker exec celestia-validator celestia-appd q hyperlane hooks merkle-tree-hooks --node "$CELESTIA_RPC" -o json | jq -r '.merkle_tree_hooks | length')

if [ "$EXISTING_HOOKS" -eq 0 ]; then
  docker exec celestia-validator celestia-appd tx hyperlane hooks merkle create \
    "$MAILBOX_ID" \
    --from hyp --fees 800utia --yes --node "$CELESTIA_RPC" \
    > /dev/null 2>&1
  sleep 3
  echo "  ✓ Merkle Tree Hook initialized"
else
  echo "  ✓ Merkle Tree Hook already exists"
fi

# Step 3: Deploy Synthetic token on Celestia
echo "Step 3: Deploying Synthetic token on Celestia..."
TX_HASH=$(docker exec celestia-validator celestia-appd tx warp create-synthetic-token \
  0x68797065726c616e650000000000000000000000000000000000000000000000 \
  --from hyp --fees 800utia --yes --node "$CELESTIA_RPC" \
  -o json | jq -r '.txhash')

echo "  ✓ Transaction: $TX_HASH"
sleep 5

# Get the Synthetic token ID
SYNTHETIC_TOKEN_ID=$(docker exec celestia-validator celestia-appd q warp tokens --node "$CELESTIA_RPC" -o json | \
  jq -r '.tokens[] | select(.token_type == "HYP_TOKEN_TYPE_SYNTHETIC") | .id' | tail -1)

if [ -z "$SYNTHETIC_TOKEN_ID" ]; then
  echo "ERROR: Failed to create Synthetic token"
  exit 1
fi

echo "  ✓ Synthetic token: $SYNTHETIC_TOKEN_ID"

# Step 4: Configure ISM for Synthetic token
echo "Step 4: Configuring ISM..."
docker exec celestia-validator celestia-appd tx warp set-token \
  "$SYNTHETIC_TOKEN_ID" \
  --ism-id "$ISM_ID" \
  --from hyp --fees 800utia --yes --node "$CELESTIA_RPC" \
  > /dev/null 2>&1

sleep 3
echo "  ✓ ISM configured"

# Step 5: Enroll remote routers
echo "Step 5: Enrolling remote routers..."

# Enroll Synthetic token on HypNative
cast send "$HYPNATIVE_ADDRESS" \
  "enrollRemoteRouter(uint32,bytes32)" \
  "$DOMAIN_CELESTIA" "$SYNTHETIC_TOKEN_ID" \
  --private-key "$HYP_KEY" \
  --rpc-url "$RPC_URL" \
  --quiet > /dev/null 2>&1

echo "  ✓ Enrolled on HypNative"

# Enroll HypNative on Synthetic token (pad address to 32 bytes)
HYPNATIVE_PADDED=$(printf "000000000000000000000000%s" "${HYPNATIVE_ADDRESS#0x}")

docker exec celestia-validator celestia-appd tx warp enroll-remote-router \
  "$SYNTHETIC_TOKEN_ID" "$DOMAIN_EVM" "0x$HYPNATIVE_PADDED" 0 \
  --from hyp --fees 800utia --yes --node "$CELESTIA_RPC" \
  > /dev/null 2>&1

sleep 3
echo "  ✓ Enrolled on Synthetic token"

# Save configuration
cat > .warp-route-config <<EOF
HYPNATIVE_ADDRESS=$HYPNATIVE_ADDRESS
SYNTHETIC_TOKEN_ID=$SYNTHETIC_TOKEN_ID
DOMAIN_CELESTIA=$DOMAIN_CELESTIA
DOMAIN_EVM=$DOMAIN_EVM
RPC_URL=$RPC_URL
CELESTIA_RPC=$CELESTIA_RPC
EOF

# Step 6: Restart relayer to pick up Merkle Tree Hook
echo "Step 6: Restarting relayer..."
docker restart relayer > /dev/null 2>&1
sleep 2
echo "  ✓ Relayer restarted"

echo ""
echo "==> Deployment Complete"
echo "  HypNative:      $HYPNATIVE_ADDRESS"
echo "  Synthetic:      $SYNTHETIC_TOKEN_ID"
echo "  Config saved:   .warp-route-config"
