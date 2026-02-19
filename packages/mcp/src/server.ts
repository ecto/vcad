/**
 * MCP server implementation with vcad tools.
 */

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { Engine } from "@vcad/engine";
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

export async function createServer(): Promise<Server> {
  // Initialize the WASM engine
  const engine = await Engine.init();

  const server = new Server(
    {
      name: "vcad",
      version: "0.1.0",
    },
    {
      capabilities: {
        tools: {},
      },
    },
  );

  // List available tools
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
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
      },
      {
        name: "open_in_browser",
        description:
          "Generate a shareable URL to open a CAD document in vcad.io. " +
          "Takes an IR document (JSON or compact format) and returns a URL that opens the document in the browser. " +
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

  // Handle tool calls
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args } = request.params;

    try {
      switch (name) {
        case "create_cad_document":
          return createCadDocument(args);

        case "create_cad_loon":
          return createCadLoon(args, engine);

        case "export_cad":
          return exportCad(args, engine);

        case "inspect_cad":
          return inspectCad(args, engine);

        case "import_step":
          return importStep(args, engine);

        case "open_in_browser":
          return openInBrowser(args);

        case "create_robot_env":
          return await createRobotEnv(args);

        case "gym_step":
          return gymStep(args);

        case "gym_reset":
          return gymReset(args);

        case "gym_observe":
          return gymObserve(args);

        case "gym_close":
          return gymClose(args);

        case "batch_create_envs":
          return await batchCreateEnvs(args);

        case "batch_step":
          return batchStep(args);

        case "batch_reset":
          return batchReset(args);

        case "get_changelog":
          return getChangelog(args);

        case "create_schematic":
          return createSchematic(args);

        case "place_components":
          return placeComponents(args);

        case "route_nets":
          return routeNets(args);

        case "run_drc":
          return runDrc(args);

        case "run_erc":
          return runErc(args);

        case "export_gerber":
          return exportGerber(args);

        case "calc_impedance":
          return calcImpedance(args);

        default:
          return {
            content: [{ type: "text", text: `Unknown tool: ${name}` }],
            isError: true,
          };
      }
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
