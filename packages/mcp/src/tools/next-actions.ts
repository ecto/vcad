/**
 * The error side of the verified agent loop: turn a failed tool call into a
 * recoverable one. Instead of returning a bare `Error: <message>` string that
 * leaves the agent guessing, every failure carries structured `next_actions` —
 * an ordered list of recovery steps, each optionally naming a tool (and
 * ready-to-run args) the agent can call to get unstuck in a single turn.
 *
 * Two entry points cover both failure conventions in the codebase:
 *   - buildErrorResult — for THROWN errors, used by the server's central catch
 *     (registry CRUD, planner errors, kernel traps, "Unknown document_id").
 *   - enrichErrorResult — for tools that RETURN {isError:true} instead of
 *     throwing (the whole ECAD / sheet-metal / DFM surface). The only
 *     intentional carve-outs are disabled-pack and unknown-tool results.
 *
 * Pure, deterministic, and dependency-light so it's unit-testable without
 * booting the server or the kernel.
 */

import { CREATE_PARAM_HINTS } from "./registry-dispatch.js";

/** One recovery step. */
export interface NextAction {
  /** Imperative instruction the agent can act on. */
  action: string;
  /** A tool to call to recover, when one applies. */
  tool?: string;
  /** Ready-to-run args for `tool`, when derivable from context. */
  args?: Record<string, unknown>;
}

/** An MCP error result with structured recovery attached. */
export interface McpErrorResult {
  content: Array<{ type: "text"; text: string }>;
  structuredContent: { error: string; next_actions: NextAction[] };
  isError: true;
}

const docIdOf = (args: Record<string, unknown>): string | undefined =>
  typeof args.document_id === "string" ? args.document_id : undefined;

/**
 * Map (tool, args, error message) to a small ordered list of recovery actions.
 * Ordered most-specific-first; the generic "inspect then retry" is the floor so
 * the agent always has somewhere to go.
 */
export function suggestNextActions(
  toolName: string,
  args: Record<string, unknown>,
  message: string,
  opts: { kernelTrap?: boolean } = {},
): NextAction[] {
  const docId = docIdOf(args);
  const lower = message.toLowerCase();

  // A kernel trap already reset the shared instance — retrying verbatim will
  // likely re-trap. Steer toward different inputs.
  if (opts.kernelTrap) {
    return [
      {
        action:
          "The kernel reset after a trap. Retry with simpler parameters (avoid degenerate or zero-size geometry, or split a chained boolean into steps). If it persists it's a kernel bug — report the document.",
      },
    ];
  }

  // The session isn't live (cold instance, wrong id, or never opened).
  if (lower.includes("unknown document_id")) {
    return [
      {
        action:
          "No live session for that id. Open a session (or re-open the saved document), then pass its id as document_id.",
        tool: "open_document",
      },
    ];
  }

  // Server-side misconfiguration — not recoverable from the client. Checked
  // before the create/update branch since this surfaces on those tools too.
  if (lower.includes("planner unavailable") || lower.includes("setwasm")) {
    return [
      {
        action:
          "Server misconfiguration (kernel planner not initialized). Not recoverable from the client — report it.",
      },
    ];
  }

  // PCB order-of-operations: the board has no schematic / no board yet. These
  // are the most common multi-step ECAD mistakes, and the recovery is exact.
  if (lower.includes("no schematic")) {
    return [
      {
        action:
          "This session has no schematic yet. Declare connectivity first with create_schematic, then retry.",
        tool: "create_schematic",
        ...(docId ? { args: { document_id: docId } } : {}),
      },
    ];
  }
  if (lower.includes("no pcb") || lower.includes("pcbboard") || lower.includes("no board")) {
    return [
      {
        action:
          "This session has no board yet. Create one with place_components (or board_from_solid for an existing solid), then retry.",
        tool: "place_components",
        ...(docId ? { args: { document_id: docId } } : {}),
      },
    ];
  }

  // A guessed catalog path that doesn't exist — list the library, don't `read`
  // the current document (which only knows already-placed parts).
  if (lower.includes("unknown part")) {
    return [
      {
        action: "List available catalog parts to get a valid id/path, then retry.",
        tool: "search_parts",
      },
    ];
  }

  // A bad or missing part reference — the fix is always to list the real ids.
  if (
    lower.includes("missing `part_id`") ||
    lower.includes("missing part_id") ||
    lower.includes("no part with id")
  ) {
    return [
      {
        action: "List the document's parts to get a valid part_id, then retry.",
        tool: "read",
        ...(docId ? { args: { document_id: docId } } : {}),
      },
    ];
  }

  // Malformed create/update params — surface the exact expected shape. `update`
  // carries no `type` (its schema is {node_id, params}), so the Type-Catalog
  // hint is create-only; an unmatched update falls through to the floor below,
  // which carries document_id and is more apt for a node edit.
  if (toolName === "create" || toolName === "update") {
    const type = String((args as { type?: unknown }).type ?? "").toLowerCase();
    const hint = CREATE_PARAM_HINTS[type];
    const actions: NextAction[] = [];
    if (hint) {
      actions.push({
        action: `Correct the params for "${type}" and retry: ${hint}`,
        tool: toolName,
      });
    } else if (toolName === "create") {
      actions.push({
        action: `Check the Type Catalog in this server's instructions for "${type || "this type"}" params, then retry.`,
        tool: "create",
      });
    }
    const miss = /missing field `([^`]+)`/.exec(message);
    if (miss) {
      actions.push({
        action: `Provide the required field "${miss[1]}".`,
        tool: toolName,
      });
    }
    // Booleans take numeric ids of pre-created children, not inline geometry.
    if (
      (type === "union" || type === "difference" || type === "intersection") &&
      lower.includes("node")
    ) {
      actions.push({
        action:
          "Create both child nodes first, then reference their numeric ids — inline child definitions aren't supported.",
      });
    }
    if (actions.length) return actions;
  }

  // Floor: inspect current state, then retry with corrected arguments.
  return [
    {
      action: "Inspect the current state, then retry with corrected arguments.",
      tool: "read",
      ...(docId ? { args: { document_id: docId } } : {}),
    },
  ];
}

/**
 * Build the MCP error result for a failed tool call: the human-readable error
 * line plus a machine-readable `next_actions:` JSON tail (so text-only clients
 * see recovery too) and the same actions in `structuredContent` for richer
 * hosts.
 */
export function buildErrorResult(
  toolName: string,
  args: Record<string, unknown>,
  message: string,
  opts: { kernelTrap?: boolean } = {},
): McpErrorResult {
  const next = suggestNextActions(toolName, args, message, opts);
  const head = opts.kernelTrap
    ? `Error: kernel trap during '${toolName}' (${message}). The kernel was reset; other documents are unaffected.`
    : `Error: ${message}`;
  const tail = next.length ? `\nnext_actions: ${JSON.stringify(next)}` : "";
  return {
    content: [{ type: "text", text: `${head}${tail}` }],
    structuredContent: { error: message, next_actions: next },
    isError: true,
  };
}

/** Shape of an already-formed tool result (mutated in place by enrich). */
interface MutableResult {
  content: Array<{ type: string; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

/** Errors that are intentionally NOT made recoverable — a disabled pack or an
 *  unknown tool aren't design mistakes the agent can retry its way out of. */
function isCarveOut(message: string): boolean {
  return /^unknown tool:/i.test(message) || /belongs to a pack/i.test(message);
}

/**
 * Attach `next_actions` to a tool result that RETURNED `{isError:true}` instead
 * of throwing. The whole ECAD / sheet-metal / DFM surface reports failures this
 * way (a normal success-path return), so without this they'd bypass the central
 * catch and carry no recovery — leaving the most multi-step, order-of-operations
 * -prone tools (e.g. "Document has no schematic" → create_schematic) unhelped.
 * Idempotent: a result already carrying next_actions (via buildErrorResult) is
 * left alone. Injects into the JSON body when the text is a JSON object, else
 * appends a parseable tail.
 */
export function enrichErrorResult(
  result: MutableResult,
  toolName: string,
  args: Record<string, unknown>,
): void {
  if (!result.isError) return;
  if (result.structuredContent && "next_actions" in result.structuredContent) {
    return;
  }
  const block = result.content.find((b) => b.type === "text");
  if (!block) return;

  // Recover the human message: ECAD returns "Error: <msg>" text; order.ts and
  // friends return a {"error": "<msg>"} JSON body.
  let message = block.text;
  try {
    const parsed = JSON.parse(block.text) as { error?: unknown };
    if (parsed && typeof parsed === "object" && typeof parsed.error === "string") {
      message = parsed.error;
    }
  } catch {
    // not JSON — use the raw text
  }
  if (isCarveOut(message)) return;

  const next = suggestNextActions(toolName, args, message);
  if (!next.length) return;

  try {
    const parsed = JSON.parse(block.text) as Record<string, unknown>;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      parsed.next_actions = next;
      block.text = JSON.stringify(parsed);
    } else {
      block.text = `${block.text}\nnext_actions: ${JSON.stringify(next)}`;
    }
  } catch {
    block.text = `${block.text}\nnext_actions: ${JSON.stringify(next)}`;
  }
  result.structuredContent = {
    ...(result.structuredContent ?? {}),
    next_actions: next,
  };
}
