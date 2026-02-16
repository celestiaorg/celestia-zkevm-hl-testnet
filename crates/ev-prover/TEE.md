# TEE Deployment Guide

This guide covers deploying and testing the Celestia zkEVM prover with Trusted Execution Environment (TEE) support using Phala Network.

## Security: Critical
For development purposes, the circuit does not yet fully constrain the execution environment. The following TEE measurements need to be asserted in the circuit:

- `os_image_hash` - Hash of the operating system image
- `mr_system` - Measurement register for system components
- `mr_aggregated` - Aggregated measurement register
- `mrtd` - Measurement register for TDX domains
- `rtmr0-3` - Runtime Measurement Registers (0 through 3)
- `compose_hash` - Hash of the compose configuration

These constraints will ensure that proofs can only be generated from authorized TEE environments with verified configurations.

> **⚠️ WARNING: Without these additional constraints the TEE circuit is NOT SAFE TO USE IN PRODUCTION!**

## Overview

The TEE integration provides hardware-based security guarantees for the zkEVM prover. The architecture is designed for minimal dependencies:

- **Prover**: Fetches block inputs and light blocks from Celestia/EVM nodes
- **TEE App**: Receives serialized data via POST, performs native verification, generates attestation
- **No middleware required**: The prover communicates directly with the TEE app

This guide includes:
- **Deploying your own TEE instance on Phala Cloud** (recommended for development)
- **Using the internal testnet** (for team members with access)

---

## Quick Start: Deploy Your Own TEE on Phala

This is the recommended approach for development and testing.

### Prerequisites

- Git access to [celestiaorg/evolve-tee](https://github.com/celestiaorg/evolve-tee)
- Phala Cloud account

### Setup Steps

**1. Install the Phala CLI**

Follow the [Phala Cloud CLI documentation](https://docs.phala.com/phala-cloud/phala-cloud-cli/overview) to install the CLI tool.

**2. Clone and deploy the TEE**

```bash
# Clone the TEE repository
git clone git@github.com:celestiaorg/evolve-tee
cd evolve-tee

# Deploy interactively to Phala Cloud
phala deploy --interactive
```

**3. Configure celestia-zkevm**

After deployment, you'll receive a dashboard link with your TEE instance details. Copy the **RPC endpoint** from the dashboard and add it to your `celestia-zkevm/.env` file:

```env
RETH_RPC_URL="http://127.0.0.1:8545"
RETH_WS_URL="ws://127.0.0.1:8546"
SEQUENCER_RPC_URL="http://127.0.0.1:7331"
CELESTIA_RPC_URL="http://127.0.0.1:26658"
TENDERMINT_RPC_URL="http://127.0.0.1:26657"
CELESTIA_GRPC_ENDPOINT="http://127.0.0.1:9090"
TEE_APP_URL="https://fc87b8918d4489663dfe47b82f48ed4b117dc518-8080.dstack-pha-prod5.phala.network"
```

> **Note:** The prover sends all necessary data (block inputs and light blocks) directly to the TEE app via POST request. No middleware is required.
```

## Internal Testnet Deployment

> **For team members only:** This section describes deploying to the internal testnet infrastructure. You'll need SSH access to internal servers (see Lazybridging Testnet: TEE notion doc for details).

### Architecture

The internal testnet consists of:
- **Testnet infrastructure**: Local blockchain environment for testing (ev-node, ev-reth, celestia-app)
- **TEE instance**: Phala-hosted TEE for secure proof generation
- **Prover**: Fetches data from testnet and sends to TEE for attestation

### Deployment Steps

**Step 1: Deploy the Testnet**

SSH into the internal server:

```bash
cd /home/tee/celestia-zkevm
make stop && make start && make deploy-ism-tee && make update-ism
```

Wait for all services to initialize completely before proceeding.

**Step 2: Start the Prover**

On your local machine:

```bash
cd celestia-zkevm

# Clean existing state
rm -rf ~/.ev-prover

# Initialize and start prover
RUST_LOG="ev_prover=debug" cargo run --release --features tee_mode -p ev-prover init
RUST_LOG="ev_prover=debug" cargo run --release --features tee_mode -p ev-prover start
```

The prover service is now running in TEE mode. The prover fetches block inputs and light blocks from the local testnet, then sends them to the TEE app for verification and attestation.

**Step 3: Test with Transactions**

On the internal server, submit test transactions:

```bash
cd /home/tee/evolve-tee

# Test bridge transfers
make transfer       # Bridge assets to L2
make transfer-back  # Bridge assets back to L1
```

Monitor the prover logs to see transaction processing through the TEE.

---

## Troubleshooting

### TEE connection issues
- Verify the `TEE_APP_URL` in your `.env` file is correct
- Check that your TEE instance is running in the Phala dashboard
- Ensure network connectivity to the Phala Cloud endpoints
- Test the TEE app health endpoint: `curl <TEE_APP_URL>/health`

### Prover initialization failures
- Remove `~/.ev-prover` directory and reinitialize
- Verify the `tee_mode` feature is enabled in your build
- Check that all dependencies are installed with `cargo check -p ev-prover --features tee_mode`

### Block verification failures
- Ensure Celestia/EVM RPC endpoints are accessible from the prover
- Check that light blocks can be fetched from Tendermint RPC
- Verify the TEE app logs for detailed error messages

---

## Next Steps

- Learn about [TEE security guarantees](https://docs.phala.com/)
- Explore the [evolve-tee source code](https://github.com/celestiaorg/evolve-tee)
- Review the [ev-prover documentation](../README.md)
