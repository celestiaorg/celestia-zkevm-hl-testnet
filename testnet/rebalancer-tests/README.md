# Celestia Rebalancer - Test Suite

This directory contains scripts for testing the Celestia Rebalancer tool with the celestia-zkevm testnet environment.

## Prerequisites

1. **Clone celestia-rebalancer repo:**
   ```bash
   git clone https://github.com/bcp-innovations/celestia-rebalancer.git
   cd celestia-rebalancer
   go build -o celestia-rebalancer ./cmd/celestia-rebalancer
   ```

2. **Start testnet infrastructure:**
   ```bash
   # In this repo (celestia-zkevm)
   docker compose up -d
   ```

3. **Wait for services to be ready:**
   ```bash
   # Check celestia is producing blocks
   docker exec celestia-validator celestia-appd status

   # Check relayer is running
   docker logs relayer
   ```

## Running Tests

### 1. Setup Multisig Account

```bash
cd testnet/rebalancer-tests
chmod +x setup-multisig.sh
./setup-multisig.sh
```

This creates a 2-of-3 multisig account for testing. Note the multisig address for later use.

### 2. Enroll Remote Routers

```bash
chmod +x enroll-routers.sh
./enroll-routers.sh
```

This enrolls the EV-RETH WarpRoute as a valid destination.

### 3. Run Rebalancer Tests

```bash
chmod +x test-rebalancer.sh

# Set the path to your celestia-rebalancer repo
export REBALANCER_REPO=/path/to/celestia-rebalancer

# Set the multisig address (from step 1)
export MULTISIG_ADDRESS=celestia13xkxkeywvktfpla6vfpuq7l8yc8tc7zhdk98f4

# Run tests
./test-rebalancer.sh
```

To include cross-chain transfer test (takes ~30s):
```bash
RUN_TRANSFER_TEST=true ./test-rebalancer.sh
```

## What Gets Tested

### Parser Test ✅
- gRPC client functionality
- Blockchain query correctness
- Transaction parsing
- Error handling

### Message Generation Test ✅
- Route parsing from JSON
- MsgRemoteTransfer creation
- Address formatting (32-byte padding)
- Field validation

### Cross-Chain Transfer Test (Optional) ✅
- Message dispatch on Celestia
- Relayer message indexing
- Delivery to EV-RETH
- Balance verification

## Test Files

- `setup-multisig.sh` - Creates 2-of-3 multisig account
- `enroll-routers.sh` - Enrolls remote routers for token transfers
- `test-rebalancer.sh` - Main end-to-end test script
- `README.md` - This file

## Expected Output

```
╔══════════════════════════════════════════════════════════════════╗
║         CELESTIA REBALANCER - END-TO-END TEST                    ║
╚══════════════════════════════════════════════════════════════════╝

━━━━ TEST 1: Rebalancer Parser ━━━━
✅ PASS: Parser executed without gRPC errors

━━━━ TEST 2: Message Generation ━━━━
✅ PASS: Recipient properly formatted (32 bytes)

╔══════════════════════════════════════════════════════════════════╗
║                    TEST RESULTS                                  ║
╠══════════════════════════════════════════════════════════════════╣
║  gRPC Parsing ......................  ✅ PASS                     ║
║  Message Generation ................  ✅ PASS                     ║
║  32-byte Address Padding ...........  ✅ PASS                     ║
╚══════════════════════════════════════════════════════════════════╝

✅ All tests passed!
```

## Troubleshooting

### "Rebalancer repo not found"
Set the `REBALANCER_REPO` environment variable to point to your celestia-rebalancer clone.

### "celestia-validator not running"
Run `docker compose up -d` in the celestia-zkevm repo root.

### "Multisig account not found"
Run `./setup-multisig.sh` first.

### "gRPC query error"
Ensure the fixes from celestia-rebalancer PR are applied (see CHANGES_SUMMARY.md in rebalancer repo).

## Integration with CI/CD

These tests can be integrated into CI/CD pipelines:

```yaml
# Example GitHub Actions
- name: Start testnet
  run: docker compose up -d

- name: Wait for chain
  run: sleep 30

- name: Setup multisig
  run: ./testnet/rebalancer-tests/setup-multisig.sh

- name: Enroll routers
  run: ./testnet/rebalancer-tests/enroll-routers.sh

- name: Run rebalancer tests
  run: |
    export REBALANCER_REPO=../celestia-rebalancer
    export MULTISIG_ADDRESS=celestia13xkxkeywvktfpla6vfpuq7l8yc8tc7zhdk98f4
    ./testnet/rebalancer-tests/test-rebalancer.sh
```

## References

- [Celestia Rebalancer Repo](https://github.com/bcp-innovations/celestia-rebalancer)
- [FINAL_VALIDATION_REPORT.md](../../celestia-rebalancer/FINAL_VALIDATION_REPORT.md) - Complete test documentation
- [Hyperlane Docs](https://docs.hyperlane.xyz/)
