# TEE Testnet Deployment Guide

Deploy testnet with Phala TEE instance and middleware.

---

## Prerequisites

SSH into the internal server (details in the Lazybridging Testnet: TEE notion doc).

---

## Deployment Steps

### 1. Start Middleware

**On internal server:**

```bash
cd /home/tee/evolve-tee
cargo run -p middleware
```

### 2. Deploy Testnet

**On internal server:**

```bash
cd /home/tee/celestia-zkevm
make stop && make start && make deploy-ism-tee && make update-ism
```

> Wait for everything to start.

### 3. Start the Prover

**On your local machine:**

```bash
cd celestia-zkevm
rm -rf ~/.ev-prover
cargo run -p ev-prover init
RUST_LOG="ev_prover=debug" cargo run --release --features tee_mode -p ev-prover start
```

> The prover service is now running and waiting for transactions (similar to batch_mode).

### 4. Submit Test Transactions

**On internal server:**

```bash
cd /home/tee/evolve-tee
make transfer
make transfer-back
```

---

## Deploying Your Own TEE on Phala

To start your own TEE instance on Phala, follow these steps:

1. Install the Phala CLI as per the [Phala Cloud CLI documentation](https://docs.phala.com/phala-cloud/phala-cloud-cli/overview)

2. Clone the evolve-tee repository:
   ```bash
   git clone git@github.com:celestiaorg/evolve-tee
   ```

3. Navigate to the repository:
   ```bash
   cd evolve-tee
   ```

4. Deploy using the Phala CLI:
   ```bash
   phala deploy --interactive
   ```

5. This will return a link to the dashboard where you can view all necessary information, including the RPC endpoint that must be set in celestia-zkevm's `.env` file.
