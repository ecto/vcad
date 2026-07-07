# waldo — the universal ordering rail (browser-agent adapter)

*Architecture proposal. 2026-07-07. Companion to
[ACP-CM](../agentic-commerce-custom-manufacturing.md) and the
[convergence strategy](2026-07-06-convergence-strategy.md) (asset 6: agents
that buy atoms). Working name: **waldo** — see Naming.*

## The problem

The ordering rail is built and fail-closed — quote → hash-bound mandate →
atomic wallet debit → `place_order` — but it terminates at a counterparty
that mostly doesn't exist. The manufacturing-API survey found sheet metal has
**zero** buyer APIs industry-wide; JLCPCB denied the API application; the
long tail of anodizers, powder coaters, and job shops will never have one.
Today's answer is the handoff layer (`fabricate/handoff.ts`): give the human
everything and let them drive the fab's website.

Every supplier does have one API: their website. A browser agent is the
adapter of last resort — and because it is last-resort, it is the *universal*
one. The system that drives it well can order from every supplier across
every domain: sheet metal, PCB/PCBA, filament, fasteners, metal stock,
finishing services. That is the missing half of ACP-CM: the protocol defines
the mandate; this defines the hands that execute it.

The handoff module's comment says "deliberately NOT browser automation of a
fab's checkout." That was correct — *for that layer, without rails*. What
makes checkout automation defensible is not a better selector library; it is
financial containment, fail-closed confirmation, and an autonomy ladder the
human controls. Those rails are this document.

## Thesis

Treat every supplier website as an undocumented, unstable, **untrusted** API,
and build a driver stack for it the way the kernel treats floating point:
deterministic where possible, adaptive where necessary, evidence everywhere,
and fail-closed at every money boundary.

Four load-bearing decisions:

1. **The mandate compiles into the payment instrument.** At `place_order`,
   the wallet debit funds a single-use virtual card: amount-capped at the
   authorized total (+ shipping tolerance), locked to the first merchant that
   charges it, expiring in ~48 h. A fully compromised agent cannot overspend
   — the failure domain is bounded by card economics, not model behavior.
   The card *is* the sandbox; declines are the enforcement.
2. **Playbooks are data; the agent is the repair mechanism.** Deterministic,
   versioned step graphs handle the 95% case cheaply and testably. A
   computer-use agent is the fallback when a step breaks or no playbook
   exists — and its second job is to emit a playbook patch (PR with new
   selectors + fixtures). The driver registry maintains itself.
3. **Checkout is a distributed transaction; treat it like one.** The buy
   click is two-phase: record intent + evidence, click once, then confirm
   via **two independent oracles** (confirmation page, confirmation email,
   card settlement — any two). Ambiguity → `RECONCILING`, never a blind
   retry. The idempotency key goes into the vendor's PO/notes field so
   reconciliation can find the order server-side.
4. **Autonomy is a per-vendor ladder, not a switch.** L0 handoff (today) →
   L1 assisted (waldo fills everything, human clicks buy) → L2 supervised
   (waldo clicks buy under mandate + virtual card) → L3 standing (repeat
   orders under standing mandates). Rungs are earned by canary history and
   set per-vendor by policy — some vendors stay pinned at L1 forever, and
   that is a feature.

## Separate repo: yes

Sibling repo, same pattern as `tang`: `ecto/waldo`, consumed by vcad.

- **Security blast radius.** This system holds vendor sessions, an email
  inbox, and payment-instrument handles, and it executes real-money actions.
  Its dependency tree (Playwright, browser binaries, card-issuer SDKs) and
  its review bar should not be vcad's.
- **Nothing about it is CAD.** Driving sendcutsend.com and driving
  digikey.com are the same machinery. Any design surface — or any agent —
  can be the client. ACP-CM is the protocol; waldo is the reference buyer-
  agent executor. That story is bigger standalone.
- **Churn isolation.** Vendor sites break weekly. Playbook patch releases
  must not ride vcad's release train, and vcad must be able to pin.
- **The registry is a community project.** Per-vendor driver packages with
  CI canaries — the terraform-provider model for procurement. It needs its
  own contributor identity to become "the place you check whether an agent
  can buy from X."

The seam is exactly ACP-CM's role boundary, which is why the split is clean:

| stays in vcad (money plane) | moves to waldo (execution plane) |
|---|---|
| wallet, `debit_wallet`, margin | browser runtime, sessions, vault |
| spend authorizations + human approval UI | driver registry (playbooks, fixtures, canaries) |
| quotes, `doc_hash` binding, DFM gates | job state machine, evidence pipeline |
| orders table, receipt assembly | email inbox oracle, tracking scrapes |
| `ManufacturerAdapter` broker | virtual-card *use* (vcad issues; waldo types) |

vcad never learns selectors; waldo never holds funds.

### Naming

The working name is **waldo**: Heinlein's remotely-operated manipulator
hands, which became real engineering slang for teleoperated arms in nuclear
labs. The metaphor teaches the architecture — a waldo is *teleoperation with
interlocks*, not autonomy: the human grants authority, the machine executes
precisely, hard physical limits bound the failure. Register-compatible with
tang/loon/phyz. Runners-up, should waldo collide: **factor** (the historical
mercantile agent who buys on a principal's behalf — semantically perfect,
lexically overloaded), **supercargo** (the ship's officer who transacts for
the cargo owner), **prokura** (civil-law signing authority granted to a
merchant's agent). Check npm/crates squatting before publishing; `@vcad/…`
scoping sidesteps it if needed.

## Architecture

```
vcad MCP (money plane)                 waldo (execution plane)
┌─────────────────────────┐            ┌──────────────────────────────┐
│ quote_manufacturing     │  quote job │ waldo-mcp / job API          │
│  └ BrowserAdapter ──────┼───────────►│  ├ queue (pg-boss)           │
│ authorize_spend         │            │  ├ engine: playbook exec     │
│  └ human approves (OOB) │            │  │   └ agent fallback+repair │
│ place_order             │  order job │  ├ runtime: Playwright/CDP   │
│  ├ verify mandate+hash  │───────────►│  │   ├ desktop sidecar       │
│  ├ debit wallet         │            │  │   └ cloud worker          │
│  ├ issue virtual card ──┼─card ref──►│  ├ vault (sessions, creds)   │
│  └ order row: SUBMITTED │◄─events────│  ├ inbox (orders+key@…)      │
│ get_order_status        │            │  └ evidence store            │
│ build_receipt ◄─claims──┼────────────│      canaries (scheduled)    │
└─────────────────────────┘            └──────────────────────────────┘
```

### Driver model — three tiers per step

- **Tier 0, HTTP:** many instant-quote sites are SPAs over clean JSON
  endpoints. Where stable and permitted, skip the DOM. Fastest, but the most
  ToS-delicate — per-vendor policy knob, off by default.
- **Tier 1, playbook:** versioned step graph as *data* (JSON): navigate,
  upload, select-by-label, extract-price — with semantic selectors, recorded
  DOM fixtures for offline regression tests, and **assertions on every
  money-adjacent step** ("cart total == authorized ± shipping tolerance",
  "material label == '5052 H32 0.125\"'", "line items == 9"). Assertions
  fail closed before any click.
- **Tier 2, agent:** a computer-use loop carrying the typed intent (never a
  prose goal). Invoked on playbook step failure or unknown vendor. Recording
  mode distills its trace into a new Tier 1 playbook; repair mode emits a
  playbook patch as a PR with fresh fixtures. New-vendor bring-up = one
  supervised Tier 2 run.

### The registry

`waldo-registry/<vendor>/` — one package per vendor: manifest (domains,
processes, capability matrix `quote|order|track|cancel`, autonomy ceiling,
**config schema** for the vendor's option space with vendor-native labels —
`shop_profile` generalized), playbooks, fixtures, canary spec. Semver per
vendor; vcad pins. Scheduled quote-only canaries (upload fixture DXF → walk
to price → assert) turn "brittle" into a measured SLO: adapter freshness is
data (`verified 6h ago`), drift alerts auto-open a Tier 2 repair job, and the
public scoreboard — *can an agent buy from X today?* — is the community
flywheel.

### Payment containment (the legendary part)

`place_order` today: verify mandate ∧ hash ∧ caps ∧ expiry → atomic debit.
Added step: the debit funds a **single-use virtual card** (Stripe Issuing /
Lithic / Privacy.com) capped at the authorized total, merchant-locked on
first settlement, short-expiry. waldo receives a card *reference*; the
runtime types the PAN into the payment iframe via CDP **outside model
context** — the model sees a placeholder, never the number. The issuer's
settlement webhook is then an independent oracle: *the vendor charged what
the human authorized* becomes a receipt claim checked by the card network,
not by the agent grading itself. Overspend, double-charge, and card exfil
are not "mitigated" — they are financially impossible.

### Prompt-injection defense (the page is untrusted input)

- Typed `OrderIntent`, no free-text goals in the driving agent.
- **Capability gating:** the runtime withholds the confirm-click capability
  until a separate auditor (cheap model or deterministic differ) signs that
  the review-page extraction matches the intent. The operator agent cannot
  click buy; it can only *request* the click.
- Per-job domain allowlist enforced at the proxy — the browser physically
  cannot navigate off-vendor.
- And the card bounds whatever survives all of the above.

### Exactly-once ordering

`SUBMITTED / SUBMIT_FAILED / RECONCILING` already exist in `OrderState` —
the state machine anticipated this. waldo's contribution is the discipline:
pre-click intent snapshot; one click; two-oracle confirmation (page scrape,
inbox parse via plus-addressed email `orders+<job>@…`, card settlement);
timeout → `RECONCILING` → scrape vendor order history / inbox for the
idempotency key *before any retry*. The buy click is never blindly retried.

### Evidence → receipts

Per job, an evidence bundle (hash-manifested like `FabArtifactRef`): step
log, screenshots with payment fields masked, DOM snapshots of quote/review/
confirmation, confirmation email, settlement record, tracking events. Plus
one elegant trick: the runtime feeds the file inputs, so it hashes the
**exact uploaded bytes in flight** — closing the chain
`doc_hash → DXF sha256 → uploaded-bytes sha256 → vendor order → invoice →
box`. New `OracleRef` ids: `waldo/upload-hash`, `waldo/confirmation-email`,
`waldo/card-settlement`, `waldo/tracking`. An order whose confirmation could
not be scraped is `Unverifiable`, not assumed — the receipt house rule,
applied to commerce.

### Deployment: desktop-first

The runtime's first home is a **sidecar on the user's machine** (vcad
already ships a Tauri app): residential IP, real browser profile, existing
logged-in sessions, and the takeover UX is trivial — a window opens, you
watch your agent shop, you grab the wheel for CAPTCHAs/2FA/final-click-at-L1,
it resumes. This one choice defuses most anti-bot friction *and* most trust
friction at once. Cloud workers (headless + live screencast into the app's
Orders panel) come second, for teams/scheduled canaries/tracking scrapes.
Human takeover is a first-class job state, not an error.

### Transports beyond the browser

The adapter contract stays transport-agnostic. Three transports satisfy it:
**api** (DigiKey, Mouser, Xometry — and JLCPCB the day they relent),
**browser** (waldo), **email-RFQ** (the true long tail: send files + spec,
parse the quoted PDF, human approves, reply with PO — slow-motion checkout,
same state machine, same evidence discipline). Intent is a tagged union:
`ConfiguratorIntent` (files + process config — SCS, JLC),
`CatalogIntent` (SKU + qty — McMaster, DigiKey, Amazon),
`RfqIntent` (files + prose spec — job shops). Those three cover every
commerce shape a hardware project needs, which is what "every supplier
across domains" cashes out to: transports × registry, not N integrations.

## ToS posture (honest version)

Automating checkout on a user's own account is against some vendors' terms;
datacenter browsers get fought by anti-bot. Posture: (1) desktop-first is
the user's own browser, their account, human-authorized spend, human-speed
interaction, order-scale volume — user-agent automation, not scraping;
(2) per-vendor autonomy ceilings are policy: a vendor that has said no to
automation (JLCPCB denied the API app — going around that is a relationship
decision, not a technical one) stays at **L1 assisted**, where waldo does
upload/configure/cart and the human performs the purchase click; (3) the
endgame is conversion — every waldo playbook is a demand signal, and "here
are N orders we sent you this quarter through this rail; here is the API we
wish you had" is the ACP-CM partner pitch. Adapters are designed to be
*retired* into api-transport gracefully. L1 alone already removes ~95% of
the friction and carries near-zero ToS/anti-bot/payment risk — which is why
it is the MVP, not a compromise.

## vcad integration points (concrete)

- `fabricate/broker.ts`: `CONTRACTED_FABS` hardcode → registry-driven
  capability data; add a `BrowserAdapter implements ManufacturerAdapter`
  that runs a waldo quote job and returns `pricing_basis: "binding"` —
  real SCS prices in the agent loop is the first shipped value, before any
  ordering.
- `fabricate/handoff.ts`: L1 evolves the handoff — same struct, plus a
  staged waldo job ("cart is loaded; here's the review screenshot; click
  buy here").
- `place_order`: after debit, issue VCC + submit waldo order job; order rows
  gain `waldo_job_id` + evidence artifact refs; events stream back into
  `orders.events`.
- `vcad-receipt`: new oracle ids listed above; `build_receipt` gains the
  commerce claims.
- Supabase: `waldo_jobs` mirror table (or event log) for the app's Orders
  panel; migration alongside 024/027.

## Sequencing

- **Wave 0 (now):** glockenspiel goes by hand — lead time is the long pole
  and the demo must not wait on infrastructure. In parallel: bootstrap
  `ecto/waldo` (core schemas, job state machine, evidence bundle) + the SCS
  **quote-only** playbook. Zero money, zero account, public instant-quote —
  and it immediately upgrades sheet-metal quotes from `estimate` to
  `binding`.
- **Wave 1:** L1 assisted checkout for SCS (desktop sidecar, staged cart,
  human clicks buy) + inbox oracle + evidence bundle → receipt claims. The
  *second* glockenspiel (or the 3DP wave) ships through it.
- **Wave 2:** L2 for SCS: virtual-card issuing wired to `place_order`,
  two-oracle confirmation, `RECONCILING` machinery, canaries + freshness in
  quote options. "Order via MCP" becomes literally true.
- **Wave 3:** second vendor to prove the registry generalizes (OSH Cut —
  nearly the same shape) + one `CatalogIntent` vendor (McMaster: easiest
  full-money-loop in existence, and every build needs fasteners). Then
  JLCPCB at L1 (gerber/BOM/CPL upload + part matching is the deepest flow;
  the PCBA rail is worth it).
- **Wave 4:** email-RFQ transport; registry goes public with the canary
  scoreboard; Tier 2 repair-PR loop.

## Open questions

1. **Card issuer.** Stripe Issuing (fits merchant-of-record posture, real
   API, webhooks) vs Lithic (built for exactly this) vs Privacy.com
   (consumer-grade, fastest to prototype). Leaning Stripe Issuing for the
   platform story; Privacy for a week-one spike.
2. **Desktop sidecar packaging.** Inside the existing Tauri app (an
   "Orders" surface + bundled runtime) vs a separate `waldo` daemon the app
   talks to. Leaning in-app for distribution, daemon-shaped internally.
3. **Registry publicity timing.** The scoreboard is a flywheel but also an
   anti-bot bat-signal. Private until Wave 3, public with the partner-pitch
   framing?
4. **Engine substrate.** Own thin engine on Playwright + computer-use
   (recommended: the playbook format, assertion language, and capability
   gating *are* the product) vs building on Stagehand and inheriting its
   act/extract caching. Steal ideas either way.
5. **The name.** waldo / factor / supercargo / prokura — cheap to decide
   before the repo exists, expensive after.
