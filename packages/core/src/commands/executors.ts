import type { ExecutionResult, SummarySegment } from "./types.js";
import { useDocumentStore } from "../stores/document-store.js";
import { useEngineStore } from "../stores/engine-store.js";
import { useUiStore } from "../stores/ui-store.js";
import type { Vec3 } from "@vcad/ir";
import {
  loadPartsManifest as loadPartsManifestFromEngine,
  searchParts as searchPartsFromEngine,
  defaultParamsFor as defaultParamsForEngine,
  type PartManifestEntry,
} from "@vcad/engine";
import { expandBboxFromPositions, type Bbox } from "../camera-framing.js";

type DocStore = ReturnType<typeof useDocumentStore.getState>;
type UiStore = ReturnType<typeof useUiStore.getState>;

// ---------------------------------------------------------------------------
// Geometry helpers used by tube / place / rotate-with-pivot / inspect_part.
// ---------------------------------------------------------------------------

/** Read a part's current world-space AABB from the evaluated scene. Returns
 *  null when the scene hasn't been evaluated yet or when the part isn't in
 *  the scene (e.g. it was just created this tick and is still cooking). */
function computePartWorldBbox(partId: string, docStore: DocStore): Bbox | null {
  const scene = useEngineStore.getState().scene;
  if (!scene) return null;
  const idx = docStore.parts.findIndex((p) => p.id === partId);
  if (idx >= 0) {
    const evalPart = scene.parts[idx];
    if (evalPart) return expandBboxFromPositions(null, evalPart.mesh.positions);
  }
  if (scene.instances) {
    for (const inst of scene.instances) {
      const id =
        (inst as { id?: string; instanceId?: string; partDefId?: string }).id ??
        (inst as { id?: string; instanceId?: string; partDefId?: string }).instanceId ??
        (inst as { id?: string; instanceId?: string; partDefId?: string }).partDefId;
      if (id === partId) return expandBboxFromPositions(null, inst.mesh.positions);
    }
  }
  return null;
}

/** Try to read a part's world AABB; if the scene is stale (the part was
 *  just created and the RAF-debounced eval hasn't fired yet), force a
 *  synchronous evaluation of the current document and retry once. This
 *  removes the most common cause of the cryptic "scene may not be evaluated
 *  yet" error from the AI's path: it lets the AI chain `tube` → `place`
 *  in the same turn without waiting for the next animation frame.
 *
 *  Force-eval is bounded — it skips clash detection (the O(n²) cost) and
 *  reads the existing scene cache when possible, so the worst case on a
 *  large scene is a single tessellation pass on the dirty subgraph. */
function computePartWorldBboxForceEval(
  partId: string,
  docStore: DocStore,
): Bbox | null {
  const first = computePartWorldBbox(partId, docStore);
  if (first) return first;
  const engineStore = useEngineStore.getState();
  const engine = engineStore.engine;
  if (!engine || typeof engine.evaluate !== "function") return null;
  try {
    const scene = engine.evaluate(docStore.document, { skipClashDetection: true });
    engineStore.setScene(scene);
  } catch {
    return null;
  }
  return computePartWorldBbox(partId, docStore);
}

/** Build a structured-error JSON string the AI can act on. Keeping it as
 *  a single line keeps it cheap to emit from any executor while still
 *  giving the model enough handles to recover (a code, hints about what
 *  was looked up, and an explicit suggestion). */
function structuredError(
  code: string,
  message: string,
  details?: Record<string, unknown>,
): string {
  return JSON.stringify({ ok: false, code, message, ...(details ?? {}) });
}

/** Build a compact JSON summary for successful CRUD operations. Gives the
 *  model enough information to decide what to do next (bbox for placing,
 *  resulting transform for chaining, material for bulk ops) without a
 *  follow-up `inspect_part` round-trip. Force-evaluates if the scene is
 *  stale so fresh parts report real coordinates, not zeros. */
function buildPartSnapshot(
  partId: string,
  docStore: DocStore,
  extra?: Record<string, unknown>,
): Record<string, unknown> {
  const part = docStore.partIndex.get(partId);
  const translate = getCurrentOffset(partId, docStore);
  const rotate = getCurrentAngles(partId, docStore);
  const doc = docStore.document as unknown as {
    partMaterials?: Record<string, string>;
  };
  const material = doc.partMaterials?.[partId] ?? null;
  const snapshot: Record<string, unknown> = {
    ok: true,
    part_id: partId,
    name: part?.name ?? null,
    kind: part?.kind ?? null,
    translate,
    rotate,
    material,
    ...(extra ?? {}),
  };
  const bbox = computePartWorldBboxForceEval(partId, docStore);
  if (bbox) {
    const center = bboxCenter(bbox);
    const size = bboxSize(bbox);
    snapshot.bbox = {
      min: { x: bbox.min[0], y: bbox.min[1], z: bbox.min[2] },
      max: { x: bbox.max[0], y: bbox.max[1], z: bbox.max[2] },
      center,
      size,
    };
  }
  return snapshot;
}

function snapshotJson(
  partId: string,
  docStore: DocStore,
  extra?: Record<string, unknown>,
): string {
  return JSON.stringify(buildPartSnapshot(partId, docStore, extra));
}

function bboxCenter(b: Bbox): Vec3 {
  return {
    x: (b.min[0] + b.max[0]) / 2,
    y: (b.min[1] + b.max[1]) / 2,
    z: (b.min[2] + b.max[2]) / 2,
  };
}

function bboxSize(b: Bbox): Vec3 {
  return {
    x: b.max[0] - b.min[0],
    y: b.max[1] - b.min[1],
    z: b.max[2] - b.min[2],
  };
}

/** Read the current Translate.offset wrapping a part, or (0,0,0) if missing. */
function getCurrentOffset(partId: string, docStore: DocStore): Vec3 {
  const part = docStore.partIndex.get(partId);
  const p = part as { translateNodeId?: number | string } | undefined;
  if (!p?.translateNodeId) return { x: 0, y: 0, z: 0 };
  const node = docStore.document.nodes[String(p.translateNodeId)];
  const op = node?.op as { type?: string; offset?: Vec3 } | undefined;
  if (op?.type !== "Translate" || !op.offset) return { x: 0, y: 0, z: 0 };
  return { x: op.offset.x, y: op.offset.y, z: op.offset.z };
}

/** Read the current Rotate.angles (Euler XYZ degrees), or zeros if missing. */
function getCurrentAngles(partId: string, docStore: DocStore): Vec3 {
  const part = docStore.partIndex.get(partId);
  const p = part as { rotateNodeId?: number | string } | undefined;
  if (!p?.rotateNodeId) return { x: 0, y: 0, z: 0 };
  const node = docStore.document.nodes[String(p.rotateNodeId)];
  const op = node?.op as { type?: string; angles?: Vec3 } | undefined;
  if (op?.type !== "Rotate" || !op.angles) return { x: 0, y: 0, z: 0 };
  return { x: op.angles.x, y: op.angles.y, z: op.angles.z };
}





/** Validate that a part ID exists in the document. */
function validatePartId(partId: string, docStore: DocStore, label: string): ExecutionResult | null {
  if (!docStore.partIndex.get(partId)) {
    const available = docStore.parts.map((p) => p.id).slice(0, 10).join(", ");
    return {
      status: "error",
      result: `${label} "${partId}" not found. Available parts: [${available}]${docStore.parts.length > 10 ? ` (+${docStore.parts.length - 10} more)` : ""}`,
    };
  }
  return null;
}

/** Render a part ID as a clickable segment, falling back to last 4 chars if unknown. */
function link(id: string, docStore: DocStore): SummarySegment {
  const part = docStore.partIndex.get(id);
  return {
    type: "partLink",
    partId: id,
    name: part?.name ?? id.slice(-4),
  };
}

/** Shorthand for a text segment. */
function text(s: string): SummarySegment {
  return { type: "text", text: s };
}

/** Execute a CRUD tool by name, measuring duration. */
export function executeCrud(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const t0 = performance.now();
  // The engine lowers the tools it knows (vcad_app::tools) and applies them
  // as DocCommands; "unsupported" tools fall through to the paths below.
  const engineResult = tryExecuteViaEngine(tool, args, docStore, uiStore);
  if (engineResult) {
    engineResult.duration = performance.now() - t0;
    return engineResult;
  }
  const result = executeCrudInner(tool, args, docStore, uiStore);
  result.duration = performance.now() - t0;
  return result;
}

/** Tools the Rust lowering handles at the top level (besides `create`). */
const ENGINE_TOOLS = new Set([
  "create",
  "linear_pattern",
  "circular_pattern",
  "mirror",
  "tube",
  "polyline_tube",
  "delete",
  "set_material",
  "update",
]);

/**
 * Run a tool through `docStore.applyTool` (Rust lowering + `DocCommand`).
 * Returns `null` to fall through to {@link executeCrudInner}: a tool the
 * engine reports as unsupported, or one outside {@link ENGINE_TOOLS}.
 */
function tryExecuteViaEngine(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult | null {
  if (!ENGINE_TOOLS.has(tool)) return null;
  const kind = tool === "create" ? (CREATE_TYPE_ALIASES[args.type as string] ?? (args.type as string)) : tool;
  let callArgs: Record<string, unknown> = tool === "create" ? { ...args, type: kind } : args;
  // Booleans without operands use the current two-part selection.
  if (tool === "create" && (kind === "union" || kind === "difference" || kind === "intersection")) {
    const params = { ...((args.params as Record<string, unknown>) ?? {}) };
    if (!params.left || !params.right) {
      const selected = Array.from(uiStore.selectedPartIds);
      if (selected.length !== 2) {
        return { status: "error", result: "Boolean requires left and right part IDs, or exactly 2 parts selected" };
      }
      params.left = selected[0];
      params.right = selected[1];
    }
    callArgs = { ...callArgs, params };
  }
  const envelope = docStore.applyTool(tool, callArgs);
  if (!envelope) return { status: "error", result: "engine not ready" };
  if (!envelope.ok) {
    if (envelope.error.kind === "unsupported") return null;
    return { status: "error", result: envelope.error.message };
  }
  if (tool === "delete") uiStore.clearSelection();
  const params = (tool === "create" ? args.params : args) as Record<string, unknown> | undefined;
  const created = envelope.result.createdFeatureId;
  const partId = created ?? (params?.child as string | undefined) ?? (args.part_id as string | undefined);
  if (!partId || tool === "delete") return { status: "success", result: `Applied ${kind}${partId ? ` to ${partId}` : ""}` };
  if (created) uiStore.select(created);
  return {
    status: "success",
    result: snapshotJson(partId, docStore, created ? { created: kind } : { applied: kind }),
    partId,
    display: {
      summary: [text(created ? `+ ${kind} ` : `${kind} `), link(partId, docStore)],
      fields: paramFields(params),
      affectedPartIds: [partId],
    },
  };
}

/** Display fields for a tool call: its numeric and {x, y, z} params. */
function paramFields(params: Record<string, unknown> | undefined): Array<{ label: string; value: string }> {
  const fields: Array<{ label: string; value: string }> = [];
  for (const [label, v] of Object.entries(params ?? {})) {
    if (typeof v === "number") fields.push({ label, value: `${v}` });
    else if (v && typeof v === "object" && "x" in v && "y" in v && "z" in v) {
      const p = v as Vec3;
      fields.push({ label, value: `(${p.x}, ${p.y}, ${p.z})` });
    } else if (typeof v === "string" && label === "plane") fields.push({ label, value: v });
  }
  return fields;
}



function executeCrudInner(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  switch (tool) {
    case "create":
      // Every create kind is lowered by the engine; reaching here means it
      // reported the kind unsupported.
      if (((args.type as string) ?? "").replace(/_2_d$/, "_2d") === "sketch_2d") {
        return { status: "success", result: "Sketch2D is typically used inline with extrude/revolve. Use create(type: 'extrude', ...) with an inline sketch instead." };
      }
      return { status: "error", result: `Unknown create type: ${args.type as string}` };
    case "read":
      return executeRead(args.part_id as string | undefined, docStore);
    case "inspect_part":
      return executeInspectPart(args.part_id as string, docStore);
    case "place":
      return executePlace(args, docStore);
    case "describe_scene":
      return executeDescribeScene(args, docStore);
    case "search_parts":
      return executeSearchParts(args, docStore);
    case "place_part":
      return executePlacePart(args, docStore, uiStore);
    default:
      return { status: "error", result: `Unknown tool: ${tool}` };
  }
}

// ---------------------------------------------------------------------------
// inspect_part — world-frame bbox + size + material + material-mass. Gives
// the AI a way to verify geometry between tool calls without spending tokens
// on a screenshot. Return shape is JSON in `result` so the model can parse it.
// ---------------------------------------------------------------------------
function executeInspectPart(partId: string, docStore: DocStore): ExecutionResult {
  if (!partId) return { status: "error", result: "inspect_part requires part_id" };
  const part = docStore.partIndex.get(partId);
  if (!part) {
    const err = validatePartId(partId, docStore, "inspect_part part_id");
    return err ?? { status: "error", result: `Part ${partId} not found` };
  }
  // Force-eval if the scene is stale (e.g. this part was just created
  // within the same AI turn). This was the #1 source of failed `inspect`
  // calls: the AI would create a tube, then immediately ask to inspect it,
  // and the RAF-debounced eval hadn't fired yet.
  const bbox = computePartWorldBboxForceEval(partId, docStore);
  const offset = getCurrentOffset(partId, docStore);
  const angles = getCurrentAngles(partId, docStore);
  const kind = part.kind;
  const name = part.name;

  // Material: read from the document's material assignments if present.
  const doc = docStore.document as unknown as { materials?: Record<string, unknown>; partMaterials?: Record<string, string> };
  const material = doc.partMaterials?.[partId] ?? null;

  const out: Record<string, unknown> = {
    id: partId,
    name,
    kind,
    translate: offset,
    rotate: angles,
    material,
  };
  if (bbox) {
    const center = bboxCenter(bbox);
    const size = bboxSize(bbox);
    out.bbox = {
      min: { x: bbox.min[0], y: bbox.min[1], z: bbox.min[2] },
      max: { x: bbox.max[0], y: bbox.max[1], z: bbox.max[2] },
      center,
      size,
    };
    // Anchors: canonical named points on the part's world-space AABB that
    // `place` can target. Keeps both tools in sync on what a name resolves to.
    out.anchors = {
      center,
      min: { x: bbox.min[0], y: bbox.min[1], z: bbox.min[2] },
      max: { x: bbox.max[0], y: bbox.max[1], z: bbox.max[2] },
      top:    { x: center.x,   y: center.y,   z: bbox.max[2] },
      bottom: { x: center.x,   y: center.y,   z: bbox.min[2] },
      front:  { x: center.x,   y: bbox.min[1], z: center.z   },
      back:   { x: center.x,   y: bbox.max[1], z: center.z   },
      left:   { x: bbox.min[0], y: center.y,   z: center.z   },
      right:  { x: bbox.max[0], y: center.y,   z: center.z   },
    };
  } else {
    out.bbox = null;
    out.bbox_note = "scene not yet evaluated — try again after a microtask, or after creating/updating this part";
  }
  return {
    status: "success",
    result: JSON.stringify(out),
    partId,
    display: {
      summary: [text("⌕ Inspect "), link(partId, docStore)],
      affectedPartIds: [partId],
    },
  };
}

// ---------------------------------------------------------------------------
// describe_scene — one-call snapshot of every part's position, bbox, and
// material. Replaces the AI's habit of calling `inspect_part` 8× in a row
// while debugging coordinate drift. Accepts optional `part_ids` to scope
// the response, and `limit` to cap output. Force-evaluates the scene once
// at the start so positions are fresh.
// ---------------------------------------------------------------------------
function executeDescribeScene(
  args: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult {
  const requestedIds = args.part_ids as string[] | undefined;
  const limit = (args.limit as number | undefined) ?? 100;
  // Force one eval up front so every snapshot below can reuse the fresh
  // scene cache without N separate force-eval calls.
  const engineStore = useEngineStore.getState();
  const engine = engineStore.engine;
  if (engine && (!engineStore.scene || docStore.parts.length > 0)) {
    try {
      const scene = engine.evaluate(docStore.document, { skipClashDetection: true });
      engineStore.setScene(scene);
    } catch {
      // If eval fails, fall through — we'll emit whatever we have.
    }
  }
  const ids = requestedIds && requestedIds.length > 0
    ? requestedIds
    : docStore.parts.map((p) => p.id).slice(0, limit);
  const missing: string[] = [];
  const parts: Array<Record<string, unknown>> = [];
  for (const id of ids) {
    const part = docStore.partIndex.get(id);
    if (!part) {
      missing.push(id);
      continue;
    }
    parts.push(buildPartSnapshot(id, docStore));
  }
  return {
    status: "success",
    result: JSON.stringify({
      ok: true,
      part_count: parts.length,
      parts,
      missing: missing.length > 0 ? missing : undefined,
    }),
    display: {
      summary: [text(`⌕ Describe scene (${parts.length} parts)`)],
      affectedPartIds: [],
    },
  };
}

// ---------------------------------------------------------------------------
// place — anchor-based positioning. Translates a part so that its `from`
// anchor lands on the world-space `to` point (or another part's anchor).
// Optional `align` rotates the part so an axis on it matches a world-space
// direction. This is the high-level alternative to computing translate+rotate
// by hand; the AI can say "place handlebar's center on stem's top".
// ---------------------------------------------------------------------------

type AnchorName =
  | "center" | "min" | "max"
  | "top" | "bottom" | "front" | "back" | "left" | "right";

type AnchorRef =
  | AnchorName
  | Vec3
  | { part: string; anchor: AnchorName };

function resolveAnchorOnPart(
  partId: string,
  anchor: AnchorName,
  docStore: DocStore,
): Vec3 | null {
  const bbox = computePartWorldBboxForceEval(partId, docStore);
  if (!bbox) return null;
  const c = bboxCenter(bbox);
  switch (anchor) {
    case "center": return c;
    case "min": return { x: bbox.min[0], y: bbox.min[1], z: bbox.min[2] };
    case "max": return { x: bbox.max[0], y: bbox.max[1], z: bbox.max[2] };
    case "top":    return { x: c.x, y: c.y, z: bbox.max[2] };
    case "bottom": return { x: c.x, y: c.y, z: bbox.min[2] };
    case "front":  return { x: c.x, y: bbox.min[1], z: c.z };
    case "back":   return { x: c.x, y: bbox.max[1], z: c.z };
    case "left":   return { x: bbox.min[0], y: c.y, z: c.z };
    case "right":  return { x: bbox.max[0], y: c.y, z: c.z };
  }
}

const ANCHOR_NAMES: AnchorName[] = [
  "center", "min", "max", "top", "bottom", "front", "back", "left", "right",
];

function resolveAnchor(
  ref: AnchorRef | undefined,
  selfPartId: string,
  docStore: DocStore,
): Vec3 | null {
  if (!ref) return null;
  if (typeof ref === "string") {
    return resolveAnchorOnPart(selfPartId, ref as AnchorName, docStore);
  }
  if ("x" in ref && "y" in ref && "z" in ref) {
    return { x: (ref as Vec3).x, y: (ref as Vec3).y, z: (ref as Vec3).z };
  }
  if ("part" in ref && "anchor" in ref) {
    return resolveAnchorOnPart((ref as { part: string }).part, (ref as { anchor: AnchorName }).anchor, docStore);
  }
  return null;
}

function executePlace(
  args: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult {
  const partId = args.part_id as string;
  if (!partId) return { status: "error", result: "place requires part_id" };
  const err = validatePartId(partId, docStore, "place part_id");
  if (err) return err;

  const from = (args.from as AnchorRef | undefined) ?? ("center" as AnchorName);
  const to = args.to as AnchorRef | undefined;
  if (!to) return { status: "error", result: "place requires a `to` anchor (Vec3, named anchor, or {part,anchor})" };

  const fromWorld = resolveAnchor(from, partId, docStore);
  if (!fromWorld) {
    return {
      status: "error",
      result: structuredError(
        "ANCHOR_UNRESOLVED",
        "place: could not resolve `from` anchor",
        {
          part_id: partId,
          from,
          available_anchors: ANCHOR_NAMES,
          suggestion:
            "If you just created this part, the scene is still evaluating. Try `inspect_part` first, or pass `from` as an explicit Vec3 (e.g. {x,y,z}).",
        },
      ),
    };
  }
  const toWorld = resolveAnchor(to, partId, docStore);
  if (!toWorld) {
    return {
      status: "error",
      result: structuredError(
        "ANCHOR_UNRESOLVED",
        "place: could not resolve `to` anchor",
        {
          to,
          available_anchors: ANCHOR_NAMES,
          suggestion:
            "If `to` references another part, that part may not exist or may not yet be evaluated. Try `inspect_part` on it first, or pass `to` as an explicit Vec3.",
        },
      ),
    };
  }

  const currentOffset = getCurrentOffset(partId, docStore);
  const newOffset = {
    x: currentOffset.x + (toWorld.x - fromWorld.x),
    y: currentOffset.y + (toWorld.y - fromWorld.y),
    z: currentOffset.z + (toWorld.z - fromWorld.z),
  };
  docStore.setTranslation(partId, newOffset);

  return {
    status: "success",
    result: snapshotJson(partId, docStore, {
      applied: "place",
      from_world: fromWorld,
      to_world: toWorld,
    }),
    partId,
    display: {
      summary: [
        text("◎ Place "),
        link(partId, docStore),
        text(` at (${toWorld.x.toFixed(1)}, ${toWorld.y.toFixed(1)}, ${toWorld.z.toFixed(1)})`),
      ],
      fields: [
        { label: "from", value: typeof from === "string" ? from : JSON.stringify(from) },
        { label: "to", value: typeof to === "string" ? to : JSON.stringify(to) },
      ],
      affectedPartIds: [partId],
    },
  };
}



/** Apply a user-provided name to a freshly created part, if any. */
function applyName(docStore: DocStore, partId: string | null | undefined, name: string | undefined): void {
  if (!partId || !name) return;
  if (typeof docStore.renamePart !== "function") return;
  try {
    docStore.renamePart(partId, name);
  } catch {
    // non-fatal: rename failures shouldn't break the create call
  }
}


/** Tool-name aliases — Claude occasionally hallucinates extra underscores in
 * snake_case (`sketch_2_d` instead of `sketch_2d`). Normalize at the boundary
 * so legitimate prompts don't fail just because the model picked the wrong
 * shape of underscore. */
const CREATE_TYPE_ALIASES: Record<string, string> = {
  sketch_2_d: "sketch_2d",
  text_2_d: "text_2d",
};


function executeRead(
  partId: string | undefined,
  docStore: DocStore,
): ExecutionResult {
  try {
    if (!partId) {
      const parts = docStore.parts.map((p) => ({
        id: p.id,
        name: p.name,
        kind: p.kind,
      }));
      return { status: "success", result: JSON.stringify(parts) };
    }

    const part = docStore.partIndex.get(partId);
    if (!part) return { status: "error", result: `Part ${partId} not found` };

    const doc = docStore.document;
    const nodes: Array<{ nodeId: string; type: string; params: Record<string, unknown> }> = [];

    const walkNode = (nodeId: string | number) => {
      const node = doc.nodes[String(nodeId)];
      if (!node) return;
      const op = node.op;
      const opType = (op as { type?: string }).type;
      if (opType) {
        const params = { ...op } as Record<string, unknown>;
        delete params.type;
        nodes.push({ nodeId: String(nodeId), type: opType, params });
      }
    };

    // All PartInfo variants have translateNodeId as the outermost node
    const p = part as { translateNodeId?: number | string };
    if (p.translateNodeId !== undefined) {
      walkNode(p.translateNodeId);
    }

    return {
      status: "success",
      result: JSON.stringify({
        id: part.id,
        name: part.name,
        kind: part.kind,
        nodes,
      }),
      partId,
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Read failed" };
  }
}




// ---------------------------------------------------------------------------
// search_parts / place_part — stdlib parts library integration.
// ---------------------------------------------------------------------------

/**
 * Pull the parts manifest from the running engine's kernel module. Returns
 * null if the engine hasn't booted yet (searching before init just yields an
 * empty list to the model).
 */
function getPartsManifest(): PartManifestEntry[] | null {
  try {
    const engineState = useEngineStore.getState() as unknown as {
      kernel?: Parameters<typeof loadPartsManifestFromEngine>[0];
      engine?: { kernel?: Parameters<typeof loadPartsManifestFromEngine>[0] };
    };
    const kernel = engineState.kernel ?? engineState.engine?.kernel;
    if (!kernel) return null;
    return loadPartsManifestFromEngine(kernel);
  } catch (err) {
    console.warn("[executors] parts manifest unavailable:", err);
    return null;
  }
}

function executeSearchParts(
  args: Record<string, unknown>,
  _docStore: DocStore,
): ExecutionResult {
  const manifest = getPartsManifest();
  if (!manifest || manifest.length === 0) {
    return {
      status: "success",
      result: JSON.stringify({ results: [], note: "parts library not loaded or empty" }),
    };
  }
  const query = typeof args.query === "string" ? args.query : "";
  const category = typeof args.category === "string" ? args.category : undefined;
  const limit = typeof args.limit === "number" ? args.limit : 10;
  const hits = searchPartsFromEngine(manifest, { query, category, limit });
  const results = hits.map((p) => ({
    id: `std:${p.id}`,
    name: p.name,
    category: p.category,
    description: p.description,
    version: p.version,
    params: p.params,
    xrefs: p.xrefs,
  }));
  return {
    status: "success",
    result: JSON.stringify({ results, count: results.length }),
    display: {
      summary: [text(`🔍 search_parts: ${results.length} match${results.length === 1 ? "" : "es"} for "${query}"`)],
      fields: results.slice(0, 5).map((r) => ({ label: r.id, value: r.name })),
      affectedPartIds: [],
    },
  };
}

function executePlacePart(
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const path = typeof args.path === "string" ? args.path : null;
  if (!path) {
    return { status: "error", result: "place_part: missing `path` argument" };
  }
  const manifest = getPartsManifest();
  if (!manifest) {
    return { status: "error", result: "parts library not loaded yet — retry after init" };
  }
  const idLookup = path.startsWith("std:") ? path.slice(4) : path;
  const entry = manifest.find((p) => p.id === idLookup);
  if (!entry) {
    return { status: "error", result: `place_part: unknown part "${path}"` };
  }
  const userParams = (args.params && typeof args.params === "object")
    ? (args.params as Record<string, unknown>)
    : {};
  const params = { ...defaultParamsForEngine(entry), ...userParams };
  const version = entry.version;

  const partId = docStore.addPartInstance(path, version, params);
  if (!partId) {
    return { status: "error", result: "place_part: document store refused insertion" };
  }

  // Apply optional position after insertion.
  const position = args.position as Vec3 | undefined;
  if (position && typeof position === "object") {
    const offset: Vec3 = {
      x: typeof position.x === "number" ? position.x : 0,
      y: typeof position.y === "number" ? position.y : 0,
      z: typeof position.z === "number" ? position.z : 0,
    };
    docStore.setTranslation(partId, offset);
  }

  const displayName = typeof args.name === "string" ? args.name : entry.name;
  applyName(docStore, partId, displayName);
  uiStore.select(partId);

  return {
    status: "success",
    result: `Placed part ${path} with id: ${partId}`,
    partId,
    display: {
      summary: [text(`+ ${entry.name} (${entry.category}) `), link(partId, docStore)],
      fields: Object.entries(params).map(([k, v]) => ({
        label: k,
        value: String(v),
      })),
      affectedPartIds: [partId],
    },
  };
}
