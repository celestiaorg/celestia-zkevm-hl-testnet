# Native EVM Token to Celestia Warp Route

Automated deployment and testing scripts for Hyperlane Warp routes.

## Prerequisites

```bash
npm install -g @hyperlane-xyz/cli
# Foundry, Docker, jq must be installed
```

## Quick Start

```bash
# 1. Start the testnet
make start

# 2. Deploy warp route (fully automated, no intervention required)
./scripts/deploy-native-warp-route.sh

# 3. Test transfers
./scripts/test-warp-transfer.sh to-celestia
./scripts/test-warp-transfer.sh query-celestia
```

## Scripts

### `deploy-native-warp-route.sh`

Deploys a complete warp route between EVM and Celestia. No user input required.

**Steps:**
1. Deploys HypNative contract on EVM
2. Initializes Merkle Tree Hook on Celestia (required for relayer)
3. Creates Synthetic token on Celestia
4. Configures ISM
5. Enrolls remote routers bidirectionally
6. Restarts relayer to pick up new configuration
7. Saves config to `.warp-route-config`

**Output:**
```
==> Deployment Complete
  HypNative:      0x...
  Synthetic:      0x...
  Config saved:   .warp-route-config
```

### `test-warp-transfer.sh`

Test token transfers after deployment with automatic validation.

**Commands:**
- `to-celestia` - Transfer ETH from EVM to Celestia (with before/after balance check)
- `to-evm` - Transfer tokens from Celestia to EVM (with before/after balance check)
- `query-evm` - Check balance on EVM
- `query-celestia` - Check bridged supply on Celestia

**Features:**
- Shows token details before transfer
- Queries balances before and after
- Waits up to 60s for relayer to process
- Validates transfer success automatically
- Provides helpful error messages

**Environment Variables:**
- `AMOUNT` - Transfer amount (default: 10000000 for EVM, 1000 for Celestia)
- `RECIPIENT` - Recipient address (32-byte padded hex)
- `ACCOUNT` - Account to query balance for

**Examples:**
```bash
# Transfer with automatic validation
./scripts/test-warp-transfer.sh to-celestia

# Custom amount transfer
AMOUNT=5000000 ./scripts/test-warp-transfer.sh to-celestia

# Query specific account
ACCOUNT=0xYourAddress ./scripts/test-warp-transfer.sh query-evm
```

**Expected Output:**
```
==> Transfer from EVM to Celestia

Token Details:
  HypNative:  0x032e1B988eB5Ac8F0C8617E09de92a664cABf37D
  Synthetic:  0x726f757465725f61707000000000000000000000000000020000000000000001
  Amount:     10000000 wei

Balances Before:
  Celestia bridged supply: 0

Sending transfer...
  ✓ Transaction: 0x...

Waiting for relayer to process message...

Balances After:
  Celestia bridged supply: 10000000

✓ Transfer successful! Bridged supply increased by 10000000
```

## Configuration

Default values (override via environment variables):
- `HYP_KEY`: `0x82bfcfadbf1712f6550d8d2c00a39f05b33ec78939d0167be2a737d691f33a6a`
- `RPC_URL`: `http://localhost:8545`
- `CELESTIA_RPC`: `http://localhost:26657`
- `DOMAIN_EVM`: `1234`
- `DOMAIN_CELESTIA`: `69420`

## Troubleshooting

**Deployment fails:** Ensure `make start` completed and containers are healthy.

**Transfer doesn't reflect:** Verify Hyperlane relayer is running (`docker ps | grep relayer`).

**"replacement transaction underpriced":** Wait a few seconds and retry.
