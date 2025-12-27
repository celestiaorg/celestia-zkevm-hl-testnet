## What we’re building

A “launch kit” for new chains (starting with Eden) that lets users **arrive funded** from anywhere with great UX. Users mostly take a **solver-first fast path** (solvers fulfill from inventory on the destination). Celestia provides **settlement rails** (Hyperlane routes + forwarding) and a **stable partner integration surface** (`quote → orderId → status`) that stays stable as execution providers evolve.

## Principles

- **Fast by default:** solvers fulfill; underlying rails settle later.
- **No Celestia wallet requirement:** users shouldn’t need a Cosmos wallet or a second signature.
- **Partners integrate once:** consistent API + status model.
- **Roam later:** chain abstraction UX starts with Privy/Para (speed), then migrates to Celestia MPC (hardening).

## Actors

User • Eden UI • Launch Kit API • Order Server • Observer/Verifier (Polymer-first) • Solvers/LiFi • Celestia rails (Hyperlane registry + bank + forwarding)

---

# Phase 0 — Pilot v0: Settlement primitive + optional slow path (9–14 engineering weeks)

## Goal

Ship the minimal Celestia-side primitive that removes “second signature on Celestia” from any Celestia-routed transfer, primarily to enable clean solver settlement rails.

### 0.1 Call-bound forwarding module (7–10 engineering weeks)

**Enables:** a deposit address committed to `(asset, destinationDomain, recipient)` where forwarding cannot be redirected and can be safely retried.

**Why needed:** without it, routing via Celestia requires a Celestia wallet signature and breaks hub routing UX.

**Engineering work**

- Spec commitment format + deterministic address derivation scheme
- Implement forwarding execution (transfer-only) with strict commitment checks
- Idempotent execution + replay protection at module level
- Asset/domain allowlists integrated with existing Hyperlane registry
- End-to-end integration tests across initial routes (Ethereum/Solana/Celestia → Celestia → Eden)
- Failure modes: “hold” state and operator-assisted recovery path

### 0.2 Forwarding executor service (1–2 engineering weeks)

**Enables:** forwarding happens automatically when deposits arrive.

**Why needed:** without it, deposits sit idle and require manual ops.

**Engineering work**

- Watch forwarding deposit credits (bank module balances for forwarding addresses)
- Submit forwarding executions and retry transient failures
- Minimal tracing output (deposit seen → forward submitted → forwarded)

### 0.3 Safety + recovery (1–2 engineering weeks)

**Enables:** safe pilot operations.

**Why needed:** even pilots need blast-radius controls and deterministic recovery.

**Engineering work**

- Pause switches per asset/domain/route + simple caps
- CLI: trace a forwarding address, show last actions, show held funds
- Basic incident playbook (pause, drain/return funds, resume)

**Eden user flow (Phase 0)**

- Default: user uses solver route to arrive funded (fast).
- Optional: slow route via underlying rails; forwarding prevents needing a Celestia wallet.
- Solvers: use forwarding + underlying rails for settlement/rebalancing after fills.

---

# Phase 1 — v0 Productization: solver-first UX + stable partner API + status

Phase 1 is intentionally **LI.FI-style** first: order/status is off-chain keyed by `orderId`. Optionally, terminal outcomes can be anchored on Celestia later for auditability.

---

## Phase 1A (default) — Off-chain order server + canonical status API (20–33 engineering weeks)

### 1A.1 Quotes API (5–8 engineering weeks)

**Enables:** Eden calls `GET /quote` and always receives a stable schema.

**Why needed:** partners shouldn’t be coupled to LiFi/solver-specific fields.

**Engineering work**

- `GET /quote` proxy to LiFi (initially) or direct solver RFQ
- Normalize fields: input/output amounts, fees, expiry, constraints, route metadata
- Quote validation: min/max bounds, expiry enforcement, partner allowlists
- Partner auth keys + rate limiting + logging

### 1A.2 Order server + lifecycle (4–7 engineering weeks)

**Enables:** `POST /orders → orderId` and simple lifecycle: `CREATED → IN_FLIGHT → ARRIVED/REFUNDED/FAILED`.

**Why needed:** without it, tracking is a mess of tx hashes across chains/providers.

**Engineering work**

- Deterministic `orderId` generation from normalized order payload
- `POST /orders` creates canonical record; stores refund policy + expiry
- `GET /status/:orderId` returns state + references (dest tx hash, solver ref, timestamps)
- Timeout/expiry transitions + refund-required vs manual-recovery-required outcomes
- Basic admin tooling for support (search by recipient, source tx hash)

### 1A.3 Completion observer/verifier (Polymer-first) (5–8 engineering weeks)

**Enables:** “ARRIVED means arrived” by watching Eden completion and finalizing status.

**Why needed:** solver dashboards are not a neutral source of truth.

**Engineering work**

- Define completion signals per destination (Eden): “user received funds”
- Observer service subscribes to chain events / tx outcomes
- Polymer-assisted verification (or trusted RPC fallback at first)
- Finalize status transitions + attach references
- Handle duplicate signals and idempotent updates (Celestia has single-slot finality)

### 1A.4 Partner integration pack (4–7 engineering weeks)

**Enables:** chains integrate quickly and consistently.

**Why needed:** otherwise each chain integration is bespoke.

**Engineering work**

- Partner API documentation with example flows + test vectors
- Standard config template (domains/assets supported, recipient formats)
- Sandbox environment + integration checklist
- Go-live guide and escalation path

### 1A.5 Safety + recovery (orders-level) (2–3 engineering weeks)

**Enables:** operational control and predictable support.

**Why needed:** public product surface needs “stop intake” and “trace by orderId.”

**Engineering work**

- Order intake allowlists/caps by partner, asset, route
- Kill-switch for creating new orders; allow finishing in-flight orders
- CLI / admin endpoints: trace(orderId), show linked refs, manual overrides (restricted)

**Eden user flow (Phase 1A)**

1. Eden → Launch Kit: `GET /quote`
2. Eden → Launch Kit: `POST /orders` → `orderId`
3. User follows solver flow (LiFi / solver), signs only what’s needed on their source chain
4. Solver fulfills on Eden from inventory (fast)
5. Observer marks `ARRIVED`; Eden shows completion via `GET /status/:orderId`
6. Solver later settles using underlying rails (Hyperlane routes + forwarding)

**Solver expectations (Phase 1A)**

- Participate in quoting (via LiFi or direct RFQ)
- Fulfill on Eden from inventory
- Provide destination completion reference (tx hash/event) so observer can finalize status
    
    Solvers do not post anything to Celestia in Phase 1A.
    

---

## Phase 1B (optional) — Anchor terminal receipts on Celestia (+6–12 engineering weeks)

This adds an auditable record of terminal outcomes on Celestia. It does not change fulfillment.

### 1B.1 Terminal receipt registry module (4–7 engineering weeks)

**Enables:** on-chain record of `orderId → ARRIVED/REFUNDED/FAILED` with references.

**Why needed:** becomes valuable once Roam/BTC custody increases platform responsibility; partners may want a neutral on-chain record.

**Engineering work**

- Minimal module: store terminal receipt keyed by `orderId`
- Enforce single terminal outcome (no conflicting writes)
- Store references (dest tx hash, verifier attestation ids) + timestamps
- Query endpoints (CLI and API) for partner verification

### 1B.2 Receipt poster + optional 2-of-3 attesters (2–5 engineering weeks)

**Enables:** automatic posting of terminal receipts; optional multi-attester acceptance.

**Why needed:** reduces reliance on a single observer service and strengthens assurances.

**Engineering work**

- Receipt poster service that submits Celestia txs
- Attester signature format + threshold verification logic (optional)
- Operational key management and rotation procedures

**How Phase 1B affects later phases**

- Phase 2+: optionally anchor “withdraw completed” or “custody credit completed”
- Phase 3+: optionally anchor “BTC credited” and “BTC withdrawal completed”
- Phase 5+: optionally anchor “credit issued” and “liquidation executed”

---

# Phase 2 — v1 Roam: chain abstraction UX via Privy/Para (28–47 engineering weeks)

## Goal

A NEAR-like user experience without a Celestia wallet: email/passkey login, protocol-provided deposit addresses, withdraw anywhere, and basic safety policies.

### 2.1 Identity + sessions (3–6 engineering weeks)

**Enables:** email/passkey onboarding + sessions; optional wallet linking.

**Why needed:** core “no wallet switching” UX.

**Engineering work**

- Integrate Privy/Para auth + user model
- Session issuance/refresh and secure backend auth
- Optional wallet linking and account recovery flows

### 2.2 Embedded wallets + EVM deposit addresses (4–8 engineering weeks)

**Enables:** protocol-provided per-user EVM deposit addresses.

**Why needed:** chain abstraction requires protocol-provided deposit instructions.

**Engineering work**

- Create embedded wallets per user
- Address registry and retrieval endpoints
- Deposit detection for EVM addresses (provider-indexed or RPC-based)
- Credit Roam balances + attach references for support

### 2.3 Balances + internal reservations (6–10 engineering weeks)

**Enables:** unified balances with “available vs reserved” semantics.

**Why needed:** prevents race conditions and enables later locks/credit.

**Engineering work**

- Balance ledger per user/asset + state transitions
- Reservation API tied to `orderId` (reserve/release/expire)
- Consistency checks: cannot withdraw reserved funds
- Audit logs and admin tooling for support

### 2.4 EVM execution adapter (withdrawals) (6–10 engineering weeks)

**Enables:** withdraw ETH/USDC to any EVM address.

**Why needed:** Roam must support reliable outbound execution.

**Engineering work**

- Transaction builder for ETH + ERC20
- Gas/fee selection and nonce management
- Broadcast + completion tracking and reconciliation to Roam ledger
- Withdrawal references for support and status updates (and optional Phase 1B receipts)

### 2.5 Smart-account-lite policies (5–9 engineering weeks)

**Enables:** safer custody: registered withdrawal addresses, delays, limits.

**Why needed:** basic defenses against account takeover and mistakes before MPC hardening.

**Engineering work**

- Policy engine: per-user limits, allowlists, change-delay for destination addresses
- Step-up auth for sensitive actions (new withdrawal address)
- Risk flags and basic anomaly detection hooks

### 2.6 Safety + recovery (2–4 engineering weeks)

**Enables:** custody-specific kill-switches and recovery.

**Why needed:** custody introduces new incident classes.

**Engineering work**

- Pause withdrawals per asset/chain
- Session invalidation + emergency rotation procedures
- Runbooks and minimal incident tooling

**Eden user flow (Phase 2)**

- Default: solver-first “arrive funded” (Phase 1A)
- Additional: user logs into Roam → deposits to protocol address → withdraws to Eden

---

# Phase 3 — v2+: BTC rails (then DOGE/LTC)

## Phase 3A — BTC MVP (7–14 engineering weeks)

### 3A.1 BTC deposit addresses (1–2 engineering weeks)

**Engineering work:** address generation via provider, user mapping, deposit instructions API.

### 3A.2 BTC deposits ingestion (2–4 engineering weeks)

**Engineering work:** provider/indexer integration, txid:vout dedupe, credit Roam ledger with references.

### 3A.3 BTC withdrawals via PSBT (4–8 engineering weeks)

**Engineering work:** coin selection, fee policy, PSBT signing flow, broadcast and completion tracking.

## Phase 3B — BTC scale/hardening (optional +7–15 engineering weeks)

- Own node + scanner (3–6)
- Batching + UTXO consolidation (2–5)
- Fee bump strategy (2–4)

Optional: BTC-backed representation on Eden (+10–18 engineering weeks)

DOGE/LTC add-ons: +6–10 engineering weeks each

---

# Phase 4 — Hardening: migrate signing to Celestia-run MPC (24–42 engineering weeks)

- MPC signer network MVP (18–30 engineering weeks): threshold signing nodes, policy gating, ops procedures
- Address migration (6–12 engineering weeks): cutover plan, fund migration, user messaging, fallbacks

---

# Phase 5 — Remote collateral + cross-chain state access (58–106 engineering weeks)

- State access SDK (10–18)
- Two venue adapters (14–26)
- Credit/risk rules (10–18)
- Locks/reservations + settlement flow (8–14)
- Liquidation/unwind executors (12–22)
- Safety controls (4–8)

---

## Summary (engineering weeks)

- Phase 0: **9–14 engineering weeks**
- Phase 1A: **20–33 engineering weeks**
- Phase 1B (optional): **+6–12 engineering weeks**
- Phase 2: **28–47 engineering weeks**
- Phase 3A BTC MVP: **7–14 engineering weeks**
- Phase 4 MPC hardening: **24–42 engineering weeks**
- Phase 5 remote collateral: **58–106 engineering weeks**