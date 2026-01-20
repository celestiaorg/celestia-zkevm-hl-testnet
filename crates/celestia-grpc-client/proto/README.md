# Proto Generation for jonas/aggregation Branch

This directory contains proto definitions including those from the `jonas/aggregation` branch of hyperlane-cosmos.

## Setup

The hyperlane ISM proto files (NoopISM, AggregationISM) are copied from the local hyperlane-cosmos repository:
- Source: `/Users/chef/Desktop/hyperlane-cosmos/proto/hyperlane/core/interchain_security/v1/`
- Destination: `./hyperlane/core/interchain_security/v1/`

## Regenerating Proto Files

### 1. Update from hyperlane-cosmos jonas/aggregation branch

If you need to update the hyperlane ISM proto definitions:

```bash
# Ensure hyperlane-cosmos is on jonas/aggregation branch
cd /Users/chef/Desktop/hyperlane-cosmos
git checkout jonas/aggregation
git pull

# Copy updated proto files
cd /Users/chef/Desktop/celestia-zkevm/crates/celestia-grpc-client/proto
cp /Users/chef/Desktop/hyperlane-cosmos/proto/hyperlane/core/interchain_security/v1/tx.proto \
   ./hyperlane/core/interchain_security/v1/
cp /Users/chef/Desktop/hyperlane-cosmos/proto/hyperlane/core/interchain_security/v1/types.proto \
   ./hyperlane/core/interchain_security/v1/
```

### 2. Generate Rust code

```bash
cd /Users/chef/Desktop/celestia-zkevm/crates/celestia-grpc-client/proto

# Update dependencies
buf dep update

# Generate celestia-grpc-client proto code
buf generate --template buf.gen.yaml

# Generate only the specific hyperlane ISM protos we need
buf generate --template buf.gen.yaml \
  --path hyperlane/core/interchain_security/v1/tx.proto
```

**Note**: We only generate the specific hyperlane proto files we need (tx.proto) to avoid pulling in unnecessary dependencies like google.api, gogoproto, etc.

### 3. Verify compilation

```bash
cd /Users/chef/Desktop/celestia-zkevm
cargo check --package celestia-grpc-client
cargo check --package ev-prover
```

## Proto Files Structure

```
proto/
├── celestia/zkism/v1/        # Celestia zkism module protos
├── hyperlane/                 # Hyperlane protos from jonas/aggregation branch
│   └── core/
│       └── interchain_security/
│           └── v1/
│               ├── tx.proto
│               └── types.proto
├── buf.yaml                   # Buf configuration
├── buf.gen.yaml              # Code generation config
└── buf.lock                   # Dependency lock file
```

## Messages Available

From `hyperlane/core/interchain_security/v1/tx.proto`:
- `MsgCreateNoopIsm` / `MsgCreateNoopIsmResponse`
- `MsgCreateAggregationIsm` / `MsgCreateAggregationIsmResponse`
- `MsgSetAggregationIsmModules` / `MsgSetAggregationIsmModulesResponse`
- `MsgUpdateAggregationIsmOwner` / `MsgUpdateAggregationIsmOwnerResponse`

These are used in `ev-prover` to create ISMs with 2/2 threshold verification.
