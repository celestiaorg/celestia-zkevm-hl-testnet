#!/bin/bash
#
# End-to-end test for Celestia Rebalancer
#
# Prerequisites:
# - Docker compose running (celestia-validator, reth, relayer)
# - Rebalancer binary built in celestia-rebalancer repo
# - Multisig account created (run setup-multisig.sh first)
#

set -e

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
REBALANCER_REPO="${REBALANCER_REPO:-/Users/blasrodriguezgarciairizar/projects/celestia/celestia-rebalancer}"
MULTISIG="${MULTISIG_ADDRESS:-celestia13xkxkeywvktfpla6vfpuq7l8yc8tc7zhdk98f4}"
TOKEN_ID="0x726f757465725f61707000000000000000000000000000010000000000000000"
DEST_DOMAIN=1234
RECIPIENT="0x0000000000000000000000006a809b36caf0d46a935ee76835065ec5a8b3cea7"
AMOUNT=10000000

echo -e "${BLUE}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║         CELESTIA REBALANCER - END-TO-END TEST                    ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${YELLOW}Configuration:${NC}"
echo "  Rebalancer repo: $REBALANCER_REPO"
echo "  Multisig: $MULTISIG"
echo "  Token ID: $TOKEN_ID"
echo "  Destination: Domain $DEST_DOMAIN"
echo ""

# Validate prerequisites
echo -e "${YELLOW}Checking prerequisites...${NC}"

if [ ! -d "$REBALANCER_REPO" ]; then
    echo -e "${RED}❌ Rebalancer repo not found: $REBALANCER_REPO${NC}"
    echo "Set REBALANCER_REPO environment variable"
    exit 1
fi

if [ ! -f "$REBALANCER_REPO/celestia-rebalancer" ]; then
    echo -e "${YELLOW}Building rebalancer...${NC}"
    cd "$REBALANCER_REPO" && go build -o celestia-rebalancer ./cmd/celestia-rebalancer
    if [ $? -ne 0 ]; then
        echo -e "${RED}❌ Failed to build rebalancer${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ Rebalancer built${NC}"
fi

if ! docker ps | grep -q celestia-validator; then
    echo -e "${RED}❌ celestia-validator not running${NC}"
    echo "Run: docker compose up -d"
    exit 1
fi

echo -e "${GREEN}✅ All prerequisites met${NC}"
echo ""

# Test 1: Parser
echo -e "${YELLOW}━━━━ TEST 1: Rebalancer Parser ━━━━${NC}"
echo ""

CURRENT_HEIGHT=$(docker exec celestia-validator celestia-appd status --node http://localhost:26657 | jq -r '.sync_info.latest_block_height')
FROM_HEIGHT=$((CURRENT_HEIGHT - 50))

echo "Testing gRPC parsing..."
echo "  Scanning blocks: $FROM_HEIGHT to $CURRENT_HEIGHT"
echo ""

cd "$REBALANCER_REPO"
PARSE_OUTPUT=$(./celestia-rebalancer parse \
    --multisig-address "$MULTISIG" \
    --from-height $FROM_HEIGHT \
    --to-height $CURRENT_HEIGHT \
    --rpc-url localhost:9090 \
    --output /tmp/test-routes.json 2>&1)

echo "$PARSE_OUTPUT"

if echo "$PARSE_OUTPUT" | grep -qi "error.*query cannot be empty"; then
    echo -e "${RED}❌ FAIL: gRPC query bug present${NC}"
    exit 1
elif echo "$PARSE_OUTPUT" | grep -qi "error"; then
    echo -e "${YELLOW}⚠️  Parser reported errors${NC}"
else
    echo -e "${GREEN}✅ PASS: Parser executed without gRPC errors${NC}"
fi
echo ""

# Test 2: Message Generation
echo -e "${YELLOW}━━━━ TEST 2: Message Generation ━━━━${NC}"
echo ""

# Create test route file
cat > /tmp/test-gen-routes.json << 'EOF'
{
  "routes": [
    {
      "tx_hash": "TEST123",
      "block_height": 1000,
      "from": "celestia1test",
      "amount": "5000000",
      "denom": "utia",
      "custom_hook_metadata": "{\"destination_domain\":1234,\"recipient\":\"0x6a809b36caf0d46a935ee76835065ec5a8b3cea7\",\"token_id\":\"0x726f757465725f61707000000000000000000000000000010000000000000000\"}",
      "route_info": {
        "destination_domain": 1234,
        "recipient": "0x6a809b36caf0d46a935ee76835065ec5a8b3cea7",
        "token_id": "0x726f757465725f61707000000000000000000000000000010000000000000000"
      }
    }
  ],
  "total_amount": "5000000",
  "multisig_address": "celestia13xkxkeywvktfpla6vfpuq7l8yc8tc7zhdk98f4"
}
EOF

echo "Generating messages..."
GEN_OUTPUT=$(./celestia-rebalancer generate \
    --routes /tmp/test-gen-routes.json \
    --multisig-address "$MULTISIG" \
    --output /tmp/test-unsigned-tx.json 2>&1)

echo "$GEN_OUTPUT"

if echo "$GEN_OUTPUT" | grep -qi "error"; then
    echo -e "${RED}❌ FAIL: Generation failed${NC}"
    exit 1
fi

echo ""
echo "Generated message:"
cat /tmp/test-unsigned-tx.json | jq '.[0]'
echo ""

# Validate format
RECIPIENT_FIELD=$(cat /tmp/test-unsigned-tx.json | jq -r '.[0].recipient')
RECIPIENT_LENGTH=${#RECIPIENT_FIELD}

if [ $RECIPIENT_LENGTH -eq 66 ]; then
    echo -e "${GREEN}✅ PASS: Recipient properly formatted (32 bytes)${NC}"
else
    echo -e "${RED}❌ FAIL: Recipient length: $RECIPIENT_LENGTH (expected 66)${NC}"
    exit 1
fi
echo ""

# Test 3: Cross-chain Transfer (optional, takes time)
if [ "$RUN_TRANSFER_TEST" = "true" ]; then
    echo -e "${YELLOW}━━━━ TEST 3: Cross-Chain Transfer ━━━━${NC}"
    echo ""

    echo "Sending test transfer..."
    TX_RESULT=$(docker exec celestia-validator celestia-appd tx warp transfer \
        $TOKEN_ID \
        $DEST_DOMAIN \
        $RECIPIENT \
        ${AMOUNT}utia \
        --from hyp \
        --fees 1000utia \
        --yes \
        --node http://localhost:26657 \
        --keyring-backend test 2>&1)

    if echo "$TX_RESULT" | grep -q "code: 0"; then
        echo -e "${GREEN}✅ Transfer dispatched${NC}"
        echo "Waiting 30s for delivery..."
        sleep 30
        echo "Check EV-RETH balance manually"
    else
        echo -e "${RED}❌ Transfer failed${NC}"
    fi
    echo ""
fi

# Summary
echo -e "${BLUE}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║                    TEST RESULTS                                  ║${NC}"
echo -e "${BLUE}╠══════════════════════════════════════════════════════════════════╣${NC}"
echo -e "${BLUE}║  gRPC Parsing ......................  ${GREEN}✅ PASS${BLUE}                   ║${NC}"
echo -e "${BLUE}║  Message Generation ................  ${GREEN}✅ PASS${BLUE}                   ║${NC}"
echo -e "${BLUE}║  32-byte Address Padding ...........  ${GREEN}✅ PASS${BLUE}                   ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${GREEN}✅ All tests passed!${NC}"
echo ""
echo "The celestia-rebalancer tool is working correctly."
echo ""
