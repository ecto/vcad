/**
 * Chat ⇄ MCP bridge: exposes the browser-safe MCP tool registry
 * (`@vcad/mcp/browser-tools`) to the in-app assistant, so the ChatSidebar
 * gains the long tail of kernel capability (inspect_cad, measure,
 * check_clearance, dfm_check, analyze_structure, topology_optimize, …)
 * from the SAME tool-definition source the MCP server advertises — no
 * hand-mirroring; a new pure-compute MCP tool appears here automatically.
 *
 * Execution model: each call registers a deep clone of the live document as
 * an MCP session, runs the real MCP handler against the app's WASM engine,
 * then routes any document mutation back through the store so it lands on
 * the normal CRDT undo/redo stack (`addFromIR` → `import_ir`). The clone
 * means an MCP handler can never mutate app state behind the store's back.
 */

import {
  useDocumentStore,
  useEngineStore,
  useUiStore,
  type ExecutionDisplay,
  type AnthropicTool,
} from "@vcad/core";
import type { Document } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import {
  browserToolDefs,
  runBrowserTool,
  documents as mcpSessions,
  registerSession,
} from "@vcad/mcp/browser-tools";
import type { ToolCall } from "@/lib/chat-api";

/**
 * MCP tools we do NOT surface in-app. Two reasons a tool lands here:
 * - It already exists on the app's own command registry under another name
 *   (the registry version routes through the store natively).
 * - It mutates the document *in place* in ways the generic add-parts
 *   write-back below can't translate into store mutations yet
 *   (dfm_apply_fix rewrites existing nodes; record_measurement and
 *   set_parameters write session state the app owns elsewhere).
 */
const EXCLUDED_IN_APP = new Set([
  "dfm_apply_fix",
  "record_measurement",
  "set_parameters",
]);

const activeDefs = browserToolDefs.filter((d) => !EXCLUDED_IN_APP.has(d.name));

/** Names of MCP tools callable from the in-app assistant. */
export const MCP_CHAT_TOOL_NAMES = new Set(activeDefs.map((d) => d.name));

export const MCP_TOOLS_SYSTEM_PROMPT_APPENDIX = `

## Analysis & verification tools

You also have the vcad solver/verification tools (inspect_cad, measure, check_clearance, dfm_check, analyze_structure, analyze_tolerance_stackup, solve_thermal, topology_optimize, predict_print, predict_physics, list_parameters, parameter_gradient). They operate on the current document automatically — never pass document_id or document arguments. Quantitative results from these tools are measured by the kernel; quote them with their verdicts (a claim can be pass, fail, or unverifiable — never present an unverifiable or predicted value as verified). topology_optimize adds its result to the document as a new part (undoable like any other edit).`;

/** Strip the session-plumbing arguments from an MCP inputSchema — the app
 *  injects the live document's session itself, the model never sees ids. */
function stripDocArgs(
  schema: Record<string, unknown>,
): Record<string, unknown> {
  const props = { ...((schema.properties as Record<string, unknown>) ?? {}) };
  delete props.document_id;
  delete props.document;
  const required = Array.isArray(schema.required)
    ? (schema.required as string[]).filter(
        (r) => r !== "document_id" && r !== "document",
      )
    : undefined;
  return {
    ...schema,
    properties: props,
    ...(required ? { required } : {}),
  };
}

/** The MCP tool surface as Anthropic tool descriptors for the chat turn. */
export function mcpChatTools(): AnthropicTool[] {
  return activeDefs.map((def) => ({
    name: def.name,
    description: def.description,
    input_schema: stripDocArgs(def.inputSchema),
  }));
}

// ---------------------------------------------------------------------------
// Result → display projection
// ---------------------------------------------------------------------------

/** Human labels for the compact field grid — everything else falls back to
 *  the raw JSON view in the tool card. */
function fieldsFromPayload(
  payload: Record<string, unknown>,
): Array<{ label: string; value: string }> {
  const fields: Array<{ label: string; value: string }> = [];
  for (const [key, value] of Object.entries(payload)) {
    if (fields.length >= 10) break;
    if (key === "success" || key === "document_id") continue;
    if (
      typeof value === "number" ||
      typeof value === "string" ||
      typeof value === "boolean"
    ) {
      fields.push({
        label: key.replace(/_/g, " "),
        value:
          typeof value === "number"
            ? Number.isInteger(value)
              ? String(value)
              : value.toFixed(3)
            : String(value),
      });
    }
  }
  return fields;
}

function displayFor(
  def: { name: string; title?: string },
  payload: Record<string, unknown>,
): ExecutionDisplay {
  return {
    summary: [{ type: "text", text: def.title ?? def.name }],
    fields: fieldsFromPayload(payload),
  };
}

// ---------------------------------------------------------------------------
// Write-back: land parts an MCP handler added on the store's undo stack
// ---------------------------------------------------------------------------

/**
 * Diff the session clone against the pre-call document and import any parts
 * the handler added (topology_optimize's frozen result mesh, for example)
 * via `addFromIR`, which routes through the CRDT engine's `import_ir` — a
 * tracked mutation, so ⌘Z undoes it exactly like a manual edit.
 */
function importAddedParts(before: Document, after: Document): number {
  const beforeNodes = new Set(Object.keys(before.nodes));
  const beforeRoots = new Set(before.roots.map((r) => String(r.root)));
  const newRoots = after.roots.filter((r) => !beforeRoots.has(String(r.root)));
  if (newRoots.length === 0) return 0;

  const mini = createDocument();
  for (const [id, node] of Object.entries(after.nodes)) {
    if (!beforeNodes.has(id)) mini.nodes[id] = node;
  }
  mini.roots = newRoots;
  useDocumentStore.getState().addFromIR(mini);
  return newRoots.length;
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

export interface McpToolExecution {
  result: string;
  status: "success" | "error";
  display?: ExecutionDisplay;
  duration?: number;
}

/** Execute one MCP browser tool against the live document + engine. */
export async function executeMcpChatTool(
  tool: ToolCall,
): Promise<McpToolExecution> {
  const started = performance.now();
  const engine = useEngineStore.getState().engine;
  if (!engine) {
    return {
      result: "Engine not ready yet — try again in a moment.",
      status: "error",
      duration: performance.now() - started,
    };
  }
  const def = activeDefs.find((d) => d.name === tool.name);
  if (!def) {
    return {
      result: `Unknown tool: ${tool.name}`,
      status: "error",
      duration: performance.now() - started,
    };
  }

  // Register a deep clone of the live doc as a throwaway MCP session. The
  // handler mutates the clone freely; anything it added flows back through
  // the store (undo-tracked) in importAddedParts below.
  const liveDoc = useDocumentStore.getState().document as unknown as Document;
  const before = JSON.parse(JSON.stringify(liveDoc)) as Document;
  const sessionDoc = JSON.parse(JSON.stringify(liveDoc)) as Document;
  const sessionId = registerSession(sessionDoc);

  try {
    const res = await runBrowserTool(
      tool.name,
      { ...tool.args, document_id: sessionId },
      engine,
    );
    const text = res.content?.[0]?.text ?? "";
    const duration = performance.now() - started;
    if (res.isError) {
      let message = text;
      try {
        const parsed = JSON.parse(text) as { error?: string };
        if (parsed.error) message = parsed.error;
      } catch {
        /* raw text error */
      }
      return { result: message, status: "error", duration };
    }

    let payload: Record<string, unknown> = {};
    try {
      payload = JSON.parse(text) as Record<string, unknown>;
    } catch {
      /* non-JSON success body — leave payload empty */
    }

    // Land added parts on the store's undo stack (topology_optimize et al.).
    if (def.behavior.writesDoc) {
      importAddedParts(before, sessionDoc);
    }

    // check_clearance: draw the min-distance witness line in the viewport.
    if (tool.name === "check_clearance") {
      const wp = (payload.worst_pair ?? null) as {
        point_a?: [number, number, number];
        point_b?: [number, number, number];
      } | null;
      if (wp?.point_a && wp?.point_b) {
        useUiStore.getState().setClearanceIndicator({
          pointA: wp.point_a,
          pointB: wp.point_b,
          distanceMm:
            typeof payload.measured_mm === "number" ? payload.measured_mm : 0,
          pass: payload.pass === true,
          ...(typeof payload.label === "string"
            ? { label: payload.label }
            : {}),
        });
      }
    }

    return {
      result: text,
      status: "success",
      display: displayFor(def, payload),
      duration,
    };
  } finally {
    mcpSessions.delete(sessionId);
  }
}
