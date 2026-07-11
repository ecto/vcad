# The commerce trust boundary

vcad agents do two things that must never touch: they **ingest untrusted
content** (imported STEP/KiCad/Eagle files, part descriptions, datasheets,
supplier listings), and they **hold spend authority** (`authorize_spend`,
`place_order`). A poisoned part description that talks an agent into an
ordering decision is the canonical prompt-injection loss for an agent-facing
CAD tool. This document is the contract that makes that loss structurally
impossible — enforced mechanically in the MCP server, not by trusting the
model.

## The confinement rules

Enforced by `packages/mcp/src/trust-boundary.ts` at the single dispatch
choke-point in `server.ts`, before any handler runs. Fail-closed; refusals
carry a stable `TRUST_BOUNDARY:` prefix. CI proof:
`packages/mcp/src/__tests__/trust-boundary.test.ts`.

### 1. Money-plane tools accept opaque ids only

`authorize_spend` and `place_order` operate exclusively on ids minted by
vcad tools (`order_id`, `authorization_id`, `idempotency_key`,
`document_id`), restricted to `[A-Za-z0-9._:-]`, ≤128 chars. Free text — a
"part number" from a datasheet, a URL, prose — is refused before the handler
sees it. Parts reach orders only through the resolution pipeline
(`resolve_part` → catalog `family_id`), never as strings.

### 2. Artifact references are store-scoped

`fab_artifact_id` binds fab files to an order by reference. Accepted forms:
a bare `art_…` id, a relative `/artifacts/<id>[/<file>]` path (no dot
segments), or an absolute URL on an allowlisted vcad host. An external URL
planted in imported content can never be bound to an order — and the bytes
only ever come from the artifact store by id; the server never fetches a
caller-supplied URL.

### 3. Fab-bound free text stays plain

`ship_to`, `material`, and `finish` travel to the fabricator. They must be
plain bounded text: no URLs, no control characters, flat scalar fields,
length-capped. An address is not a place for instructions.

## What already stood (and this layer completes)

The money plane was designed with hard gates before this boundary existed:

- **Human approval is out-of-band.** `authorize_spend` only ever creates a
  `pending_human` authorization; the *only* path to `authorized` is a human
  on `vcad.io/authorize/<id>`. No MCP tool can approve spend.
- **`doc_hash` gate** — `place_order` re-hashes the document against the
  quote; any drift kills the order.
- **Receipt gate** — durable claims re-verify at order time, fail-closed;
  a violated receipt refuses the order.
- **Debit chokepoint** — idempotent, capped, single-use authorizations.

Those gates verify *the design and the money*. The trust boundary verifies
*the provenance of the words* — closing the remaining channel where
untrusted content could steer what gets ordered, where it ships, or which
files get fabricated.

## Extending the boundary

When adding a commerce-plane tool (anything that spends, ships, or binds
artifacts to money): add it to `COMMERCE_TOOLS` and classify each argument
as opaque-id / artifact-ref / fab-text in the corresponding table. A
commerce tool with an unclassified free-text argument is a review blocker.
