# Agent-Native Factory — the purple cow spec

**One sentence:** an agent designs a part in chat, you watch it move, tap approve,
and a verified physical object shows up at your door — with a receipt that proves
it's in spec *and* that the order provably happened.

Nobody else can close this loop. vcad owns the design surface and the receipt;
[kerf](https://github.com/ecto/kerf) ("Stripe for metal", the reference
implementation of [ACP-CM](agentic-commerce-custom-manufacturing.md)) owns the
execution rail — supplier drivers, the payment instrument, the out-of-band human
approval, and the evidence. Four workstreams:

1. **Play button** — live physics replay in the MCP Apps viewer (M1)
2. **Order tracking** — the fused vcad+kerf lifecycle rendered in the viewer (M2)
3. **Approval + receipts** — elicitation-native approval, receipt-gated money (M3–M4)
4. **Deprecated-surface removal** — audit result + 2026-07-28 RC prep (M0)

UI mockups for the transport bar, order dock, and approval flow were reviewed in
the design session of 2026-07-08; the UI contracts below encode their decisions.

> **Status: IMPLEMENTED 2026-07-08** (M0–M4 + kerf rail; M5 remains deferred).
> As-built deltas from the plan below:
> - **FK is server-side, not client-side.** `get_sim_replay` returns per-step
>   `instance_transforms` computed via kernel `solveForwardKinematics` (the
>   `record_simulation` pattern); the viewer applies node TRS and does zero
>   joint math. `get_preview_kinematics` was therefore never built. Kernel
>   stays the single FK source of truth; FK parity is exact by construction.
> - **No new flags.** The kerf rail is live whenever `KERF_URL` is set
>   (degrades to `estimate` basis when unreachable); elicitation is gated on
>   client capability only; the receipt + doc-hash gates in `place_order` are
>   always on (fail-closed when a receipt/spec exists, `unverified` when not).
>   `VCAD_FABRICATE_ORDERING=1` (pre-existing money seam) still gates
>   authz/place.
> - **kerf side implemented on branch `feat/http-quote-api`** (kerf repo):
>   `POST /api/quote` (+ `GET /api/jobs/:id`, `/evidence`), assertTransition-
>   guarded job store, Browser-Use CDP host (live mode pending
>   `BROWSER_USE_API_KEY` + first real SCS run), ScriptedHost exported for
>   deterministic CI. Quote jobs terminate `STAGED → DELIVERED` (deliberate
>   core transition addition; money invariants untouched).
> - Operator actions outstanding: push kerf branch + deploy, `supabase db push`
>   migration 034, build the `/authorize/<id>` web-app route (L2 lane).

---

## 0. Current state (verified against source, 2026-07-08)

**Viewer (SEP-1865, dual-host).** Single live canvas, not one-iframe-per-call.
Template/data split enforced through `viewerMetaFor()` in
`packages/mcp/src/server.ts:561` and locked by
`src/__tests__/viewer-meta.test.ts`. Milestone tools (`open_document`,
`create_cad_loon`, `place_components`, `build_receipt`, …) mount the iframe;
data tools carry only a `{document_id, document_version}` handle. The viewer
fetches geometry itself via app-only tools `get_preview_glb` /
`get_preview_version` (`tools/preview.ts`), polling adaptively
(2.5s fast / 10s idle, `viewer-app/main.ts:1354`). Hosts: Claude/Cursor via
`@modelcontextprotocol/ext-apps` 1.7.4 `App`, ChatGPT via `openai-shim.ts`.
Viewer→model channel exists (`updateModelContext` selection grounding).

**Physics gym.** 8 tools in `tools/gym.ts` + `record_simulation` in
`tools/record.ts`. Per-step state from WASM `PhysicsSim`:
`joint_positions` (deg/mm), `joint_velocities`, `end_effector_poses`
(7-float pose per EE) — **no per-link transforms**. `record_simulation`
already proves the render path: it writes joint positions into the cloned
document's `joints[j].state` and lets kernel FK reconstruct body poses
(`record.ts:287`), then renders a GIF. Envs live in in-process `Map`s.

**Fabricate.** Order spine in `tools/order.ts` (quote/status/list, always on)
and `tools/ordering.ts` (`authorize_spend`/`place_order`, gated behind
`VCAD_FABRICATE_ORDERING=1`). Prepaid wallet, atomic `debit_wallet` RPC,
16-state lifecycle (`fabricate/types.ts:24`). The asymmetric-capability seam:
agent proposes (`authorize_spend` → `pending_human`), **human approves
out-of-band**, agent then calls `place_order`. Persistence: in-memory or
Supabase (`fabricate/store.ts:484`). `margin_hidden: true` — fab cost and
margin never leave the server.

**Receipts.** `DesignReceipt` (`vcad.receipt/1`, `crates/vcad-receipt`) is
fail-closed and attaches to **documents**, not orders. Ordering and receipts
are disjoint today — M4 closes that.

**kerf (pre-alpha, Wave 0 in progress).** Contract layer + registry are real
and CI-enforced; SendCutSend quote-only playbook first. Key types:
`OrderIntent` (files pinned by content hash, `budget_cap` in minor units,
vendor-native config labels), `VendorQuote` with
`pricing_basis: estimate | quoted | binding` bound to an `intent_hash`,
`JobState` machine (17 states; `PLACING` entered at most once, ever;
`CONFIRMED` requires two independent oracles), `EvidenceBundle` — "the
per-job bundle handed back to the design surface's receipt", verdicts
`pass | fail | unverifiable` inherited from vcad-receipt. Autonomy ladder:
L0 handoff → L1 assisted (human clicks buy in live view) → L2 supervised
(single-use virtual card under mandate) → L3 standing. Agent-facing surface:
remote MCP; jobs are eve durable workflows.

**Deprecated MCP surface.** Audit result: **zero usage.** No sampling, roots,
or logging anywhere in first-party source; capabilities never advertised them;
only handlers registered are ListTools/ListResources/ReadResource/CallTool
(`server.ts:1031–1150`). Transport is already Streamable HTTP. SDK 1.29.0,
negotiates spec 2025-11-25.

---

## The kerf rail — integration architecture

**Posture:** vcad calls kerf as a service (kerf's rule: "you integrate kerf;
kerf does not integrate you"). No vendoring. The design surface never learns a
CSS selector or touches a card; kerf never evaluates geometry.

- `FulfillmentBroker` (`packages/mcp/src/fabricate/`) gains a **kerf driver**:
  submits `ConfiguratorIntent`s built from the fab bundle (`FabArtifactRef`
  files, already sha256-pinned — kerf's `kerf/upload-hash` oracle verifies the
  exact bytes), receives `VendorQuote`s and job-state updates (webhook →
  session event spine → `get_order_feed`).
- **Pricing-basis upgrade:** today `quote_manufacturing` returns vcad's own
  cost model (`estimate`). Through kerf Wave 0, sheet-metal quotes become the
  fab's own displayed price (`quoted`). `estimate` never gates money; `quoted`
  may where the cart preserves price; `binding` is fab-committed.
- **Intent-hash discipline:** a quote is only meaningful with its
  `intent_hash`. Geometry edit after quoting ⇒ quote is dead ⇒ re-quote.
  `place_order` enforces this (see M4).

### State mapping (vcad OrderState × kerf JobState → dock chip)

| dock chip | vcad | kerf |
|---|---|---|
| quoted | `QUOTED` | quote job `DELIVERED` |
| approval | `AUTHORIZED` pending | `TAKEOVER_WAIT` (L1) / mandate pending (L2) |
| placing | `PAID`/`SUBMITTED` | `STAGING`,`STAGED`,`AUDIT`,`PLACING`,`CONFIRMING`,`RECONCILING`* |
| confirmed | `SUBMITTED`→ | `CONFIRMED`, `RECONCILED_PLACED` |
| production | `IN_PRODUCTION`/`SHIPPED` | `TRACKING` |
| delivered | `DELIVERED` | `DELIVERED` |
| failed (red) | `SUBMIT_FAILED` etc. | `AUDIT_FAILED`, `FAILED`, `RECONCILED_ABSENT` |

\* `RECONCILING` renders as an explained wait ("click outcome ambiguous,
checking vendor history + inbox"), never a retry button. Raw states remain in
the per-order event log.

### Wave gating

| kerf wave | unlocks in vcad |
|---|---|
| Wave 0 (SCS quote-only) | `quoted` pricing basis; dock quote cards. **Integratable now.** |
| Wave 1 (L1 assisted) | approve-in-live-view button; elicitation URL → live view |
| Wave 2 (L2 + Issuing) | mandate-funded virtual cards; two-oracle confirm; `RECONCILING` |
| Wave 3 (more vendors) | vendor choice in `fab_options`; catalog parts (McMaster) |

---

## M0 — Deprecated fns: remove nothing, guard everything

There is nothing to delete. The work is a tripwire plus forward-compat prep:

- **Tripwire test** (`src/__tests__/deprecated-surface.test.ts`): assert no
  handlers for `sampling/createMessage`, `roots/list`, `logging/setLevel`, and
  no `sampling`/`roots`/`logging` capability keys. Elicitation is NOT
  deprecated — it's the replacement pattern and M3 builds on it.
- **2026-07-28 RC prep (notes only until the SDK ships):** `document_id`
  handles already match the stateless pattern; add `ttlMs`/`cacheScope` on
  list responses when supported (`private` for tools — pack-config-varying;
  `public` + long TTL for the viewer HTML resource); `services/mcp` can route
  on `Mcp-Method` once clients send it; error `-32002` → `-32602` at bump.

Effort: half a day. Zero user-visible change.

---

## M1 — The play button (live physics in the viewer)

**Goal:** `create_robot_env` mounts the canvas; a ▶ button plays the rollout
the agent just ran — scrub, pause, speed — inside the chat thread, on both
Claude and ChatGPT.

### Design decision: joint trajectories + client-side FK, not frames

`record_simulation` proves poses reconstruct from joint states. Frames are
heavy and dead; `number[][]` trajectories are tiny and scrubbable for free.
Viewer FK: revolute = rotate child subtree about anchor axis (degrees),
prismatic = translate along axis (mm), matching `packages/engine/src/physics.ts:12`.

### 1a. Trajectory capture (server)

Ring buffer on the gym env record (`gym.ts` env `Map`): every
`gym_step`/`batch_step` appends `joint_positions` + reward + done (cap 600
steps, matching `record_simulation`). `gym_reset` truncates.

### 1b. New app-only tools (`visibility: ["app"]`, hidden from model)

| tool | args | returns |
|---|---|---|
| `get_sim_replay` | `{ env_id }` | `{ document_id, dt, substeps, joint_trajectory: number[][], rewards: number[], target_pose?, version }` |
| `get_sim_version` | `{ env_id }` | `{ env_id, step_count, version }` — cheap change token |
| `get_preview_kinematics` | `{ document_id }` | `{ joints: [{ id, type, axis, anchor_mm, parent_instance, child_instance, state }], instances: [{ id, node_name }] }` |

`version` = FNV-1a over `(env_id, step_count)` (pattern: `preview.ts:206`).
`target_pose` comes from the env's reward spec when one exists, so the viewer
can draw the target marker.

### 1c. Segmented GLB

`get_preview_glb` gains `{ segmented?: boolean }` — one named node per part
instance so the viewer can bind FK targets. **Spike first:** `buildGlb` may
already emit named per-instance nodes; if so this is a contract test, not a
feature.

### 1d. Viewer UI contract (per reviewed mockup)

- Transport bar at canvas bottom: ▶/⏸ toggle, scrub slider (free — the
  trajectory is data), step counter `t / N`, speed select (0.25×–4×).
- Reward sparkline with progress dot, fed by `rewards[]` — the visible
  "watch it learn" signal across successive rollouts.
- Joint readout line (`j1 55.0° · j2 30.0° · reward −0.12`) + `dt`/rate —
  grounds it as simulation, not animation.
- End-effector trail polyline from FK. Target marker from `target_pose`.
- **Live-follow badge:** while `get_sim_version.step_count` advances, pin the
  scrub head to newest and poll fast (2.5s); otherwise slow (10s). Same
  adaptive loop as geometry.
- Playback wall-clock rate = `dt × substeps × speed`, interpolating rows.

### 1e. Behavior/meta changes

- `create_robot_env`: `behavior({ mount: true, geometry: true })`.
- `record_simulation` unchanged — GIF stays the durable transcript artifact.
- Regen `tool-surface.fixture.json` (deliberate, reviewed); extend
  `viewer-meta.test.ts` for the three new app-only tools.

Constraints: ChatGPT parity via `callServerTool` (no new host capability);
in-process envs mean replay works within a warm instance only (same caveat as
the gym itself — document it).

Effort: ~3–5 days. Demo: *"watch the arm learn to reach, in the chat."*

---

## M2 — Order tracking in the MCP UI

**Goal:** the mounted canvas grows an order dock rendering the fused
vcad+kerf lifecycle.

### 2a. New app-only tool

| tool | args | returns |
|---|---|---|
| `get_order_feed` | `{ document_id }` | `{ orders: [{ order_id, state_chip, raw_state, kerf_job_state?, process, quantity, total_amount_usd, pricing_basis, vendor_display_name, lead_time_days, quote_expires_at, created_at, events: [{ at, type, note }], authorization: { status, cap_usd, expires_at, approve_url } \| null, tracking, receipt: { status, claims_pass, claims_total } \| null, evidence: { items, oracles_pass, oracles_needed } \| null }], wallet_balance_usd, version }` |

Backed by `FabricateStore.listOrders` + `spend_authorizations` + kerf job
webhooks on the event spine; owner-scoped like the tools it mirrors.
**Margin invariant preserved:** only totals, never fab internals.

### 2b. Order dock UI contract (per reviewed mockup)

- Collapsible panel on the mounted canvas; renders when feed is non-empty.
- Per-order card: part name + qty + material/thickness · vendor + lead time +
  quote expiry · total with **pricing-basis pill** (`estimate` gray,
  `quoted` amber, `binding` green — ACP-CM colors users learn to trust).
- Six-stop timeline (mapping table above); failure states red; `RECONCILING`
  = explained wait, never a retry affordance. Event log expander shows raw
  vcad+kerf states.
- `pending_human` ⇒ warning banner: cap, mandate kind, TTL countdown +
  **"Approve in live view"** (`openLink` → kerf live view at L1, mandate page
  at L2) + **Decline**. The widget never approves — buttons leave the iframe.
- Receipt chip (design half: `holds 9/9` green / stale / violated /
  unverified) **separate from** evidence chip (commerce half: item count +
  `confirmation oracles n/2`). Both link to the receipt view.
- Footer: wallet balance + poll note.
- Poll cadence: slow (10s); fast while any order is transitional
  (`approval`, `placing`).

### 2c. Mount + security invariants

- `quote_manufacturing`: `behavior({ mount: true })` — money entering the
  story is a milestone.
- **The iframe is read-only for money.** `get_order_feed` is the only new
  app-callable and it reads. No ordering tool is `widgetAccessible` — assert
  in `viewer-meta.test.ts`. The asymmetric seam (agent proposes, human
  approves out-of-band, agent places) is now double-enforced: vcad's wallet
  side and kerf's card side.

Effort: ~3–4 days after M1 (shared panel/poll plumbing) + kerf driver work.

---

## M3 — Elicitation approval (URL mode)

**Goal:** protocol-native approval that carries the human to the money moment.

- Gate: `VCAD_FABRICATE_ELICIT=1` **and** client advertises elicitation
  (check `server.getClientCapabilities()` at call time).
- **Two lanes by kerf autonomy rung, identical elicitation call, different URL:**
  - **L1 (SendCutSend first):** URL = the **kerf live view**. kerf has staged
    the cart, every assertion green (`STAGED`); the human's click on the
    vendor's own buy button *is* the approval. vcad flips the authz
    `authorized`→`consumed` when kerf reports `CONFIRMING`.
  - **L2:** URL = `vcad.io/authorize/<authorization_id>`. Approval mints the
    mandate that funds kerf's single-use virtual card (amount-capped,
    merchant-locked, short-expiry); kerf's auditor gates the click.
- `decline` → revoke authz → kerf job `CANCELED` pre-`PLACING`. kerf's
  at-most-once `PLACING` invariant means a declined mandate can never race a
  buy click.
- **URL mode is mandatory** (spec rule: form mode never carries
  credentials/sensitive approvals). Fallback: capability absent or flag off ⇒
  the M2 dock button covers the same journey — the dock is the floor,
  elicitation is the accelerator.
- Prereq: deep-linkable `/authorize/<id>` route in `packages/app` (L2 lane).

Effort: ~2 days server-side + the app route + kerf Wave 1 for the L1 lane.

---

## M4 — One receipt, two halves (receipt-gated ordering)

kerf's `EvidenceBundle` is *designed* as "the per-job bundle handed back to
the design surface's receipt" — same fail-closed verdict vocabulary. M4 is a
merge, not an invention:

- **Commerce claims:** map kerf `OracleClaim`s 1:1 into `DesignReceipt.claims`
  — `kerf/upload-hash` (exact quoted bytes were uploaded; closes the loop
  with `FabArtifactRef` sha256s), price-match (charged = quoted), the two
  confirmation oracles, delivery. Namespace: `commerce.*` alongside `mech.*`.
- **Gate in `place_order`:** (1) design claims re-verified at place time —
  `Stale`/`Violated` ⇒ refuse with failing claims (the `export_gerber`
  dirty-DRC precedent, applied to money); (2) `intent_hash` must match the
  quote — geometry edited after quoting ⇒ refuse with "re-quote" (never
  silent re-pricing). No receipt at all ⇒ proceed but flag **unverified**
  in the feed (fail-closed only when a receipt exists and fails).
- **Persistence:** orders gain `receipt_fingerprint`, `receipt_status`,
  `intent_hash` (+ fold in the `fab_artifact` column gap, `store.ts:242`).
  Supabase migration.
- **Widget:** the two chip families from M2 converge post-delivery into one
  receipt: *"geometry in spec, these exact bytes manufactured, this price
  paid, two independent oracles saw the order, delivered."* The screenshot.
- Later (standards play): propose `io.vcad/receipt` as an MCP extension via
  the extensions framework — vcad + kerf as the reference implementation for
  verified agentic fabrication.

Effort: ~2–3 days + migration review (+ kerf Wave 2 for card-settlement
oracle).

---

## M5 — Tasks extension (deferred)

kerf jobs are eve durable workflows that sleep through production lead times —
exactly the shape `io.modelcontextprotocol/tasks` wants. When the SDK ships it
(2026-07-28 spec), `place_order` returns a task handle backed by the kerf job.
Do not build against the RC; the M2 feed delivers the UX today.

---

## Rollout & flags

| flag | gates |
|---|---|
| (none) | M0 tripwire, M1 play button, M2 dock read-only + Wave-0 `quoted` basis |
| `VCAD_FABRICATE_ORDERING=1` | authz/place (existing) |
| `VCAD_KERF_RAIL=1` | kerf driver in FulfillmentBroker (per-wave) |
| `VCAD_FABRICATE_ELICIT=1` | M3 elicitation |
| `VCAD_FABRICATE_RECEIPT_GATE=1` | M4 gate (flip default after burn-in) |

Sequencing: M0 → M1 → M2 (+ kerf Wave 0 in parallel) → M3 ∥ M4 → M5.
M1 first — no money-path risk, shares viewer plumbing with M2.

## Test plan

- `viewer-meta.test.ts`: new app-only tools template-less + app-visibility;
  no ordering tool widget-accessible; `create_robot_env`/`quote_manufacturing`
  carry the template.
- `tool-surface.fixture.json`: one deliberate regen per milestone.
- Golden trajectory: fixed-seed (`setSeed`) 2-joint arm, scripted torques →
  `get_sim_replay` matches committed fixture.
- FK parity: viewer FK vs `record_simulation` kernel-FK for the same
  trajectory (EE-pose numeric check against `end_effector_poses`).
- Ordering: receipt-gate verdict matrix (Holds passes; Stale/Violated refuse;
  absent flags unverified); intent-hash mismatch refuses; elicitation
  capability-fallback; state-mapping table exhaustively covers all 17 kerf ×
  16 vcad states (no unmapped state reaches the widget).
- `scripts/test-local-mcp-apps.mjs`: mount `create_robot_env`, read replay
  tools, read `get_order_feed`.

## Open questions

1. Does `buildGlb` already emit named per-instance nodes? (Spike before 1c.)
2. kerf transport into vcad: consume kerf's remote MCP from the server, or a
   thin REST/webhook pair? (Webhooks → session event spine feels right for
   the feed; MCP for submit/quote.)
3. Where does the L2 `/authorize/<id>` route live in `packages/app`, and what
   does it need from the session event spine?
4. Supabase migration numbering/ownership for `orders.receipt_fingerprint` +
   `intent_hash` + `fab_artifact` (`store.ts:242` note).
5. ChatGPT shim: confirm `openLink` → `openai.openExternal` is allowed for
   vcad.io/kerf live-view URLs from the skybridge sandbox.
6. kerf is pre-alpha — pin the integration to its contract layer
   (`@kerf/core` types) and gate each wave behind `VCAD_KERF_RAIL`; who owns
   the cross-repo contract test?
