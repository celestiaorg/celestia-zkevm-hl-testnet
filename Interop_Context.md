Context Export: Celestia Interop “Launch Kit” + Roam Roadmap (Solver-First)
1) Goal

We’re designing a Celestia product for new chain launches (starting with an EVM chain like Eden) that lets their users onboard any asset from anywhere “instantly” with a great UX.

Core idea: users mostly use a solver/aggregator fast path (solvers fulfill orders from inventory on the destination chain). Celestia provides:

Settlement rails (Hyperlane routes + a Celestia forwarding primitive) used mainly by solvers to rebalance/settle after fills.

A stable partner integration layer (quote → orderId → status) so chains integrate once and can swap execution providers later.

Later, a chain-abstracted account product (Roam) using Privy/Para first (UX speed), later hardened to Celestia-operated MPC signing.

2) Current state & constraints

Celestia already has a Hyperlane module with a registry of assets/domains/routes for EVM and SVM chains.

Celestia has a bank module holding token balances on Celestia.

Hyperlane has existing monitoring/observability dashboards (e.g., Hyperlane explorer).

Initial underlying routes (mainly for solver settlement) are:

USDC/ETH from Ethereum

SOL from Solana

TIA on Celestia

Celestia finality: single-slot finality (don’t mention reorgs).

Pilot assumes validator multisig/attestation is acceptable initially; “hardening” comes later.

3) User experience targets

Users should not need a Celestia/Cosmos wallet at all.

Users should be able to “bring their own wallet” on whatever chain they’re starting from.

Solver-first default: user wants “arrive funded on Eden” and gets filled quickly from solver inventory.

Slow underlying route exists but is mostly for solvers; end users rarely use it.

4) Key problems driving direction

Two signatures problem when routing via Celestia hub today:

User signs on source chain to bridge to Celestia, then must sign on Celestia to forward to destination.

Worse: for EVM users this can mean needing an EVM wallet + a Cosmos wallet.

Solver custody risk / operational friction:

Solvers/LP vaults don’t want funds held in EOAs/multisigs (legal/attack surface).

Solvers want a no-custody or minimized custody settlement/rebalancing path, and to avoid pre-positioning capital on long-tail chains.

Demand for “blue chip” spot assets on new chains (ETH/SOL/USDC, later BTC/DOGE/LTC).

5) Settlement primitive choice (v0)

We chose: Call-bound forwarding first.
We explicitly decided: vault+sweeper comes later once MPC custody exists.

Call-bound forwarding concept (counterfactual ICA style)
Assets arriving on Celestia to a committed address can only be forwarded to the committed destination and recipient.

Derivation sketch (example)

forwardCall = transferRemote(token, destDomain, destRecipient)
callDigest = H(encode(forwardCall))
salt = H("CELESTIA_ICA_FORWARD_V1", callDigest)
// owner = Celestia forwarding router identity
forwardAddr = ICAAddress(owner, salt)


Flow:

Off-chain computes forwardAddr

User or solver deposits to forwardAddr (via Hyperlane or CEX withdrawal)

Executor submits forwarding execution

Celestia forwards tokens to destination

Properties:

Transfer-only in v0

Permissionless execution; cannot redirect funds

Safe retries; funds remain held if execution not submitted

Refund-to-sender policy is a later/Phase 1 concern; solver fees/sponsorship cover costs

6) Important product architecture decisions
A) Solver-first fulfillment model

Users mostly use solvers (via LiFi aggregator initially) to fulfill orders instantly on Eden.

Underlying routes + forwarding are settlement rails for solvers to settle/rebalance after filling users.

End users may have an optional slow path but it’s not the primary UX.

B) Phase 1 order/status tracking approach

We compared NEAR vs LiFi patterns:

NEAR intents: more on-chain anchoring via a verifier contract execution flow.

LiFi: typically off-chain tracking via a status API keyed by tx hashes / IDs, with an “order server” in their intents approach.

Recommendation chosen:

Phase 1A: LI.FI-style off-chain order server + stable orderId + canonical /status API.

Phase 1B (optional): anchor terminal receipts (ARRIVED/REFUNDED/FAILED) on Celestia as an auditable record later.

Key clarification: “who posts orderId/status to Celestia?”

In Phase 1A, they are off-chain (Launch Kit backend).

In Phase 1B, a Celestia-run receipt poster (optionally 2-of-3 attesters) posts terminal receipts on-chain.

7) Roadmap phases (engineering weeks)

All estimates are in engineering weeks (1 strong FTE engineer for 1 week). Team assumed strong + AI-assisted.

Phase 0 — Pilot v0: settlement primitive + optional slow path (9–14 engineering weeks)

Goal: remove Celestia-wallet requirement from any Celestia-routed transfer (primarily for solver settlement rails).

0.1 Call-bound forwarding module — 7–10 weeks

Commitment format + deterministic address derivation

Transfer-only forwarding execution with strict checks

Idempotency/retry safety

Allowlist integration with Hyperlane route registry

E2E tests for initial routes (ETH/SOL/TIA → Celestia → Eden)

0.2 Forwarding executor — 1–2 weeks

Watch deposits into forwarding addresses

Trigger forwarding; retry transient failures

Minimal trace logs

0.3 Safety + recovery (pilot) — 1–2 weeks

Pause/caps

Simple CLI trace (“where are funds”)

Basic incident playbook

Eden flow in Phase 0:

Default: user uses solver route (fast)

Optional: slow path uses forwarding (no Celestia wallet)

Phase 1 — v0 Productization: solver-first UX + partner API + status
Phase 1A (default): off-chain order server + canonical status (20–33 engineering weeks)

1A.1 Quotes API — 5–8 weeks

GET /quote proxy LiFi initially (or direct solver)

Normalize fields (expected output, fees, expiry, constraints)

Auth/rate limiting/logging

1A.2 Order server + lifecycle — 4–7 weeks

POST /orders → orderId

GET /status/:orderId with states: CREATED → IN_FLIGHT → ARRIVED/REFUNDED/FAILED

expiry + refund policy fields

support/admin search tools

1A.3 Completion observer/verifier (Polymer-first) — 5–8 weeks

Observe destination completion on Eden (tx/event that user received funds)

Verify via Polymer (or RPC fallback)

Update status deterministically (Celestia single-slot finality)

1A.4 Partner integration pack — 4–7 weeks

Docs, config templates, test plan, go-live checklist

1A.5 Safety + recovery (orders-level) — 2–3 weeks

order intake allowlists/caps

stop intake switch

trace by orderId; restricted manual overrides

Solver requirements (Phase 1A):

Provide quotes (via LiFi or direct RFQ)

Fulfill from destination inventory (fast)

Provide destination completion reference for observer

Solvers do not post anything to Celestia in Phase 1A

Eden user flow (Phase 1A):

Eden → Launch Kit GET /quote

Eden → Launch Kit POST /orders → orderId

User completes solver flow (LiFi/solver)

Solver fulfills on Eden from inventory

Observer marks ARRIVED → Eden polls GET /status/:orderId

Solver later settles via underlying rails (Hyperlane + forwarding)

Phase 1B (optional): terminal receipts anchored on Celestia (+6–12 engineering weeks)

Adds auditable outcomes; does not change fulfillment.

1B.1 Terminal receipt registry module — 4–7 weeks

Store orderId → ARRIVED/REFUNDED/FAILED + references

Enforce single terminal outcome

Query endpoints

1B.2 Receipt poster + optional 2-of-3 attesters — 2–5 weeks

Post terminal receipts

Optional threshold attestation verification

How 1B affects later phases:

Can anchor Roam withdrawals, BTC credits/withdrawals, remote collateral terminal events.

Phase 2 — v1 Roam: chain abstraction UX via Privy/Para (28–47 engineering weeks)

Goal: NEAR-like feel without a Celestia wallet: passkey/email login, protocol deposit addresses, withdraw anywhere.

2.1 Identity + sessions — 3–6 weeks

Privy/Para auth + user model

Sessions, account recovery, optional wallet linking

2.2 Embedded wallets + EVM deposit addresses — 4–8 weeks

Create embedded wallets per user

Address registry + deposit detection + credit balances

2.3 Roam balances + internal reservations — 6–10 weeks

ledger for balances; reserve/release/expire tied to actions/orders

consistency and admin tooling

2.4 EVM execution adapter (withdraw) — 6–10 weeks

ETH/ERC20 tx builder, gas/nonce management

broadcast + completion tracking; update ledger

2.5 Smart-account-lite policies — 5–9 weeks

withdrawal allowlists, limits, step-up auth, destination change delays

2.6 Safety + recovery (custody) — 2–4 weeks

withdrawal pauses, session invalidation, runbooks

Eden user flow (Phase 2):

Option A: solver-first “arrive funded” (Phase 1A)

Option B: user logs into Roam → deposits to protocol address → withdraws to Eden

Phase 3 — v2+: BTC rails (then DOGE/LTC)

BTC added post-Roam. Minted representations optional after rails.

Phase 3A BTC MVP (7–14 engineering weeks)

BTC deposit addresses — 1–2 weeks (provider-managed)

BTC deposits ingestion — 2–4 weeks (indexer/provider, txid:vout dedupe, credit ledger)

BTC withdrawals PSBT — 4–8 weeks (UTXO select, fee policy, PSBT signing, broadcast, tracking)

Phase 3B BTC hardening (optional +7–15 weeks)

own node+scanner, batching/UTXO consolidation, fee bump strategy

Optional:

BTC-backed representation on Eden: +10–18 weeks

DOGE/LTC rails add-ons: +6–10 weeks each

Phase 4 — Hardening: migrate signing to Celestia-run MPC (24–42 engineering weeks)

Goal: keep Roam UX stable but replace Privy/Para signing with Celestia-operated threshold signing.

MPC signer network MVP: 18–30 weeks

Address migration + cutover: 6–12 weeks

Phase 5 — Remote collateral + cross-chain state access (58–106 engineering weeks)

After BTC rails + Roam maturity.

State access SDK (Polymer-first): 10–18

2 venue adapters: 14–26

credit/risk rules: 10–18

locks/reservations + settlement: 8–14

liquidation/unwind executors: 12–22

safety controls: 4–8

8) Notes on how this evolves technically

Early phases are UX-first and rely on existing providers (LiFi for quotes, Polymer for state/root access, Privy/Para for embedded wallets & signing).

Celestia-specific on-chain expansion is minimized early:

Phase 0: forwarding module (core)

Phase 1A: mostly off-chain

Phase 1B: optional terminal receipts module

Later: Roam ledger/policies and eventually MPC hardening

Useful links / references (from conversation)
Core references

Lean intent framework inspiration: https://github.com/polymerdao/lean-intent-framework

NEAR chain abstraction / chain signatures: https://docs.near.org/chain-abstraction/chain-signatures

Diagram reference: https://docs.near.org/assets/images/chain-abstract-2-95e9600b99bb1a2837ca24f1e4ad9767.svg

NEAR multichain examples: https://github.com/near-examples/near-multichain

NEAR MPC reference: https://github.com/near/mpc

Near intents repo: https://github.com/near/intents

Aggregation / intents standards

Open Intents Framework docs: https://docs.openintents.xyz/docs

APIs: https://docs.openintents.xyz/docs/apis

Aggregators overview: https://docs.openintents.xyz/docs/aggregators/overview

OneBalance resource locks (types): https://docs.onebalance.io/concepts/resource-locks#types-of-resource-locks

LiFi “intents + resource locks” background: https://li.fi/knowledge-hub/li-fi-intents-are-taking-over-resource-locks-make-them-scale/

Uniswap Compact (resource lock primitive): https://github.com/Uniswap/the-compact

Identity / embedded wallets

Privy consumer overview: https://www.privy.io/consumer

Privy funding/wallets: https://docs.privy.io/wallets/funding/overview

Privy user object: https://docs.privy.io/user-management/users/the-user-object

Privy create wallet: https://docs.privy.io/wallets/wallets/create/create-a-wallet

Privy Bitcoin signing: https://docs.privy.io/wallets/using-wallets/bitcoin/sign-transaction-inputs

Para fintech: https://www.getpara.com/fintech

Cross-chain state access / “composers”

Spiceflow docs: https://spiceflow-docs.spicenet.io/

LiFi Composer examples: https://docs.li.fi/introduction/user-flows-and-examples/lifi-composer

Polymer (oracle/state roots): (used as third-party root provider initially; later can be replaced by Celestia light clients)

Issuance references

Eco issuers: https://eco.com/docs/getting-started/solutions/issuers

Crossmint minting: https://docs.crossmint.com/minting/introduction

Observability

Hyperlane explorer: https://explorer.hyperlane.xyz/

Short “starter prompt” for a fresh chat

We’re building a Celestia Interop Launch Kit for new chains (starting with Eden). Users primarily use solver-first fulfillment (LiFi initially). Celestia rails (Hyperlane routes + a call-bound forwarding module) are mainly for solver settlement/rebalancing; end users should never need a Celestia/Cosmos wallet. Phase 1 should be LI.FI-style: off-chain order server with orderId and /status, and optional Phase 1B to anchor terminal receipts on Celestia later. Phase 2 introduces Roam (chain abstraction UX) using Privy/Para first, then harden to Celestia-run MPC; BTC rails come after Roam. Use the roadmap and estimates included above.