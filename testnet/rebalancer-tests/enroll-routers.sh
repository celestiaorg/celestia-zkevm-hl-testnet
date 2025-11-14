#!/bin/bash
#
# Enroll remote routers for rebalancer testing
#

set -e

echo "=== Enrolling Remote Routers ==="
echo ""

# Configuration
TOKEN_ID="${TOKEN_ID:-0x726f757465725f61707000000000000000000000000000010000000000000000}"
EVRETH_DOMAIN="${EVRETH_DOMAIN:-1234}"
EVRETH_WARP="${EVRETH_WARP:-0x345a583028762De4d733852c9D4f419077093A48}"

echo "Token ID: $TOKEN_ID"
echo "EV-RETH Domain: $EVRETH_DOMAIN"
echo "EV-RETH WarpRoute: $EVRETH_WARP"
echo ""

# Check if router already enrolled
ROUTERS=$(docker exec celestia-validator celestia-appd query warp remote-routers $TOKEN_ID --node http://localhost:26657 2>/dev/null || echo "")

if echo "$ROUTERS" | grep -q "receiver_domain: $EVRETH_DOMAIN"; then
    echo "✅ Router already enrolled for domain $EVRETH_DOMAIN"
else
    echo "Enrolling router for domain $EVRETH_DOMAIN..."
    docker exec celestia-validator celestia-appd tx warp enroll-remote-router \
        $TOKEN_ID \
        $EVRETH_DOMAIN \
        $EVRETH_WARP \
        --from hyp \
        --fees 1000utia \
        --yes \
        --node http://localhost:26657 \
        --keyring-backend test

    sleep 3
    echo "✅ Router enrolled"
fi

echo ""
echo "Current routers:"
docker exec celestia-validator celestia-appd query warp remote-routers $TOKEN_ID --node http://localhost:26657
echo ""
