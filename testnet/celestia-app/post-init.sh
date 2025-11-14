#!/bin/sh

# Post-initialization script - runs after the chain has started
# Creates the MerkleTreeHook required by the Hyperlane relayer

MARKER_FILE="/home/celestia/.celestia-app/post_init_done"

if [ ! -f "$MARKER_FILE" ]; then
    echo "Waiting for chain to be ready..."

    # Wait for the chain to start and produce blocks
    until celestia-appd status 2>/dev/null | grep -q "latest_block_height"; do
        echo "Waiting for chain to start..."
        sleep 2
    done

    # Wait for at least 3 blocks to ensure chain is stable
    while true; do
        HEIGHT=$(celestia-appd status 2>/dev/null | jq -r '.sync_info.latest_block_height' 2>/dev/null || echo "0")
        if [ "$HEIGHT" -gt 3 ]; then
            echo "Chain is ready at height $HEIGHT"
            break
        fi
        echo "Waiting for blocks... current height: $HEIGHT"
        sleep 2
    done

    echo "Creating MerkleTreeHook for Hyperlane relayer..."

    # Get the mailbox ID (this should be the standard Hyperlane mailbox ID)
    MAILBOX_ID="0x68797065726c616e650000000000000000000000000000000000000000000000"

    # Create the merkle tree hook
    celestia-appd tx hyperlane hooks merkle create "$MAILBOX_ID" \
        --from hyp \
        --fees 1000utia \
        --yes \
        --node http://localhost:26657

    # Wait for the transaction to be mined
    sleep 5

    # Verify the hook was created
    if celestia-appd query hyperlane hooks merkle-tree-hooks 2>/dev/null | grep -q "merkle_tree_hooks"; then
        echo "MerkleTreeHook created successfully!"
        touch "$MARKER_FILE"
    else
        echo "Warning: MerkleTreeHook creation may have failed"
    fi
else
    echo "Skipping post-init, already completed."
fi
