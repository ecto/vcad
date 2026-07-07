# kerf architecture

*Founding design doc, 2026-07-07. Ported from
`ecto/vcad/docs/plans/2026-07-07-kerf-browser-ordering-rail.md`; this copy
is canonical and moves with the code.*

kerf is the **execution plane** for agentic hardware procurement — the
buyer agent's hands. A design surface (reference implementation:
[vcad](https://github.com/ecto/vcad)) remains the **money plane**: quotes,
DFM gates, the prepaid wallet, hash-bound spend authorizations approved by
a human out-of-band, and receipts. The protocol between them is
[ACP-CM](https://github.com/ecto/vcad/blob/main/docs/agentic-commerce-custom-manufacturing.md).
kerf never holds funds; the design surface never learns a CSS selector.

## The problem

Most suppliers have no purchasing API — sheet metal has zero buyer APIs
industry-wide, JLCPCB denied its API application, and the long tail of job
shops never will have one. Every supplier does have one API: their
website. A browser agent is the adapter of last resort, and because it is
last-resort it is universal. kerf treats every supplier website as an
undocumented, unstable, **untrusted** API and builds a driver stack for
it: deterministic where possible, adaptive where necessary, evidence
everywhere, fail-closed at every money boundary.

## Four load-bearing decisions

1. **The mandate compiles into the payment instrument.** At order time the
   money plane debits its wallet and funds a **single-use virtual card**
   (Stripe Issuing): amount-capped at the authorized total plus a shipping
   tolerance, merchant-locked on first settlement, ~48 h expiry. kerf
   receives a card *reference*; a server-side workflow step resolves it
   and types the PAN via CDP — the number never enters any model context.
   A fully compromised agent cannot overspend: the failure domain is
   bounded by card economics, not model behavior. The settlement webhook
   doubles as an independent oracle that the vendor charged what the human
   authorized.
2. **Playbooks are data; the agent is the repair mechanism.** Versioned
   JSON step graphs (semantic selectors, recorded DOM fixtures, assertions
   on every money-adjacent step) execute deterministically — Tier 1. A
   computer-use agent (Tier 2) is summoned per failed step or for unknown
   vendors, and its second job is emitting a playbook patch as a PR with
   fresh fixtures. Daily quote-only canaries turn "brittle" into a
   measured SLO and auto-open repair jobs on drift. Tier 0 (a vendor's own
   stable XHR endpoints) exists as a per-vendor policy option, off by
   default.
3. **Checkout is a distributed transaction.** The buy click is two-phase:
   durably record the placing attempt + review-page evidence, click once,
   then confirm via **two independent oracles** — confirmation page,
   plus-addressed confirmation email, card settlement (any two; they share
   no failure mode). Ambiguity parks the job in `RECONCILING`, whose first
   move is scanning the vendor's order history and the inbox for the
   idempotency key (written into the vendor's PO/notes field when
   supported). The buy click is never blindly retried; a provably-absent
   order is re-attempted only as a *new* job.
4. **Autonomy is a per-vendor ladder, not a switch.** L0 structured
   handoff → **L1 assisted** (kerf uploads, configures, carts; the human
   clicks buy in the live view — near-zero ToS/anti-bot/payment risk, and
   ~95% of the friction removed) → L2 supervised (kerf clicks buy under
   mandate + card) → L3 standing mandates. Rungs are **policy**, earned by
   canary history and capped by the vendor manifest's `autonomy_ceiling`.
   A vendor that has declined automation stays pinned at L1 forever;
   that is a feature, and the exit ramp is conversion — order volume
   through kerf is the pitch for the API we wish they had, and browser
   adapters are designed to be retired into `api` transport gracefully.

## Stack: eve, fully adopted

kerf is an [eve](https://vercel.com/eve) project end to end (decision
2026-07-07: no framework hedging).

- **The Tier-2 operator is an eve agent** (`agent/` — filesystem-first:
  `instructions.md`, `tools/`, `skills/`, `schedules/`). Its toolset is
  capability-gated: it can `request_confirmation` and `request_takeover`;
  it has no buy-click tool and never sees payment data. Capability gating
  is workflow structure, not prompting.
- **Jobs are durable workflows** (`workflows/`). Every money-adjacent
  action is its own step — retried on infra failure, memoized once
  complete (a finished buy click never re-executes on replay), fully
  traced (the trace is a free chunk of the evidence bundle). **Hooks**
  suspend a run for out-of-band humans and webhooks; **sleep** spans
  production lead times. One durable run spans quote → delivery:

  | lifecycle moment | primitive |
  |---|---|
  | upload / configure / assert / extract | step (retried, memoized) |
  | human approves mandate (out-of-band) | hook ← money-plane webhook |
  | CAPTCHA / 2FA / L1 buy click | hook ← human resolves in live view |
  | the L2 buy click | dedicated step, gated by the auditor step |
  | confirmation email | hook ← inbound-email webhook |
  | card settlement | hook ← Stripe Issuing webhook |
  | production + shipping | sleep (days–weeks) + tracking steps |
  | delivery | final steps emit the evidence bundle |

- **Canaries are eve schedules** (`agent/schedules/`).
- **The browser is a cloud `BrowserHost`** — Browser Use cloud first (the
  `@browser_use/eve` integration and template exist), alternates behind
  the same interface. Sessions persist by id independently of function
  invocations, so a suspended workflow reattaches via CDP on resume;
  live-view URLs give watch-and-takeover from any device; proxies and
  profile persistence are host features, not kerf code. Per-job domain
  allowlists (from the vendor manifest) are enforced at the session — the
  browser physically cannot navigate off-vendor.
- **Agent-facing surface: remote MCP** (quote/order/track/cancel jobs +
  live status). Design surfaces may also integrate service-to-service
  (HTTP + signed webhooks) under the same job API.

## Prompt-injection defense

The page is untrusted input. Layered: the operator carries a typed
`OrderIntent`, never a prose goal; a separate **auditor** step re-extracts
the review page and compares it against the intent before the workflow
enables the click (`kerf/intent-audit`); the session's domain allowlist is
enforced below the model; and the merchant-locked card bounds whatever
survives all of the above.

## Evidence → receipts

Per job, a hash-manifested `EvidenceBundle`: step trace, screenshots
(payment fields masked at capture), DOM snapshots of quote/review/
confirmation, the confirmation email, settlement record, tracking events.
The runtime feeds the vendor's file inputs, so it hashes the **exact
uploaded bytes in flight** — closing the chain
`design hash → file sha256 → uploaded-bytes sha256 → vendor order →
invoice → the box`. Oracle verdicts are `pass | fail | unverifiable` and
the house rule is fail-closed: an order whose confirmation could not be
scraped is *unverifiable*, never assumed. Oracles:
`kerf/upload-hash`, `kerf/quote-extraction`, `kerf/intent-audit`,
`kerf/confirmation-page`, `kerf/confirmation-email`,
`kerf/card-settlement`, `kerf/tracking`, `kerf/canary`.

## The registry

`packages/registry/<vendor>/` — data, not code: manifest (domains =
allowlist, processes, capability × transport matrix, autonomy ceiling,
vendor-native `config_schema`, canary spec with `budget_minor: 0` typed as
literal — canaries never spend), playbooks, fixtures. Vendors are
versioned packages; clients pin. Capability freshness ("green, verified
6 h ago") rides along with quotes. Three intent shapes cover every
commerce surface — `configurator` (SendCutSend, JLCPCB), `catalog`
(McMaster, DigiKey), `rfq` (email the job shop) — and three transports
(`browser`, `api`, `email`) satisfy the same contract, which is what
"order from every supplier across domains" cashes out to: transports ×
registry, not N bespoke integrations.

## Roadmap

- **Wave 0** — `quoteJob` + SCS quote playbook recorded via the
  vendor-bringup skill. Zero money; upgrades the design surface's
  sheet-metal pricing basis from `estimate` to `binding`.
- **Wave 1** — L1 assisted checkout (staged cart, human clicks buy in the
  live view), inbound-email oracle, evidence bundles.
- **Wave 2** — L2: Stripe Issuing wired, auditor + one-shot placing step,
  two-oracle confirmation, `RECONCILING`, canaries live.
- **Wave 3** — OSH Cut (registry generalization proof), McMaster-Carr
  (first `catalog` vendor, simplest full money loop), JLCPCB at L1.
- **Wave 4** — email-RFQ transport, public registry + canary scoreboard,
  Tier-2 repair PRs fully automated.
