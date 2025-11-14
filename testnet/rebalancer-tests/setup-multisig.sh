#!/bin/bash
#
# Setup script for creating multisig account for rebalancer testing
#

set -e

echo "=== Celestia Rebalancer - Multisig Setup ==="
echo ""

# Check if running inside docker
if [ -f "/.dockerenv" ]; then
    CMD_PREFIX=""
else
    CMD_PREFIX="docker exec celestia-validator"
fi

echo "Creating signer accounts..."

# Create three signer accounts
echo "Creating signer1..."
$CMD_PREFIX celestia-appd keys add signer1 --keyring-backend test 2>/dev/null || echo "signer1 already exists"

echo "Creating signer2..."
$CMD_PREFIX celestia-appd keys add signer2 --keyring-backend test 2>/dev/null || echo "signer2 already exists"

echo "Creating signer3..."
$CMD_PREFIX celestia-appd keys add signer3 --keyring-backend test 2>/dev/null || echo "signer3 already exists"

echo ""
echo "Getting signer addresses..."
SIGNER1=$($CMD_PREFIX celestia-appd keys show signer1 -a --keyring-backend test)
SIGNER2=$($CMD_PREFIX celestia-appd keys show signer2 -a --keyring-backend test)
SIGNER3=$($CMD_PREFIX celestia-appd keys show signer3 -a --keyring-backend test)

echo "  Signer 1: $SIGNER1"
echo "  Signer 2: $SIGNER2"
echo "  Signer 3: $SIGNER3"
echo ""

echo "Creating 2-of-3 multisig account..."
$CMD_PREFIX celestia-appd keys add test-multisig \
    --multisig signer1,signer2,signer3 \
    --multisig-threshold 2 \
    --keyring-backend test 2>/dev/null || echo "test-multisig already exists"

MULTISIG=$($CMD_PREFIX celestia-appd keys show test-multisig -a --keyring-backend test)

echo ""
echo "✅ Multisig account created successfully!"
echo ""
echo "   Address: $MULTISIG"
echo "   Type: 2-of-3 threshold multisig"
echo "   Signers: $SIGNER1, $SIGNER2, $SIGNER3"
echo ""

echo "Funding multisig account with test tokens..."
$CMD_PREFIX celestia-appd tx bank send hyp $MULTISIG 100000000utia \
    --fees 1000utia \
    --yes \
    --node http://localhost:26657 \
    --keyring-backend test > /dev/null 2>&1 || echo "Failed to fund (may already have funds)"

sleep 3

BALANCE=$($CMD_PREFIX celestia-appd query bank balances $MULTISIG --node http://localhost:26657 -o json | jq -r '.balances[0].amount' 2>/dev/null || echo "0")
echo "Multisig balance: $BALANCE utia"
echo ""

echo "✅ Setup complete!"
echo ""
echo "Export this for use in tests:"
echo "export MULTISIG_ADDRESS=$MULTISIG"
echo ""
