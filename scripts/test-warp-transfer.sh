#!/bin/bash
set -e

[ -f ".warp-route-config" ] && source .warp-route-config

HYP_KEY="${HYP_KEY:-0x82bfcfadbf1712f6550d8d2c00a39f05b33ec78939d0167be2a737d691f33a6a}"
RPC_URL="${RPC_URL:-http://localhost:8545}"
CELESTIA_RPC="${CELESTIA_RPC:-http://localhost:26657}"
DOMAIN_CELESTIA="${DOMAIN_CELESTIA:-69420}"
DOMAIN_EVM="${DOMAIN_EVM:-1234}"
CELESTIA_ACCOUNT="${CELESTIA_ACCOUNT:-celestia1y3kf30y9zprqzr2g2gjjkw3wls0a35pfs3a58q}"

transfer_to_celestia() {
  AMOUNT="${AMOUNT:-10000000}"
  RECIPIENT="${RECIPIENT:-0000000000000000000000006A809B36CAF0D46A935EE76835065EC5A8B3CEA7}"

  echo "==> Transfer from EVM to Celestia"
  echo ""

  # Show token details
  echo "Token Details:"
  echo "  HypNative:  $HYPNATIVE_ADDRESS"
  echo "  Synthetic:  $SYNTHETIC_TOKEN_ID"
  echo "  Amount:     $AMOUNT wei"
  echo ""

  # Query balances before
  echo "Balances Before:"
  BALANCE_BEFORE=$(docker exec celestia-validator celestia-appd q warp bridged-supply "$SYNTHETIC_TOKEN_ID" --node "$CELESTIA_RPC" -o json | jq -r '.bridged_supply.amount')
  echo "  Celestia bridged supply: $BALANCE_BEFORE"
  echo ""

  # Send transfer
  echo "Sending transfer..."
  TX_HASH=$(cast send "$HYPNATIVE_ADDRESS" \
    "transferRemote(uint32, bytes32, uint256)(bytes32)" \
    "$DOMAIN_CELESTIA" "$RECIPIENT" "$AMOUNT" \
    --private-key "$HYP_KEY" \
    --rpc-url "$RPC_URL" \
    --value "$AMOUNT" \
    --json | jq -r '.transactionHash')

  echo "  ✓ Transaction: $TX_HASH"
  echo ""

  # Wait for relayer to process
  echo "Waiting for relayer to process message..."
  for i in {1..12}; do
    sleep 5
    BALANCE_AFTER=$(docker exec celestia-validator celestia-appd q warp bridged-supply "$SYNTHETIC_TOKEN_ID" --node "$CELESTIA_RPC" -o json | jq -r '.bridged_supply.amount')
    if [ "$BALANCE_AFTER" != "$BALANCE_BEFORE" ]; then
      break
    fi
    echo "  Attempt $i/12..."
  done

  echo ""
  echo "Balances After:"
  echo "  Celestia bridged supply: $BALANCE_AFTER"
  echo ""

  # Validate
  if [ "$BALANCE_AFTER" != "$BALANCE_BEFORE" ]; then
    DIFF=$((BALANCE_AFTER - BALANCE_BEFORE))
    echo "✓ Transfer successful! Bridged supply increased by $DIFF"
  else
    echo "⚠ Transfer pending or failed - bridged supply unchanged after 60s"
    echo "  Check relayer logs: docker logs relayer --tail 50"
  fi
}

transfer_to_evm() {
  AMOUNT="${AMOUNT:-1000}"
  RECIPIENT="${RECIPIENT:-0x000000000000000000000000aF9053bB6c4346381C77C2FeD279B17ABAfCDf4d}"

  echo "==> Transfer from Celestia to EVM"
  echo ""

  # Show token details
  echo "Token Details:"
  echo "  Synthetic:  $SYNTHETIC_TOKEN_ID"
  echo "  HypNative:  $HYPNATIVE_ADDRESS"
  echo "  Amount:     $AMOUNT"
  echo ""

  # Query balances before
  echo "Balances Before:"
  BRIDGED_BEFORE=$(docker exec celestia-validator celestia-appd q warp bridged-supply "$SYNTHETIC_TOKEN_ID" --node "$CELESTIA_RPC" -o json | jq -r '.bridged_supply.amount')
  RECIPIENT_ADDR="${RECIPIENT#0x000000000000000000000000}"
  EVM_BEFORE=$(cast call "$HYPNATIVE_ADDRESS" "balanceOf(address)(uint256)" "0x$RECIPIENT_ADDR" --rpc-url "$RPC_URL")
  echo "  Celestia bridged supply: $BRIDGED_BEFORE"
  echo "  EVM balance (recipient): $EVM_BEFORE wei"
  echo ""

  # Send transfer
  echo "Sending transfer..."
  TX_HASH=$(docker exec celestia-validator celestia-appd tx warp transfer \
    "$SYNTHETIC_TOKEN_ID" "$DOMAIN_EVM" "$RECIPIENT" "$AMOUNT" \
    --from hyp --fees 800utia --max-hyperlane-fee 100utia \
    --node "$CELESTIA_RPC" --yes -o json | jq -r '.txhash')

  echo "  ✓ Transaction: $TX_HASH"
  echo ""

  # Wait for relayer to process
  echo "Waiting for relayer to process message..."
  for i in {1..12}; do
    sleep 5
    EVM_AFTER=$(cast call "$HYPNATIVE_ADDRESS" "balanceOf(address)(uint256)" "0x$RECIPIENT_ADDR" --rpc-url "$RPC_URL")
    if [ "$EVM_AFTER" != "$EVM_BEFORE" ]; then
      break
    fi
    echo "  Attempt $i/12..."
  done

  BRIDGED_AFTER=$(docker exec celestia-validator celestia-appd q warp bridged-supply "$SYNTHETIC_TOKEN_ID" --node "$CELESTIA_RPC" -o json | jq -r '.bridged_supply.amount')

  echo ""
  echo "Balances After:"
  echo "  Celestia bridged supply: $BRIDGED_AFTER"
  echo "  EVM balance (recipient): $EVM_AFTER wei"
  echo ""

  # Validate
  if [ "$EVM_AFTER" != "$EVM_BEFORE" ]; then
    DIFF=$((EVM_AFTER - EVM_BEFORE))
    echo "✓ Transfer successful! EVM balance increased by $DIFF wei"
  else
    echo "⚠ Transfer pending or failed - EVM balance unchanged after 60s"
    echo "  Check relayer logs: docker logs relayer --tail 50"
  fi
}

query_evm() {
  ACCOUNT="${ACCOUNT:-0xaF9053bB6c4346381C77C2FeD279B17ABAfCDf4d}"

  echo "==> EVM Balance Query"
  echo ""
  echo "HypNative: $HYPNATIVE_ADDRESS"
  echo "Account:   $ACCOUNT"
  echo ""

  BALANCE=$(cast call "$HYPNATIVE_ADDRESS" \
    "balanceOf(address)(uint256)" "$ACCOUNT" \
    --rpc-url "$RPC_URL")

  echo "Balance:   $BALANCE wei"
}

query_celestia() {
  echo "==> Celestia Balance Query"
  echo ""
  echo "Synthetic Token: $SYNTHETIC_TOKEN_ID"
  echo ""

  BRIDGED=$(docker exec celestia-validator celestia-appd q warp bridged-supply "$SYNTHETIC_TOKEN_ID" --node "$CELESTIA_RPC" -o json | jq -r '.bridged_supply.amount')

  echo "Bridged Supply:  $BRIDGED"

  if [ -n "${ACCOUNT:-}" ]; then
    echo ""
    echo "Account: $ACCOUNT"
    docker exec celestia-validator celestia-appd q bank balances "$ACCOUNT" \
      --node "$CELESTIA_RPC" --denom "hyperlane/$SYNTHETIC_TOKEN_ID" 2>/dev/null || echo "Balance: 0"
  fi
}

case "${1:-}" in
  to-celestia) transfer_to_celestia ;;
  to-evm) transfer_to_evm ;;
  query-evm) query_evm ;;
  query-celestia) query_celestia ;;
  *)
    echo "Usage: $0 {to-celestia|to-evm|query-evm|query-celestia}"
    exit 1
    ;;
esac
