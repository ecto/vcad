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
import { VALID_LAYERS } from "./pcb-validate.js";

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

  // PCB serde / layer-validation failures. These surface when a malformed
  // layer name (e.g. "F.Cu" instead of "FCu") poisons the board and the
  // kernel rejects it during render/export/DRC serialization.
  const pcbSerdeMatch = detectPcbSerdeError(message);
  if (pcbSerdeMatch) {
    const accepted = [...VALID_LAYERS];
    const actions: NextAction[] = [
      {
        action: pcbSerdeMatch.hint,
        tool: "set_stackup",
        ...(docId ? { args: { document_id: docId } } : {}),
      },
    ];
    if (pcbSerdeMatch.field) {
      actions.push({
        action: `Offending field: ${pcbSerdeMatch.field}. Value: "${pcbSerdeMatch.value ?? "unknown"}". Accepted layer names: ${accepted.join(", ")}.`,
      });
    }
    return actions;
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

/**
 * The success side of the same idea: emit `next_actions` on a SUCCEEDING tool so
 * the canonical PCB flow is discoverable from a good result, not only by tripping
 * over an ordering error (set_design_rules before the board exists, …). Keyed by
 * the tool that just succeeded → the ordered next steps. Returns `[]` for any
 * tool not on the happy path, so non-PCB tools are untouched.
 *
 *   create_schematic → place_components, then run_erc
 *   place_components → set_design_rules
 *   set_design_rules → save_document (checkpoint before add_zone), then route_nets
 *   route_nets       → run_drc + render_pcb (cross-check)
 */
export function happyPathNext(toolName: string, docId?: string): NextAction[] {
  const withDoc = (a: NextAction): NextAction =>
    docId ? { ...a, args: { document_id: docId } } : a;
  switch (toolName) {
    case "create_schematic":
      return [
        withDoc({
          action: "Place the components to create the board.",
          tool: "place_components",
        }),
        withDoc({
          action:
            "Check the schematic for electrical errors (unconnected pins, conflicting drivers).",
          tool: "run_erc",
        }),
      ];
    case "place_components":
      return [
        withDoc({
          action:
            "Set the design rules (clearances, net classes) before routing — power/HV nets want a wider class than signals.",
          tool: "set_design_rules",
        }),
      ];
    case "set_design_rules":
      return [
        withDoc({
          action:
            "Checkpoint the board with save_document before pouring copper zones (add_zone) — a zone fill is a heavy, hard-to-undo edit.",
          tool: "save_document",
        }),
        withDoc({ action: "Route the nets.", tool: "route_nets" }),
      ];
    case "route_nets":
      return [
        withDoc({
          action: "Run DRC to check the routed board against the design rules.",
          tool: "run_drc",
        }),
        withDoc({
          action: "Render the board to visually cross-check the layout.",
          tool: "render_pcb",
        }),
      ];
    default:
      return [];
  }
}

/**
 * Attach happy-path `next_actions` to a SUCCEEDING tool result (the mirror of
 * enrichErrorResult). No-op unless the tool is on the canonical PCB flow and the
 * result doesn't already carry next_actions (a buffered set_design_rules ships
 * its own place_components hint and must be left alone). The document_id is read
 * from the result body first — create_schematic mints the id server-side, so it
 * isn't in `args` — then falls back to the call args.
 */
export function enrichSuccessResult(
  result: MutableResult,
  toolName: string,
  args: Record<string, unknown>,
): void {
  if (result.isError) return;
  if (result.structuredContent && "next_actions" in result.structuredContent) return;

  const block = result.content.find((b) => b.type === "text");
  let docId = docIdOf(args);
  let parsedBody: Record<string, unknown> | undefined;
  if (block) {
    try {
      const parsed = JSON.parse(block.text) as Record<string, unknown>;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        parsedBody = parsed;
        if (typeof parsed.document_id === "string") docId = parsed.document_id;
      }
    } catch {
      // not a JSON body — append a parseable tail instead
    }
  }

  const next = happyPathNext(toolName, docId);
  if (!next.length) return;

  if (parsedBody && block) {
    parsedBody.next_actions = next;
    block.text = JSON.stringify(parsedBody);
  } else if (block) {
    block.text = `${block.text}\nnext_actions: ${JSON.stringify(next)}`;
  }
  result.structuredContent = {
    ...(result.structuredContent ?? {}),
    next_actions: next,
  };
}

/** Errors that should NOT get a generic recovery floor — either not recoverable
 *  (disabled pack / unknown tool / ordering off) or self-explanatory with their
 *  own instruction (a spend awaiting human approval must NOT be retried, and a
 *  generic "inspect then read" would be actively misleading). */
function isCarveOut(message: string): boolean {
  return (
    /^unknown tool:/i.test(message) ||
    /belongs to a pack/i.test(message) ||
    /ordering is disabled/i.test(message) ||
    /pending human approval/i.test(message)
  );
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

// ── PCB serde error detection ──────────────────────────────────────────────

interface PcbSerdeHint {
  hint: string;
  field?: string;
  value?: string;
}

/** @internal exported for tests */
export function detectPcbSerdeError(message: string): PcbSerdeHint | null {
  const lower = message.toLowerCase();

  // Rust serde: 'unknown variant `F.Cu`, expected one of ...'
  const unknownVariant = /unknown variant [`"]([^`"]+)[`"]/i.exec(message);
  if (unknownVariant && /layer|cu|silk|mask|paste|fab|crt|edge/i.test(unknownVariant[1]!)) {
    return {
      hint: `The board has a malformed layer name "${unknownVariant[1]}". Fix it with set_stackup (use serde names like "FCu", not dotted "F.Cu").`,
      field: "pcb.stackup.layers[].layer",
      value: unknownVariant[1],
    };
  }

  // Our validatePcb pre-flight: 'not a valid PcbLayer'
  if (lower.includes("not a valid pcblayer")) {
    const fieldMatch = /field[`": ]+([^`",]+)/i.exec(message);
    const valueMatch = /value[`": ]+([^`",]+)/i.exec(message);
    return {
      hint: `The board has an invalid layer name. Fix the stackup with set_stackup using valid serde names (e.g. "FCu", "BCu", "In1Cu").`,
      field: fieldMatch?.[1],
      value: valueMatch?.[1],
    };
  }

  // Kernel WASM serde wrapper: 'pcb json: ...'
  if (lower.startsWith("pcb json:") || lower.includes("pcb json:")) {
    const detail = message.replace(/^.*pcb json:\s*/i, "");
    return {
      hint: `The board failed kernel deserialization: ${detail}. The document is still safe — read it, fix the offending field, and retry.`,
    };
  }

  // Generic serde/JSON parse failures on PCB tools
  if (
    (lower.includes("deserialize") || lower.includes("invalid type") ||
     lower.includes("missing field")) &&
    (lower.includes("pcb") || lower.includes("layer") || lower.includes("stackup"))
  ) {
    return {
      hint: `The board contains invalid data that fails deserialization. Read the document, check stackup/layer fields for typos, and fix with set_stackup.`,
    };
  }

  return null;
}
