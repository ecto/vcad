# kerf

**Agents that buy atoms.** A durable browser-agent ordering rail with
fail-closed payment containment — the execution plane for
[ACP-CM](https://github.com/ecto/vcad/blob/main/docs/agentic-commerce-custom-manufacturing.md)
(agentic commerce for custom manufacturing).

A *kerf* is the width of material the cutting process removes — the part of
the stock the process itself takes. This system is the cut between a design
and delivered atoms: the ordering rail, and the margin it carries.

> **Status: incubating.** This tree currently lives at `kerf/` inside
> [`ecto/vcad`](https://github.com/ecto/vcad) until the standalone repo
> exists. Extract with history:
>
> ```bash
> # from a vcad checkout
> git subtree split -P kerf -b kerf-extract
> git push git@github.com:ecto/kerf.git kerf-extract:main
> ```

## Why

Most suppliers have no purchasing API. Sheet metal has **zero** buyer APIs
industry-wide; JLCPCB denied the API application; the long tail of job
shops, anodizers, and powder coaters never will have one. Every supplier
does have one API: their website. A browser agent is the adapter of last
resort — and because it is last-resort, it is the *universal* one.

kerf treats every supplier website as an undocumented, unstable,
**untrusted** API and builds a driver stack for it: deterministic where
possible, adaptive where necessary, evidence everywhere, fail-closed at
every money boundary.

## The four load-bearing decisions

1. **The mandate compiles into the payment instrument.** A human-approved
   spend authorization funds a single-use virtual card (Stripe Issuing):
   amount-capped, merchant-locked, short-expiry. A fully compromised agent
   cannot overspend — the failure domain is bounded by card economics, not
   model behavior. The card *is* the sandbox; declines are the enforcement.
2. **Playbooks are data; the agent is the repair mechanism.** Versioned
   step graphs with assertions handle the 95% case cheaply and testably. A
   computer-use agent is the fallback when a step breaks — and its second
   job is emitting a playbook patch as a PR with fresh fixtures. The driver
   registry maintains itself under scheduled canaries.
3. **Checkout is a distributed transaction.** The buy click is two-phase:
   record intent + evidence, click once, confirm via **two independent
   oracles** (confirmation page, confirmation email, card settlement).
   Ambiguity → `RECONCILING`, never a blind retry.
4. **Autonomy is a per-vendor ladder, not a switch.** L0 handoff → L1
   assisted (kerf stages the cart, the human clicks buy in a live view) →
   L2 supervised (kerf clicks buy under mandate + card) → L3 standing.
   Rungs are policy, earned by canary history; some vendors stay pinned at
   L1 forever, and that is a feature.

Full design: [`docs/architecture.md`](docs/architecture.md).

## Stack

Built on [eve](https://vercel.com/eve) (Vercel's durable agent framework)
end to end: the Tier-2 operator is an eve agent, jobs are durable
workflows (steps memoized once complete, hooks for approvals/webhooks,
sleep through production lead times — one durable run spans quote →
delivery), canaries are eve schedules, and the browser lives in a cloud
`BrowserHost` (Browser Use cloud first) with live-view takeover from any
device. Payments: Stripe Issuing. Agent-facing surface: remote MCP.

## Layout

```
kerf/
├── agent/                    # the eve agent (Tier-2 operator)
│   ├── agent.ts              #   model + runtime config
│   ├── instructions.md       #   the operator constitution
│   ├── tools/                #   typed tools (capability-gated: no buy click)
│   ├── skills/               #   vendor bring-up (recording mode)
│   └── schedules/            #   daily quote-only canaries
├── workflows/                # durable jobs: quote, order (step boundaries = the contract)
├── packages/
│   ├── core/                 # @kerf/core — intents, quotes, job state machine,
│   │                         #   playbook format + assertion grammar, evidence, registry types
│   └── registry/             # per-vendor driver packages (data, not code)
│       └── sendcutsend/      #   first vendor: manifest + quote playbook
├── scripts/                  # validate-registry (runs in CI)
└── docs/architecture.md      # the founding design doc
```

## Quickstart (standalone repo, after extraction)

```bash
npm install
npx eve@latest init .        # sync any scaffold drift with current eve conventions
npm run check                # typecheck @kerf/core + validate registry
npm run dev                  # eve dev server
```

Environment (see eve + Browser Use docs):

```
BROWSER_USE_API_KEY=bu_…     # browser-use.com cloud browser
AI_GATEWAY_API_KEY=…         # or a linked Vercel project
STRIPE_API_KEY=…             # Issuing: single-use virtual cards (server-side only)
```

Requires Node 24+ for eve dev; `npm run check` works on Node 22+.

## Roadmap

- **Wave 0** — repo bootstrap; SendCutSend quote-only playbook (zero money,
  public instant quote). Upgrades the design surface's sheet-metal quotes
  from `estimate` to `binding`.
- **Wave 1** — L1 assisted checkout for SCS: workflow stages the cart,
  suspends on a hook, human clicks buy in the live view. Inbox oracle +
  evidence bundle.
- **Wave 2** — L2 for SCS: Stripe Issuing virtual cards, two-oracle
  confirmation, `RECONCILING`, canaries with freshness surfaced in quotes.
- **Wave 3** — second sheet-metal vendor (OSH Cut) to prove the registry
  generalizes; first catalog vendor (McMaster-Carr); JLCPCB at L1.
- **Wave 4** — email-RFQ transport; public registry + canary scoreboard;
  Tier-2 self-repair PRs.

## Relationship to vcad

[vcad](https://github.com/ecto/vcad) is the reference **design surface and
merchant of record**: quotes, DFM gates, the wallet, hash-bound spend
authorizations, human approval UI, and receipts stay there. kerf is the
**execution plane**: browser sessions, drivers, evidence. vcad never learns
a CSS selector; kerf never holds funds. Any design surface — or any agent —
can be a kerf client.

## License

Apache-2.0
