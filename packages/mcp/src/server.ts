/**
 * MCP server implementation with vcad tools.
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
import { exportCad, exportCadSchema } from "./tools/export.js";
import { inspectCad, inspectCadSchema } from "./tools/inspect.js";
import {
  renderView,
  renderViewSchema,
  renderPcb,
  renderPcbSchema,
  renderRatsnest,
  renderRatsnestSchema,
  renderStackup,
  renderStackupSchema,
} from "./tools/render.js";
import { recordSimulation, recordSimulationSchema } from "./tools/record.js";
import {
  verifyPart,
  verifyPartSchema,
  listEvalTasks,
  listEvalTasksSchema,
} from "./tools/verify.js";
import { importStep, importStepSchema } from "./tools/import.js";
import {
  importKicad,
  importKicadSchema,
  importEagle,
  importEagleSchema,
} from "./tools/import-pcb.js";
import { openInBrowser, openInBrowserSchema } from "./tools/share.js";
import {
  openDocument,
  openDocumentSchema,
  getDocumentTool,
  getDocumentSchema,
  closeDocument,
  closeDocumentSchema,
  saveDocument,
  saveDocumentSchema,
  loadDocument,
  loadDocumentSchema,
  documents,
  registerSession,
  getSession,
  hydrateSession,
  persistSession,
  dropSession,
  runInSessionScope,
  recordHistorySnapshot,
} from "./tools/session.js";
import {
  continueDocument,
  continueDocumentSchema,
} from "./tools/continue-doc.js";
import {
  checkpointDocument,
  checkpointDocumentSchema,
  branchFrom,
  branchFromSchema,
} from "./tools/checkpoint.js";
import {
  createSessionStore,
  createSessionEventStore,
  createShareStore,
  sessionStoreInfo,
  warnIfSessionStoreNotDurable,
} from "./session-store.js";
// Re-exported so the Vercel entry (services/mcp/entry.ts) and standalone
// /health (http.ts) report the same durability state as server_info.
export { sessionStoreInfo } from "./session-store.js";
import { createFabricateStore } from "./fabricate/store.js";
import {
  quoteManufacturing,
  quoteManufacturingSchema,
  getOrderStatus,
  getOrderStatusSchema,
  listOrders,
  listOrdersSchema,
} from "./tools/order.js";
import {
  authorizeSpend,
  authorizeSpendSchema,
  placeOrder,
  placeOrderSchema,
} from "./tools/ordering.js";
import {
  shareSession,
  shareSessionSchema,
  unshareSession,
  unshareSessionSchema,
} from "./tools/live-share.js";
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
  createRobotEnv,
  createRobotEnvSchema,
  gymStep,
  gymStepSchema,
  gymReset,
  gymResetSchema,
  gymObserve,
  gymObserveSchema,
  gymClose,
  gymCloseSchema,
  batchCreateEnvs,
  batchCreateEnvsSchema,
  batchStep,
  batchStepSchema,
  batchReset,
  batchResetSchema,
} from "./tools/gym.js";
import {
  loadStructure,
  loadStructureSchema,
  inspectMoleculeTool,
  inspectMoleculeSchema,
  minimizeEnergyTool,
  minimizeEnergySchema,
  mdRun,
  mdRunSchema,
  designMaterial,
  designMaterialSchema,
  homogenizeMaterialTool,
  homogenizeMaterialSchema,
  renderMolecule,
  renderMoleculeSchema,
} from "./tools/atoms.js";
import { getChangelog, getChangelogSchema } from "./tools/changelog.js";
import {
  searchPartsTool,
  searchPartsSchema,
  placePartTool,
  placePartSchema,
} from "./tools/parts.js";
import {
  createSchematic,
  createSchematicSchema,
  placeComponents,
  placeComponentsSchema,
  routeNets,
  routeNetsSchema,
  routeDiffPair,
  routeDiffPairSchema,
  critiqueRoute,
  critiqueRouteSchema,
  runDrc,
  runDrcSchema,
  runErc,
  runErcSchema,
  exportGerber,
  exportGerberSchema,
  exportKicad,
  exportKicadSchema,
  validateForFab,
  validateForFabSchema,
  calcImpedance,
  calcImpedanceSchema,
  sizeImpedance,
  sizeImpedanceSchema,
  sizePdn,
  sizePdnSchema,
  calcCoil,
  calcCoilSchema,
  sizeCoil,
  sizeCoilSchema,
  calcRf,
  calcRfSchema,
  addCoil,
  addCoilSchema,
  addCoilArray,
  addCoilArraySchema,
  windingLayout,
  windingLayoutSchema,
  boardFromSolid,
  boardFromSolidSchema,
  addTrace,
  addTraceSchema,
  getPadPositions,
  getPadPositionsSchema,
  getFootprint,
  getFootprintSchema,
  describePcb,
  describePcbSchema,
  addVia,
  addViaSchema,
  setStackup,
  setStackupSchema,
  setPlacement,
  setPlacementSchema,
  setBoardOutline,
  setBoardOutlineSchema,
  addZone,
  addZoneSchema,
  deleteZone,
  deleteZoneSchema,
  deleteTrace,
  deleteTraceSchema,
  deleteVia,
  deleteViaSchema,
  undo,
  undoSchema,
  setDesignRules,
  setDesignRulesSchema,
  sizeTraceForCurrent,
  sizeTraceForCurrentSchema,
  addViaArray,
  addViaArraySchema,
  addMotorWinding,
  addMotorWindingSchema,
  calcMotor,
  calcMotorSchema,
  searchElectronicParts,
  searchElectronicPartsSchema,
  resolvePart,
  resolvePartSchema,
  findAlternatives,
  findAlternativesSchema,
  verifySubstitution,
  verifySubstitutionSchema,
  buildReceipt,
  buildReceiptSchema,
  verifyReceipt,
  verifyReceiptSchema,
  listFootprints,
  listFootprintsSchema,
  searchFootprints,
  searchFootprintsSchema,
} from "./tools/ecad.js";
import { checkEnclosureFit, checkEnclosureFitSchema } from "./tools/enclosure.js";
import { createCadLoon, createCadLoonSchema } from "./tools/loon.js";
import {
  dfmCheck,
  dfmCheckSchema,
  dfmExplain,
  dfmExplainSchema,
  dfmSuggestFix,
  dfmSuggestFixSchema,
  dfmApplyFix,
  dfmApplyFixSchema,
} from "./tools/dfm.js";
import {
  sheetMetalCreate,
  sheetMetalCreateSchema,
  sheetMetalUnfold,
  sheetMetalUnfoldSchema,
  sheetMetalCheck,
  sheetMetalCheckSchema,
  sheetMetalMaterials,
  sheetMetalMaterialsSchema,
  sheetMetalBendTable,
  sheetMetalBendTableSchema,
  sheetMetalCost,
  sheetMetalCostSchema,
  sheetMetalSuggestFix,
  sheetMetalSuggestFixSchema,
  sheetMetalSequence,
  sheetMetalSequenceSchema,
  sheetMetalNest,
  sheetMetalNestSchema,
} from "./tools/sheet-metal.js";
import {
  getPreviewGlb,
  getPreviewGlbSchema,
  getPreviewVersion,
  getPreviewVersionSchema,
  previewVersion,
} from "./tools/preview.js";
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

// Re-exported so the Vercel transport entry can drain in-flight PostHog
// captures before a serverless instance freezes (see services/mcp/entry.ts).
export { flushTelemetry };

/** Tools that produce or modify geometry and should show the 3D viewer.
 *  Registry-driven kernel tools (create, update, delete, …) are added
 *  dynamically in `createServer` once their names are known. */
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

const GEOMETRY_TOOLS = new Set([
  "create_cad_loon",
  "import_step",
  "import_kicad",
  "open_document",
  "get_document",
  // Opens a web doc (from a "Continue in Claude" share token) as a live session
  // seeded with its geometry — render it like open_document/load_document.
  "continue_document",
  "place_part",
  "set_material",
  "dfm_apply_fix",
  "sheet_metal_create",
  // Doesn't mutate, but its result carries the flat pattern the viewer
  // renders as a 2D drawing (cut profile + bend lines).
  "sheet_metal_unfold",
  // PCB session mutators — the board (PcbBoard node) renders in the viewer.
  "place_components",
  "route_nets",
  "add_coil",
  "add_coil_array",
  "add_trace",
  "add_via",
  // Removing copper / rewinding a mutation changes the board — re-render it.
  "delete_zone",
  "delete_trace",
  "delete_via",
  "undo",
  "set_stackup",
  "add_motor_winding",
  // Materializes a saved board into a live session — show it like open_document.
  "load_document",
  // Re-opens a checkpoint snapshot as a live session — seeded with geometry.
  "branch_from",
]);

/**
 * Switch-path tools that create or mutate a session Document, so the dispatch
 * layer persists them to the durable store after they run. Registry-path
 * mutators (create / update / delete / set_material) are detected separately
 * via `dispatchableTools`. Readers, exporters, calculators, and planners
 * (`read`, `inspect_cad`, `export_*`, `board_from_solid`, `winding_layout`,
 * `calc_*` / `size_*`, `run_drc`, …) are intentionally absent — they never
 * change the stored document.
 */
const SWITCH_DOC_WRITERS = new Set<string>([
  // creators
  "open_document",
  "create_cad_loon",
  "import_step",
  "import_kicad",
  "create_schematic",
  "sheet_metal_create",
  // Seeds a new session from a web doc's geometry — persist it to the user's
  // account so the continued part survives a cold instance and shows at vcad.io.
  "continue_document",
  // load_document materializes a saved board into a live session; its
  // local-disk read is a no-op on the serverless deploy, but on success
  // persisting the loaded doc to the user's account is desirable.
  "load_document",
  // Forks/restores a checkpoint into a session; persist so the branch (or the
  // in-place restore) survives a cold instance like every other session.
  "branch_from",
  // CAD / sheet-metal / DFM mutators
  "place_part",
  "dfm_apply_fix",
  // PCB / ECAD mutators
  "place_components",
  "route_nets",
  "route_diff_pair",
  "add_coil",
  "add_coil_array",
  "add_motor_winding",
  "add_trace",
  "add_via",
  "add_via_array",
  "add_zone",
  "delete_zone",
  "delete_trace",
  "delete_via",
  "undo",
  "set_stackup",
  "set_placement",
  "set_board_outline",
  "set_design_rules",
]);

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
 * Tools that MOUNT the live 3D canvas — i.e. carry the UI template
 * (`ui.resourceUri` / `openai/outputTemplate`). Per the MCP Apps spec
 * (SEP-1865), the template is static and should be referenced by a SMALL
 * set of tools, not attached to every result: "If you attach a widget
 * template to every tool call, [the host] can re-render your iframe too
 * often." So only the tools that BEGIN a viewable session reference it.
 *
 * Everything else (create/update/delete, route/add_*, place_part, …) is a
 * data tool: it returns `structuredContent` ({document_id, document_version,
 * changed}) and NO template, so it never spawns a fresh iframe. The canvas
 * mounted here stays live by polling `get_preview_version` and re-fetching
 * geometry only when the version changes — one durable surface across a long
 * session instead of one heavy iframe per mutation.
 */
const MOUNT_TOOLS = new Set<string>([
  // CAD session openers
  "open_document",
  "create_cad_loon",
  "import_step",
  "load_document",
  "continue_document",
  // Re-opens a checkpoint into a (new or restored) session — mount its canvas.
  "branch_from",
  // PCB: place_components creates the first board geometry (create_schematic
  // has none yet, so it must NOT mount — the canvas would fetch an empty board)
  "place_components",
  // Sheet metal: the part, and the flat-pattern drawing
  "sheet_metal_create",
  "sheet_metal_unfold",
  // Verification ledger artifact
  "build_receipt",
]);

/** Tools the viewer iframe calls itself but that must NOT mount a template:
 *  readers reached over the postMessage bridge (deep-link IR fetch, ledger
 *  re-run). They keep `widgetAccessible` so ChatGPT permits the call. */
const WIDGET_CALLABLE_TOOLS = new Set<string>(["get_document", "verify_receipt"]);

/** App-only geometry/version fetchers the viewer polls. Hidden from the model
 *  (`visibility: ["app"]`); never carry a template (they return data the
 *  iframe consumes, not a surface to render). */
const PREVIEW_FETCH_TOOLS = new Set<string>([
  "get_preview_glb",
  "get_preview_version",
]);

/**
 * Single source of truth for viewer `_meta` across the whole tool list.
 * Applied once to the assembled ListTools array so a new tool can never
 * accidentally inherit the template — it has to opt into MOUNT_TOOLS. Strips
 * any stray template meta off data tools.
 */
function applyViewerMeta<T extends { name: string; _meta?: Record<string, unknown> }>(
  tools: T[],
): T[] {
  return tools.map((t) => {
    if (PREVIEW_FETCH_TOOLS.has(t.name)) {
      return { ...t, _meta: { ...WIDGET_CALLABLE_META, ui: { visibility: ["app"] } } };
    }
    if (MOUNT_TOOLS.has(t.name)) {
      return { ...t, _meta: { ...UI_META } };
    }
    if (WIDGET_CALLABLE_TOOLS.has(t.name)) {
      return { ...t, _meta: { ...WIDGET_CALLABLE_META } };
    }
    // Data tool: no viewer template. Drop any inherited _meta so it returns
    // structuredContent only and the host never mounts a per-call iframe.
    if (t._meta) {
      const { _meta: _drop, ...rest } = t;
      void _drop;
      return rest as T;
    }
    return t;
  });
}

/**
 * Domain tool packs. The surface is a small always-on core — the
 * make → see → measure → verify → ship loop (session, loon/CRUD
 * authoring, parts library, inspect, render, export, share) — plus
 * these opt-out packs for specialized workflows.
 *
 * `VCAD_MCP_PACKS` trims what is advertised: a comma-separated list of
 * pack names to enable (e.g. "sheet_metal,dfm"), or "none" for core
 * only. Unset enables every pack — backward compatible. Calls to a
 * tool in a disabled pack return an error pointing at the env var.
 */
const TOOL_PACKS: Record<string, readonly string[]> = {
  fabricate: [
    "quote_manufacturing",
    "get_order_status",
    "list_orders",
    "authorize_spend",
    "place_order",
  ],
  dfm: ["dfm_check", "dfm_explain", "dfm_suggest_fix", "dfm_apply_fix"],
  sheet_metal: [
    "sheet_metal_create",
    "sheet_metal_unfold",
    "sheet_metal_check",
    "sheet_metal_materials",
    "sheet_metal_bend_table",
    "sheet_metal_cost",
    "sheet_metal_suggest_fix",
    "sheet_metal_sequence",
    "sheet_metal_nest",
  ],
  physics: [
    "create_robot_env",
    "gym_step",
    "gym_reset",
    "gym_observe",
    "gym_close",
    "record_simulation",
    "batch_create_envs",
    "batch_step",
    "batch_reset",
  ],
  atoms: [
    "load_structure",
    "inspect_molecule",
    "minimize_energy",
    "md_run",
    "design_material",
    "homogenize_material",
    "render_molecule",
  ],
  ecad: [
    "create_schematic",
    "place_components",
    "route_nets",
    "get_pad_positions",
    "describe_pcb",
    "add_trace",
    "add_via",
    "add_via_array",
    "set_stackup",
    "set_placement",
    "set_board_outline",
    "add_zone",
    "delete_zone",
    "delete_trace",
    "delete_via",
    "set_design_rules",
    "size_trace_for_current",
    "add_coil",
    "add_coil_array",
    "add_motor_winding",
    "winding_layout",
    "board_from_solid",
    "check_enclosure_fit",
    "import_kicad",
    "import_eagle",
    "run_drc",
    "run_erc",
    "export_gerber",
    "validate_for_fab",
    "render_pcb",
    "render_ratsnest",
    "render_stackup",
    "calc_impedance",
    "size_impedance",
    "size_pdn",
    "calc_coil",
    "size_coil",
    "calc_rf",
    "calc_motor",
    "search_electronic_parts",
    "list_footprints",
    "search_footprints",
    "resolve_part",
    "find_alternatives",
    "verify_substitution",
    "build_receipt",
    "verify_receipt",
  ],
  // Mecheval self-grading oracle. The benchmark harness already excludes
  // these during scored runs; hosts that don't want the benchmark
  // vocabulary at all can drop the pack.
  eval: ["verify_part", "list_eval_tasks"],
};

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
    "",
    "PCB workflow: `create_schematic` (declare connectivity as data via `nets`) → `place_components` → `route_nets` / `add_coil` / `add_coil_array` → `run_drc` → `validate_for_fab` → `export_gerber`. All take the `document_id` from create_schematic and mutate that session — never re-send the document. `validate_for_fab` is the single 'is this board ready?' gate (DRC + renderability + Gerber serialization + blockers, all fail-closed); `export_gerber` enforces a clean DRC by default and blocks a dirty board. `board_from_solid` turns a solid part (e.g. an enclosure or stator disc in a CAD session) into an outline polygon for `place_components`. For motors, plan the winding first with `winding_layout` (slots + poles → per-coil phase/polarity/winding-factor, as data — it touches no board), then realize it with `add_coil_array`. `run_drc` returns a summary by default (counts by rule + net-pair, worst clearance, a capped sample); pass `detail:'full'` for every violation.",
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

/** Tool names hidden by the `VCAD_MCP_PACKS` env var (empty = none).
 *  Exported for tests. */
export function disabledToolNames(): Set<string> {
  const env = process.env.VCAD_MCP_PACKS?.trim();
  if (!env) return new Set();
  const enabled = new Set(
    env.split(",").map((s) => s.trim().toLowerCase()).filter(Boolean),
  );
  const disabled = new Set<string>();
  for (const [pack, tools] of Object.entries(TOOL_PACKS)) {
    if (!enabled.has(pack)) for (const t of tools) disabled.add(t);
  }
  return disabled;
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
  // handle. Computed once after wasm bootstrap so the call-site switch can
  // route by name without re-querying the registry per tool call.
  const dispatchableTools = registryDispatchableNames();

  // Every geometry-mutating tool shows the inline 3D viewer: the static
  // set plus all registry-driven kernel tools (they all mutate a session
  // document, so a preview is always meaningful).
  const uiTools = new Set([...GEOMETRY_TOOLS, ...dispatchableTools]);

  // A tool call writes the session document when it's a switch-path creator/
  // mutator OR a registry mutator (every dispatchable tool except `read`,
  // which only inspects). Gates the post-dispatch durable persist so readers
  // don't trigger a needless write-back.
  const isDocWriter = (toolName: string): boolean =>
    SWITCH_DOC_WRITERS.has(toolName) ||
    (dispatchableTools.has(toolName) && toolName !== "read");

  // Tools hidden by VCAD_MCP_PACKS (resolved once at server creation).
  const disabledTools = disabledToolNames();

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
        tools: {},
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

  // List available tools
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    // Single chokepoint for viewer `_meta`: MOUNT_TOOLS get the template,
    // everything else returns data only — so a long session is one live
    // canvas, not one heavy iframe per tool call.
    tools: applyViewerMeta([
      // ── Session lifecycle ──────────────────────────────────────
      {
        name: "open_document",
        description:
          "Open an editing session for a CAD document. Returns a `document_id` to pass to subsequent tool calls (create, update, place_part, inspect_cad, …). Pass an `initial` IR to begin editing an existing document; omit it for a fresh empty document.",
        inputSchema: openDocumentSchema,
      },
      {
        name: "get_document",
        description:
          "Return the full IR Document JSON for an open session. Use after a series of mutations to capture the result, or to feed into `export_cad` / `open_in_browser`. Very large documents come back as a compact artifact handle instead ({document_id, artifact_url, manifest with sha256, …}) — download the full IR at `artifact_url`.",
        inputSchema: getDocumentSchema,
        // Widget-callable: the viewer's "Open in vcad.io" button fetches
        // the IR through this tool to build the deep link.
      },
      {
        name: "close_document",
        description:
          "Close a document session and free its memory. Idempotent — closing an unknown id reports `closed: false`.",
        inputSchema: closeDocumentSchema,
      },
      {
        name: "save_document",
        description:
          "Persist a live session to disk as `<name>.vcad` under VCAD_MCP_STATE_DIR " +
          "(or the working directory) so it survives a restart and can be reopened " +
          "by name with load_document. Sessions are otherwise in-memory only.",
        inputSchema: saveDocumentSchema,
      },
      {
        name: "load_document",
        description:
          "Reopen a previously saved `<name>.vcad` into a fresh session and return " +
          "its new document_id. The cheap way to resume a board/part across runs " +
          "instead of rebuilding it.",
        inputSchema: loadDocumentSchema,
      },
      {
        name: "checkpoint_document",
        description:
          "Snapshot a session's current state as a durable, restorable " +
          "checkpoint. Returns a `checkpoint_id`. Use it at known-good milestones " +
          "(post-schematic, post-place, post-route) so you can rewind with " +
          "branch_from instead of rebuilding. The full IR is captured — the " +
          "netlist (the most expensive, most stable artifact) is the anchor. On a " +
          "durable deploy a checkpoint survives a redeploy; check server_info for " +
          "durable:true.",
        inputSchema: checkpointDocumentSchema,
      },
      {
        name: "branch_from",
        description:
          "Re-open a checkpoint (from checkpoint_document). Omit `into` to BRANCH " +
          "into a fresh session id — a variant to explore. Pass `into: <document_id>` " +
          "to RESTORE the checkpoint into an existing session in place (same id). " +
          "The cheap undo for a bad route or place: rewind to a good state rather " +
          "than rebuilding the netlist.",
        inputSchema: branchFromSchema,
      },
      {
        name: "continue_document",
        description:
          "Open the user's vcad.io part as an editing session from a 'Continue " +
          "in Claude' handoff (a share `token`, or an inline `doc` for accountless " +
          "handoffs). The web app hands you this in the starter prompt; call this " +
          "first, then render_view it and continue the user's work. Returns a " +
          "`document_id` for subsequent tool calls. The geometry is fetched " +
          "server-side — never paste it.",
        inputSchema: continueDocumentSchema,
        // Directory-ready hints: read-only intent (it opens, never mutates the
        // source doc), and it reaches the network to resolve the handoff.
        annotations: {
          title: "Continue from vcad.io",
          readOnlyHint: true,
          destructiveHint: false,
          idempotentHint: false,
          openWorldHint: true,
        },
      },
      {
        name: "server_info",
        description:
          "Report the running build's identity: version, git sha (if stamped), " +
          "tool count, enabled packs, whether the kernel WASM loaded, and whether " +
          "sessions are durable (`durable` — survive a redeploy/cold start, vs. " +
          "in-memory only). Call this to confirm a tool exists in THIS build " +
          "before assuming a stale or version-skewed deploy, and to check " +
          "durable:true before relying on checkpoints across a long session.",
        inputSchema: serverInfoSchema,
      },
      // ── vcad Fabricate: order custom-manufactured parts ───────
      {
        name: "quote_manufacturing",
        description:
          "Quote manufacturing a part: measures the design, runs light DFM, and returns margin-inclusive price options per fab (pcb/cnc/3dprint/sheet_metal/cast_metal). Pass `ir` (inline Document — stateless, no open_document needed, serverless-safe, parallel-safe) OR a `document_id` from an open session. Persists a quote + a QUOTED order. Phase 0 is quote-only — prices are estimates and ordering/payment ship next; no money moves. For sheet_metal the result includes `fab_handoff`: curated US instant-quote shops (SendCutSend/OSH Cut/Fabworks), the exact file recipe (DXF via sheet_metal_unfold or folded STEP via export_cad), and what to enter at upload — everything needed to finish the order on the fab's site today.",
        inputSchema: quoteManufacturingSchema,
      },
      {
        name: "get_order_status",
        description:
          "Return the lifecycle row for a Fabricate order (state, fab, totals, event timeline). Read-only.",
        inputSchema: getOrderStatusSchema,
      },
      {
        name: "list_orders",
        description:
          "List the caller's Fabricate orders, newest first. Optional status filter and limit. Read-only.",
        inputSchema: listOrdersSchema,
      },
      {
        name: "authorize_spend",
        description:
          "Propose a spend authorization for a QUOTED order. Creates a DB-backed, revocable authorization (status pending_human) and records the proposal on the session's event log. A HUMAN must approve it in the vcad app before place_order can charge — the agent cannot approve its own spend. Flag-gated (test-mode); no money moves here.",
        inputSchema: authorizeSpendSchema,
      },
      {
        name: "place_order",
        description:
          "Place a QUOTED order once its authorization has been human-approved: performs one atomic wallet debit and moves the order to PAID (fab submission follows in a later step). Refuses if the authorization is still pending approval. Flag-gated (test-mode).",
        inputSchema: placeOrderSchema,
      },
      // ── Live review window: share a watchable session link ────
      {
        name: "share_session",
        description:
          "Share this session as a live, watchable link (mcp.vcad.io/live/<id>). Sessions are PRIVATE by default — this is the explicit opt-in that makes one viewable. Anyone with the returned link can watch the geometry + full event log (read-only) and drop annotations, so the result includes a clear public-link warning. Revoke anytime with unshare_session.",
        inputSchema: shareSessionSchema,
      },
      {
        name: "unshare_session",
        description:
          "Revoke a session's live link — it goes dead and the session is private again.",
        inputSchema: unshareSessionSchema,
      },
      // ── Stdlib parts library (session-aware) ──────────────────
      {
        name: "search_parts",
        description:
          "Search the stdlib parts library (fasteners, bearings, …). Matches across name, category, synonyms, and catalog part numbers (McMaster / ISO / DIN). Returns an array of {id, name, category, params, xrefs, synonyms} — use `id` with `place_part`. Part numbers like '91290A320' or 'ISO 4762' match directly.",
        inputSchema: searchPartsSchema,
      },
      {
        name: "place_part",
        description:
          "Insert a stdlib part into the session's document. Takes a `document_id`, a `path` (from `search_parts.id`), and an optional `params` map; missing params use declared defaults. The part remains parametric — end users can edit its params from the feature tree.",
        inputSchema: placePartSchema,
      },
      // ── Registry-driven kernel tools (auto-exposed) ───────────
      // The next block iterates `commandRegistry.toAnthropicTools()` so the
      // schema lives in one place — the kernel WASM. Same tools, same
      // behavior as the in-app chat surface; viewport-only tools (camera
      // and scene-evaluation tools) are filtered out via blocklists in
      // tools/registry-dispatch.ts. They are DATA tools: each mutates a
      // session document and returns structuredContent (document_id +
      // document_version + changed), but carries no UI template — the live
      // canvas self-refreshes from the version token. (applyViewerMeta is the
      // single place that decides viewer `_meta`.)
      ...registryToolDescriptors(),
      // ── MCP Apps: app-only preview fetch + version poll ────────
      // visibility: ["app"] (set by applyViewerMeta) — spec-compliant hosts
      // hide these from the agent's tool list; only the viewer iframe calls
      // them (via app.callServerTool). Keeps multi-hundred-KB GLB payloads
      // out of model-visible tool results, and lets the canvas poll a cheap
      // change token to self-refresh without re-evaluating geometry.
      {
        name: "get_preview_glb",
        description:
          "Return a base64 GLB preview of an open session document. Internal to the inline 3D viewer — agents should use `export_cad` for geometry exports.",
        inputSchema: getPreviewGlbSchema,
      },
      {
        name: "get_preview_version",
        description:
          "Return a cheap {document_id, version} change token for an open session document (no geometry eval). Internal to the inline 3D viewer's self-refresh poll — agents should ignore it.",
        inputSchema: getPreviewVersionSchema,
      },
      // ── Loon DSL one-shot ──────────────────────────────────────
      {
        name: "create_cad_loon",
        description:
          "The preferred authoring tool for whole parts and multi-feature models — one call, full vocabulary. Create a CAD document from loon source code. Loon is a Lisp-like language for parametric CAD — the FULL modeling vocabulary (patterns, sketches, extrude/revolve/sweep/loft, assemblies) is available here even where no dedicated MCP tool exists. For incremental single-node edits to an open session, use create/update/delete instead.\n\n" +
          "Primitives: [cube x y z], [cylinder r h], [sphere r], [cone r-bottom r-top h]\n" +
          "Booleans (subject-last): [difference tool subject], [union other subject], [intersection other subject]\n" +
          "Transforms (subject-last): [translate x y z s], [rotate rx ry rz s], [scale sx sy sz s]\n" +
          "Features: [fillet r s], [chamfer d s], [shell t s]\n" +
          "Patterns (subject-last): [linear-pattern dx dy dz count spacing s], [circular-pattern ox oy oz ax ay az count angle s] — e.g. a bolt circle is [circular-pattern 0 0 0 0 0 1 6 360 bolt-hole]\n" +
          "Sketches: [sketch ox oy oz xx xy xz yx yy yz #[segments]] with [line x1 y1 x2 y2] and [arc x1 y1 x2 y2 cx cy ccw]\n" +
          "Sketch ops (sketch-last): [extrude dx dy dz sk], [revolve aox aoy aoz adx ady adz angle sk], [sweep-line sx sy sz ex ey ez sk], [sweep-helix radius pitch height turns sk], [loft #[sk1 sk2 …]]\n" +
          "Assemblies: [assembly #[parts] #[instances] #[joints] ground-id] with [part name solid \"material\"], [instance name part-name x y z], [revolute-joint …], [prismatic-joint …], [fixed-joint …], [ball-joint …]\n" +
          "Pipe: [pipe [cube 50 30 5] [difference [cylinder 3 10]] [fillet 1.0]]\n" +
          "Let bindings: [let body [cube 50 30 5]]\n" +
          "Scene: [root solid \"material-name\"]",
        inputSchema: createCadLoonSchema,
      },
      {
        name: "export_cad",
        description:
          "Export a CAD document to a file. Supports STL (3D printing), GLB (visualization), and — for sheet-metal documents — STEP AP214 of the FOLDED body with true cylindrical bend faces (fab 3D pipelines like SendCutSend auto-detect bends/angles/directions; zero data entry). Format is determined by file extension.",
        inputSchema: exportCadSchema,
      },
      {
        name: "inspect_cad",
        description:
          "Inspect an open session document to get aggregate geometry properties: volume, surface area, bounding box, center of mass, triangle count, and mass (if material density is known). For per-part inspection use the chat-surface `inspect_part` / `describe_scene` tools (deferred from this MCP surface in v1).",
        inputSchema: inspectCadSchema,
      },
      // ── Verify-and-iterate loop: eyes + oracle ──────────────────
      {
        name: "render_view",
        description:
          "Render an open session document to an isometric PNG image so you can SEE the current geometry — silhouettes, holes, creases — not just numbers. Drafting-style line art, Z-up, same renderer as the vcad CLI. Call after mutations to visually confirm the part matches intent before declaring done.",
        inputSchema: renderViewSchema,
      },
      {
        name: "verify_part",
        description:
          "Grade an open session document against a mecheval benchmark task using the official deterministic graders (the exact binaries the leaderboard runs). Returns pass/fail per check — bounding box, mass properties, hole positions, STEP round-trip, … — with measured-vs-expected details so you can iterate until green. Use list_eval_tasks to browse task ids.",
        inputSchema: verifyPartSchema,
      },
      {
        name: "list_eval_tasks",
        description:
          "List mecheval benchmark tasks (id, suite, tier, title, prompt, check count). Suites: A authoring, B kernel, C mech/physics, D visual, F fit. Pair with verify_part for self-graded practice and verification.",
        inputSchema: listEvalTasksSchema,
      },
      // ── DFM (Design for Manufacturing) ──────────────────────────
      {
        name: "dfm_check",
        description:
          "Run Design-for-Manufacturing checks against an open session document. For solid parts pick a mechanical process (cnc_3axis, fdm, sla, injection, sheet_metal, casting_sand, casting_investment) and get back severities, measurements, face references, and suggested fixes. For PCB documents pick a fab profile (pcb_jlcpcb, pcb_pcbway, pcb_generic_2layer, pcb_generic_4layer) to check the board against that fab's published process capability — min annular ring, min drill, min trace/space by copper weight, copper-to-edge, soldermask dam/sliver, silk-over-pad, acid traps, and via-in-pad — returning a per-rule pass/fail report naming the profile. Each rule's threshold is sourced from a TOML pack at lib/dfm/<process>.toml — pass `rule_pack_toml` to override.",
        inputSchema: dfmCheckSchema,
      },
      {
        name: "dfm_explain",
        description:
          "Return the long-form explanation for a specific DFM issue from the most recent `dfm_check` run on this document.",
        inputSchema: dfmExplainSchema,
      },
      {
        name: "dfm_suggest_fix",
        description:
          "Return the suggested patch (set_param / wrap_op / replace_op / manual) for a DFM issue. Inspect the patch; only call `dfm_apply_fix` when you're ready to mutate the IR.",
        inputSchema: dfmSuggestFixSchema,
      },
      {
        name: "dfm_apply_fix",
        description:
          "Apply an approved DFM fix to the session document. v1 supports `set_param` patches (raise a fillet radius, thicken a wall) — other kinds throw and require manual edits. Re-run `dfm_check` afterwards to confirm the issue cleared.",
        inputSchema: dfmApplyFixSchema,
      },
      // ── Sheet metal (AI-native manufacturability surface) ───────
      {
        name: "sheet_metal_create",
        description:
          "Create a sheet-metal part: a rectangular or polygon base flange plus an ordered chain of edge flanges, hems, and jogs. Supports `shop_profile` (e.g. \"sendcutsend\") to resolve bend radii/K-factors from the fab's published catalog, and `bend_relief` to cut relief notches at bend ends. Returns a `document_id` (usable with sheet_metal_unfold/check, inspect_cad, export_cad, open_in_browser), the panel/bend model summary, flat bbox + area, and DFM violations.",
        inputSchema: sheetMetalCreateSchema,
      },
      {
        name: "sheet_metal_unfold",
        description:
          "Return the flat pattern (panel outlines, holes, creases, area, bbox) for a sheet-metal session document, plus a fab-ready merged single-silhouette DXF (millimetres): one closed exterior polyline + holes on CUT, DASHED bend centerlines on BEND_UP/BEND_DOWN. DXF carries no bend angles (entered in the fab's UI); for zero data entry export the folded body as STEP via export_cad instead.",
        inputSchema: sheetMetalUnfoldSchema,
      },
      {
        name: "sheet_metal_check",
        description:
          "Run sheet-metal manufacturability for a session document against a shop profile (brake length, min R/t, flange height, hole→bend, bend→bend, bend relief, fixed radius). `shop_profile` is a catalog id string (e.g. \"sendcutsend\") or a capabilities object (field-tolerant: omit keys for generic defaults). Returns structured violations the agent can use to adjust the part and re-check.",
        inputSchema: sheetMetalCheckSchema,
      },
      {
        name: "sheet_metal_materials",
        description:
          "List the built-in sheet-metal materials registry (aluminum soft/hard, mild + stainless steel, brass, copper) with min R/t, yield, modulus, density, and a coarse springback estimate. Use to pick a `material` for sheet_metal_create.",
        inputSchema: sheetMetalMaterialsSchema,
      },
      {
        name: "sheet_metal_bend_table",
        description:
          "Read the kernel's curated bend table — `(material, thickness, radius) → K-factor` rows used to compute bend allowance. Pass `shop_profile` (e.g. \"sendcutsend\") to instead read that fab service's published catalog: fixed radii, K-factors, die widths, min flange sizes, and relief depths per material/thickness.",
        inputSchema: sheetMetalBendTableSchema,
      },
      {
        name: "sheet_metal_cost",
        description:
          "Estimate the manufacturing cost of a sheet-metal session document: material (mass × $/kg), cut (length × $/m), pierces, bends, amortized setup, plus shop markup. Returns a line-itemed breakdown so the agent can see which line dominates and which design changes would lower it. `rates` is field-tolerant; omit it to use generic low-volume laser defaults.",
        inputSchema: sheetMetalCostSchema,
      },
      {
        name: "sheet_metal_suggest_fix",
        description:
          "Translate the structured violations from sheet_metal_check into concrete parameter changes the agent can apply (radius up, flange longer, bends spread, etc.). Pass `violation_index` to target one, omit it to get a suggestion for every open violation. Closes the create → check → fix → re-check self-heal loop.",
        inputSchema: sheetMetalSuggestFixSchema,
      },
      {
        name: "sheet_metal_sequence",
        description:
          "Return a feasible press-brake bend sequence for a sheet-metal part — outermost bends first so the remaining flat stays small and earlier bends don't collide with later ones. Each step includes the springback-compensated brake angle and a one-line rationale.",
        inputSchema: sheetMetalSequenceSchema,
      },
      {
        name: "sheet_metal_nest",
        description:
          "Pack multiple sheet-metal parts on stock sheets using bottom-left fill decreasing. Each part is either a session `document_id` (footprint inferred from the flat pattern) or an explicit `{width_mm, height_mm}`. Returns per-instance placements, sheets used, and utilization — enough to drive a multi-part DXF and a real quote.",
        inputSchema: sheetMetalNestSchema,
      },
      {
        name: "import_step",
        description:
          "Import geometry from a STEP file (.step or .stp). Returns an IR document with ImportedMesh nodes. " +
          "Supports AP203/AP214 STEP files commonly exported from Fusion 360, SolidWorks, Onshape, etc.",
        inputSchema: importStepSchema,
      },
      {
        name: "import_kicad",
        description:
          "Import an existing KiCad .kicad_pcb board into a live session — board " +
          "outline, footprints with pads + nets, design rules, and any routed " +
          "traces/vias/zones. Returns a document_id ready for render_pcb, " +
          "run_drc, get_pad_positions, route_nets, and export_gerber. Pass " +
          "content_base64 on hosted servers.",
        inputSchema: importKicadSchema,
        _meta: UI_META,
      },
      {
        name: "import_eagle",
        description:
          "Import an Eagle .brd file (not yet supported). Export your board from " +
          "Eagle as KiCad (.kicad_pcb) and use import_kicad instead.",
        inputSchema: importEagleSchema,
      },
      {
        name: "open_in_browser",
        description:
          "Generate a shareable URL to open a CAD document in vcad.io. " +
          "Takes an IR document (JSON or VCode format) and returns a URL that opens the document in the browser. " +
          "Documents are compressed (gzip + base64url) for URL embedding. " +
          "Note: Very large documents may exceed URL length limits (~2KB).",
        inputSchema: openInBrowserSchema,
      },
      {
        name: "create_robot_env",
        description:
          "Create a physics simulation environment from a vcad assembly. " +
          "Returns an environment ID that can be used with gym_step, gym_reset, and gym_observe. " +
          "The environment provides a gym-style interface for RL training.",
        inputSchema: createRobotEnvSchema,
      },
      {
        name: "gym_step",
        description:
          "Step the physics simulation with an action. " +
          "action_type can be 'torque' (Nm), 'position' (degrees/mm), or 'velocity' (deg/s or mm/s). " +
          "Returns observation (joint positions/velocities, end effector poses), reward, and done flag.",
        inputSchema: gymStepSchema,
      },
      {
        name: "gym_reset",
        description:
          "Reset the simulation environment to its initial state. Returns the initial observation.",
        inputSchema: gymResetSchema,
      },
      {
        name: "gym_observe",
        description:
          "Get the current observation from the simulation without stepping. " +
          "Returns joint positions, velocities, and end effector poses.",
        inputSchema: gymObserveSchema,
      },
      {
        name: "gym_close",
        description: "Close and clean up a simulation environment.",
        inputSchema: gymCloseSchema,
      },
      {
        name: "load_structure",
        description:
          "Import an atomic structure from XYZ / extended-XYZ text (or accept a MoleculeSystem) and return the molecule plus a summary (formula, atom count, radius of gyration, bonds, periodicity). Units are Ångström.",
        inputSchema: loadStructureSchema,
      },
      {
        name: "inspect_molecule",
        description:
          "Structural analysis of a molecule: Hill-order formula, per-element counts, mass, center of mass, radius of gyration, bounding box, bond count, periodicity. The atomic-domain analog of inspect_cad.",
        inputSchema: inspectMoleculeSchema,
      },
      {
        name: "minimize_energy",
        description:
          "Relax a structure to a local energy minimum with FIRE. Force field via config (Lennard-Jones default, harmonic bonds, Coulomb, or the ML-potential stub). Returns the relaxed molecule, a result summary (energy, max force, convergence), and a reproducibility receipt.",
        inputSchema: minimizeEnergySchema,
      },
      {
        name: "md_run",
        description:
          "Run molecular dynamics (velocity-Verlet, optional Berendsen thermostat) for N steps and return the final observation (energies, temperature, max force) and the evolved structure.",
        inputSchema: mdRunSchema,
      },
      {
        name: "design_material",
        description:
          "Inverse design: search an isotropic scale factor that drives a geometric property (nearest-neighbor distance or radius of gyration) to a target value, returning the reshaped molecule and a receipt. The energy-objective inverse design (gradients through the simulation) lives in the Rust kernel.",
        inputSchema: designMaterialSchema,
      },
      {
        name: "homogenize_material",
        description:
          "Homogenize a periodic crystal into bulk material properties — density (kg/m³), cubic elastic constants C11/C12/C44 and VRH isotropic moduli (GPa) — the atoms-to-continuum bridge. Requires a fully periodic cell.",
        inputSchema: homogenizeMaterialSchema,
      },
      {
        name: "render_molecule",
        description:
          "Render a molecule as an isometric ball-and-stick (or space-filling) SVG with CPK colors and depth sorting — agent eyes on atomic structures.",
        inputSchema: renderMoleculeSchema,
      },
      {
        name: "record_simulation",
        description:
          "Step an open physics env N times and return an animated GIF of the run — your eyes on the simulation. " +
          "Drives the env created via create_robot_env, mutates the paired session document's joint states each step, " +
          "and re-renders through the same kernel SVG pipeline as render_view. " +
          "Defaults to passive playback (zero torque) under gravity; pass `action` (constant) or `actions[steps][action_dim]` (per-step) for active control. " +
          "Hard caps: steps ≤ 600, width_px ≤ 1024. " +
          "Requires the optional `@resvg/resvg-js` rasterizer and `gifenc` encoder; degrades to a JSON joint-trajectory dump when either is missing.",
        inputSchema: recordSimulationSchema,
      },
      {
        name: "batch_create_envs",
        description:
          "Create N parallel simulation environments from a single robot assembly. " +
          "Returns a batch_id for use with batch_step and batch_reset. " +
          "Enables parallel RL training across multiple environments.",
        inputSchema: batchCreateEnvsSchema,
      },
      {
        name: "batch_step",
        description:
          "Step all environments in a batch simultaneously with per-env actions. " +
          "Returns observations, rewards, and done flags for all environments. " +
          "action_type can be 'torque', 'position', or 'velocity'.",
        inputSchema: batchStepSchema,
      },
      {
        name: "batch_reset",
        description:
          "Reset all environments in a batch to their initial state. " +
          "Returns initial observations for all environments.",
        inputSchema: batchResetSchema,
      },
      {
        name: "get_changelog",
        description:
          "Query vcad changelog by version, category, feature, or MCP tool. " +
          "Returns recent changes, new features, breaking changes, and migration guides.",
        inputSchema: getChangelogSchema,
      },
      {
        name: "create_schematic",
        description:
          "Create a schematic from components plus connectivity, and open it " +
          "as a server-side session. Declare connectivity as data with `nets` " +
          '({"PHA": ["L1.1", "J1.1"]}) — more reliable than wire/label ' +
          "coordinates. Returns a document_id for place_components / " +
          "route_nets / export_gerber, plus the resolved netlist so broken " +
          "connectivity is visible immediately.",
        inputSchema: createSchematicSchema,
      },
      {
        name: "place_components",
        description:
          "Create the board and place schematic components on it. Mutates the " +
          "session document (pass document_id). Outline: rectangle " +
          "(board_width/height), circle with optional center bore " +
          "(board_shape — e.g. a motor stator), or any polygon (outline, e.g. " +
          "from board_from_solid). strategy=radial rings components for " +
          "annular boards. Returns `placement_drc` — the pre-routing DRC subset " +
          "(shorts, pad clearance, courtyard overlaps, off-board parts); when " +
          "`placement_drc.clean` is false, fix the floorplan with set_placement " +
          "before route_nets instead of routing on top of the fault. Also " +
          "returns a `utilization` report (board vs occupied area, % used, " +
          "component bounding box, and an advisory suggested_outline) so you can " +
          "right-size an over-large board in one step.",
        inputSchema: placeComponentsSchema,
      },
      {
        name: "route_nets",
        description:
          "Route electrical nets on the PCB with copper traces. Connects pads " +
          "belonging to the same net. A net with a copper-pour zone (a plane) " +
          "is connected by stitching each pad to the plane with a via instead " +
          "of tracing it — those nets come back in `plane_stitched`. " +
          "`locked_nets` preserves hand-placed copper from rip-up. Mutates the " +
          "session document (pass document_id).",
        inputSchema: routeNetsSchema,
      },
      {
        name: "add_coil",
        description:
          "Add a spiral copper coil (Archimedean) to the PCB — the primitive " +
          "for PCB-motor stators and planar inductors. Generates the trace " +
          "geometry on a layer, assigns it to a net, validates turn-to-turn " +
          "clearance, and optionally drops a via at the (otherwise trapped) " +
          "inner endpoint. Returns endpoints, copper length, and a DC " +
          "resistance estimate.",
        inputSchema: addCoilSchema,
      },
      {
        name: "add_coil_array",
        description:
          "Lay a ring of `count` spiral coils evenly around `center` at " +
          "`pitch_radius` — the placement primitive for a PCB-motor stator. " +
          "Net per coil comes from `net_sequence` (cycled); `chirality` sets " +
          "winding sense. GEOMETRY ONLY: it has no notion of phases — derive " +
          "correct per-coil phase/polarity with `winding_layout` first, then " +
          "map it onto net_sequence/chirality.",
        inputSchema: addCoilArraySchema,
      },
      {
        name: "winding_layout",
        description:
          "Plan a balanced polyphase motor winding (slots + poles → per-coil " +
          "phase, polarity, winding factor, feasibility) as DATA. Pure — it " +
          "does NOT take a board or modify anything; inspect the plan, then " +
          "realize it with add_coil_array/add_coil. Catches infeasible " +
          "slot/pole combos and wrong polarity before any copper is drawn.",
        inputSchema: windingLayoutSchema,
      },
      {
        name: "board_from_solid",
        description:
          "Derive a PCB outline polygon (with cutouts, e.g. a center bore) " +
          "from a solid part in a CAD session by projecting its geometry onto " +
          "the XY plane. Bridges solid modeling and PCB layout: feed the " +
          "returned `outline` to place_components.",
        inputSchema: boardFromSolidSchema,
      },
      {
        name: "check_enclosure_fit",
        description:
          "Cross-check a board (board session) against the enclosure it ships " +
          "in (a CAD session holding the case solid) — the verification axis no " +
          "EDA tool has, because vcad owns both a BRep kernel and a PCB engine. " +
          "Extracts the case cavity, standoffs, and wall cutouts from the solid " +
          "mesh, then verifies: board fits with clearance, tall parts clear the " +
          "lid, mounting holes land on standoffs, and connectors line up with " +
          "the wall openings. Pass `derive:true` to also get a board outline + " +
          "holes seeded from the cavity. Surfaced in build_receipt too.",
        inputSchema: checkEnclosureFitSchema,
      },
      {
        name: "list_footprints",
        description:
          "List the footprint families the parametric engine resolves, each " +
          "with a canonical example id to drop into create_schematic's " +
          "`footprint`. Optional `kind` filter (passive/ic/transistor/diode/" +
          "power/connector). Use this instead of guessing id spellings.",
        inputSchema: listFootprintsSchema,
      },
      {
        name: "search_footprints",
        description:
          "Fuzzy-search footprint families by name/alias (e.g. 'SOIC 8', " +
          "'jst', 'qfn') and get ranked matches with a canonical example id — " +
          "resolve a footprint id without a failed create_schematic round-trip.",
        inputSchema: searchFootprintsSchema,
      },
      {
        name: "get_pad_positions",
        description:
          "Return every footprint pad's absolute board-frame (x, y), copper " +
          "layer, and net — the coordinates manual routing (add_trace / " +
          "add_via / add_via_array) needs so trace endpoints land exactly on " +
          "pads instead of being eyeballed from component centers. Read-only. " +
          "Optional `net` / `ref` filters narrow the result for targeted routing.",
        inputSchema: getPadPositionsSchema,
      },
      {
        name: "get_footprint",
        description:
          "Introspect ONE footprint's land pattern in BOTH the footprint-local " +
          "and board frames — origin, courtyard AABB, and every pad (with the " +
          "explicit rotation convention) — so connector/IC pad locations are " +
          "known exactly instead of render-and-guessed. Two modes: `ref` reads " +
          "a placed footprint (real transform + nets) from the session; " +
          "`footprint` resolves an id PRE-placement (pass `at`/`rotation`/" +
          "`side` to project a hypothetical placement). Read-only.",
        inputSchema: getFootprintSchema,
      },
      {
        name: "describe_pcb",
        description:
          "Inspect the session PCB as compact, structured data: board size + " +
          "outline, stackup (layer names + copper weights), net classes / " +
          "design rules, zones (net/layer/bbox/fill), trace & via counts by net " +
          "and layer, component count, the current DRC status, and an " +
          "exportability/renderability probe that actually serializes the board " +
          "for fab + 3D preview — surfacing the 'DRC-clean but unexportable' " +
          "state get_document/read can't see. Read-only.",
        inputSchema: describePcbSchema,
      },
      {
        name: "add_trace",
        description:
          "Lay an explicit copper trace: a polyline of segments on a layer, " +
          "assigned to a net. The general-purpose routing primitive — use it " +
          "for coil interconnect, buses, and hand-routes that route_nets " +
          "(pad-driven) won't make. Mutates the session document.",
        inputSchema: addTraceSchema,
      },
      {
        name: "add_via",
        description:
          "Drop a via at a point connecting two layers on a net (defaults " +
          "FCu→BCu, diameter/drill from design rules). Pairs with add_trace " +
          "for multi-layer routing. Mutates the session document.",
        inputSchema: addViaSchema,
      },
      {
        name: "set_stackup",
        description:
          "Set the board stackup copper weight (e.g. copper_oz: 2) and/or " +
          "per-layer thickness/material, so DC-resistance and impedance " +
          "estimates reflect the real fab stackup instead of a default 1 oz. " +
          "Mutates the session document.",
        inputSchema: setStackupSchema,
      },
      {
        name: "set_placement",
        description:
          "Place footprints at explicit board-frame coordinates by ref — the " +
          "floorplan realizer the auto-placer (grid/force_directed/radial) can't " +
          "express: thermal rings, a quiet IMU corner, rim connectors. Batch; " +
          "sets position/rotation/side and warns on off-board, in-cutout, or " +
          "stacked landings. Mutates the session document. Returns the updated " +
          "`placement_drc` (same shape as place_components) so a move can be " +
          "re-checked in one call without running run_drc.",
        inputSchema: setPlacementSchema,
      },
      {
        name: "set_board_outline",
        description:
          "Resize or reshape the board outline in place — rectangle " +
          "(board_width/height), circle/annulus (board_shape), or any polygon " +
          "(outline) — WITHOUT re-placing components, traces, vias, or zones. " +
          "Unlike re-running place_components, the floorplan is preserved; any " +
          "footprint whose origin ends up off the new board is reported in " +
          "`off_board` rather than silently relocated. Mutates the session document.",
        inputSchema: setBoardOutlineSchema,
        _meta: UI_META,
      },
      {
        name: "add_zone",
        description:
          "Add a copper pour (ground/power plane) on a net+layer — fills are not " +
          "traces. `fill_board:true` pours the whole outline (cutouts become " +
          "voids); or give an explicit polygon for a partial plane. Mutates the " +
          "session document.",
        inputSchema: addZoneSchema,
      },
      {
        name: "delete_zone",
        description:
          "Remove a copper pour from the board — the take-back for a bad add_zone, " +
          "without rebuilding the session. Target by `index` (0-based, the add " +
          "order) or by `net`/`layer` when exactly one zone matches. Returns a " +
          "`changed` diff of what was removed. To undo the very last mutation of " +
          "any kind, use `undo` instead. Mutates the session document.",
        inputSchema: deleteZoneSchema,
      },
      {
        name: "delete_trace",
        description:
          "Remove a single routed trace segment by `index` (0-based, the add " +
          "order) or by an unambiguous `net`/`layer` match. The take-back for a " +
          "stray add_trace. Returns a `changed` diff. Mutates the session document.",
        inputSchema: deleteTraceSchema,
      },
      {
        name: "delete_via",
        description:
          "Remove a single via by `index` (0-based, the add order) or by an " +
          "unambiguous `net` match. The take-back for a stray add_via. Returns a " +
          "`changed` diff. Mutates the session document.",
        inputSchema: deleteViaSchema,
      },
      {
        name: "undo",
        description:
          "Rewind the most recent mutation on a session — the snapshot taken " +
          "before the last add_zone / add_trace / add_via / delete_* / route_nets " +
          "/ place_components (or a CAD create/update/delete) is restored, without " +
          "re-sending the document. Repeated calls walk further back. Returns a " +
          "`changed` diff of the board elements the rewind moved.",
        inputSchema: undoSchema,
      },
      {
        name: "set_design_rules",
        description:
          "Set the board design rules run_drc enforces (clearance, track width, " +
          "via, edge/hole/annular) and net classes — the way to give a power or " +
          "high-voltage class wider clearance than signal nets. run_drc already " +
          "reads pcb.rules; this writes them. Mutates the session document.",
        inputSchema: setDesignRulesSchema,
      },
      {
        name: "size_trace_for_current",
        description:
          "IPC-2221 conductor ampacity solved for trace width: given current, " +
          "copper weight, allowed temp rise, and layer (outer/inner), returns the " +
          "minimum width. The ampacity sibling of size_impedance/size_pdn — pure " +
          "calc, no document.",
        inputSchema: sizeTraceForCurrentSchema,
      },
      {
        name: "add_via_array",
        description:
          "Place many vias at once — a grid over a rectangular `region` (thermal " +
          "vias under FETs, GND-plane stitching) or an explicit `points` list. " +
          "Grid vias are clipped to the board outline by default. Mutates the " +
          "session document.",
        inputSchema: addViaArraySchema,
      },
      {
        name: "add_motor_winding",
        description:
          "One-shot motor winding realizer: plans a balanced slots/poles/" +
          "phases winding, drops a spiral coil per tooth with correct phase + " +
          "polarity, series-connects each phase, and ties the wye/delta " +
          "termination as a net-tie — closing the winding_layout plan into " +
          "actual copper. Mutates the session document.",
        inputSchema: addMotorWindingSchema,
      },
      {
        name: "calc_motor",
        description:
          "Evaluate motor performance AS DATA: torque constant Kt, back-EMF " +
          "constant Ke, no-load speed, stall torque, and a speed–torque " +
          "curve. Supply air-gap flux directly or compute it from magnet " +
          "geometry via the first-order MEC field model. Pure: no board, no " +
          "mutation. First-order steady state (no slotting/fringing/losses).",
        inputSchema: calcMotorSchema,
      },
      {
        name: "render_pcb",
        description:
          "Render a flat, top-down, per-layer 2D image of a PCB (copper, silk, " +
          "drills, outline) — agent eyes for boards. Pick `layers` (e.g. " +
          "[\"F.Cu\", \"F.SilkS\", \"Edge_Cuts\"]); returns a PNG. Complements " +
          "the isometric render_view and numeric run_drc.",
        inputSchema: renderPcbSchema,
      },
      {
        name: "render_ratsnest",
        description:
          "Render the board with its unrouted-connection ratsnest (per-net MST " +
          "airwires) overlaid as dashed lines — judge placement quality and " +
          "crossing density BEFORE routing. Returns a PNG plus the airwire " +
          "(unconnected-pair) count.",
        inputSchema: renderRatsnestSchema,
      },
      {
        name: "render_stackup",
        description:
          "Render each copper layer of a multilayer board to its own image " +
          "(with the board edge for framing), so inner planes are legible " +
          "instead of buried under an all-layers composite. Returns one image " +
          "per layer plus a layer→image index.",
        inputSchema: renderStackupSchema,
      },
      {
        name: "run_drc",
        description:
          "Run Design Rule Check (DRC) on a PCB. Checks clearance, trace width, " +
          "drill size, annular ring, hole-to-hole, and edge clearance. Every " +
          "violation is tagged with `provenance` (intra_footprint / " +
          "inter_component / routing) and `generated` (involves a synthesized " +
          "footprint land pattern); the summary adds `byProvenance`, " +
          "`generatedArtifacts`, and `realViolations` so the headline count " +
          "excludes footprint artifacts without hand-triage.",
        inputSchema: runDrcSchema,
      },
      {
        name: "search_electronic_parts",
        description:
          "Spec-search the generative parts catalog (offline). A query like " +
          "'10k 0603 1%' parses to value+package+tolerance and returns the best " +
          "match plus E-series neighbours, each with a generated footprint, symbol, " +
          "and 3D body. A part is family+value+package, not a scraped row.",
        inputSchema: searchElectronicPartsSchema,
      },
      {
        name: "resolve_part",
        description:
          "Resolve a spec query (e.g. '10k 0603 1%') into ONE fully-specified part: " +
          "E-series-snapped value plus a generated footprint + schematic symbol + 3D " +
          "body (one parametric source of truth) and any MPN cross-references.",
        inputSchema: resolvePartSchema,
      },
      {
        name: "find_alternatives",
        description:
          "Propose spec-compatible substitutes for the part a query resolves to. " +
          "Each alternative keeps the value, varies the package, and is labelled " +
          "identical / needs-reroute / incompatible by re-deriving its footprint.",
        inputSchema: findAlternativesSchema,
      },
      {
        name: "verify_substitution",
        description:
          "PROVE a part swap on the session PCB: replace `reference` with the part " +
          "`candidate` resolves to, re-derive its footprint, re-place at the same " +
          "anchor, re-run DRC (incl. connectivity), and return the before/after " +
          "violation delta with a `drop_in` verdict. An alternative is only drop-in " +
          "when it adds no new violations and preserves pin numbering.",
        inputSchema: verifySubstitutionSchema,
      },
      {
        name: "build_receipt",
        description:
          "Build a re-runnable verification Receipt for the session PCB: a content " +
          "hash, the DRC backend, a canonicalized DRC summary, and per-part " +
          "provenance — a durable proof that round-trips and re-verifies later as " +
          "Holds / Stale / Violated. Renders as an audit ledger in the inline viewer.",
        inputSchema: buildReceiptSchema,
      },
      {
        name: "verify_receipt",
        description:
          "Re-run a prior Receipt (from build_receipt) against the session's current " +
          "board and return the verdict — Holds (same board, clean), Stale (board " +
          "changed), or Violated. Powers the ledger's Re-run button.",
        inputSchema: verifyReceiptSchema,
      },
      {
        name: "route_diff_pair",
        description:
          "Route a declared differential pair (net_p/net_n) coupled and length-matched, " +
          "using the pair's diff-pair net-class gap and width. Routes straight (best on a " +
          "clear channel); verify with run_drc / critique_route afterwards.",
        inputSchema: routeDiffPairSchema,
      },
      {
        name: "critique_route",
        description:
          "Audit one net's routing without changing anything: total length, via/" +
          "layer-change count, the closest approach to other-net copper, and any " +
          "clearance/short/unconnected DRC issues it's in. Inspect a route before trusting it.",
        inputSchema: critiqueRouteSchema,
      },
      {
        name: "run_erc",
        description:
          "Run Electrical Rule Check (ERC) on a schematic. " +
          "Checks for duplicate references, unconnected pins, and pin type conflicts.",
        inputSchema: runErcSchema,
      },
      {
        name: "export_gerber",
        description:
          "Export Gerber RS-274X fabrication files from a PCB design. " +
          "Generates copper layer files, drill file, pick-and-place CSV, and BOM. " +
          "Gated on a clean DRC by default (require_clean_drc) — a dirty or " +
          "unverifiable board is BLOCKED with its DRC summary instead of emitting " +
          "an invalid bundle. Run validate_for_fab first for the full readiness verdict.",
        inputSchema: exportGerberSchema,
      },
      {
        name: "export_kicad",
        description:
          "Export the session as a native, editable KiCad 9 file. " +
          "filename ending in .kicad_pcb writes the board (footprints, pads, nets, " +
          "traces, vias, zones, layers, outline); .kicad_sch writes the schematic. " +
          "Unlike export_gerber (fab-only output), this round-trips: a human can open " +
          "it in KiCad to finish routing nets the autorouter couldn't close, then " +
          "re-import. Large files respect the inline byte cap (use output_dir for those).",
        inputSchema: exportKicadSchema,
      },
      {
        name: "validate_for_fab",
        description:
          "The single 'is this board ready to fabricate?' oracle. Runs the whole " +
          "readiness gate in one call and returns ONE structured verdict: DRC " +
          "(fail-closed — a board that won't parse is 'unverifiable', never clean), " +
          "renderability, Gerber-exportability (attempts serialization; names the " +
          "exact failing field when it can't), unsupported features, the precise " +
          "blockers, and suggested fixes. Read-only. Use before export_gerber / " +
          "quote_manufacturing to know — not guess — whether the board is shippable.",
        inputSchema: validateForFabSchema,
      },
      {
        name: "calc_impedance",
        description:
          "Calculate trace impedance using IPC-2141 formulas. " +
          "Supports microstrip, stripline, and differential pair configurations. " +
          "Returns Z0, effective Er, and propagation delay. Pass document_id + " +
          "net to gate the number on realized copper: an impedance for a trace " +
          "that isn't actually routed/continuous is blocked, not reported.",
        inputSchema: calcImpedanceSchema,
      },
      {
        name: "size_impedance",
        description:
          "Inverse of calc_impedance: solve trace geometry for a TARGET impedance. " +
          "Given a target Z0 (and diff Z0 for pairs) + stackup, returns the trace " +
          "width (and spacing) AS DATA, snapped to the fab grid and re-verified " +
          "against the same model. Reports a binding DFM min-width/spacing bound " +
          "and whether the target is reachable — it will not silently hand back a " +
          "width that misses spec. Pure: no board, no mutation.",
        inputSchema: sizeImpedanceSchema,
      },
      {
        name: "size_pdn",
        description:
          "Size copper-segment widths across a power-distribution resistor mesh " +
          "so each load node's IR-drop meets its budget with minimal copper. " +
          "Solves G·V=I for node voltages and drives drop→budget with a bounded " +
          "gradient tuner; returns per-segment widths AS DATA with drops " +
          "recomputed from a forward solve, and flags any node it can't meet " +
          "within the width bounds. Pure by default; pass document_id + net to " +
          "REFUSE a PASS when that power plane isn't galvanically continuous " +
          "(returns coverage %, stitching-via count, and the worst island).",
        inputSchema: sizePdnSchema,
      },
      {
        name: "calc_coil",
        description:
          "Analyze a planar spiral coil: inductance (modified Wheeler), DC " +
          "resistance, copper length, and L/R time constant. The analyzer for " +
          "the planar-magnetics archetype (inductors, sensor coils, motor " +
          "stators). Pure.",
        inputSchema: calcCoilSchema,
      },
      {
        name: "size_coil",
        description:
          "Inverse of calc_coil: solve the turn count for a target inductance " +
          "in a given annulus (Wheeler L ∝ turns², so it's closed-form). Reports " +
          "continuous + integer turns, the inductance achieved, and whether that " +
          "many turns fit the radial band (else fit-limited). Pure.",
        inputSchema: sizeCoilSchema,
      },
      {
        name: "calc_rf",
        description:
          "Frequency-domain (AC) analysis of an RLC resonator: sweeps complex " +
          "impedance over frequency and reports |Z|, phase, and S11/return-loss " +
          "vs a reference Z0, plus resonance, Q, and the best match in the band. " +
          "The RF/AC analyzer (calc_impedance is geometry-only). Pure.",
        inputSchema: calcRfSchema,
      },
    ].filter((t) => !disabledTools.has(t.name))),
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

  type ToolResult = {
    content: Array<{ type: string; text: string; annotations?: unknown }>;
    structuredContent?: Record<string, unknown>;
    isError?: boolean;
    _meta?: Record<string, unknown>;
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
      const disabledResult = {
        content: [
          {
            type: "text",
            text: `Tool '${name}' belongs to a pack disabled by VCAD_MCP_PACKS. Enable its pack to use it.`,
          },
        ],
        isError: true,
      };
      fireToolAlert(name, args, disabledResult);
      await stampResultMeta(disabledResult);
      return disabledResult;
    }

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
    if (incomingId && name !== "undo" && isDocWriter(name) && documents.has(incomingId)) {
      recordHistorySnapshot(incomingId);
    }

    // Inner dispatch. Encloses BOTH the registry path and the switch so the
    // single persist site below covers every writer — the registry path used
    // to early-return straight out of the handler, skipping post-processing.
    const runTool = async (): Promise<ToolResult> => {
      let result: ToolResult;

      // Registry-driven dispatch: any kernel tool from
      // `commandRegistry.toAnthropicTools()` (minus the browser-only and
      // deferred sets in tools/registry-dispatch.ts) routes through the
      // shared planner + applyToolOutcome path. Falls through to the
      // preview block below so these mutations render in the inline viewer.
      if (dispatchableTools.has(name)) {
        result = dispatchRegistryTool(name, args);
        const docId = resolvePreviewDocumentId(name, result, args, engine);
        if (docId) {
          attachPreviewHandle(result, docId, name);
          slimPreviewForInlineUi(result, docId, name, clientHasInlineUi());
        }
        fireToolAlert(name, args, result);
        return result;
      }

      switch (name) {
        case "open_document":
          result = openDocument(args);
          break;

        case "get_document":
          result = getDocumentTool(args);
          break;

        case "close_document":
          result = closeDocument(args);
          break;

        case "search_parts":
          result = searchPartsTool(args, engine);
          break;

        case "place_part":
          result = placePartTool(args, engine);
          break;

        case "get_preview_glb":
          result = await getPreviewGlb(getSession(String(args.document_id ?? "")), engine);
          break;

        case "get_preview_version":
          result = getPreviewVersion(
            getSession(String(args.document_id ?? "")),
            String(args.document_id ?? ""),
          );
          break;

        case "create_cad_loon":
          result = createCadLoon(args, engine);
          break;

        case "export_cad":
          result = exportCad(args, engine);
          break;

        case "inspect_cad":
          result = inspectCad(args, engine);
          break;

        case "quote_manufacturing":
          result = await quoteManufacturing(args, engine, fabricateStore, context.user);
          break;

        case "get_order_status":
          result = await getOrderStatus(args, fabricateStore, context.user);
          break;

        case "list_orders":
          result = await listOrders(args, fabricateStore, context.user);
          break;

        case "authorize_spend":
          result = await authorizeSpend(args, fabricateStore, eventStore, context.user);
          break;

        case "place_order":
          result = await placeOrder(args, fabricateStore, eventStore, context.user);
          break;

        case "share_session":
          result = await shareSession(args, shareStore, context.user);
          break;

        case "unshare_session":
          result = await unshareSession(args, shareStore);
          break;

        case "render_view":
          // Image content blocks don't fit the text-only local result
          // type; the MCP SDK accepts them as-is.
          result = (await renderView(args)) as unknown as typeof result;
          break;

        case "verify_part":
          result = await verifyPart(args);
          break;

        case "list_eval_tasks":
          result = listEvalTasks(args);
          break;

        case "dfm_check":
          result = await dfmCheck(args, engine);
          break;

        case "dfm_explain":
          result = dfmExplain(args);
          break;

        case "dfm_suggest_fix":
          result = dfmSuggestFix(args);
          break;

        case "dfm_apply_fix":
          result = dfmApplyFix(args);
          break;

        case "sheet_metal_create":
          result = sheetMetalCreate(args, engine);
          break;

        case "sheet_metal_unfold":
          result = sheetMetalUnfold(args, engine);
          break;

        case "sheet_metal_check":
          result = sheetMetalCheck(args, engine);
          break;

        case "sheet_metal_materials":
          result = sheetMetalMaterials(args, engine);
          break;

        case "sheet_metal_bend_table":
          result = sheetMetalBendTable(args, engine);
          break;

        case "sheet_metal_cost":
          result = sheetMetalCost(args, engine);
          break;

        case "sheet_metal_suggest_fix":
          result = sheetMetalSuggestFix(args, engine);
          break;

        case "sheet_metal_sequence":
          result = sheetMetalSequence(args, engine);
          break;

        case "sheet_metal_nest":
          result = sheetMetalNest(args, engine);
          break;

        case "import_step":
          result = importStep(args, engine);
          break;

        case "import_kicad":
          result = (await importKicad(args)) as unknown as typeof result;
          break;

        case "import_eagle":
          result = importEagle(args) as unknown as typeof result;
          break;

        case "open_in_browser":
          result = openInBrowser(args);
          break;

        case "create_robot_env":
          result = await createRobotEnv(args);
          break;

        case "gym_step":
          result = gymStep(args);
          break;

        case "gym_reset":
          result = gymReset(args);
          break;

        case "gym_observe":
          result = gymObserve(args);
          break;

        case "gym_close":
          result = gymClose(args);
          break;

        case "load_structure":
          result = await loadStructure(args);
          break;

        case "inspect_molecule":
          result = await inspectMoleculeTool(args);
          break;

        case "minimize_energy":
          result = await minimizeEnergyTool(args);
          break;

        case "md_run":
          result = await mdRun(args);
          break;

        case "design_material":
          result = await designMaterial(args);
          break;

        case "homogenize_material":
          result = await homogenizeMaterialTool(args);
          break;

        case "render_molecule":
          result = await renderMolecule(args);
          break;

        case "record_simulation":
          result = (await recordSimulation(args)) as unknown as typeof result;
          break;

        case "batch_create_envs":
          result = await batchCreateEnvs(args);
          break;

        case "batch_step":
          result = batchStep(args);
          break;

        case "batch_reset":
          result = batchReset(args);
          break;

        case "get_changelog":
          result = getChangelog(args);
          break;

        case "create_schematic":
          result = await createSchematic(args);
          break;

        case "place_components":
          result = await placeComponents(args);
          break;

        case "route_nets":
          result = await routeNets(args);
          break;

        case "add_coil":
          result = addCoil(args);
          break;

        case "add_coil_array":
          result = addCoilArray(args);
          break;

        case "winding_layout":
          result = windingLayout(args);
          break;

        case "board_from_solid":
          result = boardFromSolid(args, engine);
          break;

        case "check_enclosure_fit":
          result = await checkEnclosureFit(args, engine);
          break;

        case "list_footprints":
          result = listFootprints(args);
          break;

        case "search_footprints":
          result = searchFootprints(args);
          break;

        case "get_pad_positions":
          result = getPadPositions(args);
          break;

        case "get_footprint":
          result = await getFootprint(args);
          break;

        case "describe_pcb":
          result = await describePcb(args);
          break;

        case "add_trace":
          result = addTrace(args);
          break;

        case "add_via":
          result = addVia(args);
          break;

        case "set_stackup":
          result = setStackup(args);
          break;

        case "set_placement":
          result = await setPlacement(args);
          break;

        case "set_board_outline":
          result = setBoardOutline(args);
          break;

        case "add_zone":
          result = addZone(args);
          break;

        case "delete_zone":
          result = deleteZone(args);
          break;

        case "delete_trace":
          result = deleteTrace(args);
          break;

        case "delete_via":
          result = deleteVia(args);
          break;

        case "undo":
          result = undo(args);
          break;

        case "set_design_rules":
          result = setDesignRules(args);
          break;

        case "size_trace_for_current":
          result = sizeTraceForCurrent(args);
          break;

        case "add_via_array":
          result = addViaArray(args);
          break;

        case "add_motor_winding":
          result = addMotorWinding(args);
          break;

        case "calc_motor":
          result = await calcMotor(args);
          break;

        case "render_pcb":
          result = (await renderPcb(args)) as unknown as typeof result;
          break;

        case "render_ratsnest":
          result = (await renderRatsnest(args)) as unknown as typeof result;
          break;

        case "render_stackup":
          result = (await renderStackup(args)) as unknown as typeof result;
          break;

        case "save_document":
          result = saveDocument(args);
          break;

        case "load_document":
          result = loadDocument(args);
          break;

        case "continue_document":
          result = await continueDocument(args, sessionStore);
          break;

        case "checkpoint_document":
          result = await checkpointDocument(args, sessionStore);
          break;

        case "branch_from":
          result = await branchFrom(args, sessionStore);
          break;

        case "server_info": {
          const buildInfo = getBuildInfo();
          // expected_build_sha/is_stale come from Edge Config (per-request,
          // TTL-cached): on a warm instance pinned behind a fresh deploy this
          // flips to is_stale:true so the agent knows to reconnect.
          const staleness = await getStaleness(buildInfo.build_sha);
          result = {
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
                  packs: process.env.VCAD_MCP_PACKS ?? "all",
                }),
              },
            ],
          };
          break;
        }

        case "run_drc":
          result = await runDrc(args);
          break;

        case "search_electronic_parts":
          result = await searchElectronicParts(args);
          break;

        case "resolve_part":
          result = await resolvePart(args);
          break;

        case "find_alternatives":
          result = await findAlternatives(args);
          break;

        case "verify_substitution":
          result = await verifySubstitution(args);
          break;

        case "build_receipt":
          result = await buildReceipt(args, engine);
          break;

        case "verify_receipt":
          result = await verifyReceipt(args);
          break;

        case "route_diff_pair":
          result = await routeDiffPair(args);
          break;

        case "critique_route":
          result = await critiqueRoute(args);
          break;

        case "run_erc":
          result = await runErc(args);
          break;

        case "export_gerber":
          result = await exportGerber(args);
          break;

        case "export_kicad":
          result = await exportKicad(args);
          break;

        case "validate_for_fab":
          result = await validateForFab(args);
          break;

        case "calc_impedance":
          result = await calcImpedance(args);
          break;

        case "size_impedance":
          result = sizeImpedance(args);
          break;

        case "size_pdn":
          result = await sizePdn(args);
          break;

        case "calc_coil":
          result = calcCoil(args);
          break;

        case "size_coil":
          result = sizeCoil(args);
          break;

        case "calc_rf":
          result = calcRf(args);
          break;

        default: {
          const unknownResult = {
            content: [{ type: "text", text: `Unknown tool: ${name}` }],
            isError: true,
          };
          fireToolAlert(name, args, unknownResult);
          return unknownResult;
        }
      }

      // ── MCP Apps: attach preview handle for geometry tools ──────
      // The viewer fetches the actual GLB via the app-only
      // `get_preview_glb` tool, so results stay lean for the model.
      if (uiTools.has(name) && result.content.length > 0 && !result.isError) {
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
      if (!result.isError && isDocWriter(name)) {
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
 * Attach the preview document id to a tool result: in structuredContent
 * (the spec path) and, when the result text doesn't already mention the
 * id, as a small JSON text block too. Cursor has known gaps forwarding
 * structuredContent to widgets, and the id is useful to agents anyway
 * (it opens the result up for follow-up mutations).
 */
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

/** Tools whose text body is a machine-parseable document that consumers
 *  JSON.parse verbatim — appending a handle block would corrupt it with
 *  trailing characters (this broke the mecheval harness's .vcad
 *  extraction). They get structuredContent only. */
const PURE_JSON_RESULT_TOOLS = new Set(["get_document"]);

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
 * attached to the result (`structuredContent.document_id` for uiTools, or a
 * `{ "document_id": … }` text block, which is how switch-path ECAD tools that
 * aren't uiTools — create_schematic, route_diff_pair, add_via_array, add_zone,
 * set_placement, set_design_rules — surface theirs). Every candidate is guarded
 * by `documents.has`, so only a live session is ever persisted. Deliberately
 * does NOT call resolvePreviewDocumentId, which registers a fresh session for
 * import_step / create_cad_loon and would double-mint here.
 */
/**
 * Build the payload for a `kernel` session_events row: the tool name, its args
 * (minus document_id), and the compact `changed` parts diff the registry path
 * already merged into the result. Capped so a fat call can't bloat the spine —
 * mirrors the >8KB result-slimming discipline; tool + changed (the cheap,
 * high-value parts) are always kept.
 */
function buildKernelEventPayload(
  name: string,
  args: Record<string, unknown>,
  result: { content: Array<{ type: string; text: string }> },
): Record<string, unknown> {
  const { document_id: _docId, ...rest } = args;
  void _docId;
  let changed: unknown;
  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as { changed?: unknown };
      if (parsed && parsed.changed !== undefined) {
        changed = parsed.changed;
        break;
      }
    } catch {
      // not JSON — skip
    }
  }
  const payload: Record<string, unknown> = { tool: name, args: rest };
  if (changed !== undefined) payload.changed = changed;
  try {
    if (JSON.stringify(payload).length > 8192) {
      payload.args = { _omitted: true };
    }
  } catch {
    payload.args = { _omitted: true };
  }
  return payload;
}

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
