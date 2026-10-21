/**
 * Generic dispatcher for the kernel-tier chat tools (the ones in
 * `commandRegistry.toAnthropicTools()`). The MCP server registers these
 * under their original names — the schema is identical to what the in-app
 * chat sends to Anthropic — and routes each mutation through the Rust
 * document engine (`WasmDocumentEngine.apply_tool`).
 *
 * Result: schema and behavior come from the kernel; the MCP wrapper is a
 * thin session-aware shell. No tool definitions duplicated in this package.
 */

import type { AnthropicTool } from "@vcad/core";
import { commandRegistry, listPartsFromDocument } from "@vcad/core";
import { getSession, recordLastChanged, recordTriangles } from "./session.js";
import { storeEngine, takeCachedEngine } from "./session-core.js";
import {
  isStablePartId,
  partIdDeprecation,
  partIdForRoot,
  rootForPartId,
} from "./part-ids.js";
import { getKernelWasmSync } from "@vcad/engine";
import { applyJointState, jointStateSchemaProp } from "./pose.js";
import { appendIntegrity, computeIntegrity } from "./integrity.js";
import {
  describeSceneResult,
  inspectPartResult,
  MeasureError,
} from "./measure.js";

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
 * Registry tools MCP serves through its own ToolDefs instead of this
 * dispatcher. (The feature builders and `place` used to be deferred here;
 * they are now lowered by the document engine.)
 */
const DEFERRED_TOOLS = new Set<string>([
  // search_parts / place_part are served by parts.ts (place_part still
  // mutates through the engine, via runMutation).
  "search_parts",
  "place_part",
]);

/**
 * Dispatchable registry tools that only READ the scene — no mutation, no undo
 * snapshot, no persist. `read` inspects the IR directly; `inspect_part` /
 * `describe_scene` evaluate the scene and measure tessellations. The server
 * derives each registry tool's `writesDoc` flag from this set.
 */
export const READ_ONLY_REGISTRY_TOOLS = new Set<string>([
  "read",
  "inspect_part",
  "describe_scene",
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
    "{left, right} — node ids of existing nodes (numeric or string). Create both children first, then combine; inline child definitions are not supported.",
  difference:
    "{left, right} — node ids of existing nodes (left minus right; numeric or string). Create both children first; inline child definitions are not supported.",
  intersection:
    "{left, right} — node ids of existing nodes (numeric or string). Create both children first; inline child definitions are not supported.",
  translate: "{child: nodeId, offset: {x, y, z}}",
  rotate: "{child: nodeId, angles: {x, y, z}} — degrees",
  scale: "{child: nodeId, factor: {x, y, z}}",
  extrude: "{sketch: nodeId, direction: {x, y, z}, twist_angle?, scale_end?}",
  revolve: "{sketch: nodeId, axis_origin: {x, y, z}, axis_dir: {x, y, z}, angle_deg}",
  shell: "{child: nodeId, thickness}",
  fillet: "{child: nodeId, radius}",
  chamfer: "{child: nodeId, distance}",
  edge_blend:
    '{child: nodeId, edges: {type: "All"} | {type: "Near", point: {x,y,z}} | {type: "Direction", axis: {x,y,z}, tol_deg}, profile: {type: "Constant", size, shape} | {type: "Keyed", keys: [{t, size, shape}]}} — shape 0 = chamfer, 1 = fillet; size in mm',
};

/** Param keys that reference another node by id (CsgOp child refs). */
const NODE_REF_PARAM_KEYS = ["child", "left", "right", "sketch"] as const;

/** Coerce a single node reference: the Rust planner's serde parse wants
 *  JSON numbers for CsgOp child refs, but ids travel as strings everywhere
 *  else on the MCP surface. Digit-strings become numbers; a stable part id
 *  ("0:7", from a prior result's part_id) becomes that part's root NodeId. */
function coerceNodeRef(v: unknown, doc?: import("@vcad/ir").Document): unknown {
  if (typeof v === "string" && /^\d+$/.test(v)) return Number(v);
  if (typeof v === "string" && doc && isStablePartId(v)) {
    const r = rootForPartId(doc, v);
    if (r) return r.entry.root;
  }
  return v;
}

/**
 * Unify the two id dialects at the dispatch boundary so agents can pass
 * either. Top-level `node_id`/`part_id` are strings on the planner side —
 * accept numbers by stringifying. Child refs inside create/update `params`
 * (child/left/right/sketch/sketches) are numeric NodeIds on the planner
 * side — accept digit-strings by numbering. Returns a copy; never mutates.
 */
export function normalizeNodeRefs(
  toolName: string,
  args: Record<string, unknown>,
  doc?: import("@vcad/ir").Document,
): Record<string, unknown> {
  const out = { ...args };
  for (const key of ["node_id", "part_id", "parent_part_id"]) {
    if (typeof out[key] === "number") out[key] = String(out[key]);
  }
  // The planner addresses a create's parent part by root NodeId.
  if (doc && typeof out.parent_part_id === "string") {
    const r = rootForPartId(doc, out.parent_part_id);
    if (r) out.parent_part_id = String(r.entry.root);
  }
  if (
    (toolName === "create" || toolName === "update") &&
    out.params &&
    typeof out.params === "object" &&
    !Array.isArray(out.params)
  ) {
    const params = { ...(out.params as Record<string, unknown>) };
    for (const key of NODE_REF_PARAM_KEYS) {
      if (key in params) params[key] = coerceNodeRef(params[key], doc);
    }
    if (Array.isArray(params.sketches)) {
      params.sketches = params.sketches.map((v) => coerceNodeRef(v, doc));
    }
    out.params = params;
  }
  return out;
}

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
  // Pose-aware reads: measuring a jointed assembly at a real pose is the
  // point of `joint_state` (see tools/pose.ts).
  if (tool.name === "inspect_part" || tool.name === "describe_scene") {
    properties.joint_state = jointStateSchemaProp;
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
export interface PartsDiff {
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
export function snapshotParts(
  doc: import("@vcad/ir").Document,
): Map<string, { name?: string; fingerprint: string }> {
  const map = new Map<string, { name?: string; fingerprint: string }>();
  for (const root of doc.roots) {
    const nodeKey = String(root.root);
    const partId = partIdForRoot(doc, root);
    const pieces: string[] = [
      `material:${String(root.material ?? "")}:${String(
        (doc.part_materials as Record<string, unknown> | undefined)?.[nodeKey] ?? "",
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
      name: doc.nodes[nodeKey]?.name ?? undefined,
      fingerprint: pieces.join("|"),
    });
  }
  return map;
}

/** Diff two part snapshots. Returns null when nothing changed. */
export function diffParts(
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

/**
 * Surface a `changed` diff on a mutation result in BOTH carriers:
 *
 *  1. `structuredContent.changed` — the inline 3D viewer reads this to
 *     flash the parts an edit just touched. ChatGPT's widget bridge
 *     (`window.openai.toolOutput`) exposes ONLY structuredContent, so a
 *     diff that lived solely in the text block would be invisible there.
 *  2. the single JSON text block — kept for the agent and for hosts
 *     (Cursor) with known gaps forwarding structuredContent to widgets.
 */
export function appendChanged(
  result: {
    content: Array<{ type: "text"; text: string }>;
    structuredContent?: Record<string, unknown>;
  },
  changed: PartsDiff,
): void {
  result.structuredContent = { ...result.structuredContent, changed };

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
  engine?: import("@vcad/engine").Engine,
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

  // `inspect_part` / `describe_scene` need an evaluated scene, not the Rust
  // planner (they were app-only, reading the browser docstore + engine
  // scene). Answer them directly from the session document + engine.
  if (toolName === "inspect_part" || toolName === "describe_scene") {
    // These two read measured geometry, so they honour `joint_state` — the
    // pose is applied to a clone, never to the session document.
    const { joint_state: jointState, ...readArgs } = toolArgs;
    const posed = applyJointState(doc, jointState);
    return handleMeasureRead(toolName, posed.doc, readArgs, engine, posed.pose);
  }

  const before = snapshotParts(doc);
  const result = runMutation(toolName, toolArgs, doc, documentId);
  const changed = diffParts(before, snapshotParts(doc));
  if (changed) {
    appendChanged(result, changed);
    // Remember what this mutation touched so `render_view
    // {highlight_changed: true}` can spotlight it.
    recordLastChanged(documentId, [
      ...changed.added.map((p) => p.part_id),
      ...changed.modified.map((p) => p.part_id),
    ]);
    // Every mutation carries its own integrity certificate (volume, bbox,
    // CoM, watertightness, CoM-vs-pattern-axis): silently corrupt geometry
    // must be visible in the mutation response, not only via an
    // out-of-band inspect_cad (torr session-3 field report).
    if (engine) {
      const integrity = computeIntegrity(doc, engine);
      if (integrity) {
        appendIntegrity(result, integrity);
        recordTriangles(documentId, integrity.triangles);
      }
    }
  }
  return result;
}

/**
 * Tools `vcad_app::tools::lower` handles (crates/vcad-app/src/tools.rs) —
 * core's ENGINE_TOOLS plus the geometry tools `lower` also accepts. Only the
 * subset MCP dispatches (not in DEFERRED_TOOLS) can reach this path.
 */
const ENGINE_LOWERED_TOOLS = new Set<string>([
  "create",
  "update",
  "delete",
  "set_material",
  "linear_pattern",
  "circular_pattern",
  "mirror",
  "translate",
  "rotate",
  "scale",
  "union",
  "difference",
  "intersection",
  "fillet",
  "chamfer",
  "shell",
  "tube",
  "polyline_tube",
  "place",
  // Reached via parts.ts (place_part handler), not registry dispatch.
  "place_part",
  // Reached via topopt.ts (topology_optimize result), not registry dispatch.
  "add_mesh_part",
]);

interface EngineEnvelope {
  ok: boolean;
  result?: {
    document: import("@vcad/ir").Document;
    parts?: Array<{ id: string; name?: string | null; primitiveNodeId?: number }>;
    consumedPartIds?: string[];
    warnings?: string[];
    createdFeatureId?: string;
  };
  error?: { kind: string; message?: string; id?: string; tool?: string };
}

interface ToolEngine {
  import_ir(irJson: string): unknown;
  apply_tool?(tool: string, argsJson: string): string;
  free?(): void;
}

/** Part reference (stable id, legacy root NodeId, or name) → the engine's
 *  stable id. Unresolvable values pass through so the engine reports them
 *  as `unknown_id`. */
function toStableId(doc: import("@vcad/ir").Document, v: unknown): unknown {
  if (typeof v !== "string" && typeof v !== "number") return v;
  return rootForPartId(doc, v)?.partId ?? String(v);
}

/** Index of the scene root whose subtree contains `nodeId` (an exact root
 *  match wins). Roots don't share subtrees — a boolean consumes its operand
 *  roots — so containment names the one owning part. */
function owningRootIndex(doc: import("@vcad/ir").Document, nodeId: string): number {
  const exact = doc.roots.findIndex((r) => String(r.root) === nodeId);
  if (exact >= 0) return exact;
  return doc.roots.findIndex((r) => {
    const stack: unknown[] = [r.root];
    const seen = new Set<string>();
    while (stack.length) {
      const id = String(stack.pop());
      if (id === nodeId) return true;
      if (seen.has(id)) continue;
      seen.add(id);
      const op = (doc.nodes[id] as { op?: Record<string, unknown> } | undefined)?.op;
      if (!op) continue;
      for (const k of ["child", "left", "right"]) if (op[k] !== undefined) stack.push(op[k]);
      if (Array.isArray(op.children)) stack.push(...op.children);
    }
    return false;
  });
}

/** NodeId operand (left/right/child/parent after normalizeNodeRefs, or an
 *  `update` node_id) → the stable id of the part that owns that node. The
 *  engine addresses features by stable id and renumbers nodes on every
 *  materialize, so NodeIds are only meaningful against the session doc —
 *  resolve them here, before the engine sees them. */
function nodeRefToStableId(doc: import("@vcad/ir").Document, v: unknown): unknown {
  if (typeof v === "number" || (typeof v === "string" && /^\d+$/.test(v))) {
    const idx = owningRootIndex(doc, String(v));
    if (idx >= 0) return partIdForRoot(doc, idx);
  }
  return toStableId(doc, v);
}

function engineArgs(
  toolName: string,
  args: Record<string, unknown>,
  doc: import("@vcad/ir").Document,
): Record<string, unknown> {
  const out = toolName === "create" ? applyParamDefaults({ ...args }) : { ...args };
  if ("part_id" in out) out.part_id = toStableId(doc, out.part_id);
  if (Array.isArray(out.part_ids)) out.part_ids = out.part_ids.map((v) => toStableId(doc, v));
  if (out.parent_part_id !== undefined) out.parent_part_id = nodeRefToStableId(doc, out.parent_part_id);
  // `lower_update` accepts the owning part's stable id as `node_id` and sets
  // the params on that feature; a session NodeId would be looked up against
  // the engine's renumbered nodes instead.
  if (toolName === "update" && out.node_id !== undefined) {
    out.node_id = nodeRefToStableId(doc, out.node_id);
  }
  if (out.hide_part_id !== undefined) out.hide_part_id = toStableId(doc, out.hide_part_id);
  if (toolName === "place") {
    for (const key of ["from", "to"]) {
      const ref = out[key];
      if (ref && typeof ref === "object" && "part" in ref) {
        out[key] = { ...ref, part: toStableId(doc, (ref as { part: unknown }).part) };
      }
    }
  }
  if (toolName === "create" && out.params && typeof out.params === "object") {
    const params = { ...(out.params as Record<string, unknown>) };
    for (const key of ["left", "right", "child"]) {
      if (key in params) params[key] = nodeRefToStableId(doc, params[key]);
    }
    out.params = params;
  }
  return out;
}

/** Engine `error.kind` → the error text style the legacy MCP path throws. */
function engineErrorMessage(
  toolName: string,
  args: Record<string, unknown>,
  err: NonNullable<EngineEnvelope["error"]>,
): string {
  switch (err.kind) {
    case "unknown_id":
      return `${toolName}: no part with id "${err.id ?? "?"}"`;
    case "invalid":
      // Same per-type param hints the legacy planner errors carry.
      return enrichPlannerError(toolName, args, `${toolName}: ${err.message ?? "invalid arguments"}`);
    default:
      return `${toolName}: ${err.message ?? err.kind}`;
  }
}

/**
 * Run an engine-lowered mutation through the Rust document engine: import
 * the session IR into a fresh `WasmDocumentEngine`, `apply_tool`, and replace
 * the session document — in place, so the caller's reference and the
 * `changed` diff keep working — with the engine's `result.document`, which
 * carries `partIds`. Throws when the tool isn't engine-lowered, the kernel
 * WASM lacks `WasmDocumentEngine.apply_tool`, or the engine refuses the call.
 *
 * One engine is cached per session (session-core `takeCachedEngine` /
 * `storeEngine`): it is reused while the session still holds exactly the IR
 * that engine produced, and rebuilt via `import_ir` otherwise.
 */
function runMutationViaEngine(
  toolName: string,
  toolArgs: Record<string, unknown>,
  doc: import("@vcad/ir").Document,
  documentId: string,
): { content: Array<{ type: "text"; text: string }> } {
  if (!ENGINE_LOWERED_TOOLS.has(toolName)) {
    throw new Error(`Tool "${toolName}" is not lowered by the document engine.`);
  }
  const wasm = getKernelWasmSync() as unknown as {
    WasmDocumentEngine?: { withReplica?(id: number): ToolEngine };
  } | null;
  const factory = wasm?.WasmDocumentEngine;
  if (!factory || typeof factory.withReplica !== "function") {
    throw new Error(
      `${toolName}: kernel WASM has no WasmDocumentEngine — ensure the kernel is initialized and up to date.`,
    );
  }
  const cachedEngine = takeCachedEngine<ToolEngine>(documentId, doc);
  const engine = cachedEngine ?? factory.withReplica(0);
  // Ownership: until handed back to the cache on success, `engine` is freed
  // in `finally`, so an error or decline never leaves a stale engine cached.
  let keep = false;
  try {
    if (typeof engine.apply_tool !== "function") {
      throw new Error(
        `${toolName}: kernel WASM predates WasmDocumentEngine.apply_tool — rebuild the kernel WASM.`,
      );
    }
    const deprecation =
      typeof toolArgs.part_id === "string"
        ? partIdDeprecation(toolArgs.part_id, rootForPartId(doc, toolArgs.part_id))
        : undefined;
    const args = engineArgs(toolName, toolArgs, doc);
    if (!cachedEngine) engine.import_ir(JSON.stringify(doc));
    const envelope = JSON.parse(engine.apply_tool(toolName, JSON.stringify(args))) as EngineEnvelope;
    if (!envelope.ok || !envelope.result) {
      const err = envelope.error ?? { kind: "invalid", message: "engine returned no result" };
      throw new Error(engineErrorMessage(toolName, args, err));
    }
    const next = envelope.result.document;
    const parts = envelope.result.parts ?? [];
    // The materializer names the primitive node and leaves the part's
    // wrapper root (Translate) unnamed; part listings read the root's name,
    // so carry the part name up to keep listings/`changed` stable.
    for (const p of parts) {
      if (!p.name) continue;
      const rootIdx = next.roots.findIndex((_, i) => next.partIds?.[String(next.roots[i]!.root)] === p.id);
      const rootNode = rootIdx >= 0 ? (next.nodes[String(next.roots[rootIdx]!.root)] as { name?: string | null }) : undefined;
      if (rootNode && !rootNode.name) rootNode.name = p.name;
    }
    for (const k of Object.keys(doc)) delete (doc as unknown as Record<string, unknown>)[k];
    Object.assign(doc, next);
    // The session IR is now exactly the engine's own output: cache it.
    storeEngine(documentId, doc, engine);
    keep = true;

    const warnings = [...(envelope.result.warnings ?? []), ...(deprecation ? [deprecation] : [])];
    const payload: Record<string, unknown> = { document_id: documentId };
    if (toolName === "delete") {
      payload.removed = args.part_id;
    } else if (toolName === "set_material") {
      payload.part_id = args.part_id ?? args.part_ids;
      payload.material = toolArgs.material;
    } else {
      // Prefer the engine's createdFeatureId as the new part_id.
      // In-place edits (translate/rotate/scale) create no feature: report
      // the part they edited.
      const params = args.params as Record<string, unknown> | undefined;
      const edited = args.part_id ?? args.child ?? params?.child;
      const partId =
        envelope.result.createdFeatureId ??
        (typeof edited === "string" ? edited : undefined);
      if (partId) {
        payload.part_id = partId;
        // Legacy returned the created op's node: the primitive (or the
        // feature's own node) in the NEW materialized doc, else the root.
        const part = parts.find((p) => p.id === partId);
        const root = rootForPartId(doc, partId);
        const nodeId = part?.primitiveNodeId ?? root?.entry.root;
        if (nodeId !== undefined) payload.node_id = nodeId;
      }
    }
    if (envelope.result.consumedPartIds?.length) {
      payload.consumed_part_ids = envelope.result.consumedPartIds;
    }
    if (warnings.length) payload.warnings = warnings;
    return { content: [{ type: "text", text: JSON.stringify(payload) }] };
  } finally {
    if (!keep) engine.free?.();
  }
}

/** Execute a mutating registry tool against an open session document.
 *  Every mutation runs through the Rust document engine; there is no
 *  TS-side fallback, so an engine refusal is the tool's answer. */
export function runMutation(
  toolName: string,
  rawArgs: Record<string, unknown>,
  doc: import("@vcad/ir").Document,
  documentId: string,
): { content: Array<{ type: "text"; text: string }> } {
  const toolArgs = normalizeNodeRefs(toolName, rawArgs, doc);
  return runMutationViaEngine(toolName, toolArgs, doc, documentId);
}

/**
 * Answer `inspect_part` / `describe_scene` from the evaluated scene. Both are
 * pure reads (the mesh math lives in measure.ts). Requires the engine — over
 * MCP it is always threaded from the connection; a missing one is a wiring bug,
 * surfaced clearly rather than as an opaque undefined-deref.
 */
function handleMeasureRead(
  toolName: string,
  doc: import("@vcad/ir").Document,
  args: Record<string, unknown>,
  engine?: import("@vcad/engine").Engine,
  pose?: import("./pose.js").PoseInfo,
): { content: Array<{ type: "text"; text: string }> } {
  if (!engine) {
    throw new Error(
      `Tool "${toolName}" needs the kernel engine — none was threaded into the dispatcher.`,
    );
  }
  let payload: Record<string, unknown>;
  try {
    payload =
      toolName === "inspect_part"
        ? inspectPartResult(doc, engine, String(args.part_id ?? ""))
        : describeSceneResult(
            doc,
            engine,
            Array.isArray(args.part_ids)
              ? (args.part_ids as unknown[]).map((x) => String(x))
              : undefined,
            typeof args.limit === "number" ? args.limit : undefined,
          );
  } catch (e) {
    if (e instanceof MeasureError) throw new Error(e.message);
    throw e;
  }
  if (pose) payload.pose = pose;
  return {
    content: [{ type: "text", text: JSON.stringify(payload) }],
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
  const resolved = rootForPartId(doc, String(partId));
  if (!resolved) {
    throw new Error(`read: no part with id "${partId}"`);
  }
  const root = resolved.entry;
  const deprecation = partIdDeprecation(String(partId), resolved);
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
          part_id: resolved.partId,
          node_id: String(root.root),
          material: root.material,
          nodes: tree,
          ...(deprecation ? { warnings: [deprecation] } : {}),
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
    case "EdgeBlend":
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
