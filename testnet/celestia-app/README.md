# Patched celestia-app (SP1 v6 zkism verifier)

The testnet runs a `celestia-app` image whose `x/zkism` Groth16 verifier is
patched to accept **SP1 v6** proofs. Upstream `celestia-app` (v7 through v8 and
`main`) ships a verifier built for the **SP1 v5** proof format and rejects v6
proofs with:

```
invalid proof length: expected 260, got 356
```

## Why

`ev-prover` generates Groth16 proofs with **SP1 6.2.3**. The on-chain proof
encoding and public-input scheme changed between SP1 v5 and v6:

| | proof bytes (`SP1ProofWithPublicValues::bytes()`) | gnark public inputs |
|---|---|---|
| SP1 v5 | `selector(4) + proof(256)` = **260** | `[vkey_hash, committed_values_digest]` (2) |
| SP1 v6 | `selector(4) + exit_code(32) + vk_root(32) + proof_nonce(32) + proof(256)` = **356** | `[vkey_hash, committed_values_digest, exit_code, vk_root, proof_nonce]` (5) |

The 96 extra bytes are three new public inputs. Upstream `x/zkism` hard-codes
`PrefixLen(4) + ProofSize(256) = 260` and builds a 2-input witness, so it cannot
verify v6 proofs.

## The patch — `zkism-v6-verifier.patch`

Applied on top of `celestia-app` **main (v9)** (v7 is the latest app version
the prover's celestia client supports; v8 fails ISM creation with "Unsupported
app version: 8"). It updates `x/zkism/types`:

- `verifier.go`: expect `ProofBytesLen = 4 + 96 + 256 = 356`; parse
  `exit_code | vk_root | proof_nonce` from the metadata; enforce `exit_code == 0`
  and `vk_root == VK_ROOT` (the SP1 v6 recursion-vk root); build the 5-input
  public witness `[vkey_hash, HashBN254(publicValues), exit_code, vk_root,
  proof_nonce]`; verify the gnark proof at `proofBz[100:]`.
- `msgs.go`: update the two `ValidateBasic` length checks to `ProofBytesLen`.

This mirrors `sp1-verifier` v6.2.3 (`Groth16Verifier::verify`). The `VK_ROOT`
constant (`002f85…f25352`) is `VK_ROOT_BYTES` from that crate.

> The ISM must be created with the **SP1 v6** Groth16 wrap vk
> (`crates/ev-prover/resources/groth16_vk.bin`, 492 bytes, `sha256[:4]=4388a21c`).
> celestia derives the proof prefix from `sha256(vk)[:4]`, so a v5 vk would fail
> the prefix check. Re-run `make deploy-ism` after switching images.

## Building

The image is built **directly from a celestia-app branch** that carries the fix
(`CELESTIA_APP_REF`, default `jonas/update-sp1-v6`, based on `main (v9)`) —
no patch is applied at build time. The branch must exist on `CELESTIA_APP_REPO`
(default `celestiaorg/celestia-app`) first.

```sh
make docker-build-celestia                 # fetches the branch and builds CELESTIA_IMAGE
make docker-push-celestia                  # optional: push the image (after `docker login`)
```

`docker-compose.yml` pins the two celestia services to `CELESTIA_IMAGE`
(`ghcr.io/celestiaorg/celestia-app-standalone:v9-zkism-v6`), so once
built (or pushed) `make start` uses it without rebuilding. Build from a fork or
different branch with `--build-arg`:

```sh
docker build -f testnet/celestia-app/Dockerfile testnet/celestia-app \
  --build-arg CELESTIA_APP_REPO=https://github.com/<you>/celestia-app.git \
  --build-arg CELESTIA_APP_REF=<branch> -t <image>
```

`zkism-v6-verifier.patch` is retained as a record of the exact diff on the branch.

## Upgrading the pinned celestia-app version

Rebase the branch onto the new tag (the fix is `zkism-v6-verifier.patch`), push
it, and rebuild. Bump the `CELESTIA_APP_REF` default in `Dockerfile` if the
branch name changes.
