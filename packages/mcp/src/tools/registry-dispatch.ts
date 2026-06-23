/**
 * Generic dispatcher for the kernel-tier chat tools (the ones in
 * `commandRegistry.toAnthropicTools()`). The MCP server registers these
 * under their original names — the schema is identical to what the in-app
 * chat sends to Anthropic — and routes each call through the Rust planner
 * (`commandRegistry.planCrud`) and `applyToolOutcome`.
 *
 * Result: schema and behavior come from the kernel; the MCP wrapper is a
 * thin session-aware shell. No tool definitions duplicated in this package.
 */

import type { AnthropicTool } from "@vcad/core";
import {
  commandRegistry,
  applyToolOutcome,
  listPartsFromDocument,
} from "@vcad/core";
import { getSession } from "./session.js";

/**
 * Tools whose execution depends on browser-only state (camera, viewport
 * canvas, app DOM). The MCP surface filters these out so external agents
 * don't see things they can't do anything with.
 */
const BROWSER_ONLY_TOOLS = new Set<string>([
  "focus_part",
  "frame_all",
  "set_view",
  "screenshot_viewport",
]);

/**
 * Tools the registry exposes but whose execution path goes through
 * `executeCrudInner` (TS, app-only) rather than the Rust planner —
 * including ones that need an evaluated scene (`inspect_part`,
 * `describe_scene`, `place`) or the docstore-shaped feature builder
 * (`tube`, `polyline_tube`, `linear_pattern`, `circular_pattern`,
 * `mirror`). Skipping for v1 — these can be ported to the IR-direct
 * path as a follow-up.
 */
const DEFERRED_TOOLS = new Set<string>([
  "tube",
  "polyline_tube",
  "linear_pattern",
  "circular_pattern",
  "mirror",
  "inspect_part",
  "describe_scene",
  "place",
  // search_parts / place_part are implemented separately in parts.ts —
  // they take a Document directly and don't need the planner round-trip.
  "search_parts",
  "place_part",
]);

/**
 * MCP-context description overrides. The kernel-authored descriptions
 * assume the in-app chat surface, where a system prompt carries the type
 * catalog and material list; over MCP that context lives in the server
 * `instructions` instead, and agents choose between two authoring
 * altitudes — whole-part `create_cad_loon` vs single-node CRUD — so the
 * descriptions steer that choice explicitly.
 */
const MCP_DESCRIPTIONS: Record<string, string> = {
  create:
    "Add a single feature node to a session document: a primitive (cube/cylinder/sphere/cone), boolean (union/difference/intersection), transform (translate/rotate/scale), sketch-based feature (extrude/revolve/sweep/loft), pattern, or modifier (fillet/chamfer/shell). Per-type parameters are in the Type Catalog in this server's instructions. One node per call — for whole parts or multi-feature models prefer `create_cad_loon`.",
  update:
    "Update parameters on an existing node — pass only the fields to change. The surgical-edit tool: when geometry is misplaced or mis-sized, prefer this over delete + re-create. Use `read` to find node ids. The result reports a compact `changed` diff of affected parts.",
  set_material:
    "Set a part's material preset. Keys — metals: aluminum, steel, brass, copper, titanium, chrome, gold, silver; plastics: abs-white, abs-black, abs-red, abs-blue, pla, petg, nylon, resin, acrylic, rubber; organic: oak, walnut, leather, cork, bamboo; glass: glass, glass-tinted; composite: carbon-fiber, fiberglass, kevlar; other: concrete, ceramic, foam.",
};

/**
 * Defaults injected into `create`/`update` params before they reach the
 * Rust planner, so agents aren't forced to learn required-but-defaultable
 * fields (tessellation segment counts) by parse failure.
 */
const CREATE_PARAM_DEFAULTS: Record<string, Record<string, unknown>> = {
  cylinder: { segments: 64 },
  cone: { segments: 64 },
  sphere: { segments: 32 },
};

/**
 * Expected parameter shapes per `create` type, appended to planner parse
 * errors. serde errors like "missing field `left`" are accurate but say
 * nothing about the full shape — this does.
 */
export const CREATE_PARAM_HINTS: Record<string, string> = {
  cube: "{size: {x, y, z}}",
  cylinder: "{radius, height, segments? (default 64)} — axis along Z",
  sphere: "{radius, segments? (default 32)}",
  cone: "{radius_bottom, radius_top, height, segments? (default 64)} — axis along Z",
  union:
    "{left, right} — numeric node ids of existing nodes. Create both children first, then combine; inline child definitions are not supported.",
  difference:
    "{left, right} — numeric node ids of existing nodes (left minus right). Create both children first; inline child definitions are not supported.",
  intersection:
    "{left, right} — numeric node ids of existing nodes. Create both children first; inline child definitions are not supported.",
  translate: "{child: nodeId, offset: {x, y, z}}",
  rotate: "{child: nodeId, angles: {x, y, z}} — degrees",
  scale: "{child: nodeId, factor: {x, y, z}}",
  extrude: "{sketch: nodeId, direction: {x, y, z}, twist_angle?, scale_end?}",
  revolve: "{sketch: nodeId, axis_origin: {x, y, z}, axis_dir: {x, y, z}, angle_deg}",
  shell: "{child: nodeId, thickness}",
  fillet: "{child: nodeId, radius}",
  chamfer: "{child: nodeId, distance}",
};

/** Fill defaultable fields into create/update params in place. */
function applyParamDefaults(args: Record<string, unknown>): Record<string, unknown> {
  const type = String(args.type ?? "").toLowerCase();
  const defaults = CREATE_PARAM_DEFAULTS[type];
  const params = args.params;
  if (!defaults || !params || typeof params !== "object") return args;
  return {
    ...args,
    params: { ...defaults, ...(params as Record<string, unknown>) },
  };
}

/** Append the expected-shape hint for the requested type to a planner error. */
function enrichPlannerError(toolName: string, args: Record<string, unknown>, message: string): string {
  if (toolName !== "create" && toolName !== "update") return message;
  const type = String(args.type ?? "").toLowerCase();
  const hint = CREATE_PARAM_HINTS[type];
  if (hint) {
    return `${message}\nExpected params for "${type}": ${hint}`;
  }
  return `${message}\nPer-type param shapes are listed in the Type Catalog in this server's instructions.`;
}

/** All tool names this dispatcher will handle. */
export function registryDispatchableNames(): Set<string> {
  const all = commandRegistry.toAnthropicTools().map((t) => t.name);
  return new Set(
    all.filter(
      (n) => !BROWSER_ONLY_TOOLS.has(n) && !DEFERRED_TOOLS.has(n),
    ),
  );
}

/**
 * Build the MCP-shaped tool descriptors for every dispatchable kernel
 * tool. Each gets an extra required `document_id` arg threaded into the
 * input schema; everything else is the chat schema verbatim.
 */
export function registryToolDescriptors(): Array<{
  name: string;
  description: string;
  inputSchema: AnthropicTool["input_schema"];
}> {
  const dispatchable = registryDispatchableNames();
  return commandRegistry
    .toAnthropicTools()
    .filter((t) => dispatchable.has(t.name))
    .map((t) => withDocumentId(t));
}

function withDocumentId(tool: AnthropicTool): {
  name: string;
  description: string;
  inputSchema: AnthropicTool["input_schema"];
} {
  const original = tool.input_schema as {
    type: string;
    properties?: Record<string, unknown>;
    required?: string[];
  };
  const properties: Record<string, unknown> = {
    document_id: {
      type: "string",
      description:
        "Session id from open_document. The tool mutates this session's document.",
    },
    ...(original.properties ?? {}),
  };
  // The kernel-authored params description points at "the system prompt",
  // which doesn't exist over MCP — redirect to the server instructions.
  if (tool.name === "create" && properties.params) {
    properties.params = {
      ...(properties.params as Record<string, unknown>),
      description:
        "Parameters for the chosen `type` — see the Type Catalog in this server's instructions (e.g. cube: {size:{x,y,z}}, cylinder: {radius,height}).",
    };
  }
  const required = ["document_id", ...(original.required ?? [])];
  return {
    name: tool.name,
    description: MCP_DESCRIPTIONS[tool.name] ?? tool.description,
    inputSchema: {
      ...original,
      properties,
      required,
    } as AnthropicTool["input_schema"],
  };
}

/** One part's entry in a mutation diff. */
interface PartDiffEntry {
  part_id: string;
  name?: string;
}

/** Compact before/after diff of a mutation, reported back to the agent. */
interface PartsDiff {
  added: PartDiffEntry[];
  removed: PartDiffEntry[];
  modified: PartDiffEntry[];
}

/**
 * Structural snapshot of every part: id → name + a fingerprint of its
 * subtree (node ops, names, and material). Cheap — one JSON.stringify
 * pass over reachable nodes — and exact enough to attribute any
 * mutation to the parts it touched.
 */
function snapshotParts(
  doc: import("@vcad/ir").Document,
): Map<string, { name?: string; fingerprint: string }> {
  const map = new Map<string, { name?: string; fingerprint: string }>();
  for (const root of doc.roots) {
    const partId = String(root.root);
    const pieces: string[] = [
      `material:${String(root.material ?? "")}:${String(
        (doc.part_materials as Record<string, unknown> | undefined)?.[partId] ?? "",
      )}`,
    ];
    const stack = [root.root];
    const seen = new Set<number>();
    while (stack.length > 0) {
      const id = stack.pop()!;
      if (seen.has(id)) continue;
      seen.add(id);
      const node = doc.nodes[String(id)];
      if (!node) continue;
      pieces.push(`${id}:${node.name ?? ""}:${JSON.stringify(node.op)}`);
      for (const child of childrenOf(node.op)) stack.push(child);
    }
    pieces.sort();
    map.set(partId, {
      name: doc.nodes[partId]?.name ?? undefined,
      fingerprint: pieces.join("|"),
    });
  }
  return map;
}

/** Diff two part snapshots. Returns null when nothing changed. */
function diffParts(
  before: ReturnType<typeof snapshotParts>,
  after: ReturnType<typeof snapshotParts>,
): PartsDiff | null {
  const diff: PartsDiff = { added: [], removed: [], modified: [] };
  for (const [id, b] of before) {
    const a = after.get(id);
    if (!a) diff.removed.push({ part_id: id, name: b.name });
    else if (a.fingerprint !== b.fingerprint)
      diff.modified.push({ part_id: id, name: a.name });
  }
  for (const [id, a] of after) {
    if (!before.has(id)) diff.added.push({ part_id: id, name: a.name });
  }
  if (!diff.added.length && !diff.removed.length && !diff.modified.length) {
    return null;
  }
  return diff;
}

/** Merge a `changed` diff into a single-JSON-text-block result. */
function appendChanged(
  result: { content: Array<{ type: "text"; text: string }> },
  changed: PartsDiff,
): void {
  const block = result.content[0];
  if (!block || block.type !== "text") return;
  try {
    const parsed = JSON.parse(block.text) as Record<string, unknown>;
    parsed.changed = changed;
    block.text = JSON.stringify(parsed);
  } catch {
    // Non-JSON result — leave it alone rather than corrupt it.
  }
}

/**
 * Dispatch a registry-tier tool call against a session document. Returns
 * the MCP-shaped response. Throws if the tool isn't dispatchable, the
 * session doesn't exist, or the planner reports an error.
 *
 * Mutations close the edit feedback loop: the result carries a compact
 * `changed: {added, removed, modified}` diff of the parts the call
 * touched, so the agent sees what actually happened to the document
 * without a follow-up `read`.
 */
export function dispatchRegistryTool(
  toolName: string,
  args: Record<string, unknown>,
): { content: Array<{ type: "text"; text: string }> } {
  const dispatchable = registryDispatchableNames();
  if (!dispatchable.has(toolName)) {
    throw new Error(
      `Tool "${toolName}" is not dispatchable via the registry surface. It may be browser-only or not yet ported to the MCP path.`,
    );
  }

  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);

  // Strip document_id before forwarding to the planner — the chat-side
  // schema doesn't include it.
  const { document_id: _docId, ...toolArgs } = args;
  void _docId;

  // `read` is special: the planner just inspects the doc. We answer it
  // directly here so callers always get a structured response instead of
  // depending on the planner's text format.
  if (toolName === "read") {
    return handleRead(doc, toolArgs);
  }

  const before = snapshotParts(doc);
  const result = runMutation(toolName, toolArgs, doc, documentId);
  const changed = diffParts(before, snapshotParts(doc));
  if (changed) appendChanged(result, changed);
  return result;
}

/** Execute a mutating registry tool against an open session document. */
function runMutation(
  toolName: string,
  toolArgs: Record<string, unknown>,
  doc: import("@vcad/ir").Document,
  documentId: string,
): { content: Array<{ type: "text"; text: string }> } {
  // `delete` and `set_material` go directly to applyToolOutcome — the
  // Rust planner expects CRDT stable_ids, but the IR-direct MCP path
  // uses stringified NodeIds for part_id. The args are already
  // outcome-shaped, so the planner adds nothing. (`update` still needs
  // the planner for field-name validation; `create` needs it to lower
  // chat-tool args to a CsgOp.)
  if (toolName === "delete") {
    const partId = String((toolArgs as { part_id?: unknown }).part_id ?? "");
    if (!partId) throw new Error("delete: missing `part_id`");
    const applied = applyToolOutcome(doc, {
      kind: "remove_part",
      part_id: partId,
    });
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            removed: applied.partId,
          }),
        },
      ],
    };
  }
  if (toolName === "set_material") {
    const partId = String((toolArgs as { part_id?: unknown }).part_id ?? "");
    const material = String(
      (toolArgs as { material?: unknown }).material ?? "",
    );
    if (!partId) throw new Error("set_material: missing `part_id` (bulk forms not yet supported via MCP)");
    if (!material) throw new Error("set_material: missing `material`");
    const applied = applyToolOutcome(doc, {
      kind: "set_part_material",
      part_id: partId,
      material,
    });
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            part_id: applied.partId,
            material,
          }),
        },
      ],
    };
  }

  const plannerArgs =
    toolName === "create" ? applyParamDefaults(toolArgs) : toolArgs;
  const planned = commandRegistry.planCrud(
    toolName,
    plannerArgs,
    JSON.stringify(doc),
  );
  if (!planned) {
    throw new Error(
      `Tool "${toolName}" — Rust planner unavailable. Ensure commandRegistry.setWasm() has been called during MCP server boot.`,
    );
  }
  if (planned.status === "error") {
    throw new Error(enrichPlannerError(toolName, plannerArgs, planned.result));
  }
  if (!planned.outcome) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            result: planned.result,
          }),
        },
      ],
    };
  }

  const applied = applyToolOutcome(doc, planned.outcome);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: documentId,
          result: planned.result,
          part_id: applied.partId,
          node_id: applied.nodeId,
        }),
      },
    ],
  };
}

function handleRead(
  doc: import("@vcad/ir").Document,
  args: Record<string, unknown>,
): { content: Array<{ type: "text"; text: string }> } {
  const partId = args.part_id as string | undefined;
  if (!partId) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({ parts: listPartsFromDocument(doc) }),
        },
      ],
    };
  }
  const idx = doc.roots.findIndex((r) => String(r.root) === partId);
  if (idx < 0) {
    throw new Error(`read: no part with id "${partId}"`);
  }
  const root = doc.roots[idx];
  // Walk descendant nodes for the feature tree, keyed by node id.
  const tree: Record<string, unknown> = {};
  const stack = [root.root];
  const seen = new Set<number>();
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const node = doc.nodes[String(id)];
    if (!node) continue;
    tree[String(id)] = { id: node.id, name: node.name, op: node.op };
    for (const child of childrenOf(node.op)) stack.push(child);
  }
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          part_id: partId,
          material: root.material,
          nodes: tree,
        }),
      },
    ],
  };
}

function childrenOf(op: import("@vcad/ir").CsgOp): number[] {
  switch (op.type) {
    case "Union":
    case "Difference":
    case "Intersection":
      return [op.left, op.right];
    case "Translate":
    case "Rotate":
    case "Scale":
    case "LinearPattern":
    case "CircularPattern":
    case "Shell":
    case "Fillet":
    case "Chamfer":
      return [op.child];
    case "Extrude":
    case "Revolve":
    case "Sweep":
      return [op.sketch];
    case "Loft":
      return op.sketches ?? [];
    default:
      return [];
  }
}
