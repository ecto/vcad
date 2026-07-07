# kerf — the universal ordering rail (browser-agent adapter)

*Architecture proposal. 2026-07-07. Companion to
[ACP-CM](../agentic-commerce-custom-manufacturing.md) and the
[convergence strategy](2026-07-06-convergence-strategy.md) (asset 6: agents
that buy atoms). Named **kerf** — see Naming.*

*Rev 2 (same day): runtime is cloud-first — Vercel Workflows (WDK) + eve +
a cloud browser host; the desktop-sidecar lane is dropped. Card issuer
decided: Stripe Issuing. The goal is that ordering works entirely via MCP
from any agent session, with no user machine in the loop.*

*Rev 3 (same day): named **kerf**; eve adopted fully (no framework
hedging). The scaffold briefly incubated at `kerf/` in this repo because
this session's GitHub credential cannot create repositories.*

*Rev 4 (same day): [`ecto/kerf`](https://github.com/ecto/kerf) exists;
extraction complete via `git subtree split` (root commit `72f32f3`). The
incubation copy is removed from this repo — kerf development happens in
ecto/kerf, and the canonical architecture doc lives there at
`docs/architecture.md`.*

*Rev 5 (same day): Wave 0 is largely built in ecto/kerf. The eve app
deploys on Vercel; the SendCutSend quote flow was recorded end-to-end via
live agentic probes (anonymous priced configurator reached — $5.58/ea @ qty
1 — and the fixture DXF vendor-validated at 100 × 50 mm); and the
deterministic Tier-1 engine (`@kerf/engine`: playbook runner with
fail-closed money-adjacent assertions, `quoted`-basis quote assembly, and
the ACP-CM intent hash) is implemented and tested (17 passing). The one
piece left before the quote loop closes is the concrete Browser Use CDP
`BrowserHost` plus a first live deterministic run — it needs live
browser-use egress. This file is the point-in-time proposal; the living
design tracks in [kerf's `docs/architecture.md`](https://github.com/ecto/kerf/blob/main/docs/architecture.md).*

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
   L1 assisted (kerf fills everything, human clicks buy) → L2 supervised
   (kerf clicks buy under mandate + virtual card) → L3 standing (repeat
   orders under standing mandates). Rungs are earned by canary history and
   set per-vendor by policy — some vendors stay pinned at L1 forever, and
   that is a feature.

## Separate repo: yes

Sibling repo, same pattern as `tang`: `ecto/kerf`, consumed by vcad.

- **Security blast radius.** This system holds vendor sessions, an email
  inbox, and payment-instrument handles, and it executes real-money actions.
  Its dependency tree (Playwright, browser binaries, card-issuer SDKs) and
  its review bar should not be vcad's.
- **Nothing about it is CAD.** Driving sendcutsend.com and driving
  digikey.com are the same machinery. Any design surface — or any agent —
  can be the client. ACP-CM is the protocol; kerf is the reference buyer-
  agent executor. That story is bigger standalone.
- **Churn isolation.** Vendor sites break weekly. Playbook patch releases
  must not ride vcad's release train, and vcad must be able to pin.
- **The registry is a community project.** Per-vendor driver packages with
  CI canaries — the terraform-provider model for procurement. It needs its
  own contributor identity to become "the place you check whether an agent
  can buy from X."

The seam is exactly ACP-CM's role boundary, which is why the split is clean:

| stays in vcad (money plane) | moves to kerf (execution plane) |
|---|---|
| wallet, `debit_wallet`, margin | browser runtime, sessions, vault |
| spend authorizations + human approval UI | driver registry (playbooks, fixtures, canaries) |
| quotes, `doc_hash` binding, DFM gates | job state machine, evidence pipeline |
| orders table, receipt assembly | email inbox oracle, tracking scrapes |
| `ManufacturerAdapter` broker | virtual-card *use* (vcad issues; kerf types) |

vcad never learns selectors; kerf never holds funds.

### Naming

**Decided: kerf.** A kerf is the width of material the cutting process
removes — the part of the stock the process itself takes. The metaphor is
exact twice over: this system is the cut between a design and delivered
atoms, and the kerf is the process's take — apt for the rail that carries
the merchant-of-record margin. Register-compatible with tang/loon/phyz.
(Considered along the way: waldo — Heinlein's teleoperated manipulator
hands, real nuclear-lab slang; factor — the historical mercantile agent
who buys on a principal's behalf; supercargo; prokura. Kept for the
etymology file.)

## Architecture

```
vcad MCP (money plane)                 kerf (execution plane — Vercel)
┌─────────────────────────┐            ┌──────────────────────────────┐
│ quote_manufacturing     │  quote job │ kerf-mcp (remote MCP)       │
│  └ BrowserAdapter ──────┼───────────►│  ├ engine: WDK workflows     │
│ authorize_spend         │            │  │   ├ playbook steps        │
│  └ human approves (OOB) │            │  │   └ eve agent (Tier 2)    │
│ place_order             │  order job │  ├ hooks: approval, takeover,│
│  ├ verify mandate+hash  │───────────►│  │   inbound email, Stripe $ │
│  ├ debit wallet         │            │  ├ BrowserHost: Browser Use  │
│  ├ issue virtual card ──┼─card ref──►│  │   cloud (CDP, live view)  │
│  └ order row: SUBMITTED │◄─events────│  ├ vault (sessions, creds)   │
│ get_order_status        │            │  ├ inbox (orders+key@…)      │
│ build_receipt ◄─claims──┼────────────│  └ evidence store + canaries │
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

`kerf-registry/<vendor>/` — one package per vendor: manifest (domains,
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
Added step: the debit funds a **single-use virtual card** (Stripe Issuing —
decided) capped at the authorized total, merchant-locked on
first settlement, short-expiry. kerf receives a card *reference*; the
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
the state machine anticipated this. kerf's contribution is the discipline:
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
box`. New `OracleRef` ids: `kerf/upload-hash`, `kerf/confirmation-email`,
`kerf/card-settlement`, `kerf/tracking`. An order whose confirmation could
not be scraped is `Unverifiable`, not assumed — the receipt house rule,
applied to commerce.

### Runtime: durable workflows on Vercel — no local anything

The execution plane is cloud-native so "order via MCP" is literally true
from any agent session, phone included, with no user machine in the loop.
Two Vercel primitives carry it:

- **Workflow SDK (WDK)** is the job engine. Every money-adjacent action is
  a `'use step'`: retried on infra failure, **memoized once complete**,
  every input/output recorded (the trace is a free chunk of the evidence
  bundle). **Hooks** suspend a run until an external event; **sleep**
  spans minutes to months.
- **eve** (Vercel's agent framework, June 2026, same durable substrate) is
  the Tier-2 substrate: durable agent loop, Vercel Sandbox, and an
  existing fork-and-deploy template pairing an eve agent with a Browser
  Use cloud browser, live-watchable.

The browser itself lives in a **browser cloud** behind a `BrowserHost`
interface (Browser Use cloud first — the eve template exists;
Browserbase/Steel/Sandbox-Chromium as alternates). Sessions persist by id
independently of function invocations, so a suspended workflow reattaches
via CDP on resume; live-view URLs give watch-and-takeover from any device;
proxies and profile persistence are host features, not kerf code. This is
why a browser cloud beats running Chromium inside a sandbox with a bounded
lifetime: checkout spans takeover waits, and the order spans weeks.

**The whole order lifecycle is one durable run:**

| lifecycle moment | workflow primitive |
|---|---|
| upload / configure / assert / extract | `'use step'` (retried, memoized) |
| human approves mandate (out-of-band) | hook ← vcad approval webhook |
| CAPTCHA / 2FA / L1 buy click | hook ← human resolves in live view |
| the L2 buy click | dedicated step, gated by the auditor step |
| confirmation email | hook ← inbound-email webhook |
| card settlement | hook ← Stripe Issuing webhook |
| production + shipping lead time | sleep (days–weeks) + tracking steps |
| delivery | final steps emit receipt claims |

Step memoization reinforces the two-phase click: a completed buy-click
step never re-executes on replay. The one dangerous window — click sent,
result not yet durably recorded — is exactly what `RECONCILING` covers:
a resumed run's first move is the order-history/inbox scan for the
idempotency key, never a second click.

Payment containment is unchanged in the cloud: card entry and the buy
click are **server-side steps, not model turns**. The step fetches card
details from Stripe Issuing and types them via CDP; the PAN never enters
any model context, and the Tier-2 agent has no buy-click tool to call —
capability gating is workflow structure, not prompting.

Human surfaces shrink to two: the vcad web app (approve mandates, watch
the live session, take over in an embedded live view) and push
notifications. Takeover from a phone is strictly better than a desktop
sidecar — same trust properties, zero install — and L1 assisted mode
("cart is staged; click buy in this live view") works from anywhere.
Human takeover is a first-class job state, not an error.

kerf-mcp is a remote MCP server on Vercel for agent-facing consumption;
vcad's fabricate broker talks to the same job API service-to-service
(HTTP + signed webhooks).

### Transports beyond the browser

The adapter contract stays transport-agnostic. Three transports satisfy it:
**api** (DigiKey, Mouser, Xometry — and JLCPCB the day they relent),
**browser** (kerf), **email-RFQ** (the true long tail: send files + spec,
parse the quoted PDF, human approves, reply with PO — slow-motion checkout,
same state machine, same evidence discipline). Intent is a tagged union:
`ConfiguratorIntent` (files + process config — SCS, JLC),
`CatalogIntent` (SKU + qty — McMaster, DigiKey, Amazon),
`RfqIntent` (files + prose spec — job shops). Those three cover every
commerce shape a hardware project needs, which is what "every supplier
across domains" cashes out to: transports × registry, not N integrations.

## ToS posture (honest version)

Automating checkout on a user's own account is against some vendors' terms;
datacenter browsers get fought by anti-bot. Posture: (1) this is user-agent
automation, not scraping — the user's own vendor account, human-authorized
spend, human-speed interaction, order-scale volume; residential/stealth
egress is a per-vendor browser-host option where warranted, not a default
identity-hiding posture;
(2) per-vendor autonomy ceilings are policy: a vendor that has said no to
automation (JLCPCB denied the API app — going around that is a relationship
decision, not a technical one) stays at **L1 assisted**, where kerf does
upload/configure/cart and the human performs the purchase click; (3) the
endgame is conversion — every kerf playbook is a demand signal, and "here
are N orders we sent you this quarter through this rail; here is the API we
wish you had" is the ACP-CM partner pitch. Adapters are designed to be
*retired* into api-transport gracefully. L1 alone already removes ~95% of
the friction and carries near-zero ToS/anti-bot/payment risk — which is why
it is the MVP, not a compromise.

## vcad integration points (concrete)

- `fabricate/broker.ts`: `CONTRACTED_FABS` hardcode → registry-driven
  capability data; add a `BrowserAdapter implements ManufacturerAdapter`
  that runs a kerf quote job and returns `pricing_basis: "binding"` —
  real SCS prices in the agent loop is the first shipped value, before any
  ordering.
- `fabricate/handoff.ts`: L1 evolves the handoff — same struct, plus a
  staged kerf job ("cart is loaded; here's the review screenshot; click
  buy in this live view").
- `place_order`: after debit, issue VCC + submit kerf order job; order rows
  gain `kerf_job_id` + evidence artifact refs; events stream back into
  `orders.events`.
- `vcad-receipt`: new oracle ids listed above; `build_receipt` gains the
  commerce claims.
- Supabase: `kerf_jobs` mirror table (or event log) for the app's Orders
  panel; migration alongside 024/027.

## Sequencing

- **Wave 0 (now):** glockenspiel goes by hand — lead time is the long pole
  and the demo must not wait on infrastructure. In parallel: bootstrap
  `ecto/kerf` (core schemas, job state machine, evidence bundle) + the SCS
  **quote-only** playbook. Zero money, zero account, public instant-quote —
  and it immediately upgrades sheet-metal quotes from `estimate` to
  `binding`.
- **Wave 1:** L1 assisted checkout for SCS — the workflow stages the cart
  in a cloud browser session, suspends on a hook, and the human clicks buy
  in the live view (from any device) — + inbox oracle + evidence bundle →
  receipt claims. The *second* glockenspiel (or the 3DP wave) ships
  through it.
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

## Decided

- **Card issuer: Stripe Issuing.** Fits the merchant-of-record posture;
  issuing, spend controls (amount cap + merchant lock), and settlement
  webhooks in one API.
- **Runtime: cloud, on Vercel.** Durable workflows as the job spine, remote
  MCP as the agent-facing surface. No local computer use; the desktop
  sidecar lane is dropped.
- **Framework: eve, fully adopted.** The Tier-2 operator, the job
  workflows, and the canary schedules are all eve constructs; `@kerf/core`
  (schemas, state machine, playbook format) stays framework-free as the
  contract layer, but there is no parallel non-eve engine.
- **The name: kerf.** Repo: `ecto/kerf` (scaffold incubating at `kerf/` in
  this repo until the standalone repo exists — see Rev 3 note).

## Open questions

1. **Browser host.** Browser Use cloud (eve template exists, live view)
   vs Browserbase vs Chromium-in-Sandbox. Judged on: session persistence
   across suspended workflows, live-view takeover UX, profile/proxy
   support, price per session-hour. `BrowserHost` interface either way, so
   this is swappable.
2. **Registry publicity timing.** The scoreboard is a flywheel but also an
   anti-bot bat-signal. Private until Wave 3, public with the partner-pitch
   framing?
