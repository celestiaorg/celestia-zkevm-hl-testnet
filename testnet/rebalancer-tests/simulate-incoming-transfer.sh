#!/bin/bash
#
# Simulate an incoming transfer to the multisig with routing metadata
# This tests the rebalancer workflow without relying on Hyperlane delivery
#

set -e

echo "=== Simulating Incoming Transfer to Multisig ==="
echo ""

MULTISIG="${MULTISIG_ADDRESS:-celestia13xkxkeywvktfpla6vfpuq7l8yc8tc7zhdk98f4}"
TOKEN_ID="0x726f757465725f61707000000000000000000000000000010000000000000000"
DEST_DOMAIN=1234
RECIPIENT="0x0000000000000000000000006a809b36caf0d46a935ee76835065ec5a8b3cea7"
AMOUNT=5000000

echo "Configuration:"
echo "  Multisig: $MULTISIG"
echo "  Amount: ${AMOUNT}utia"
echo "  Destination: Domain $DEST_DOMAIN"
echo "  Recipient: $RECIPIENT"
echo ""

# Create routing metadata
METADATA=$(cat << METAEOF | jq -c .
{
  "destination_domain": $DEST_DOMAIN,
  "recipient": "$RECIPIENT",
  "token_id": "$TOKEN_ID"
}
METAEOF
)

echo "Routing metadata: $METADATA"
echo ""

echo "Sending transfer WITH routing metadata to multisig..."
docker exec celestia-validator celestia-appd tx bank send \
    hyp \
    "$MULTISIG" \
    "${AMOUNT}utia" \
    --note "$METADATA" \
    --fees 1000utia \
    --yes \
    --node http://localhost:26657 \
    --keyring-backend test

sleep 3

echo ""
echo "✅ Transfer sent to multisig with routing metadata"
echo ""
echo "Now test the rebalancer:"
echo "  1. Run parse to detect this transfer"
echo "  2. Run generate to create forwarding message"
echo "  3. Sign with multisig (2 of 3 signers)"
echo "  4. Broadcast back to destination"
echo ""

