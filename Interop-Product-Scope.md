# Interop Launch Kit - Product Scope & Engineering Investment

## Executive Summary

### What We're Building

A **Launch Kit** product that lets new chains offer users instant asset onboarding from any wallet, any chain. Users sign once on their source chain and arrive funded on the destination - no bridge UIs, no Celestia wallet, no multi-step flows.

**Pilot partner:** Eden, an EVM chain launching in ~2 months.

### Why Celestia Should Build This

1. **Verticalize around Celestia's blockspace:** Make Celestia the one-stop shop for teams building high-volume onchain markets. Data availability + seamless asset onboarding = more verticalization.

2. **Strategic positioning:** Every chain using Celestia DA becomes a potential Launch Kit customer. This creates stickiness beyond raw DA.

3. **Revenue opportunity:** Fees on asset routes flowing through Celestia infrastructure.

### How It Works

```
User wants funds on Eden
        ↓
Signs once on source chain (Ethereum, Solana, etc.)
        ↓
Solver fills the order instantly from their inventory on Eden
        ↓
User is funded in seconds
        ↓
Solver settles later via Celestia rails
```

Users get speed from solvers. Celestia provides the settlement infrastructure that makes solver operations viable, plus a stable API so partner chains integrate once and aren't locked into any single solver or aggregator.

---

## Engineering Investment

| Phase | Scope | Weeks | Calendar (3 engineers) |
|-------|-------|-------|------------------------|
| **Phase 0** | Settlement primitive | 9-14 | 1-1.5 months |
| **Phase 1A** | Partner API + order tracking | 14-24 | 1.5-2.5 months |
| **Phase 1B** | On-chain receipts *(optional)* | 6-12 | |
| **Phase 2** | Roam: chain-abstracted accounts | 28-47 | 3-5 months |
| **Phase 3A** | BTC deposits/withdrawals | 7-14 | 1-1.5 months |
| **Phase 3B** | BTC hardening *(optional)* | 7-15 | |
| **Phase 4** | MPC signing (replace Privy/Para) | 24-42 | 2.5-4.5 months |
| **Phase 5** | Remote collateral | 58-106 | 6-12 months |

*Engineering weeks = 1 strong full-time engineer for 1 week. Assumes AI-assisted development.*

### Totals

| Scope | Engineering Weeks |
|-------|-------------------|
| Core roadmap (required phases) | 140-247 |
| With optional phases | 153-274 |

### Parallelization

Phases can run in parallel with dedicated sub-teams:

- **Sequential:** Phase 0 → 1A (1A depends on 0)
- **Parallel after 1A:** Phase 2 (Roam) and Phase 3 (BTC)
- **Sequential:** Phase 4 depends on Phase 2; Phase 5 depends on 2, 3, and 4

---

## Security Model

**Key question:** How does Celestia know something happened on another chain?

This matters for:
1. **Asset transfers:** Validating that assets actually moved from Ethereum/Solana to Celestia
2. **Completion verification:** Confirming that a user actually received funds on Eden

### Approach: Extend Hyperlane for State Verification

Today, Hyperlane validators sign over **mailbox roots** (message queue commitments). We extend this to also sign over **state roots**, enabling verification of arbitrary on-chain state - not just messages.

| Component | Security Model | What It Does |
|-----------|----------------|--------------|
| Asset transfers | Hyperlane validators (mailbox roots) | Attest to cross-chain messages - existing functionality |
| Order lifecycle | LiFi | Tracks order status; we proxy their API |
| Completion verification | Hyperlane validators (state roots) | Attest to destination chain state - **new capability** |

**Why extend Hyperlane instead of adding Polymer?**
- Single validator set for both messages and state - simpler trust model
- We already depend on Hyperlane; this deepens the integration rather than adding a new party
- State root attestation is a natural extension of what validators already do

**Design principle:** Celestia integrates and normalizes - we don't attest. Hyperlane validators provide the cross-chain truth; we build on top.

### Future Upgrades (Not in Current Scope)

| Upgrade | What It Does | Rough Scope |
|---------|--------------|-------------|
| Light client verification | Replace validator multisig with cryptographic proofs | +8-15 weeks/chain |
| ZK state proofs | Succinct proofs of transactions/events | +15-25 weeks |

These are additive - security can be strengthened without rebuilding the product.

---

## Phase Details

### Phase 0: Settlement Primitive (9-14 weeks)

**Goal:** Enable Celestia-routed transfers without requiring a Celestia wallet or second signature.

**What we're building:** A forwarding module where assets deposited to a special address can only be forwarded to a pre-committed destination. No one can redirect them. This is primarily for solvers to settle after filling user orders.

**Why it matters:** Today, routing through Celestia requires a Celestia wallet signature, blocking hub-routing UX.

| Component | Weeks | Description |
|-----------|-------|-------------|
| Forwarding module | 7-10 | Address derivation, forwarding execution, safety checks, Hyperlane integration |
| Executor service | 1-2 | Watches deposits, triggers forwarding automatically |
| Safety + recovery | 1-2 | Pause switches, CLI tracing, incident playbook |

**Builds on:** Existing Hyperlane module on Celestia.

---

### Phase 1A: Partner API + Order Tracking (14-24 weeks)

**Goal:** Give partner chains a stable integration surface: `GET /quote` → `POST /orders` → `GET /status`. Partners integrate once and don't need to understand solver internals.

**What we're building:** A thin integration layer that proxies LiFi for order tracking and uses Hyperlane state root attestations for completion verification. Celestia provides API stability - not attestation.

| Component | Weeks | Description |
|-----------|-------|-------------|
| Quotes API | 4-6 | Proxy to LiFi, normalize schema, rate limiting |
| Order proxy | 2-4 | Map to LiFi orders, stable `orderId`, caching |
| Completion verification | 3-5 | Verify Hyperlane state root attestations for "funds arrived" |
| Partner integration pack | 4-7 | Docs, config templates, sandbox, go-live checklist |
| Safety + recovery | 1-2 | Rate limits, circuit breakers, admin tooling |

**Trust model:** LiFi owns order lifecycle. Hyperlane validators attest to completion (via state roots). We normalize and present a stable API.

**Eden flow:**
1. Eden calls `GET /quote` → receives normalized quote
2. Eden calls `POST /orders` → receives `orderId`
3. User signs on source chain; solver fills on Eden
4. Hyperlane state root attestation confirms arrival → status = `ARRIVED`
5. Eden polls `GET /status/:orderId` → shows completion

---

### Phase 1B: On-Chain Receipts *(optional, 6-12 weeks)*

**Goal:** Anchor terminal outcomes (ARRIVED/REFUNDED/FAILED) on Celestia for auditability.

**Why optional:** Doesn't change UX. Becomes valuable when Roam custody increases platform responsibility.

| Component | Weeks | Description |
|-----------|-------|-------------|
| Receipt registry | 4-7 | On-chain module: `orderId → outcome`, single-write enforcement |
| Receipt poster | 2-5 | Posts receipts, optional multi-attester verification |

---

### Phase 2: Roam - Chain-Abstracted Accounts (28-47 weeks)

**What is Roam?** Celestia's chain-abstracted account product. Users hold balances and transact across chains without switching wallets or managing multiple addresses.

**Goal:** NEAR-like UX: email/passkey login, protocol-provided deposit addresses, withdraw to any chain.

**What we're building:** A unified account system. Users can log in with email/passkey to get an embedded wallet, or link their existing wallet. Either way, they get deposit addresses, maintain balances, and withdraw anywhere.

| Component | Weeks | Description |
|-----------|-------|-------------|
| Identity + sessions | 3-6 | Privy/Para auth, sessions, recovery, wallet linking |
| Wallets + deposits | 4-8 | Embedded or linked wallets, deposit detection, balance crediting |
| Balances + reservations | 6-10 | Ledger with available/reserved semantics |
| Withdrawal execution | 6-10 | Tx building, gas management, broadcast, tracking |
| Policies | 5-9 | Withdrawal limits, allowlists, step-up auth |
| Safety + recovery | 2-4 | Withdrawal pauses, session invalidation, runbooks |

**User flows:**
- **Email/passkey:** Log in → get embedded wallet → deposit → withdraw
- **Bring your own wallet:** Link wallet → deposit → withdraw
- **Solver-first:** Use Phase 1A flow for instant "arrive funded"

---

### Phase 3A: BTC Rails (7-14 weeks)

**Goal:** Add Bitcoin deposits and withdrawals to Roam.

**Why it matters:** BTC is the most requested asset for new chains. ETH/SOL come via Hyperlane; BTC needs dedicated infrastructure.

| Component | Weeks | Description |
|-----------|-------|-------------|
| BTC deposit addresses | 1-2 | Provider-managed address generation, user mapping |
| BTC deposits | 2-4 | Indexer integration, deduplication, ledger crediting |
| BTC withdrawals | 4-8 | Coin selection, fee policy, PSBT signing, broadcast |

---

### Phase 3B: BTC Hardening *(optional, 7-15 weeks)*

**Goal:** Reduce reliance on third-party providers.

| Component | Weeks | Description |
|-----------|-------|-------------|
| Own node + scanner | 3-6 | Self-hosted Bitcoin infrastructure |
| Batching | 2-5 | UTXO consolidation, reduced fees |
| Fee bumping | 2-4 | RBF/CPFP for stuck transactions |

**Future add-ons (not in scope):** BTC-backed token on Eden (+10-18 weeks), DOGE/LTC rails (+6-10 weeks each).

---

### Phase 4: MPC Signing (24-42 weeks)

**Goal:** Replace Privy/Para with Celestia-operated threshold signing for Roam wallets.

**Why it matters:** Phase 2 uses Privy/Para for signing - a third-party dependency. MPC brings signing in-house with threshold security.

| Component | Weeks | Description |
|-----------|-------|-------------|
| MPC signer network | 18-30 | Threshold nodes, key generation, policy gating, ops procedures |
| Address migration | 6-12 | Migration plan, fund movement, user comms, fallbacks |

---

### Phase 5: Remote Collateral (58-106 weeks)

**Goal:** Let users use assets on one chain as collateral for actions on another, without bridging.

**Example:** User has ETH on Ethereum, uses it as margin on an Eden perps venue.

| Component | Weeks | Description |
|-----------|-------|-------------|
| State access SDK | 10-18 | Read remote chain balances/positions via Hyperlane state roots |
| Venue adapters | 14-26 | Integration with 2 trading venues |
| Credit/risk rules | 10-18 | Collateralization ratios, margin requirements |
| Locks + settlement | 8-14 | Reserve collateral, settle on liquidation |
| Liquidation executors | 12-22 | Handle undercollateralized positions |
| Safety controls | 4-8 | Circuit breakers, position limits |

---

## Milestones

| Milestone | Phases | Cumulative Weeks | What Users Can Do |
|-----------|--------|------------------|-------------------|
| **Eden Pilot** | 0 + 1A | 23-38 | Arrive funded on Eden via solver; track order status |
| **Roam Launch** | + 2 | 51-85 | Log in with email; deposit/withdraw anywhere |
| **BTC Support** | + 3A | 58-99 | Deposit/withdraw BTC |
| **Full Hardening** | + 4 | 82-141 | Celestia-operated signing (no Privy/Para) |
| **Remote Collateral** | + 5 | 140-247 | Use assets as cross-chain margin |

---

## Dependencies & Assumptions

**Existing infrastructure (not counted):**
- Hyperlane module on Celestia with asset/domain registry and warp routes

**Third-party providers:**
| Provider | Role | Phases |
|----------|------|--------|
| LiFi | Order lifecycle, solver aggregation | 1A+ |
| Hyperlane | Asset transfers (mailbox roots) + completion verification (state roots) | 0+ |
| Privy/Para | Embedded wallet custody | 2-3 (replaced in 4) |

**Hyperlane extension required:**
- Current Hyperlane validators sign over mailbox roots (message queues)
- We need them to also sign over state roots for completion verification and remote state access
- This is a capability extension, not a new trust assumption - same validator set, expanded attestation scope

**Other assumptions:**
- **Eden:** Confirmed pilot partner, launching in ~2 months
- **Team:** Current team is 3 engineers; scope informs hiring decisions
