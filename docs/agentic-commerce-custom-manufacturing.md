# Agentic Commerce for Custom Manufacturing (ACP-CM)

Draft 0.1 — 2026-07-01. Reference implementation: the vcad MCP server
(`quote_manufacturing` / `authorize_spend` / `place_order`).

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
  (vcad: parametric IR, kernel DFM, cost model). Issues quotes.
- **Buyer agent** — the AI agent acting for a human. Requests quotes,
  proposes spends. Never holds payment credentials.
- **Human principal** — approves spend out-of-band (never through the agent's
  own tool channel), funds the wallet, owns disputes.
- **Merchant of record** — charges the human, pays the fab (vcad in the
  reference implementation).
- **Fab** — manufactures. Accepts or declines each order, exactly as an ACP
  merchant does. Never sees the buyer-side economics.

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

Implemented in the vcad MCP server: quotes with `doc_hash` and artifact
binding (Phase 0, live), hash-and-cap-enforced atomic spend
(`debit_wallet`, migration 027), flag-gated `authorize_spend`/`place_order`
with out-of-band approval. Open: fab-side adoption (the partner rail),
binding-quote adapters, standing budgets.
