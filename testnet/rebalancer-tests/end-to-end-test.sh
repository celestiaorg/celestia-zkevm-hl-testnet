#!/bin/bash
set -e

echo "======================================"
echo "Celestia Rebalancer End-to-End Test"
echo "======================================"
echo ""

MULTISIG="celestia1g7fd4p7qzw0ukayus4gpkaayrrujwn02fl9ev7"
DEST_DOMAIN=1234
RECIPIENT="0x000000000000000000000000af9053bb6c4346381c77c2fed279b17abafcdf4d"
TOKEN_ID="0x726f757465725f61707000000000000000000000000000010000000000000000"
AMOUNT="3000000"
REBALANCER_PATH="/Users/blasrodriguezgarciairizar/projects/celestia/celestia-rebalancer/celestia-rebalancer"

# Step 1: Send incoming transfer with routing metadata
echo "Step 1: Sending incoming transfer to multisig with routing metadata..."
METADATA=$(cat << METAEOF | jq -c .
{
  "destination_domain": $DEST_DOMAIN,
  "recipient": "$RECIPIENT",
  "token_id": "$TOKEN_ID"
}
METAEOF
)

TX_HASH=$(docker exec celestia-validator celestia-appd tx bank send \
    hyp \
    "$MULTISIG" \
    "${AMOUNT}utia" \
    --note "$METADATA" \
    --fees 1000utia \
    --yes \
    --node http://localhost:26657 \
    --keyring-backend test -o json | jq -r '.txhash')

echo "Transfer sent: $TX_HASH"
sleep 3

# Get block height
BLOCK=$(docker exec celestia-validator celestia-appd query tx "$TX_HASH" --node http://localhost:26657 -o json | jq -r '.height')
echo "Block height: $BLOCK"

# Step 2: Parse with rebalancer
echo ""
echo "Step 2: Parsing routes with celestia-rebalancer..."
FROM_HEIGHT=$((BLOCK - 1))
TO_HEIGHT=$((BLOCK + 1))

cd /Users/blasrodriguezgarciairizar/projects/celestia/celestia-zkevm/testnet/rebalancer-tests
$REBALANCER_PATH parse \
  --config config-test.json \
  --multisig-address "$MULTISIG" \
  --rpc-url localhost:9090 \
  --from-height $FROM_HEIGHT \
  --to-height $TO_HEIGHT \
  --output /tmp/parsed-routes.json

echo ""
cat /tmp/parsed-routes.json | jq .

# Step 3: Generate unsigned transaction
echo ""
echo "Step 3: Generating unsigned MsgRemoteTransfer..."
$REBALANCER_PATH generate \
  --routes /tmp/parsed-routes.json \
  --multisig-address "$MULTISIG" \
  --output /tmp/unsigned-tx.json

echo ""
cat /tmp/unsigned-tx.json | jq .

# Step 4: Create and sign multisig transaction
echo ""
echo "Step 4: Creating and signing multisig transaction..."

docker exec celestia-validator bash -c "celestia-appd tx warp transfer \
  '$TOKEN_ID' \
  $DEST_DOMAIN \
  '$RECIPIENT' \
  '$AMOUNT' \
  --from '$MULTISIG' \
  --fees 20000utia \
  --gas 200000 \
  --gas-limit 300000 \
  --max-hyperlane-fee 10000utia \
  --generate-only \
  --keyring-backend test \
  --node http://localhost:26657 > /tmp/multisig-tx.json"

docker exec celestia-validator bash -c "celestia-appd tx sign \
  /tmp/multisig-tx.json \
  --from signer1 \
  --multisig '$MULTISIG' \
  --chain-id celestia-zkevm-testnet \
  --keyring-backend test \
  --node http://localhost:26657 > /tmp/sig1.json"

docker exec celestia-validator bash -c "celestia-appd tx sign \
  /tmp/multisig-tx.json \
  --from signer2 \
  --multisig '$MULTISIG' \
  --chain-id celestia-zkevm-testnet \
  --keyring-backend test \
  --node http://localhost:26657 > /tmp/sig2.json"

docker exec celestia-validator bash -c "celestia-appd tx multisign \
  /tmp/multisig-tx.json \
  test-multisig \
  /tmp/sig1.json \
  /tmp/sig2.json \
  --chain-id celestia-zkevm-testnet \
  --keyring-backend test \
  --node http://localhost:26657 > /tmp/signed-tx.json"

# Step 5: Broadcast
echo ""
echo "Step 5: Broadcasting multisig transaction..."
RESULT=$(docker exec celestia-validator celestia-appd tx broadcast \
  /tmp/signed-tx.json \
  --node http://localhost:26657 \
  --keyring-backend test -o json)

MULTISIG_TX_HASH=$(echo "$RESULT" | jq -r '.txhash')
CODE=$(echo "$RESULT" | jq -r '.code')

echo "Multisig TX Hash: $MULTISIG_TX_HASH"
echo "Code: $CODE"

if [ "$CODE" == "0" ]; then
    echo ""
    echo "✅ SUCCESS! End-to-end test completed successfully!"
    echo ""
    echo "Summary:"
    echo "- Incoming transfer: $TX_HASH"
    echo "- Parsed routes: $(cat /tmp/parsed-routes.json | jq -r '.routes | length') routes"
    echo "- Multisig transaction: $MULTISIG_TX_HASH"
    echo ""
    echo "Check TEST_RESULTS.md for detailed results."
else
    echo ""
    echo "❌ FAILED! Multisig transaction failed with code $CODE"
    exit 1
fi
