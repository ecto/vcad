/**
 * Browser-safe MCP tool registry — the shared tool-definition source the web
 * app's ChatSidebar consumes so in-app chat gains the MCP tool surface
 * without hand-mirroring (issue #594).
 *
 * Every module imported here must be free of Node builtins all the way down:
 * the only session dependency allowed is `session-core.ts` (the browser-safe
 * split of `session.ts`). Server-only tools (export/import/order/share/verify,
 * anything touching fs or network) stay out; they remain reachable only via
 * the real MCP server.
 *
 * A new pure-compute tool appears in-app automatically once its module is
 * added to `BROWSER_TOOL_MODULES` — one line, no schema mirroring.
 */

import type { Engine } from "@vcad/engine";
import type { ToolDef, ToolContext } from "./tools/tool-def.js";
import type { ToolResult } from "./tools/tool-result.js";
import { TOOL_METADATA } from "./tools/tool-metadata.js";

import { toolDefs as inspectTools } from "./tools/inspect.js";
import { toolDefs as measureTools } from "./tools/measure.js";
import { toolDefs as facesTools } from "./tools/faces.js";
import { toolDefs as clearanceTools } from "./tools/clearance.js";
import { toolDefs as dfmTools } from "./tools/dfm.js";
import { toolDefs as toleranceTools } from "./tools/tolerance.js";
import { toolDefs as thermalTools } from "./tools/thermal.js";
import { toolDefs as structureTools } from "./tools/structure.js";
import { toolDefs as topoptTools } from "./tools/topopt.js";
import { toolDefs as parameterTools } from "./tools/parameters.js";
import { toolDefs as printCheckTools } from "./tools/print-check.js";
import { toolDefs as physicsTools } from "./tools/physics.js";

export {
  documents,
  registerSession,
  getSession,
  recordHistorySnapshot,
  undoLastSnapshot,
  getLastChanged,
} from "./tools/session-core.js";
export type { ToolDef, ToolContext } from "./tools/tool-def.js";
export type { ToolResult } from "./tools/tool-result.js";

/** Pure-compute tool modules whose defs are safe to run in the browser
 *  against the app's own WASM engine. Add a module here and every tool it
 *  defines appears in the in-app assistant automatically. */
const BROWSER_TOOL_MODULES: ToolDef[][] = [
  inspectTools,
  measureTools,
  facesTools,
  clearanceTools,
  dfmTools,
  toleranceTools,
  thermalTools,
  structureTools,
  topoptTools,
  parameterTools,
  printCheckTools,
  physicsTools,
];

/** All browser-runnable MCP tool defs, with display titles merged from the
 *  central metadata table (same source the MCP server advertises). */
export const browserToolDefs: ToolDef[] = BROWSER_TOOL_MODULES.flat().map(
  (def) => {
    const meta = TOOL_METADATA[def.name];
    return meta
      ? { ...def, title: meta.title, annotations: meta.annotations }
      : def;
  },
);

const byName = new Map(browserToolDefs.map((d) => [d.name, d]));

/** Look up a browser tool def by name, or undefined. */
export function getBrowserTool(name: string): ToolDef | undefined {
  return byName.get(name);
}

/** An Anthropic Messages-API tool descriptor (the shape the app's /api/chat
 *  proxy forwards). */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

/** The browser tool surface as Anthropic tool descriptors, ready to merge
 *  with the app's commandRegistry tools. */
export function browserToolsToAnthropic(): AnthropicTool[] {
  return browserToolDefs.map((def) => ({
    name: def.name,
    description: def.description,
    input_schema: def.inputSchema,
  }));
}

/**
 * Run a browser tool by name against the given engine. The caller (the app's
 * chat bridge) is responsible for registering its live document as a session
 * first and passing the resulting `document_id` in `args`. Only `ctx.engine`
 * is populated — the pure-compute tools never touch the server-side stores,
 * which don't exist in the browser.
 */
export async function runBrowserTool(
  name: string,
  args: Record<string, unknown>,
  engine: Engine,
): Promise<ToolResult> {
  const def = byName.get(name);
  if (!def) {
    return {
      isError: true,
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown tool: ${name}` }) },
      ],
    };
  }
  // Pure-compute handlers read only ctx.engine; the store fields are
  // server-side services that have no browser counterpart.
  const ctx = { engine, user: null } as unknown as ToolContext;
  try {
    return await def.handler(args, ctx);
  } catch (e) {
    return {
      isError: true,
      content: [
        {
          type: "text",
          text: JSON.stringify({ error: e instanceof Error ? e.message : String(e) }),
        },
      ],
    };
  }
}
