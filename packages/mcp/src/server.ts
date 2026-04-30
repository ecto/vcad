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
import { appendGlbPreview } from "./tools/preview.js";
import { buildTool, buildSchema } from "./tools/build.js";
import { readTool, readSchema } from "./tools/read.js";
import { queryTool, querySchema } from "./tools/query.js";
import { inspectV2, inspectV2Schema } from "./tools/inspect-v2.js";
import { exportV2, exportV2Schema } from "./tools/export-v2.js";
import { shareV2, shareV2Schema } from "./tools/share-v2.js";
import { importV2, importV2Schema } from "./tools/import-v2.js";
import { partsTool, partsV2Schema } from "./tools/parts-v2.js";
import { assemble, assembleSchema } from "./tools/assemble.js";
import { simulate, simulateSchema } from "./tools/simulate.js";
import { render, renderSchema } from "./tools/render.js";
import { drawing, drawingSchema } from "./tools/drawing.js";
import {
  schematicV2,
  schematicSchema,
  layoutV2,
  layoutSchema,
  routeV2,
  routeSchema,
  checkV2,
  checkSchema,
  gerberV2,
  gerberSchema,
  calcImpedanceV2,
  calcImpedanceSchemaV2,
} from "./tools/ecad-v2.js";
import {
  getViewerHtml,
  VIEWER_RESOURCE_URI,
  VIEWER_CSP,
  MCP_APP_MIME_TYPE,
} from "./viewer.js";

/** Tools that produce or modify geometry and should show the 3D viewer. */
const GEOMETRY_TOOLS = new Set([
  "create_cad_loon",
  "import_step",
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

  const server = new Server(
    {
      name: "vcad",
      version: "2.0.0-alpha",
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
      },
      {
        name: "get_document",
        description:
          "Return the full IR Document JSON for an open session. Use after a series of mutations to capture the result, or to feed into `export_cad` / `open_in_browser`.",
        inputSchema: getDocumentSchema,
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
      },
      // ── Registry-driven kernel tools (auto-exposed) ───────────
      // The next block iterates `commandRegistry.toAnthropicTools()` so the
      // schema lives in one place — the kernel WASM. Same tools, same
      // behavior as the in-app chat surface; viewport-only tools (camera
      // and scene-evaluation tools) are filtered out via blocklists in
      // tools/registry-dispatch.ts.
      ...registryToolDescriptors(),
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
          "Export a CAD document to a file. Supports STL (3D printing) and GLB (visualization) formats. Format is determined by file extension.",
        inputSchema: exportCadSchema,
      },
      {
        name: "inspect_cad",
        description:
          "Inspect an open session document to get aggregate geometry properties: volume, surface area, bounding box, center of mass, triangle count, and mass (if material density is known). For per-part inspection use the chat-surface `inspect_part` / `describe_scene` tools (deferred from this MCP surface in v1).",
        inputSchema: inspectCadSchema,
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
      // ── v2 surface (handle-based, universal envelope) ────────────
      {
        name: "build",
        description:
          "v2: Create or edit a CAD document via an ordered list of build ops. " +
          "Pass a `doc` handle to edit a prior result; omit for a fresh document. " +
          "Returns a new versioned handle plus a preview PNG and aggregate stats. " +
          "Phase-1 ops: primitives, booleans, transforms, fillet/chamfer/shell, " +
          "linear/circular patterns, mirror, set_material, rename, delete, set_parameter, raw_ir.",
        inputSchema: buildSchema,
        _meta: UI_META,
      },
      {
        name: "read",
        description:
          "v2: Hydrate a doc handle (or inline IR) to its full IR — JSON or VCode. " +
          "Use sparingly; the universal envelope already returns stats + preview on every build.",
        inputSchema: readSchema,
      },
      {
        name: "query",
        description:
          "v2: Read-only structured introspection over a doc handle. Queries: " +
          "`{ kind: 'tree' }`, `{ kind: 'list', of: 'parts'|'sketches'|'joints'|'instances'|'materials' }`, " +
          "`{ kind: 'find', name }`, `{ kind: 'parameters' }`, `{ kind: 'dependencies', of }`, `{ kind: 'node', id }`.",
        inputSchema: querySchema,
      },
      {
        name: "inspect",
        description:
          "v2: Aggregate + per-part geometry properties (volume, surface area, bbox, COM, mass) " +
          "over a doc handle. Returns the universal envelope.",
        inputSchema: inspectV2Schema,
      },
      {
        name: "export",
        description:
          "v2: Export a doc handle to STL or GLB. Default returns an embedded base64 resource; " +
          "pass `target: { path: '...' }` for local-mode disk writes (requires MCP_LOCAL=1).",
        inputSchema: exportV2Schema,
      },
      {
        name: "share",
        description:
          "v2: Generate a vcad.io URL for a doc handle. Returns the URL plus byte sizes; " +
          "warns when the URL exceeds ~2 KB.",
        inputSchema: shareV2Schema,
      },
      {
        name: "import",
        description:
          "v2: STEP/STL import → doc handle. Accepts an embedded base64 resource " +
          "(`{ kind: 'embedded', mime, data_base64 }`) or `{ path }` (local mode only). " +
          "Auto-detects format from path/mime/magic bytes when `kind` is omitted.",
        inputSchema: importV2Schema,
      },
      {
        name: "parts",
        description:
          "v2: Stdlib parts library. `mode: 'search'` returns matches; `mode: 'place'` inserts " +
          "the part into a doc handle (or fresh doc) and returns the updated handle.",
        inputSchema: partsV2Schema,
      },
      {
        name: "assemble",
        description:
          "v2: Add or modify part instances and joints in a doc. Supports revolute / prismatic / " +
          "cylindrical / ball / fixed joints. Returns AABB-overlap interference report.",
        inputSchema: assembleSchema,
        _meta: UI_META,
      },
      {
        name: "simulate",
        description:
          "v2: Stateless physics rollout over an assembly handle. Pass an action matrix and an " +
          "optional initial state; returns trajectory + observations + rewards. Mode: 'rollout' " +
          "runs all actions, 'step' runs a single step for closed-loop control.",
        inputSchema: simulateSchema,
      },
      {
        name: "render",
        description:
          "v2: Render the doc to a PNG. Quality tiers: draft, preview (default), high, max. " +
          "Phase-1 ships draft + preview via the embedded Lambert painter; high/max return " +
          "`raytracer_unavailable` until the kernel raytracer's WASM bindings land.",
        inputSchema: renderSchema,
      },
      {
        name: "drawing",
        description:
          "v2: Generate 2D drawing views (orthographic + iso) as SVG. Phase-1 emits triangle " +
          "edges from each requested view; sections, dimensions, and GD&T are Phase 6.",
        inputSchema: drawingSchema,
      },
      {
        name: "schematic",
        description:
          "v2: Create or edit a PCB schematic — components, wires, labels. Returns an updated handle.",
        inputSchema: schematicSchema,
      },
      {
        name: "layout",
        description:
          "v2: Place components on a PCB (board outline + stackup + footprints) for a doc handle.",
        inputSchema: layoutSchema,
      },
      {
        name: "route",
        description:
          "v2: Route copper traces on a PCB. Auto or constrained mode. Returns updated handle " +
          "plus stats (routed nets, total length, unrouted nets).",
        inputSchema: routeSchema,
      },
      {
        name: "check",
        description:
          "v2: Unified DRC + ERC. Returns severity-tagged findings (errors / warnings / info).",
        inputSchema: checkSchema,
      },
      {
        name: "gerber",
        description:
          "v2: Export Gerber RS-274X (or IPC-2581) fabrication bundle for a PCB doc handle.",
        inputSchema: gerberSchema,
      },
      {
        name: "calc_impedance_v2",
        description:
          "v2: IPC-2141 trace impedance calculator (microstrip / stripline / differential).",
        inputSchema: calcImpedanceSchemaV2,
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

  // Handle tool calls
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args = {} } = request.params;

    try {
      // Registry-driven dispatch: any kernel tool from
      // `commandRegistry.toAnthropicTools()` (minus the browser-only and
      // deferred sets in tools/registry-dispatch.ts) routes through the
      // shared planner + applyToolOutcome path.
      if (dispatchableTools.has(name)) {
        return dispatchRegistryTool(name, args);
      }

      let result: {
        content: Array<{ type: string; [k: string]: unknown }>;
        isError?: boolean;
        _meta?: Record<string, unknown>;
      };

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

        case "create_cad_loon":
          result = createCadLoon(args, engine);
          break;

        case "export_cad":
          result = exportCad(args, engine);
          break;

        case "inspect_cad":
          result = inspectCad(args, engine);
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
          result = placeComponents(args);
          break;

        case "route_nets":
          result = routeNets(args);
          break;

        case "run_drc":
          result = runDrc(args);
          break;

        case "run_erc":
          result = runErc(args);
          break;

        case "export_gerber":
          result = exportGerber(args);
          break;

        case "calc_impedance":
          result = calcImpedance(args);
          break;

        // ── v2 surface ──────────────────────────────────────────
        case "build":
          result = buildTool(args, engine);
          break;

        case "read":
          result = readTool(args, engine);
          break;

        case "query":
          result = queryTool(args, engine);
          break;

        case "inspect":
          result = inspectV2(args, engine);
          break;

        case "export":
          result = exportV2(args, engine);
          break;

        case "share":
          result = shareV2(args, engine);
          break;

        case "import":
          result = importV2(args, engine);
          break;

        case "parts":
          result = partsTool(args, engine);
          break;

        case "assemble":
          result = assemble(args, engine);
          break;

        case "simulate":
          result = await simulate(args, engine);
          break;

        case "render":
          result = render(args, engine);
          break;

        case "drawing":
          result = drawing(args, engine);
          break;

        case "schematic":
          result = schematicV2(args, engine);
          break;

        case "layout":
          result = layoutV2(args, engine);
          break;

        case "route":
          result = routeV2(args, engine);
          break;

        case "check":
          result = checkV2(args, engine);
          break;

        case "gerber":
          result = gerberV2(args, engine);
          break;

        case "calc_impedance_v2":
          result = calcImpedanceV2(args);
          break;

        default:
          return {
            content: [{ type: "text", text: `Unknown tool: ${name}` }],
            isError: true,
          };
      }

      // ── MCP Apps: Append GLB preview for geometry tools ──────
      if (GEOMETRY_TOOLS.has(name) && result.content.length > 0) {
        const irDoc = extractIrDocument(name, result, args, engine);
        if (irDoc) {
          appendGlbPreview(result, irDoc, engine);
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
 * Extract an IR Document from a tool result for GLB preview generation.
 *
 * Strategy per tool:
 * - create_cad_loon: re-evaluate loon source via engine
 * - import_step: parse JSON result to extract the document field
 */
function extractIrDocument(
  toolName: string,
  result: { content: Array<{ type: string; text: string }> },
  args: Record<string, unknown>,
  engine: Engine,
): Document | null {
  try {
    const text = result.content[0]?.text;
    if (!text) return null;

    // Try parsing the text as JSON first (works for JSON format & import_step)
    try {
      const parsed = JSON.parse(text);

      // import_step wraps the document in { document, summary }
      if (parsed.document && parsed.document.version) {
        return parsed.document as Document;
      }

      // Direct IR document (JSON format output)
      if (parsed.version && parsed.nodes) {
        return parsed as Document;
      }
    } catch {
      // Not JSON — likely VCode format, handle below
    }

    // For create_cad_loon with VCode output, re-evaluate with JSON format
    if (toolName === "create_cad_loon" && args.source) {
      const jsonResult = createCadLoon({ ...args, format: "json" }, engine);
      const jsonText = jsonResult.content[0]?.text;
      if (jsonText) {
        const doc = JSON.parse(jsonText);
        if (doc.version && doc.nodes) return doc as Document;
      }
    }

    return null;
  } catch {
    return null;
  }
}
