/**
 * MCP server implementation with vcad tools.
 */

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { Engine, getKernelWasm } from "@vcad/engine";
import { commandRegistry } from "@vcad/core";
import type { Document } from "@vcad/ir";
import { exportCad, exportCadSchema } from "./tools/export.js";
import { inspectCad, inspectCadSchema } from "./tools/inspect.js";
import { importStep, importStepSchema } from "./tools/import.js";
import { openInBrowser, openInBrowserSchema } from "./tools/share.js";
import {
  openDocument,
  openDocumentSchema,
  getDocumentTool,
  getDocumentSchema,
  closeDocument,
  closeDocumentSchema,
  documents,
  registerSession,
  getSession,
} from "./tools/session.js";
import {
  registryToolDescriptors,
  registryDispatchableNames,
  dispatchRegistryTool,
} from "./tools/registry-dispatch.js";
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
  runDrc,
  runDrcSchema,
  runErc,
  runErcSchema,
  exportGerber,
  exportGerberSchema,
  calcImpedance,
  calcImpedanceSchema,
} from "./tools/ecad.js";
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
import { getPreviewGlb, getPreviewGlbSchema } from "./tools/preview.js";
import {
  getViewerHtml,
  VIEWER_RESOURCE_URI,
  VIEWER_CSP,
  MCP_APP_MIME_TYPE,
} from "./viewer.js";

/** Tools that produce or modify geometry and should show the 3D viewer.
 *  Registry-driven kernel tools (create, update, delete, …) are added
 *  dynamically in `createServer` once their names are known. */
const GEOMETRY_TOOLS = new Set([
  "create_cad_loon",
  "import_step",
  "open_document",
  "get_document",
  "place_part",
  "set_material",
  "dfm_apply_fix",
  "sheet_metal_create",
]);

/** MCP Apps UI metadata for geometry tools. */
const UI_META = {
  ui: {
    resourceUri: VIEWER_RESOURCE_URI,
  },
  // Flat key format also required by Claude Desktop MCP Apps protocol
  "ui/resourceUri": VIEWER_RESOURCE_URI,
};

export async function createServer(existingEngine?: Engine): Promise<Server> {
  // Initialize the WASM engine (or reuse one provided by the caller)
  const engine = existingEngine ?? await Engine.init();

  // Wire the kernel WASM's chat helpers into the shared commandRegistry so
  // `toAnthropicTools` and `planCrud` work on the server too. Same bootstrap
  // as `initEngineLifecycle` in @vcad/core, minus the docstore subscription
  // — we don't have a docstore here. Without this, registryToolDescriptors
  // returns the static-schemas fallback and planCrud returns null.
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

  const server = new Server(
    {
      name: "vcad",
      version: "0.1.0",
    },
    {
      capabilities: {
        tools: {},
        resources: {},
        // Acknowledge MCP Apps UI extension so Claude Desktop renders the viewer iframe.
        // The extension key is not in the typed ServerCapabilities schema so we spread as object.
        ...({ extensions: { "io.modelcontextprotocol/ui": { mimeTypes: [MCP_APP_MIME_TYPE] } } } as object),
      },
    },
  );

  // List available tools
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
      // ── Session lifecycle ──────────────────────────────────────
      {
        name: "open_document",
        description:
          "Open an editing session for a CAD document. Returns a `document_id` to pass to subsequent tool calls (create, update, place_part, inspect_cad, …). Pass an `initial` IR to begin editing an existing document; omit it for a fresh empty document.",
        inputSchema: openDocumentSchema,
        _meta: UI_META,
      },
      {
        name: "get_document",
        description:
          "Return the full IR Document JSON for an open session. Use after a series of mutations to capture the result, or to feed into `export_cad` / `open_in_browser`.",
        inputSchema: getDocumentSchema,
        _meta: UI_META,
      },
      {
        name: "close_document",
        description:
          "Close a document session and free its memory. Idempotent — closing an unknown id reports `closed: false`.",
        inputSchema: closeDocumentSchema,
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
        _meta: UI_META,
      },
      // ── Registry-driven kernel tools (auto-exposed) ───────────
      // The next block iterates `commandRegistry.toAnthropicTools()` so the
      // schema lives in one place — the kernel WASM. Same tools, same
      // behavior as the in-app chat surface; viewport-only tools (camera
      // and scene-evaluation tools) are filtered out via blocklists in
      // tools/registry-dispatch.ts. All of them mutate a session document,
      // so they all get the inline 3D viewer.
      ...registryToolDescriptors().map((t) => ({ ...t, _meta: UI_META })),
      // ── MCP Apps: app-only preview fetch ───────────────────────
      // visibility: ["app"] — spec-compliant hosts hide this from the
      // agent's tool list; only the viewer iframe calls it (via
      // app.callServerTool). Keeps multi-hundred-KB GLB payloads out of
      // model-visible tool results.
      {
        name: "get_preview_glb",
        description:
          "Return a base64 GLB preview of an open session document. Internal to the inline 3D viewer — agents should use `export_cad` for geometry exports.",
        inputSchema: getPreviewGlbSchema,
        _meta: {
          ui: {
            resourceUri: VIEWER_RESOURCE_URI,
            visibility: ["app"],
          },
        },
      },
      // ── Loon DSL one-shot ──────────────────────────────────────
      {
        name: "create_cad_loon",
        description:
          "Create a CAD document from loon source code. Loon is a Lisp-like language for parametric CAD.\n\n" +
          "Primitives: [cube x y z], [cylinder r h], [sphere r], [cone r-bottom r-top h]\n" +
          "Booleans (subject-last): [difference tool subject], [union other subject]\n" +
          "Transforms (subject-last): [translate x y z s], [rotate rx ry rz s], [scale sx sy sz s]\n" +
          "Features: [fillet r s], [chamfer d s], [shell t s]\n" +
          "Pipe: [pipe [cube 50 30 5] [difference [cylinder 3 10]] [fillet 1.0]]\n" +
          "Let bindings: [let body [cube 50 30 5]]\n" +
          "Scene: [root solid \"material-name\"]",
        inputSchema: createCadLoonSchema,
        _meta: UI_META,
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
      // ── DFM (Design for Manufacturing) ──────────────────────────
      {
        name: "dfm_check",
        description:
          "Run Design-for-Manufacturing checks against an open session document for a chosen process (cnc_3axis, fdm, sla, injection, sheet_metal, casting_sand, casting_investment). Returns a structured report with severities, measurements, face references, and suggested fixes. Each rule's threshold is sourced from a TOML pack at lib/dfm/<process>.toml — pass `rule_pack_toml` to override.",
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
        _meta: UI_META,
      },
      // ── Sheet metal (AI-native manufacturability surface) ───────
      {
        name: "sheet_metal_create",
        description:
          "Create a sheet-metal part: a rectangular or polygon base flange plus an ordered chain of edge flanges, hems, and jogs. Supports `shop_profile` (e.g. \"sendcutsend\") to resolve bend radii/K-factors from the fab's published catalog, and `bend_relief` to cut relief notches at bend ends. Returns a `document_id` (usable with sheet_metal_unfold/check, inspect_cad, export_cad, open_in_browser), the panel/bend model summary, flat bbox + area, and DFM violations.",
        inputSchema: sheetMetalCreateSchema,
        _meta: UI_META,
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
        _meta: UI_META,
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
          "Create a schematic from component and wire definitions. " +
          "Returns a vcad document with schematic data that can be used for PCB layout.",
        inputSchema: createSchematicSchema,
      },
      {
        name: "place_components",
        description:
          "Place components on a PCB from schematic data. " +
          "Creates board outline, stackup, and positions footprints. " +
          "Requires a document with schematic data.",
        inputSchema: placeComponentsSchema,
      },
      {
        name: "route_nets",
        description:
          "Route electrical nets on a PCB with copper traces. " +
          "Connects pads belonging to the same net. " +
          "Requires a document with PCB and placed footprints.",
        inputSchema: routeNetsSchema,
      },
      {
        name: "run_drc",
        description:
          "Run Design Rule Check (DRC) on a PCB. " +
          "Checks clearance, trace width, drill size, annular ring, and edge clearance.",
        inputSchema: runDrcSchema,
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
          "Generates copper layer files, drill file, pick-and-place CSV, and BOM.",
        inputSchema: exportGerberSchema,
      },
      {
        name: "calc_impedance",
        description:
          "Calculate trace impedance using IPC-2141 formulas. " +
          "Supports microstrip, stripline, and differential pair configurations. " +
          "Returns Z0, effective Er, and propagation delay.",
        inputSchema: calcImpedanceSchema,
      },
    ],
  }));

  // ── MCP Apps: List UI resources ──────────────────────────────
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

  // Handle tool calls
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args = {} } = request.params;

    try {
      let result: {
        content: Array<{ type: string; text: string; annotations?: unknown }>;
        structuredContent?: Record<string, unknown>;
        isError?: boolean;
      };

      // Registry-driven dispatch: any kernel tool from
      // `commandRegistry.toAnthropicTools()` (minus the browser-only and
      // deferred sets in tools/registry-dispatch.ts) routes through the
      // shared planner + applyToolOutcome path. Falls through to the
      // preview block below so these mutations render in the inline viewer.
      if (dispatchableTools.has(name)) {
        result = dispatchRegistryTool(name, args);
        const docId = resolvePreviewDocumentId(name, result, args, engine);
        if (docId) {
          attachPreviewHandle(result, docId);
          slimPreviewForInlineUi(result, docId, name, clientHasInlineUi());
        }
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
          result = getPreviewGlb(getSession(String(args.document_id ?? "")), engine);
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
          result = createSchematic(args);
          break;

        case "place_components":
          result = await placeComponents(args);
          break;

        case "route_nets":
          result = await routeNets(args);
          break;

        case "run_drc":
          result = await runDrc(args);
          break;

        case "run_erc":
          result = runErc(args);
          break;

        case "export_gerber":
          result = await exportGerber(args);
          break;

        case "calc_impedance":
          result = calcImpedance(args);
          break;

        default:
          return {
            content: [{ type: "text", text: `Unknown tool: ${name}` }],
            isError: true,
          };
      }

      // ── MCP Apps: attach preview handle for geometry tools ──────
      // The viewer fetches the actual GLB via the app-only
      // `get_preview_glb` tool, so results stay lean for the model.
      if (uiTools.has(name) && result.content.length > 0 && !result.isError) {
        const docId = resolvePreviewDocumentId(name, result, args, engine);
        if (docId) {
          attachPreviewHandle(result, docId);
          slimPreviewForInlineUi(result, docId, name, clientHasInlineUi());
        }
      }

      return result;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return {
        content: [{ type: "text", text: `Error: ${message}` }],
        isError: true,
      };
    }
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
function slimPreviewForInlineUi(
  result: { content: Array<{ type: string; text: string }> },
  docId: string,
  toolName: string,
  clientHasInlineUi: boolean,
): void {
  if (!clientHasInlineUi) return;
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

function attachPreviewHandle(
  result: {
    content: Array<{ type: string; text: string; annotations?: unknown }>;
    structuredContent?: Record<string, unknown>;
  },
  docId: string,
): void {
  result.structuredContent = {
    ...result.structuredContent,
    document_id: docId,
  };
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
