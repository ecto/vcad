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
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { createCadDocument, createCadDocumentSchema } from "./tools/create.js";
import { exportCad, exportCadSchema } from "./tools/export.js";
import { inspectCad, inspectCadSchema } from "./tools/inspect.js";
import { importStep, importStepSchema } from "./tools/import.js";
import { openInBrowser, openInBrowserSchema } from "./tools/share.js";
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
import {
  getViewerHtml,
  VIEWER_RESOURCE_URI,
  VIEWER_CSP,
  MCP_APP_MIME_TYPE,
} from "./viewer.js";

/** Tools that produce or modify geometry and should show the 3D viewer. */
const GEOMETRY_TOOLS = new Set([
  "create_cad_document",
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
      {
        name: "search_parts",
        description:
          "Search the stdlib parts library (fasteners, bearings, …). Matches across name, category, synonyms, and catalog part numbers (McMaster / ISO / DIN). Returns an array of {id, name, category, params, xrefs, synonyms} — use `id` with `place_part`. Part numbers like '91290A320' or 'ISO 4762' match directly.",
        inputSchema: searchPartsSchema,
      },
      {
        name: "place_part",
        description:
          "Insert a stdlib part into a document. Takes the part `path` (from `search_parts.id`) and an optional `params` map; missing params use declared defaults. Returns the updated document JSON. The part remains parametric — end users can edit its params from the feature tree.",
        inputSchema: placePartSchema,
      },
      {
        name: "create_cad_document",
        description:
          "Create a CAD document from structured geometry input. Returns an IR document that can be exported or inspected.\n\n" +
          "Part types (use ONE per part):\n" +
          "- primitive: Basic shapes (cube, cylinder, sphere, cone)\n" +
          "- extrude: Sketch (rectangle/circle/polygon) extruded to solid\n" +
          "- revolve: Sketch revolved around an axis\n" +
          "- sweep: Sketch swept along a path (line or helix)\n" +
          "- loft: Interpolate between multiple sketches\n\n" +
          "Operations: union, difference, intersection, translate, rotate, scale, " +
          "linear_pattern, circular_pattern, hole, fillet, chamfer, shell\n\n" +
          "Positioning: absolute {x,y,z}, named ('center', 'top-center'), percentage {x:'50%'}\n\n" +
          "Assembly: Optional 'assembly' block with instances and joints for physics simulation.",
        inputSchema: createCadDocumentSchema,
        _meta: UI_META,
      },
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
          "Inspect a CAD document to get geometry properties: volume, surface area, bounding box, center of mass, triangle count, and mass (if material density is known).",
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
      let result: { content: Array<{ type: string; text: string; annotations?: unknown }> };

      switch (name) {
        case "search_parts":
          result = searchPartsTool(args, engine);
          break;

        case "place_part":
          result = placePartTool(args, engine);
          break;

        case "create_cad_document":
          result = createCadDocument(args);
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
 * - create_cad_document: re-run with format: "json" to get parseable output
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

    // For create_cad_document with VCode output, re-run with JSON format
    if (toolName === "create_cad_document" && args.parts) {
      const jsonResult = createCadDocument({ ...args, format: "json" });
      const jsonText = jsonResult.content[0]?.text;
      if (jsonText) {
        const doc = JSON.parse(jsonText);
        if (doc.version && doc.nodes) return doc as Document;
      }
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
