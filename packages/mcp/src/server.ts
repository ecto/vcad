/**
 * MCP server implementation with vcad tools.
 *
 * server.ts is an ASSEMBLER: every tool is a `ToolDef` exported by its module
 * (tools/*.ts). This file collects them into the ListTools surface and a
 * name→def dispatch Map, derives all cross-cutting behavior (viewer `_meta`,
 * pack gating, doc-writer persist, geometry preview) from each def's `behavior`
 * flags + `pack`, and runs the shared middleware pipeline around every call.
 */

import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { Engine, getKernelWasm, resetKernelWasm } from "@vcad/engine";
import { commandRegistry } from "@vcad/core";
import type { Document } from "@vcad/ir";
import {
  documents,
  registerSession,
  getSession,
  hydrateSession,
  persistSession,
  dropSession,
  runInSessionScope,
  recordHistorySnapshot,
} from "./tools/session.js";
import { createCadLoon } from "./tools/loon.js";
import { previewVersion } from "./tools/preview.js";
import {
  createSessionStore,
  createSessionEventStore,
  createShareStore,
  createPackStore,
  sessionStoreInfo,
  warnIfSessionStoreNotDurable,
} from "./session-store.js";
// Re-exported so the Vercel entry (services/mcp/entry.ts) and standalone
// /health (http.ts) report the same durability state as server_info.
export { sessionStoreInfo } from "./session-store.js";
import { createFabricateStore } from "./fabricate/store.js";
import type { AuthUser } from "./oauth.js";

/** Per-connection context threaded from the transport entry point — the
 *  authenticated user when the request carried a valid Bearer access token,
 *  otherwise null (stdio, anonymous HTTP, or OAuth disabled). */
export interface ServerContext {
  user: AuthUser | null;
}
import {
  registryToolDescriptors,
  registryDispatchableNames,
  dispatchRegistryTool,
} from "./tools/registry-dispatch.js";
import {
  buildErrorResult,
  enrichErrorResult,
  enrichSuccessResult,
} from "./tools/next-actions.js";
// Runtime config read from Vercel Edge Config so warm instances reflect a flag
// flip or a fresh deploy WITHOUT a redeploy (see edge-config.ts).
import { getStaleness } from "./edge-config.js";
// Re-exported so the Vercel entry point (services/mcp/entry.ts) serves the live
// window and the artifact channel through the same handlers as the standalone
// server (http.ts).
export { handleLiveRequest } from "./live-route.js";
export { handleArtifactRequest } from "./artifact-route.js";
import {
  getViewerHtml,
  VIEWER_RESOURCE_URI,
  VIEWER_CSP,
  MCP_APP_MIME_TYPE,
  OPENAI_VIEWER_RESOURCE_URI,
  OPENAI_APP_MIME_TYPE,
  OPENAI_WIDGET_CSP,
} from "./viewer.js";
import { fireToolAlert } from "./notify.js";
import { configureTelemetry, flushTelemetry } from "./telemetry.js";

// ── ToolDef registry: one record per tool, contributed by its module ────────
import {
  behavior,
  type ToolDef,
  type ToolBehavior,
  type ToolContext,
} from "./tools/tool-def.js";
import type { ToolResult } from "./tools/tool-result.js";
import { buildKernelEventPayload } from "./tools/kernel-event.js";

import { toolDefs as sessionToolDefs } from "./tools/session.js";
import { toolDefs as checkpointToolDefs } from "./tools/checkpoint.js";
import { toolDefs as continueDocToolDefs } from "./tools/continue-doc.js";
import { toolDefs as orderToolDefs } from "./tools/order.js";
import { toolDefs as orderingToolDefs } from "./tools/ordering.js";
import { toolDefs as bomToolDefs } from "./tools/bom.js";
import { toolDefs as mechPartsToolDefs } from "./tools/mech-parts.js";
import { toolDefs as liveShareToolDefs } from "./tools/live-share.js";
import { toolDefs as partsToolDefs } from "./tools/parts.js";
import { toolDefs as previewToolDefs } from "./tools/preview.js";
import { toolDefs as loonToolDefs } from "./tools/loon.js";
import { toolDefs as exportToolDefs } from "./tools/export.js";
import { toolDefs as inspectToolDefs } from "./tools/inspect.js";
import { toolDefs as printCheckToolDefs } from "./tools/print-check.js";
import { toolDefs as renderToolDefs } from "./tools/render.js";
import { toolDefs as verifyToolDefs } from "./tools/verify.js";
import { toolDefs as verifySpecToolDefs } from "./tools/verify-spec.js";
import { toolDefs as clearanceToolDefs } from "./tools/clearance.js";
import { toolDefs as dfmToolDefs } from "./tools/dfm.js";
import { toolDefs as sheetMetalToolDefs } from "./tools/sheet-metal.js";
import { toolDefs as importToolDefs } from "./tools/import.js";
import { toolDefs as importPcbToolDefs } from "./tools/import-pcb.js";
import { toolDefs as shareToolDefs } from "./tools/share.js";
import { toolDefs as gymToolDefs } from "./tools/gym.js";
import { toolDefs as atomsToolDefs } from "./tools/atoms.js";
import { toolDefs as recordToolDefs } from "./tools/record.js";
import { toolDefs as changelogToolDefs } from "./tools/changelog.js";
import { toolDefs as ecadToolDefs } from "./tools/ecad.js";
import { toolDefs as enclosureToolDefs } from "./tools/enclosure.js";

// Re-exported so the Vercel transport entry can drain in-flight PostHog
// captures before a serverless instance freezes (see services/mcp/entry.ts).
export { flushTelemetry };

/** Build-time injected version. esbuild's `--define:__VCAD_VERSION__` (see
 *  services/mcp/build.sh) replaces this with the package.json version literal
 *  when bundling the hosted server, where the flattened layout breaks the
 *  source-relative `../package.json` require below. Undefined in the normal
 *  tsc `dist/` build — `typeof` guards against the ReferenceError. */
declare const __VCAD_VERSION__: string | undefined;

/** Build-time injected commit + timestamp. esbuild's `--define:__VCAD_BUILD_SHA__`
 *  / `__VCAD_BUILD_TIME__` (see services/mcp/build.sh) replace these with the
 *  string literals from VERCEL_GIT_COMMIT_SHA at bundle time. Undefined in the
 *  normal tsc `dist/` build — `typeof` guards against the ReferenceError. */
declare const __VCAD_BUILD_SHA__: string | undefined;
declare const __VCAD_BUILD_TIME__: string | undefined;

/** Server version + build identity, read from package.json at load time so the
 *  advertised version always matches the running build (no hardcoded literal to
 *  drift). */
const PKG_VERSION: string = (() => {
  // Bundled hosted build: the version is baked in via esbuild define, since the
  // require path below resolves to a nonexistent sibling of the single bundle.
  if (typeof __VCAD_VERSION__ === "string" && __VCAD_VERSION__) {
    return __VCAD_VERSION__;
  }
  try {
    const req = createRequire(import.meta.url);
    return (req("../package.json") as { version?: string }).version ?? "0.0.0";
  } catch {
    return "0.0.0";
  }
})();

/** Exact commit, baked at build time. The runtime env fallbacks
 *  (VERCEL_GIT_COMMIT_SHA, then the legacy VCAD_BUILD_SHA) only fire for
 *  non-bundled runs; on the hosted serverless build the define wins. "unknown"
 *  means neither was available — itself a useful signal. */
const BUILD_SHA: string =
  (typeof __VCAD_BUILD_SHA__ === "string" && __VCAD_BUILD_SHA__) ||
  process.env.VERCEL_GIT_COMMIT_SHA ||
  process.env.VCAD_BUILD_SHA ||
  "unknown";
const BUILD_TIME: string =
  (typeof __VCAD_BUILD_TIME__ === "string" && __VCAD_BUILD_TIME__) || "unknown";
const SHORT_SHA: string = BUILD_SHA === "unknown" ? "unknown" : BUILD_SHA.slice(0, 7);

/** Version advertised in the MCP `initialize` handshake. Semver build-metadata
 *  (`0.9.4+1a2b3c4`) is the protocol-native place to surface which commit a
 *  client is connected to — every MCP client sees it at connect time without
 *  calling a tool. Build metadata is ignored in semver precedence, so this stays
 *  a valid version string. */
const VERSION_WITH_BUILD: string =
  SHORT_SHA === "unknown" ? PKG_VERSION : `${PKG_VERSION}+${SHORT_SHA}`;

/** Per-process identity, fresh at every cold start. On serverless this is the
 *  difference between "the deployment is wrong" and "I'm pinned to one stale
 *  instance": two calls reporting different instance_id / build_sha means old
 *  instances are still draining behind a correct deployment. */
const INSTANCE_ID: string = randomUUID().slice(0, 8);
const PROCESS_STARTED_AT: number = Date.now();

/** Single source of truth for build/runtime identity, shared by the
 *  `server_info` tool, the `initialize` handshake, and the /health endpoint. */
export function getBuildInfo(): {
  name: string;
  version: string;
  version_full: string;
  build_sha: string;
  build_time: string;
  instance_id: string;
  uptime_s: number;
} {
  return {
    name: "vcad",
    version: PKG_VERSION,
    version_full: VERSION_WITH_BUILD,
    build_sha: BUILD_SHA,
    build_time: BUILD_TIME,
    instance_id: INSTANCE_ID,
    uptime_s: Math.round((Date.now() - PROCESS_STARTED_AT) / 1000),
  };
}

// Stamp every telemetry event with the running build identity (version, commit,
// instance) — set once at module load.
configureTelemetry(getBuildInfo());

// Boot-time durability self-check (fires once per cold start, on every entry
// point that imports this module — Vercel function, standalone HTTP, stdio). A
// production deploy without a durable session store keeps sessions in-memory
// only, so a redeploy drops every open board. Make that loud at boot; the same
// state is observable over the wire via server_info / /health (durable:false).
// No-op on stdio/local or when durable.
warnIfSessionStoreNotDurable();

/** Kernel WASM exports this server depends on; checked at startup so a stale or
 *  incomplete dist surfaces as a clear boot error rather than an opaque
 *  mid-call TypeError. Surfaced via `server_info`. */
const REQUIRED_WASM_EXPORTS = ["render_svg", "render_svg_view", "render_pcb_svg"];

const serverInfoSchema = {
  type: "object" as const,
  properties: {},
};

/** UI metadata for geometry tools — both dialects, hosts read what they
 *  understand: MCP Apps (`ui`/`ui/resourceUri`) for Claude/Cursor, and
 *  OpenAI Apps SDK (`openai/outputTemplate`) for ChatGPT. */
const UI_META = {
  ui: {
    resourceUri: VIEWER_RESOURCE_URI,
  },
  // Flat key format also required by Claude Desktop MCP Apps protocol
  "ui/resourceUri": VIEWER_RESOURCE_URI,
  // ChatGPT Apps SDK: render results through the skybridge viewer.
  "openai/outputTemplate": OPENAI_VIEWER_RESOURCE_URI,
  "openai/toolInvocation/invoking": "Modeling geometry…",
  "openai/toolInvocation/invoked": "Model updated",
};

/** Extra meta for tools the viewer iframe itself calls (GLB fetch, IR
 *  fetch for the deep link). ChatGPT blocks widget-initiated tool calls
 *  unless the tool is explicitly marked widget-accessible. */
const WIDGET_CALLABLE_META = {
  "openai/widgetAccessible": true,
};

/**
 * Every static (non-registry) tool, contributed by its module as a `ToolDef`.
 * Module order is irrelevant here — the ListTools ORDER is `LIST_TOOL_ORDER`
 * below, and dispatch is by name — so this is just the pool the assembler and
 * the derived sets draw from. (server_info is minted per-connection inside
 * createServer because it reports connection/build state; the registry-tier
 * kernel tools are generated per-connection from the WASM registry.)
 */
const STATIC_TOOL_DEFS: readonly ToolDef[] = [
  ...sessionToolDefs,
  ...checkpointToolDefs,
  ...continueDocToolDefs,
  ...orderToolDefs,
  ...orderingToolDefs,
  ...bomToolDefs,
  ...mechPartsToolDefs,
  ...liveShareToolDefs,
  ...partsToolDefs,
  ...previewToolDefs,
  ...loonToolDefs,
  ...exportToolDefs,
  ...inspectToolDefs,
  ...printCheckToolDefs,
  ...renderToolDefs,
  ...verifyToolDefs,
  ...verifySpecToolDefs,
  ...clearanceToolDefs,
  ...dfmToolDefs,
  ...sheetMetalToolDefs,
  ...importToolDefs,
  ...importPcbToolDefs,
  ...shareToolDefs,
  ...gymToolDefs,
  ...atomsToolDefs,
  ...recordToolDefs,
  ...changelogToolDefs,
  ...ecadToolDefs,
  ...enclosureToolDefs,
];

/**
 * Advertised ORDER of the static tools in ListTools. The registry-tier kernel
 * tools (create/read/update/delete/…) are spliced in right after `place_part`
 * (see `assembleToolList`); every other tool appears here exactly once. Order
 * is presentation only — dispatch and behavior key off the def, not this list.
 * A boot-time check asserts this covers `STATIC_TOOL_DEFS` + `server_info`.
 */
const LIST_TOOL_ORDER: readonly string[] = [
  // ── Session lifecycle ──────────────────────────────────────
  "open_document",
  "get_document",
  "close_document",
  "save_document",
  "load_document",
  "checkpoint_document",
  "branch_from",
  "continue_document",
  "server_info",
  "list_tool_packs",
  "set_tool_packs",
  // ── vcad Fabricate ─────────────────────────────────────────
  "quote_manufacturing",
  "get_order_status",
  "list_orders",
  "authorize_spend",
  "place_order",
  // ── Project BOM ────────────────────────────────────────────
  "bom_create",
  "bom_add_line",
  "bom_export",
  "search_mechanical_parts",
  // ── Live review window ─────────────────────────────────────
  "share_session",
  "unshare_session",
  // ── Stdlib parts library ───────────────────────────────────
  "search_parts",
  "place_part",
  // (registry-driven kernel tools are spliced in here)
  // ── MCP Apps: app-only preview fetch + version poll ────────
  "get_preview_glb",
  "get_preview_version",
  // ── Loon DSL one-shot + core see/measure/export ────────────
  "create_cad_loon",
  "export_cad",
  "inspect_cad",
  // ── Print-then-measure calibration loop (3DP) ──────────────
  "predict_print",
  "record_measurement",
  // ── Verify-and-iterate loop ────────────────────────────────
  "render_view",
  "verify_part",
  "list_eval_tasks",
  "verify_spec",
  // ── DFM ────────────────────────────────────────────────────
  "dfm_check",
  "dfm_explain",
  "dfm_suggest_fix",
  "dfm_apply_fix",
  // ── Sheet metal ────────────────────────────────────────────
  "sheet_metal_create",
  "sheet_metal_unfold",
  "sheet_metal_check",
  "sheet_metal_materials",
  "sheet_metal_bend_table",
  "sheet_metal_cost",
  "sheet_metal_suggest_fix",
  "sheet_metal_sequence",
  "sheet_metal_nest",
  // ── Import + share ─────────────────────────────────────────
  "import_step",
  "import_kicad",
  "import_eagle",
  "open_in_browser",
  // ── Physics gym ────────────────────────────────────────────
  "create_robot_env",
  "gym_step",
  "gym_reset",
  "gym_observe",
  "gym_close",
  // ── Atoms ──────────────────────────────────────────────────
  "load_structure",
  "inspect_molecule",
  "minimize_energy",
  "md_run",
  "design_material",
  "homogenize_material",
  "render_molecule",
  "record_simulation",
  "batch_create_envs",
  "batch_step",
  "batch_reset",
  "get_changelog",
  // ── ECAD (PCB) ─────────────────────────────────────────────
  "create_schematic",
  "place_components",
  "route_nets",
  "add_coil",
  "add_coil_array",
  "winding_layout",
  "board_from_solid",
  "solid_from_board",
  "check_enclosure_fit",
  "check_clearance",
  "list_footprints",
  "search_footprints",
  "get_pad_positions",
  "get_footprint",
  "describe_pcb",
  "add_trace",
  "add_via",
  "set_stackup",
  "set_placement",
  "set_board_outline",
  "add_zone",
  "delete_zone",
  "delete_trace",
  "delete_via",
  "get_copper",
  "add_net_tie",
  "delete_net_tie",
  "undo",
  "set_design_rules",
  "size_trace_for_current",
  "add_via_array",
  "add_motor_winding",
  "calc_motor",
  "check_self_start",
  "render_pcb",
  "render_ratsnest",
  "render_stackup",
  "run_drc",
  "search_electronic_parts",
  "resolve_part",
  "find_alternatives",
  "verify_substitution",
  "build_receipt",
  "verify_receipt",
  "route_diff_pair",
  "critique_route",
  "run_erc",
  "export_gerber",
  "export_kicad",
  "validate_for_fab",
  "calc_impedance",
  "size_impedance",
  "size_pdn",
  "calc_coil",
  "size_coil",
  "calc_rf",
];

/** Tools whose text body is a machine-parseable document that consumers
 *  JSON.parse verbatim — appending a handle block would corrupt it with
 *  trailing characters (this broke the mecheval harness's .vcad extraction).
 *  Derived from `behavior.pureJson` so the flag stays the single source. */
const PURE_JSON_RESULT_TOOLS = new Set(
  STATIC_TOOL_DEFS.filter((d) => d.behavior.pureJson).map((d) => d.name),
);

/**
 * Kernel system-prompt sections forwarded to MCP agents via the
 * protocol-native server `instructions` field. The in-app chat surface
 * gets this knowledge through its system prompt; MCP agents otherwise
 * never see it (type catalog, material keys, Z-up orientation gotchas,
 * world-origin rotation semantics). Extracted by header at boot so the
 * catalog never drifts from the kernel registry; a renamed or missing
 * section is silently skipped.
 */
const INSTRUCTION_SECTIONS = [
  "Available Materials",
  "Orientation Notes",
  "How translate/rotate/scale work (IMPORTANT)",
  "Sketch rules — READ CAREFULLY",
  "Type Catalog",
];

/** Build MCP server instructions: a short workflow framing plus the
 *  durable knowledge sections of the kernel chat system prompt. */
function buildInstructions(kernelPrompt: string | null): string {
  const header = [
    "vcad — parametric CAD with a real BRep kernel. Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.",
    "",
    "Workflow: `open_document` → author → see → measure → ship.",
    "- Author whole parts with `create_cad_loon` (the full modeling vocabulary in one call); make surgical edits with `create`/`update`/`delete` (one node per call — mutation results report a compact `changed` diff of affected parts).",
    "- See your work with `render_view` (isometric PNG); measure with `inspect_cad` (volume, area, bbox, center of mass).",
    "- Ship with `export_cad` (STL/GLB/STEP) or `open_in_browser` (vcad.io deep link).",
    "- Fix in place: when geometry is wrong, prefer `update` on the offending node over deleting parts and starting over.",
    "- Deliver the project bill of materials with `bom_create` → `bom_export` (markdown/CSV/JSON with landed-cost totals): link quote_manufacturing quotes on manufactured lines, and source COTS hardware (bearings, shafts, standoffs, screws, ferrite magnets) with `search_mechanical_parts`. All BOM prices are estimates and flagged as such.",
    "",
    "PCB workflow: `create_schematic` (declare connectivity as data via `nets`) → `place_components` → `route_nets` / `add_coil` / `add_coil_array` → `run_drc` → `validate_for_fab` → `export_gerber`. All take the `document_id` from create_schematic and mutate that session — never re-send the document. `validate_for_fab` is the single 'is this board ready?' gate (DRC + renderability + Gerber serialization + blockers, all fail-closed); `export_gerber` enforces a clean DRC by default and blocks a dirty board. `board_from_solid` turns a solid part (e.g. an enclosure or stator disc in a CAD session) into an outline polygon for `place_components`. For motors, plan the winding first with `winding_layout` (slots + poles → per-coil phase/polarity/winding-factor, as data — it touches no board), then realize it with `add_coil_array`. `run_drc` returns a summary by default (counts by rule + net-pair, worst clearance, a capped sample); pass `detail:'full'` for every violation. Surgical copper edits: `get_copper` lists existing traces/vias/zones (filtered by layer/net/bbox/kind) with the same indices `delete_trace`/`delete_via`/`delete_zone` accept — discover, then delete, without exporting the document. Where two nets must touch on purpose (wye neutral, split ground, shunt tap), declare it with `add_net_tie` — prefer a region-scoped tie (position+radius) so DRC stays honest away from the junction; `delete_net_tie` takes it back.",
  ].join("\n");
  if (!kernelPrompt) return header;
  const sections = new Map<string, string>();
  for (const part of kernelPrompt.split(/^## /m).slice(1)) {
    const nl = part.indexOf("\n");
    if (nl < 0) continue;
    sections.set(part.slice(0, nl).trim(), part.slice(nl + 1).trimEnd());
  }
  const picked = INSTRUCTION_SECTIONS.filter((t) => sections.has(t)).map(
    (t) => `## ${t}\n${sections.get(t)}`,
  );
  return [header, ...picked].join("\n\n");
}

/** Every distinct domain pack contributed by a ToolDef, sorted. Core
 *  (`pack: null`) tools and the registry-tier kernel tools are never packs.
 *  The runtime pack-switching meta-tools (`list_tool_packs`/`set_tool_packs`)
 *  validate names against this and report per-pack state from it. */
export const ALL_PACKS: readonly string[] = Array.from(
  new Set(STATIC_TOOL_DEFS.map((d) => d.pack).filter((p): p is string => !!p)),
).sort();

/** Parse `VCAD_MCP_PACKS` into the set of ENABLED packs, or null when the var
 *  is unset — the default, meaning "all packs". `none` yields the empty set
 *  (core only). This is the boot-time default; a connection may then flip it
 *  live via `set_tool_packs`. */
export function parseEnvPacks(): Set<string> | null {
  const env = process.env.VCAD_MCP_PACKS?.trim();
  if (!env) return null;
  const enabled = new Set(
    env.split(",").map((s) => s.trim().toLowerCase()).filter(Boolean),
  );
  enabled.delete("none");
  return enabled;
}

/** Tool names hidden given a set of ENABLED packs: a tool is hidden when its
 *  `pack` is set and not in `enabled`. `pack: null` core tools and the
 *  registry-tier kernel tools are never gated. Derived from each ToolDef's
 *  `pack` — no separate pack table. */
export function packDisabledNames(enabled: Set<string>): Set<string> {
  const disabled = new Set<string>();
  for (const d of STATIC_TOOL_DEFS) {
    if (d.pack && !enabled.has(d.pack)) disabled.add(d.name);
  }
  return disabled;
}

/** Tool names hidden by the `VCAD_MCP_PACKS` env var (empty = none). `pack:
 *  null` tools (core) and the registry-tier kernel tools are never gated.
 *  Exported for tests. */
export function disabledToolNames(): Set<string> {
  const enabled = parseEnvPacks();
  return enabled ? packDisabledNames(enabled) : new Set();
}

/**
 * Single chokepoint for viewer `_meta`, derived from a tool's behavior flags —
 * so a new tool can never accidentally inherit the template; it has to set
 * `mount` (or `appOnly`/`widgetCallable`). Precedence matches the old
 * applyViewerMeta: app-only fetchers first, then mount, then widget-callable;
 * everything else returns no `_meta` (a data tool the host never mounts).
 */
function viewerMetaFor(b: ToolBehavior): Record<string, unknown> | undefined {
  if (b.appOnly) {
    return { ...WIDGET_CALLABLE_META, ui: { visibility: ["app"] } };
  }
  if (b.mount) return { ...UI_META };
  if (b.widgetCallable) return { ...WIDGET_CALLABLE_META };
  return undefined;
}

/** Project a ToolDef to its advertised ListTools descriptor (name,
 *  description, inputSchema, optional annotations/outputSchema, and the
 *  derived viewer `_meta`). */
function toListDescriptor(def: ToolDef): Record<string, unknown> {
  const desc: Record<string, unknown> = {
    name: def.name,
    description: def.description,
    inputSchema: def.inputSchema,
  };
  if (def.annotations) desc.annotations = def.annotations;
  if (def.outputSchema) desc.outputSchema = def.outputSchema;
  const meta = viewerMetaFor(def.behavior);
  if (meta) desc._meta = meta;
  return desc;
}

export async function createServer(
  existingEngine?: Engine,
  context: ServerContext = { user: null },
): Promise<Server> {
  // Initialize the WASM engine (or reuse one provided by the caller)
  const engine = existingEngine ?? await Engine.init();

  // Durable session store for THIS connection's user. With a signed-in user +
  // a Supabase service-role key it persists sessions to the cloud `documents`
  // table — so a cold serverless instance rehydrates the board instead of
  // throwing "Unknown document_id", and the work shows up at vcad.io.
  // Otherwise an in-memory no-op store reproduces today's behavior. Held in a
  // closure (not a module global) so concurrent connections on one warm
  // instance can't clobber each other's binding.
  const sessionStore = createSessionStore(context.user);

  // The event spine for THIS connection. Every kernel mutation appends one row
  // (state = fold(log)); the sessionStore's content write is the derived
  // materialization. No-op without Supabase env, so stdio/local is unchanged.
  const eventStore = createSessionEventStore(context.user);

  // vcad Fabricate store (quotes + orders). Cloud-backed for a signed-in user
  // with the Supabase service-role key, else in-memory (local stdio). Held in
  // the connection closure like sessionStore so concurrent connections can't
  // clobber each other.
  const fabricateStore = createFabricateStore(context.user);

  // Live-window share gate (live_shares). Created per connection like the
  // others. No-op without Supabase env.
  const shareStore = createShareStore();

  // Everything a tool handler may need, threaded per connection.
  const ctx: ToolContext = {
    engine,
    user: context.user,
    sessionStore,
    eventStore,
    fabricateStore,
    shareStore,
  };

  // Wire the kernel WASM's chat helpers into the shared commandRegistry so
  // `toAnthropicTools` and `planCrud` work on the server too. Same bootstrap
  // as `initEngineLifecycle` in @vcad/core, minus the docstore subscription
  // — we don't have a docstore here. Without this, registryToolDescriptors
  // returns the static-schemas fallback and planCrud returns null.
  let kernelPrompt: string | null = null;
  try {
    const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
    const getToolSchemas = wasm.get_tool_schemas as (() => string) | undefined;
    if (getToolSchemas) commandRegistry.loadSchemas(getToolSchemas());
    const getAnthropicToolsJson = wasm.get_anthropic_tools_json as
      | (() => string)
      | undefined;
    const buildChatSystemPrompt = wasm.build_chat_system_prompt as
      | ((partsJson: string, selectionJson: string) => string)
      | undefined;
    const planChatTool = wasm.plan_chat_tool as
      | ((tool: string, argsJson: string, docJson: string) => string)
      | undefined;
    if (getAnthropicToolsJson && buildChatSystemPrompt) {
      commandRegistry.setWasm({
        get_anthropic_tools_json: getAnthropicToolsJson,
        build_chat_system_prompt: buildChatSystemPrompt,
        plan_chat_tool: planChatTool,
      });
      // Empty parts / no selection — we only want the durable knowledge
      // sections (type catalog, materials, orientation), not scene state.
      kernelPrompt = buildChatSystemPrompt("[]", "null");
    }
  } catch (e) {
    console.warn("[mcp] commandRegistry wasm bootstrap failed:", e);
  }

  // Names of every kernel-tier tool that the registry dispatcher will
  // handle. Computed once after wasm bootstrap so server_info can report the
  // count and the assembler can generate their ToolDefs.
  const dispatchableTools = registryDispatchableNames();

  // Registry-driven kernel tools, generated per connection from the WASM
  // registry as ToolDefs — one dispatch pipeline, no special-cased early
  // return. Each mutates a session document and is geometry (a preview is
  // always meaningful); `read` alone is a pure reader. viewer `_meta` is
  // derived like every other tool: they carry no template.
  const registryDefs: ToolDef[] = registryToolDescriptors().map((d) => ({
    name: d.name,
    pack: null,
    description: d.description,
    inputSchema: d.inputSchema as Record<string, unknown>,
    handler: (args: Record<string, unknown>, c: ToolContext) =>
      dispatchRegistryTool(d.name, args, c.engine) as ToolResult,
    behavior: behavior({ geometry: true, writesDoc: d.name !== "read" }),
  }));

  // ── Runtime tool packs ────────────────────────────────────────────────────
  // The enabled-pack set is mutable per connection: `set_tool_packs` flips it
  // live. On stdio/persistent transports the flip takes effect immediately and
  // emits notifications/tools/list_changed; on the stateless HTTP transport
  // it's persisted for a signed-in user (packStore) and re-derived here on the
  // next request. Initial value: the user's saved preference if any, else
  // VCAD_MCP_PACKS, else all packs (unchanged default). `enabledPacks` and the
  // derived `disabledTools` are `let` so the meta-tool can reassign them; every
  // reader (assembleToolList, the CallTool gate, server_info) reads them at
  // call time and so reflects the current state.
  const packStore = createPackStore(context.user);
  let enabledPacks: Set<string> = await (async () => {
    try {
      const saved = await packStore.load();
      if (saved) return new Set(saved);
    } catch {
      // durable read is best-effort — fall back to env / all packs
    }
    return parseEnvPacks() ?? new Set(ALL_PACKS);
  })();
  let disabledTools = packDisabledNames(enabledPacks);

  /** Compact summary of the enabled packs for `server_info`: "all", "none", or
   *  a sorted comma list. */
  const packsSummary = (): string => {
    if (enabledPacks.size === ALL_PACKS.length) return "all";
    if (enabledPacks.size === 0) return "none";
    return Array.from(enabledPacks).sort().join(",");
  };

  /** Per-pack enabled state + tool count, for the pack meta-tools. */
  const packState = (): Array<{ name: string; enabled: boolean; tool_count: number }> =>
    ALL_PACKS.map((name) => ({
      name,
      enabled: enabledPacks.has(name),
      tool_count: STATIC_TOOL_DEFS.filter((d) => d.pack === name).length,
    }));

  // Startup self-check: confirm the kernel WASM exposes the load-bearing
  // exports this build depends on. A stale/incomplete dist otherwise surfaces
  // as an opaque TypeError mid-call; here it's a clear, named boot error and is
  // reported by `server_info` so an agent can detect version skew in one call.
  let kernelWasmLoaded = false;
  let kernelWasmMissing: string[] = [];
  try {
    const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
    kernelWasmLoaded = true;
    kernelWasmMissing = REQUIRED_WASM_EXPORTS.filter((n) => typeof wasm[n] !== "function");
    if (kernelWasmMissing.length > 0) {
      console.error(
        `[mcp] STALE/INCOMPLETE BUILD: kernel WASM is missing [${kernelWasmMissing.join(", ")}] — rebuild vcad-kernel-wasm. Affected tools (render_pcb, ortho views) will fail until then.`,
      );
    }
  } catch (e) {
    console.error(
      "[mcp] kernel WASM failed to load:",
      e,
      "\n  → the dist may be stale; rebuild @vcad/engine + vcad-kernel-wasm.",
    );
  }
  console.error(
    `[mcp] vcad ${VERSION_WITH_BUILD} (instance ${INSTANCE_ID}) — ${dispatchableTools.size} kernel tools; kernel WASM ${kernelWasmLoaded ? "ok" : "UNAVAILABLE"}`,
  );

  // ── server_info: reports THIS connection's build/runtime state ────────────
  // Defined here (not in a module) because it closes over per-connection boot
  // state; still a plain ToolDef so it dispatches through the shared pipeline.
  const serverInfoDef: ToolDef = {
    name: "server_info",
    pack: null,
    description:
      "Report the running build's identity: version, git sha (if stamped), " +
      "tool count, enabled packs, whether the kernel WASM loaded, and whether " +
      "sessions are durable (`durable` — survive a redeploy/cold start, vs. " +
      "in-memory only). Call this to confirm a tool exists in THIS build " +
      "before assuming a stale or version-skewed deploy, and to check " +
      "durable:true before relying on checkpoints across a long session.",
    inputSchema: serverInfoSchema,
    handler: async (): Promise<ToolResult> => {
      const buildInfo = getBuildInfo();
      // expected_build_sha/is_stale come from Edge Config (per-request,
      // TTL-cached): on a warm instance pinned behind a fresh deploy this
      // flips to is_stale:true so the agent knows to reconnect.
      const staleness = await getStaleness(buildInfo.build_sha);
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify({
              ...buildInfo,
              ...sessionStoreInfo(),
              ...staleness,
              kernel_wasm: kernelWasmLoaded ? "ok" : "unavailable",
              ...(kernelWasmMissing.length > 0
                ? { kernel_wasm_missing_exports: kernelWasmMissing }
                : {}),
              kernel_tool_count: dispatchableTools.size,
              disabled_tool_count: disabledTools.size,
              packs: packsSummary(),
            }),
          },
        ],
      };
    },
    behavior: behavior({}),
  };

  // ── Runtime tool-pack meta-tools ──────────────────────────────────────────
  // Defined inline (like server_info) because they close over this
  // connection's mutable `enabledPacks` / `disabledTools`, its `packStore`, and
  // the `server` handle used to emit list_changed.
  const listToolPacksDef: ToolDef = {
    name: "list_tool_packs",
    pack: null,
    description:
      "List the optional tool packs and whether each is currently enabled, " +
      "with its tool count. Packs gate large domain surfaces (ecad, physics, " +
      "sheet_metal, dfm, …) off the always-on core; a smaller surface costs " +
      "fewer schema tokens and improves tool selection. Use set_tool_packs to " +
      "enable/disable them at runtime.",
    inputSchema: { type: "object", properties: {} },
    handler: async (): Promise<ToolResult> => ({
      content: [
        {
          type: "text",
          text: JSON.stringify({ packs: packState(), core_always_on: true }),
        },
      ],
    }),
    behavior: behavior({}),
  };

  const setToolPacksDef: ToolDef = {
    name: "set_tool_packs",
    pack: null,
    description:
      "Enable or disable optional tool packs at runtime (see list_tool_packs " +
      "for names). Pass `enable` and/or `disable` as arrays of pack names, or " +
      '`set` to replace the enabled set outright (an array, or the string ' +
      '"all" / "none"). On stdio/persistent connections the tool list updates ' +
      "immediately and emits notifications/tools/list_changed; on the stateless " +
      "HTTP transport the choice is saved for a signed-in user and applies on " +
      "the next request (no push notification there). Disabled-pack calls keep " +
      "returning an actionable error.",
    inputSchema: {
      type: "object",
      properties: {
        enable: {
          type: "array",
          items: { type: "string" },
          description: "Pack names to enable.",
        },
        disable: {
          type: "array",
          items: { type: "string" },
          description: "Pack names to disable.",
        },
        set: {
          description:
            'Replace the enabled set: an array of pack names, or "all" / "none".',
        },
      },
    },
    handler: async (args): Promise<ToolResult> => {
      const err = (text: string): ToolResult => ({
        content: [{ type: "text", text }],
        isError: true,
      });
      const known = new Set(ALL_PACKS);
      const bad = new Set<string>();
      const asNames = (v: unknown): string[] =>
        Array.isArray(v) ? v.map((x) => String(x).trim().toLowerCase()) : [];
      const noteUnknown = (names: string[]) => {
        for (const n of names) if (!known.has(n)) bad.add(n);
      };

      let next: Set<string>;
      if (args.set !== undefined) {
        if (args.set === "all") next = new Set(ALL_PACKS);
        else if (args.set === "none") next = new Set();
        else if (Array.isArray(args.set)) {
          const names = asNames(args.set);
          noteUnknown(names);
          next = new Set(names);
        } else {
          return err('`set` must be an array of pack names, or "all" / "none".');
        }
      } else {
        next = new Set(enabledPacks);
      }
      if (args.enable !== undefined) {
        const names = asNames(args.enable);
        noteUnknown(names);
        for (const n of names) next.add(n);
      }
      if (args.disable !== undefined) {
        const names = asNames(args.disable);
        noteUnknown(names);
        for (const n of names) next.delete(n);
      }
      if (bad.size > 0) {
        return err(
          `Unknown pack(s): ${Array.from(bad).join(", ")}. ` +
            `Known packs: ${ALL_PACKS.join(", ")}.`,
        );
      }

      enabledPacks = next;
      disabledTools = packDisabledNames(enabledPacks);

      // Persist for a signed-in user so a stateless HTTP request re-derives it.
      let persisted = false;
      try {
        await packStore.save(Array.from(enabledPacks).sort());
        persisted = packStore.durable && context.user !== null;
      } catch {
        // best-effort durable write
      }

      // Live update on persistent transports. On the stateless HTTP transport
      // there's no push channel, so this is a no-op / rejects — the next
      // request advertises the new surface instead.
      let listChangedSent = false;
      try {
        await server.sendToolListChanged();
        listChangedSent = true;
      } catch {
        // no notification channel (stateless transport / not connected)
      }

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify({
              packs: packState(),
              enabled: Array.from(enabledPacks).sort(),
              list_changed_sent: listChangedSent,
              persisted,
            }),
          },
        ],
      };
    },
    behavior: behavior({}),
  };

  // Every tool this connection can dispatch: static module defs + server_info +
  // the pack meta-tools + the generated registry defs. Name → def, for O(1)
  // CallTool lookup.
  const dispatchMap = new Map<string, ToolDef>();
  for (const d of STATIC_TOOL_DEFS) dispatchMap.set(d.name, d);
  dispatchMap.set(serverInfoDef.name, serverInfoDef);
  dispatchMap.set(listToolPacksDef.name, listToolPacksDef);
  dispatchMap.set(setToolPacksDef.name, setToolPacksDef);
  for (const d of registryDefs) dispatchMap.set(d.name, d);

  // Boot-time drift guard: LIST_TOOL_ORDER must name every static def +
  // server_info + the pack meta-tools exactly once (registry defs are spliced
  // separately). A missing or duplicated name is a wiring bug that would
  // silently drop/duplicate a tool from ListTools.
  const orderSet = new Set(LIST_TOOL_ORDER);
  const staticNames = new Set([
    ...STATIC_TOOL_DEFS.map((d) => d.name),
    serverInfoDef.name,
    listToolPacksDef.name,
    setToolPacksDef.name,
  ]);
  if (orderSet.size !== LIST_TOOL_ORDER.length) {
    throw new Error("[mcp] LIST_TOOL_ORDER contains a duplicate tool name");
  }
  for (const n of orderSet) {
    if (!staticNames.has(n)) {
      throw new Error(`[mcp] LIST_TOOL_ORDER lists unknown tool "${n}"`);
    }
  }
  for (const n of staticNames) {
    if (!orderSet.has(n)) {
      throw new Error(`[mcp] tool "${n}" is missing from LIST_TOOL_ORDER`);
    }
  }

  /** Assemble the advertised ListTools descriptors in `LIST_TOOL_ORDER`,
   *  splicing the registry-tier kernel tools in right after `place_part`, then
   *  dropping any tool disabled by VCAD_MCP_PACKS. Single chokepoint for
   *  viewer `_meta` via `toListDescriptor`. */
  const assembleToolList = (): Array<Record<string, unknown>> => {
    const ordered: ToolDef[] = [];
    for (const name of LIST_TOOL_ORDER) {
      const def = dispatchMap.get(name);
      if (def) ordered.push(def);
      if (name === "place_part") ordered.push(...registryDefs);
    }
    return ordered
      .map(toListDescriptor)
      .filter((t) => !disabledTools.has(t.name as string));
  };

  // Connect-time staleness: surface it in the `initialize` instructions so a
  // client learns at handshake — before any tool call — whether this instance
  // is behind the latest deployment. createServer runs per request on
  // serverless, so the Edge Config read is effectively per-connection (and
  // TTL-cached). When nothing is published (Edge Config unset), is_stale is
  // false and no banner is added — identical to the pre-Edge-Config world.
  let instructions = buildInstructions(kernelPrompt);
  try {
    const connectStaleness = await getStaleness(BUILD_SHA);
    if (connectStaleness.is_stale) {
      const want = connectStaleness.expected_build_sha?.slice(0, 7) ?? "newer";
      instructions +=
        `\n\n⚠️ STALE BUILD: this instance is ${SHORT_SHA} but the latest ` +
        `deployment is ${want}. You may be pinned to a draining warm instance — ` +
        `if a just-shipped tool or flag seems missing, reconnect to land on the ` +
        `new build (or verify with \`server_info\` / \`curl https://mcp.vcad.io/health\`).`;
    }
  } catch {
    // staleness is advisory; never block a connection on it
  }

  const server = new Server(
    {
      name: "vcad",
      // Build-tagged (`0.9.4+1a2b3c4`) so the commit is visible in the MCP
      // `initialize` handshake — no tool call needed to tell builds apart.
      version: VERSION_WITH_BUILD,
    },
    {
      capabilities: {
        // listChanged: `set_tool_packs` re-advertises the surface at runtime
        // and emits notifications/tools/list_changed on persistent transports.
        tools: { listChanged: true },
        resources: {},
        // Acknowledge MCP Apps UI extension so Claude Desktop renders the viewer iframe.
        // The extension key is not in the typed ServerCapabilities schema so we spread as object.
        ...({ extensions: { "io.modelcontextprotocol/ui": { mimeTypes: [MCP_APP_MIME_TYPE] } } } as object),
      },
      // Protocol-native equivalent of the in-app chat system prompt:
      // workflow framing + the kernel's type catalog, material keys, and
      // orientation semantics. Hosts surface this to the agent once. Carries a
      // STALE BUILD banner when this instance is behind the latest deployment.
      instructions,
    },
  );

  // List available tools. Single chokepoint for viewer `_meta`: MOUNT tools get
  // the template, everything else returns data only — so a long session is one
  // live canvas, not one heavy iframe per tool call.
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: assembleToolList(),
  }));

  // ── MCP Apps: List UI resources ──────────────────────────────
  // The same self-contained HTML is registered twice: once with the MCP
  // Apps profile MIME (Claude, Cursor) and once with ChatGPT's skybridge
  // MIME. The bundle detects which bridge the host injected at runtime.
  server.setRequestHandler(ListResourcesRequestSchema, async () => ({
    resources: [
      {
        uri: VIEWER_RESOURCE_URI,
        name: "vcad 3D Viewer",
        description: "Interactive 3D viewport for viewing CAD models",
        mimeType: MCP_APP_MIME_TYPE,
        _meta: {
          ui: {
            csp: VIEWER_CSP,
            prefersBorder: false,
          },
        },
      },
      {
        uri: OPENAI_VIEWER_RESOURCE_URI,
        name: "vcad 3D Viewer (ChatGPT)",
        description: "Interactive 3D viewport for viewing CAD models",
        mimeType: OPENAI_APP_MIME_TYPE,
        _meta: {
          "openai/widgetCSP": OPENAI_WIDGET_CSP,
          "openai/widgetPrefersBorder": false,
        },
      },
    ],
  }));

  // ── MCP Apps: Serve UI resource HTML ─────────────────────────
  server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
    const { uri } = request.params;

    if (uri === VIEWER_RESOURCE_URI) {
      return {
        contents: [
          {
            uri: VIEWER_RESOURCE_URI,
            mimeType: MCP_APP_MIME_TYPE,
            text: getViewerHtml(),
            _meta: {
              ui: {
                csp: VIEWER_CSP,
                prefersBorder: false,
              },
            },
          },
        ],
      };
    }

    if (uri === OPENAI_VIEWER_RESOURCE_URI) {
      return {
        contents: [
          {
            uri: OPENAI_VIEWER_RESOURCE_URI,
            mimeType: OPENAI_APP_MIME_TYPE,
            text: getViewerHtml(),
            _meta: {
              "openai/widgetCSP": OPENAI_WIDGET_CSP,
              "openai/widgetPrefersBorder": false,
            },
          },
        ],
      };
    }

    throw new Error(`Unknown resource: ${uri}`);
  });

  // True when the connected client declared the MCP Apps UI extension at
  // initialize — only then is there a viewer iframe that can stand in for
  // slimmed-out tool-result text.
  const clientHasInlineUi = (): boolean => {
    const caps = server.getClientCapabilities() as
      | Record<string, unknown>
      | undefined;
    const ext = caps?.extensions as Record<string, unknown> | undefined;
    return Boolean(ext && ext["io.modelcontextprotocol/ui"]);
  };

  /**
   * Stamp the running build/runtime identity onto EVERY tool result's `_meta`,
   * not just `server_info`. Warm-instance staleness and version skew then show
   * up inline on any call: two results with different instance_id/build_sha mean
   * old instances are still draining behind a fresh deploy, and `is_stale` flags
   * the instance you're pinned to as behind the latest deployment (compared to
   * `expected_build_sha` from Edge Config). Merges under a namespaced key so it
   * never clobbers another extension's `_meta`; best-effort — a stamp failure
   * must never turn a successful call into an error.
   */
  const stampResultMeta = async (result: ToolResult): Promise<void> => {
    if (!result || typeof result !== "object") return;
    try {
      const info = getBuildInfo();
      const { expected_build_sha, is_stale } = await getStaleness(info.build_sha);
      result._meta = {
        ...result._meta,
        "io.vcad/build": {
          build_sha: info.build_sha,
          instance_id: info.instance_id,
          version_full: info.version_full,
          uptime_s: info.uptime_s,
          expected_build_sha,
          is_stale,
        },
      };
    } catch {
      // identity stamping is observability, never load-bearing
    }
  };

  // Handle tool calls
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args = {} } = request.params;

    if (disabledTools.has(name)) {
      const pack = dispatchMap.get(name)?.pack;
      const enableHint = pack
        ? `Enable it with set_tool_packs({ enable: ["${pack}"] }) or set VCAD_MCP_PACKS.`
        : "Enable its pack to use it.";
      const disabledResult: ToolResult = {
        content: [
          {
            type: "text",
            text: `Tool '${name}' belongs to a disabled tool pack${pack ? ` ('${pack}')` : ""}. ${enableHint}`,
          },
        ],
        isError: true,
      };
      fireToolAlert(name, args, disabledResult);
      await stampResultMeta(disabledResult);
      return disabledResult;
    }

    const def = dispatchMap.get(name);

    // Run the whole call in a per-connection session scope: a signed-in user
    // gets an isolated per-request document cache (so a cache hit can't serve
    // another tenant's doc), while anonymous/stdio callers share the
    // process-wide fallback. Everything below — hydrate, dispatch, persist —
    // must run inside it so the `documents` facade routes to the right cache.
    const scopedResult = await runInSessionScope(context.user, async (): Promise<ToolResult> => {
    // ── Hydrate ───────────────────────────────────────────────────────────
    // If the call names a session that isn't in the warm cache, rehydrate it
    // from the durable store BEFORE the (synchronous) tool reads it via
    // getSession. No-op under the in-memory store, so stdio/anonymous behavior
    // is unchanged; the cold-serverless-instance fix lives here.
    const incomingId =
      typeof args.document_id === "string" ? args.document_id : null;
    if (incomingId) {
      try {
        await hydrateSession(sessionStore, incomingId);
      } catch {
        // Durable load failed — fall back to cache (no worse than today).
      }
    }

    // ── Undo snapshot ─────────────────────────────────────────────────────
    // Snapshot the document BEFORE any mutation of an existing session, so
    // `undo` can rewind it. Gated to writers that target a resident session
    // (creators mint a fresh id and have nothing prior to restore); `undo`
    // itself is excluded so it walks the stack back rather than re-pushing.
    if (
      incomingId &&
      name !== "undo" &&
      def?.behavior.writesDoc &&
      documents.has(incomingId)
    ) {
      recordHistorySnapshot(incomingId);
    }

    // Inner dispatch. Encloses the Map lookup + handler so the single persist
    // site below covers every writer uniformly. Every tool — including the
    // registry-tier kernel tools — routes through here; no special-cased path.
    const runTool = async (): Promise<ToolResult> => {
      if (!def) {
        const unknownResult: ToolResult = {
          content: [{ type: "text", text: `Unknown tool: ${name}` }],
          isError: true,
        };
        fireToolAlert(name, args, unknownResult);
        return unknownResult;
      }

      const result = await def.handler(args, ctx);

      // ── MCP Apps: attach preview handle for geometry tools ──────
      // The viewer fetches the actual GLB via the app-only `get_preview_glb`
      // tool, so results stay lean for the model.
      if (def.behavior.geometry && result.content.length > 0 && !result.isError) {
        const docId = resolvePreviewDocumentId(name, result, args, engine);
        if (docId) {
          attachPreviewHandle(result, docId, name);
          slimPreviewForInlineUi(result, docId, name, clientHasInlineUi());
        }
      }

      fireToolAlert(name, args, result);
      return result;
    };

    try {
      const result = await runTool();

      // Tools that RETURN {isError:true} (the ECAD / sheet-metal / DFM surface)
      // never reach the throw-catch below, so enrich them here — every failure
      // carries next_actions, not just the ones that throw. Successes on the
      // canonical PCB flow carry happy-path next_actions too, so the order of
      // operations is discoverable without first tripping over an ordering error.
      if (result.isError) enrichErrorResult(result, name, args);
      else enrichSuccessResult(result, name, args);

      // ── Persist ─────────────────────────────────────────────────────────
      // After a creator/mutator settles, write the (possibly newly-minted)
      // session through to the durable store. Best-effort: the tool already
      // succeeded, so a write failure must never turn it into an error.
      // effectiveDocId reads the id already resolved into the result/args — it
      // never re-runs resolvePreviewDocumentId, so minting tools
      // (import_step / create_cad_loon) aren't double-registered.
      if (!result.isError && def?.behavior.writesDoc) {
        const writtenId = effectiveDocId(result, args);
        if (writtenId) {
          try {
            await persistSession(sessionStore, writtenId);
          } catch {
            // best-effort durable write
          }
          // Append the kernel event to the spine (state = fold(log)). Same
          // best-effort discipline as persist — a spine write must never turn a
          // successful tool call into an error.
          try {
            await eventStore.append(writtenId, {
              author: context.user?.sub ?? "agent",
              kind: "kernel",
              type: name,
              payload: buildKernelEventPayload(name, args, result),
            });
          } catch {
            // best-effort event append
          }
        }
      }
      // close_document → also forget the durable row (flush-then-forget).
      if (name === "close_document" && incomingId) {
        try {
          await dropSession(sessionStore, incomingId);
        } catch {
          // best-effort durable drop
        }
      }
      return result;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // Process-wide kernel-trap net: any tool whose kernel call panicked
      // (wasm32 panics compile to an `unreachable` trap) lands here unless it
      // handled the trap itself. Recover the shared instance so this one bad
      // document can't DoS every other session — the hosted server can't be
      // restarted by a client.
      const kernelTrap = err instanceof WebAssembly.RuntimeError;
      if (kernelTrap) {
        resetKernelWasm(`${name} trapped: ${message}`);
      }
      // Every failure carries structured `next_actions` so the agent can
      // recover in one turn instead of flailing — the verified loop's
      // error side. (The success side rides on the `changed` diff.)
      const errorResult = buildErrorResult(name, args, message, { kernelTrap });
      fireToolAlert(name, args, errorResult);
      return errorResult;
    }
    });
    // Single chokepoint: stamp build identity on the way out so EVERY tool
    // result — success, tool-reported error, or thrown error — carries it.
    await stampResultMeta(scopedResult);
    return scopedResult;
  });

  return server;
}

/**
 * Replace bulky tool-result text (full VCode, large JSON IR, …) with a short
 * summary. Cursor suppresses inline MCP App UI when tool results are huge —
 * the same spill behavior we hit with inlined GLB payloads.
 *
 * Only applies when the client declared MCP Apps support at initialize.
 * On hosts with no inline viewer (Claude Code, plain CLI agents) the
 * summary's "see the inline 3D viewer" points at nothing and the agent
 * loses the entire payload — it must keep the full text result.
 */
export function slimPreviewForInlineUi(
  result: { content: Array<{ type: string; text: string }> },
  docId: string,
  toolName: string,
  clientHasInlineUi: boolean,
): void {
  if (!clientHasInlineUi) return;
  // The flat-pattern coordinates ARE the deliverable — the viewer draws
  // them, but the agent needs the numbers too. Never slim.
  if (toolName === "sheet_metal_unfold") return;
  // get_document's contract is to RETURN the full IR Document body so the
  // caller can capture / serialize / feed it back — the IR is the deliverable,
  // not a side effect of a mutation. Slimming it to a {document_id} stub
  // breaks that contract (and would be circular: the summary says "use
  // get_document for the full IR"). Same set attachPreviewHandle already
  // exempts from the handle-block append.
  if (PURE_JSON_RESULT_TOOLS.has(toolName)) return;
  const alwaysSlim = toolName === "create_cad_loon";
  const totalChars = result.content.reduce(
    (n, c) => n + (c.type === "text" ? c.text.length : 0),
    0,
  );
  if (!alwaysSlim && totalChars <= 8192) return;

  const summary =
    `CAD document ready (${docId}). Geometry is available in the inline 3D viewer. ` +
    "Use get_document for the full IR, inspect_cad for metrics, or export_cad to export.";
  result.content = [
    { type: "text", text: summary },
    { type: "text", text: JSON.stringify({ document_id: docId }) },
  ];
}

/**
 * Attach the preview document id to a tool result: in structuredContent
 * (the spec path) and, when the result text doesn't already mention the
 * id, as a small JSON text block too. Cursor has known gaps forwarding
 * structuredContent to widgets, and the id is useful to agents anyway
 * (it opens the result up for follow-up mutations).
 */
function attachPreviewHandle(
  result: {
    content: Array<{ type: string; text: string; annotations?: unknown }>;
    structuredContent?: Record<string, unknown>;
  },
  docId: string,
  toolName?: string,
): void {
  const structured: Record<string, unknown> = {
    ...result.structuredContent,
    document_id: docId,
  };
  // Cheap, geometry-free change token (FNV-1a over the IR). The live canvas
  // polls `get_preview_version` and re-fetches the GLB only when this flips —
  // so a mutation needn't carry a UI template (no per-call iframe), the one
  // mounted canvas just notices the change and updates itself.
  try {
    structured.document_version = previewVersion(getSession(docId));
  } catch {
    // session not resolvable here — the id alone is enough for the viewer
  }
  result.structuredContent = structured;
  if (toolName && PURE_JSON_RESULT_TOOLS.has(toolName)) return;
  const mentioned = result.content.some(
    (c) => c.type === "text" && c.text.includes(docId),
  );
  if (!mentioned) {
    result.content.push({
      type: "text",
      text: JSON.stringify({ document_id: docId }),
    });
  }
}

/**
 * Resolve the session document id a geometry tool result should be
 * previewed from. The viewer iframe later passes this id to the app-only
 * `get_preview_glb` tool.
 *
 * Strategy per tool:
 * - session-backed tools (registry CRUD, place_part, dfm_apply_fix,
 *   sheet_metal_create, …): the existing `document_id` from args or the
 *   result payload
 * - import_step: register the parsed document as a fresh session
 * - create_cad_loon: re-evaluate loon source and register a fresh session
 */
function resolvePreviewDocumentId(
  toolName: string,
  result: { content: Array<{ type: string; text: string }> },
  args: Record<string, unknown>,
  engine: Engine,
): string | null {
  try {
    // Session-backed args (registry CRUD tools, place_part, dfm_apply_fix)
    if (typeof args.document_id === "string" && documents.has(args.document_id)) {
      return args.document_id;
    }

    const text = result.content[0]?.text;
    if (!text) return null;

    let doc: Document | null = null;
    try {
      const parsed = JSON.parse(text);

      // Session-backed result (e.g. sheet_metal_create, open_document)
      if (
        typeof parsed.document_id === "string" &&
        documents.has(parsed.document_id)
      ) {
        return parsed.document_id;
      }

      // import_step wraps the document in { document, summary }
      if (parsed.document && parsed.document.version) {
        doc = parsed.document as Document;
      } else if (parsed.version && parsed.nodes) {
        // Direct IR document (JSON format output)
        doc = parsed as Document;
      }
    } catch {
      // Not JSON — likely VCode format, handle below
    }

    // For create_cad_loon with VCode output, re-evaluate with JSON format
    if (!doc && toolName === "create_cad_loon" && args.source) {
      const jsonResult = createCadLoon({ ...args, format: "json" }, engine);
      const jsonText = jsonResult.content[0]?.text;
      if (jsonText) {
        const parsed = JSON.parse(jsonText);
        if (parsed.version && parsed.nodes) doc = parsed as Document;
      }
    }

    return doc ? registerSession(doc) : null;
  } catch {
    return null;
  }
}

/**
 * The session id a just-finished writer tool touched — WITHOUT minting a new
 * one. Prefers the explicit `document_id` arg, then the id the dispatch already
 * attached to the result (`structuredContent.document_id` for geometry tools, or
 * a `{ "document_id": … }` text block, which is how switch-path ECAD tools that
 * aren't geometry — create_schematic, route_diff_pair, add_via_array, add_zone,
 * set_placement, set_design_rules — surface theirs). Every candidate is guarded
 * by `documents.has`, so only a live session is ever persisted. Deliberately
 * does NOT call resolvePreviewDocumentId, which registers a fresh session for
 * import_step / create_cad_loon and would double-mint here.
 */
function effectiveDocId(
  result: {
    content: Array<{ type: string; text: string }>;
    structuredContent?: Record<string, unknown>;
  },
  args: Record<string, unknown>,
): string | null {
  const fromArgs =
    typeof args.document_id === "string" ? args.document_id : null;
  if (fromArgs && documents.has(fromArgs)) return fromArgs;

  const fromStructured = result.structuredContent?.document_id;
  if (typeof fromStructured === "string" && documents.has(fromStructured)) {
    return fromStructured;
  }

  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as { document_id?: unknown };
      if (
        typeof parsed.document_id === "string" &&
        documents.has(parsed.document_id)
      ) {
        return parsed.document_id;
      }
    } catch {
      // not JSON — skip
    }
  }
  return null;
}
