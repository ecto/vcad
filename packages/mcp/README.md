# @vcad/mcp

MCP (Model Context Protocol) server for vcad CAD operations. Enables AI assistants like Claude Code, Cursor, and others to create, export, and inspect 3D geometry.

## Installation

```bash
npm install -g @vcad/mcp
```

Or add to your MCP configuration directly:

```bash
claude mcp add vcad --command "npx @vcad/mcp"
```

## Tool packs

The surface is a small always-on core — the make → see → measure → verify →
ship loop (session lifecycle, Loon/CRUD authoring, parts library,
`inspect_cad`, `render_view`, `export_cad`, `import_step`,
`open_in_browser`, `get_changelog`) — plus opt-out domain packs:

| Pack | Tools |
|------|-------|
| `dfm` | `dfm_check`, `dfm_explain`, `dfm_suggest_fix`, `dfm_apply_fix` |
| `sheet_metal` | `sheet_metal_create/unfold/check/materials/bend_table/cost/suggest_fix/sequence/nest` |
| `physics` | `create_robot_env`, `gym_*`, `batch_*` |
| `ecad` | `create_schematic`, `place_components`, `route_nets`, `run_drc`, `run_erc`, `export_gerber`, `calc_impedance` |
| `eval` | `verify_part`, `list_eval_tasks` (mecheval self-grading oracle) |

Set `VCAD_MCP_PACKS` to a comma-separated list of packs to enable
(e.g. `VCAD_MCP_PACKS=sheet_metal,dfm`), or `none` for core only.
Unset, every pack is enabled. A smaller surface costs fewer schema
tokens per request and measurably improves tool-selection accuracy for
focused workflows.

### Switching packs at runtime

`VCAD_MCP_PACKS` is only the boot-time default — an agent can also flip
packs mid-session with two always-on meta-tools:

- **`list_tool_packs`** — the packs, whether each is currently enabled,
  and its tool count.
- **`set_tool_packs`** — enable/disable packs by name. Pass `enable`
  and/or `disable` arrays, or `set` to replace the enabled set outright
  (an array of names, or `"all"` / `"none"`).

On a **persistent transport (stdio)** the change is live: the next
`tools/list` reflects it and the server emits
`notifications/tools/list_changed` so the client refetches. On the
**stateless HTTP transport** (fresh server per request) there's no push
channel — instead, a signed-in user's choice is persisted (keyed by user
in the `mcp_tool_packs` table) and applied on the next request; anonymous
HTTP callers fall back to `VCAD_MCP_PACKS`. Calling a tool whose pack is
disabled returns an actionable error pointing at `set_tool_packs`.

## Discord activity rollups

The server can post a periodic activity summary to a Discord channel —
handy for seeing that a deployed server is being used without a message
per call. Every interval the notifier posts a rollup like:

```
📊 vcad activity · last 15m
23 tool calls across 4 sessions · 1 error
`create` ×9 · `update` ×6 · `export_cad` ×4 · `inspect_cad` ×3 · `render_view` ×1
```

It's disabled until you set a webhook, fire-and-forget (never blocks or
fails a tool call), and stays quiet when idle (no "0 calls" pings). Only
tool names, call counts, error counts, and the number of distinct sessions
are sent — never argument values or document content.

Configure it in `notifyConfig` at the top of [`src/notify.ts`](src/notify.ts) —
paste your internal Discord webhook URL into `webhookUrl` (empty disables
it). `rollupMs` (default 15 min) and `username` live there too. The repo is
public, so a URL committed there lands in git history; treat it as a
low-value secret (post-only, one channel) and rotate it from Discord if
needed.

## Agent feedback loop

- **Server instructions** — the kernel's type catalog, material preset
  keys, Z-up orientation rules, and transform semantics are published as
  MCP server `instructions` (extracted from the kernel registry at boot,
  so they never drift). Hosts surface them to the agent automatically.
- **Mutation diffs** — `create`/`update`/`delete`/`set_material` results
  carry a compact `changed: {added, removed, modified}` part diff, so
  agents see what a mutation actually did without a follow-up `read`.
- **Inline viewer** — geometry tools render in an embedded vcad viewport
  (MCP Apps hosts); `sheet_metal_unfold` draws its flat pattern as a 2D
  drawing with cut and bend-line layers.
- **Pointing** — click a part in the viewer to select it (brand-pink
  highlight + a chip with name, id, and bbox). Selection is pushed
  silently via `ui/update-model-context`, so typing "make this 5 mm
  taller" in the chat resolves to the selected part; the chip's **Ask**
  button sends a part-grounded `ui/message` into the conversation. Both
  are capability-gated via `getHostCapabilities()` and degrade to a
  local inspector on hosts without them. GLB nodes carry
  `"<part_id>:<name>"` names (`buildPartLabels`) to make the mapping.

## Tools

Most tools operate on an **editing session**: `open_document` returns a
`document_id` that mutation and inspection tools take as input. The typical
loop is open → author (`create_cad_loon`, `create`, `place_part`) → look and
measure (`render_view`, `inspect_cad`) → fix → `get_document` →
`export_cad` / `open_in_browser`.

### Session lifecycle

- `open_document` — open an editing session; pass an `initial` IR document
  to edit existing geometry or omit it for an empty one. Returns the
  `document_id` used by every session-aware tool.
- `get_document` — dump the session's full IR Document JSON, e.g. to feed
  `export_cad` or `open_in_browser`.
- `close_document` — close a session and free its memory (idempotent).

### Document editing (kernel CRUD)

`create`, `read`, `update`, `delete`, `set_material` — the kernel's
registry-driven editing tools, exposed with the exact schemas the in-app
chat uses (they come from the kernel WASM at boot, so they never drift).
Each takes a `document_id`. `create` adds a feature by `type` + `params`;
`read` lists parts or returns one part's feature tree; `update` patches
node parameters; `delete` removes a part; `set_material` assigns a preset
material. Use these for surgical edits, `create_cad_loon` for whole parts.

### Loon authoring

- `create_cad_loon` — create a document in one shot from Loon source, a
  Lisp-like parametric CAD DSL. This is the *full* modeling vocabulary —
  primitives, booleans, transforms, fillet/chamfer/shell, linear/circular
  patterns, sketches with extrude/revolve/sweep/loft, and assemblies with
  joints — even where no dedicated MCP tool exists. Returns the document
  (compact VCode or JSON IR) plus a `document_id` for follow-up edits.

### Parts library

- `search_parts` — search the stdlib parts library (fasteners, bearings, …)
  by name, category, synonym, or catalog number (McMaster / ISO / DIN).
- `place_part` — insert a result into a session document by its `path`,
  with optional `params` overrides. Placed parts stay parametric.

### Geometry I/O

- `export_cad` — write an IR document (`ir` + `filename`) to `.stl`
  (3D printing), `.glb` (visualization), or — for sheet-metal documents —
  `.step`/`.stp` of the folded body with true cylindrical bend faces.
- `import_step` — import a `.step`/`.stp` file (AP203/AP214, as exported by
  Fusion 360, SolidWorks, Onshape, …) into an IR document with
  ImportedMesh nodes.
- `open_in_browser` — compress a document into a shareable vcad.io URL
  (very large documents may exceed the ~2KB URL limit).

### Inspect & verify

- `inspect_cad` — aggregate geometry properties for a session document:
  volume, surface area, bounding box, center of mass, triangle count, and
  mass when material density is known.
- `render_view` — render the session document to an isometric PNG
  (drafting-style line art, Z-up) so the agent can *see* the geometry,
  not just numbers.
- `verify_part` / `list_eval_tasks` — grade a session document against a
  mecheval benchmark task with the official deterministic graders, and
  browse the available task ids. The benchmark harness excludes these
  during scored runs.

### DFM (Design for Manufacturing)

- `dfm_check` — run process-specific manufacturability checks
  (`cnc_3axis`, `fdm`, `sla`, `injection`, `sheet_metal`, `casting_sand`,
  `casting_investment`) against a session document; thresholds come from
  TOML rule packs at `lib/dfm/<process>.toml`.
- `dfm_explain` / `dfm_suggest_fix` / `dfm_apply_fix` — long-form rationale
  for an issue, a suggested patch, and (for `set_param` patches) applying
  it to the document. Re-run `dfm_check` to confirm the issue cleared.

### Sheet metal

- `sheet_metal_create` — build a part from a base flange plus a chain of
  edge flanges, hems, and jogs; supports `shop_profile` catalogs (e.g.
  `"sendcutsend"`) and bend relief. Returns a `document_id` plus flat-bbox
  and DFM summary.
- `sheet_metal_unfold` — flat pattern (outlines, holes, creases) plus a
  fab-ready single-silhouette DXF with bend centerlines.
- `sheet_metal_check` / `sheet_metal_suggest_fix` — manufacturability
  violations against a shop profile, and the concrete parameter changes
  that resolve them (the create → check → fix → re-check loop).
- `sheet_metal_materials` / `sheet_metal_bend_table` — the built-in
  material registry and the K-factor bend table (or a fab catalog via
  `shop_profile`).
- `sheet_metal_cost` — line-itemed cost estimate (material, cut, pierces,
  bends, setup, markup).
- `sheet_metal_sequence` — a feasible press-brake bend order with
  springback-compensated angles.
- `sheet_metal_nest` — pack multiple parts onto stock sheets and report
  placements and utilization.

### Physics simulation

- `create_robot_env` — build a phyz physics environment from a vcad
  assembly; returns an environment id.
- `gym_step` / `gym_reset` / `gym_observe` / `gym_close` — gym-style RL
  interface: step with `torque`, `position`, or `velocity` actions, read
  joint positions/velocities and end-effector poses, reset, clean up.
- `batch_create_envs` / `batch_step` / `batch_reset` — the same loop across
  N parallel environments for RL training.

### ECAD

- `create_schematic` — schematic from component and wire definitions.
- `place_components` — board outline, stackup, and footprint placement
  from schematic data.
- `route_nets` — copper traces connecting pads on the same net.
- `run_drc` / `run_erc` — design and electrical rule checks.
- `export_gerber` — Gerber RS-274X fabrication files, drill file,
  pick-and-place CSV, and BOM.
- `calc_impedance` — IPC-2141 trace impedance (microstrip, stripline,
  differential pairs).

### Other

- `get_changelog` — query the vcad changelog by version, category,
  feature, or MCP tool.
- `get_preview_glb` — app-only (`visibility: ["app"]`): fetches the GLB
  payload for the inline 3D viewer. Hidden from agents on spec-compliant
  hosts; use `export_cad` for geometry exports.

## Hosted server (mcp.vcad.io)

The same server runs as a hosted Streamable-HTTP endpoint at
`https://mcp.vcad.io/mcp`, deployed on **Vercel** from this repo:

- `vercel.json` (repo root) builds vcad.io itself to `packages/app/dist`.
- [`services/mcp/`](../../services/mcp) is a separate Vercel project that
  esbuild-bundles [`entry.ts`](../../services/mcp/entry.ts) into a Build
  Output API serverless function (`/mcp`, `/health`, `/oauth/*`,
  `/.well-known/oauth-*`). `build.sh` ships the kernel WASM next to the
  bundle and bakes in the package version.

Two behaviors differ from a local stdio server, both because the function
filesystem is read-only (`/var/task`) and invocations are isolated:

- **Inline payloads.** `entry.ts` sets `VCAD_MCP_REMOTE=1`, so
  `export_cad` returns base64 file contents (capped by
  `MCP_MAX_INLINE_EXPORT_BYTES`, default 4 MiB) instead of writing to
  disk; `import_step` takes `content_base64` instead of a path; and
  `export_gerber` returns file contents inline. A `writeFileSync` on the
  serverless FS would throw `EROFS` and be invisible to the caller
  anyway.
- **OAuth 2.1 + DCR.** Sign-in (Google/GitHub via Supabase) is handled by
  [`oauth.ts`](src/oauth.ts) and wired in `entry.ts`; set
  `MCP_OAUTH_SECRET` to enable it and `MCP_REQUIRE_AUTH=1` to require a
  token on `/mcp`. See the
  [overview docs](https://docs.vcad.io/reference/mcp/overview).

Add it as a connector by URL: `https://mcp.vcad.io/mcp`.

## ChatGPT (OpenAI Apps SDK)

The hosted server doubles as a ChatGPT app: geometry tools carry
`_meta["openai/outputTemplate"]` pointing at a second registration of the
3D viewer (`text/html+skybridge`), and the viewer bundle detects
ChatGPT's `window.openai` bridge at runtime
([viewer-app/openai-shim.ts](viewer-app/openai-shim.ts)) — same HTML,
both hosts. `get_preview_glb` and `get_document` are marked
`openai/widgetAccessible` so the widget can fetch geometry and build
"Open in vcad.io" deep links; the Ask button maps to
`sendFollowUpMessage`. Part selection degrades to the local inspector
(ChatGPT has no `ui/update-model-context` equivalent).

To connect: ChatGPT → Settings → Apps & Connectors → developer mode →
add `https://mcp.vcad.io/mcp`.

## Codex CLI / IDE

```bash
# hosted (Streamable HTTP; OAuth via `codex mcp login vcad` when enabled)
codex mcp add vcad --url https://mcp.vcad.io/mcp

# or local stdio
codex mcp add vcad -- npx -y @vcad/mcp
```

Both forms land in `~/.codex/config.toml` and are shared by the Codex
IDE extension.

## Example Usage

In Claude Code or another MCP-compatible assistant:

> "Create a 50x30x5mm plate with four 3mm mounting holes at the corners, spaced 5mm from each edge. Export to mounting_plate.stl"

The assistant will:
1. Use `create_cad_loon` to build the geometry (or `open_document` +
   `create` for incremental edits)
2. Use `render_view` and `inspect_cad` to verify shape and dimensions
3. Use `get_document` + `export_cad` to write the STL file

## Development

```bash
# Build
npm run build -w @vcad/mcp

# Test
npm test -w @vcad/mcp

# Run locally
node packages/mcp/dist/index.js
```
