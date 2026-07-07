# Agentic Commerce for Custom Manufacturing (ACP-CM)

Draft 0.2 — 2026-07-07 (0.1: 2026-07-01). Reference implementations: the
vcad MCP server — the money plane (`quote_manufacturing` /
`authorize_spend` / `place_order`) — and [kerf](https://github.com/ecto/kerf)
— the execution plane (browser-rail quoting and ordering for fabs with no
API).

## The gap

Every shipped agentic-commerce rail — the OpenAI/Stripe Agentic Commerce
Protocol (ACP), Google UCP, AP2, Visa Intelligent Commerce, Mastercard Agent
Pay — assumes a **fixed-price catalog SKU**: a product feed the agent browses,
a price known before checkout, an item that exists before it is bought.

Custom manufacturing has none of that. The price is computed from geometry.
There is no feed. The "product" is a design that may still change between
quote and checkout, and a manufacturability (DFM) check can change the part —
or kill the order — after the buyer has decided to buy. No protocol in the
catalog-retail lineage can express "buy *this exact geometry* at the price
you quoted for it."

ACP-CM closes that gap with one primitive: **coupling the payment mandate to
a content hash of the design**. Everything else follows from it.

## Roles

- **Design surface** — where the geometry and its manufacturability live
  (vcad: parametric IR, kernel DFM, cost model). Produces fab-ready files
  and a `doc_hash`; issues its own design-cost *estimate*. A **client** of
  the commerce plane, not its host.
- **Buyer agent** — the AI agent acting for a human. Integrates the design
  surface and the commerce plane: carries files from one to the other,
  requests quotes, proposes spends. Never holds payment credentials, never
  sees a PAN.
- **Executor** — the hands: drives the fab's public quote/checkout surface
  under the mandate when the fab has no agent rail. Receives a card
  *reference*, never funds; every action is evidence-logged.
- **Human principal** — approves spend out-of-band (never through the agent's
  own tool channel), funds the wallet, owns disputes.
- **Merchant of record** — charges the human, pays the fab, issues the
  single-use card.
- **Fab** — manufactures. Accepts or declines each order, exactly as an ACP
  merchant does. Never sees the buyer-side economics.

**The commerce plane (0.2 refinement).** The Executor, Merchant of record,
wallet, and out-of-band approval surface consolidate into **one
deployable** — the *commerce plane* (kerf in the reference implementation),
which is *Stripe for metal*: the design surface and the buyer agent
integrate it; it never calls back into them. They consolidate for one
non-negotiable reason — the card issuer must hand the PAN to the executor's
runtime over a **server-to-server link the agent never mediates**, which is
what keeps the agent from ever holding card data. So "money plane" and
"execution plane" are not two services with the agent bridging them; they
are one. What crosses the design↔commerce boundary is **data** — the
shared receipt schema and the `doc_hash` — never a service call.

## Objects

### Quote (extends ACP's line item)

```jsonc
{
  "quote_id": "…",
  "doc_hash": "sha256(canonical design IR), truncated",   // THE binding
  "fab_artifact": {                                        // optional but recommended
    "artifact_id": "…",
    "manifest": [{ "name": "part.dxf", "sha256": "…" }]    // exact fab bytes
  },
  "process": "sheet_metal",
  "quantity": 3,
  "dfm": { "checked": true, "passed": true, "violations": [] },
  "total_amount_minor": 4250,
  "currency": "USD",
  "pricing_basis": "estimate | binding",
  "expires_at": "…"                                        // quotes decay; SKUs don't
}
```

A quote is only meaningful **with** its `doc_hash`. If the design changes, the
quote is dead — there is nothing to re-price against, only a new quote to
issue. `pricing_basis` distinguishes the design surface's own estimate from a
fab-committed price; only `binding` quotes should gate real money.

*0.2 refinement (learned by implementing):* browser-rail quoting surfaces
a basis between those two — **`quoted`**: the fab's OWN displayed price for
the exact artifact bytes and configuration, evidence-backed (DOM snapshot +
screenshot + the upload-hash chain below) but held by no server-side
reservation. Implementations MAY treat `quoted` as `binding` where the
fab's cart preserves the price through checkout, and MUST NOT gate money
on anything weaker.

### Spend authorization (the mandate)

A human-approved credential that authorizes the *agent* to place the order —
scoped, expiring, revocable, and **bound to the hash**:

```jsonc
{
  "authorization_id": "…",
  "quote_id": "…",
  "doc_hash": "…",              // mandate is void if the design changed
  "max_amount_minor": 5000,     // ceiling, not blank check
  "process_allowlist": ["sheet_metal"],
  "fab_allowlist": ["sendcutsend"],
  "expires_at": "…",
  "one_time": true,
  "status": "pending_human | approved | consumed | revoked"
}
```

Requirements that catalog-retail mandates don't have:

1. **DB-backed, not stateless.** A signed token alone cannot be revoked.
   Every `place_order` re-verifies status against the store of record.
2. **Approval happens out-of-band.** The agent *proposes* (`authorize_spend`);
   the human approves in a channel the agent does not control (web app,
   push). An agent must never be able to satisfy its own approval gate.
3. **Hash-checked at spend time.** If `doc_hash(current design) !=
   authorization.doc_hash`, the spend fails closed. This is the whole point:
   the human approved a *geometry*, not a conversation.

### Order

Standard lifecycle (`QUOTED → PENDING_HUMAN → PAID → SUBMITTED → IN_PRODUCTION
→ SHIPPED`, plus `RECONCILING`/`EXPIRED`), with two custom-manufacturing
additions: the order carries the `fab_artifact` manifest (per-file sha256 —
the order is traceable to the exact bytes the fab received), and a
**re-quote-on-DFM-change** rule: if the fab's own DFM alters the part after
payment, the order returns to a quote state and the mandate must be re-bound.
Silent post-payment mutation of a custom part is the custom-goods equivalent
of price switching.

## Flow

```
agent                       human                    fab
  │ quote_manufacturing        │                       │
  │──────────────► quote{doc_hash, price, dfm}         │
  │ authorize_spend(quote)     │                       │
  │──────────────► pending ───► approve (out-of-band)  │
  │ place_order(authorization) │                       │
  │   verify status ∧ hash ∧ caps ∧ expiry (fail closed)
  │   atomic debit ────────────┼──────────► submit(artifact manifest)
  │                            │            accept / decline (ACP semantics)
  │ get_order_status ◄─────────┴─────────── tracking / events
```

The **reorder** flow is the parametric dividend: re-evaluate the design (same
parameters or new ones) → new `doc_hash` → new quote → new mandate. A reorder
is a first-class re-derivation, never a replay of stale bytes.

## The execution plane (fabs with no rail) — added in 0.2

0.1's open list ended with "fab-side adoption (the partner rail)" — the
protocol was waiting for counterparties. 0.2 records the finding that
removed the wait: **the fab's public website is already a counterparty.**
An executor (kerf) drives the fab's own instant-quote and checkout
surfaces under the mandate, implementing the fab's half of the protocol
on its behalf. Adoption becomes a unilateral act: a fab that speaks
ACP-CM natively gets an `api` transport, a fab that speaks nothing gets a
`browser` transport, a job shop with an inbox gets `email` — the objects
above are transport-invariant, and native adoption becomes an
optimization rather than a prerequisite.

Two mechanisms make the browser transport safe enough to carry money:

- **Mandate compilation.** No delegated-payment token exists on this rail,
  so the mandate is compiled into the payment instrument itself: the
  wallet debit funds a single-use virtual card capped at the authorized
  total, merchant-locked on first settlement, short-expiry. The executor
  receives a card reference; the runtime types the number outside any
  model context. Overspend is not mitigated — it is financially
  impossible — and the issuer's settlement webhook becomes an independent
  oracle that the fab charged what the human authorized.
- **Two-oracle confirmation.** A browser-rail order is confirmed only when
  two independent witnesses agree it exists — confirmation page,
  confirmation email, card settlement (any two; they share no failure
  mode). Anything less parks the order in `RECONCILING`, whose first move
  is finding the order's idempotency key in the fab's own order history —
  never a second buy click.

### The hash chain

0.1 bound the mandate to `doc_hash` and the order to the artifact
manifest. The execution plane extends the chain to the fab's own parser:

```
doc_hash (canonical design IR)
  → fab_artifact manifest sha256 (exact fab bytes)
    → intent_hash (order parameters: files + config + qty)
      → upload-hash (bytes the executor delivered in-page)
        → the fab's echo (its parser's readback of the geometry)
```

Each link is a separately checkable claim with a named oracle
(`kerf/upload-hash` et al.). The first recorded run (SendCutSend,
2026-07-07) verified the chain end-to-end: the registry-pinned fixture,
the fetched bytes, the uploaded bytes, and the 100 × 50 mm bounding box
the fab's parser echoed back were provably the same part.

## Relationship to ACP

ACP-CM is intended as an **extension profile**, not a rival. Reused as-is:
checkout-session semantics, merchant accept/decline, delegated-payment token
shape, MCP-as-transport (ACP's `docs/mcp-binding.md` pattern). Added: the
quote object with `doc_hash` + `expires_at` + `dfm`, the hash-bound spend
authorization, artifact-manifest traceability, and re-quote-on-change. A fab
that already speaks ACP could adopt the profile by treating a quote as a
short-lived, single-SKU product feed of one.

## Consent and liability (non-negotiables)

- Human approval on every spend until standing budgets are earned trust, and
  3DS/SCA on the *funding* event (a signed mandate is not cardholder
  authentication and does not shift chargeback liability by itself).
- Custom parts are final-sale; disclose at approval time. The build receipt
  (DFM + verification evidence) is representment evidence, and is explicitly
  a manufacturability check, not a fitness-for-purpose warranty.
- Export control stays a human attestation (geometry-based classification is
  not automatable); default domestic fab lanes.

## Status

Commerce-plane seeds implemented in the vcad MCP server (to migrate into
the standalone commerce plane): quotes with `doc_hash` and artifact
binding (Phase 0, live), hash-and-cap-enforced atomic spend
(`debit_wallet`, migration 027), flag-gated `authorize_spend`/`place_order`
with out-of-band approval. Implemented in kerf (the commerce plane): the
browser transport against SendCutSend — recorded quote playbook walked to
an anonymous vendor price with money-adjacent assertions, the deterministic
Tier-1 runner + `intent_hash` binding + `quoted`-basis quote assembly
(tested), and the upload-hash chain verified end-to-end. Open: the concrete
cloud `BrowserHost` + a first live run to earn a `green` capability;
`quote` as a **kerf MCP tool the agent calls** (not a vcad broker adapter);
consolidating the wallet + approval surface + Stripe Issuing into kerf; the
L1/L2 order rail with mandate-compiled cards; standing budgets; and
fab-side native adoption — now an optimization, not a prerequisite.
