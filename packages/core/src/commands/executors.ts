import type { ExecutionResult, ExecutionDisplay, SummarySegment } from "./types.js";
import type { PlannedResponse, ToolOutcome } from "./registry.js";
import { commandRegistry } from "./registry.js";
import { useDocumentStore } from "../stores/document-store.js";
import { useEngineStore } from "../stores/engine-store.js";
import { useUiStore } from "../stores/ui-store.js";
import { vec3Cross, vec3Normalize } from "@vcad/ir";
import type { Vec3, PathCurve } from "@vcad/ir";
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

/** Apply an Euler-XYZ rotation (degrees, applied X then Y then Z, the same
 *  convention vcad-kernel uses) to a point. */
function rotateVec3(p: Vec3, eulerDeg: Vec3): Vec3 {
  const rad = (d: number) => (d * Math.PI) / 180;
  const cx = Math.cos(rad(eulerDeg.x)), sx = Math.sin(rad(eulerDeg.x));
  const cy = Math.cos(rad(eulerDeg.y)), sy = Math.sin(rad(eulerDeg.y));
  const cz = Math.cos(rad(eulerDeg.z)), sz = Math.sin(rad(eulerDeg.z));
  // Rx
  let x = p.x, y = p.y * cx - p.z * sx, z = p.y * sx + p.z * cx;
  // Ry
  let x2 = x * cy + z * sy, y2 = y, z2 = -x * sy + z * cy;
  x = x2; y = y2; z = z2;
  // Rz
  x2 = x * cz - y * sz; y2 = x * sz + y * cz; z2 = z;
  return { x: x2, y: y2, z: z2 };
}

/** Invert an Euler-XYZ rotation by negating and reversing order. Sufficient
 *  for our purposes because rotateVec3 + inverseRotateVec3 round-trip. */
function inverseRotateVec3(p: Vec3, eulerDeg: Vec3): Vec3 {
  const rad = (d: number) => (d * Math.PI) / 180;
  const cx = Math.cos(rad(eulerDeg.x)), sx = Math.sin(rad(eulerDeg.x));
  const cy = Math.cos(rad(eulerDeg.y)), sy = Math.sin(rad(eulerDeg.y));
  const cz = Math.cos(rad(eulerDeg.z)), sz = Math.sin(rad(eulerDeg.z));
  // Inverse order: Rz^-1, Ry^-1, Rx^-1
  let x = p.x * cz + p.y * sz, y = -p.x * sz + p.y * cz, z = p.z;
  let x2 = x * cy - z * sy, y2 = y, z2 = x * sy + z * cy;
  x = x2; y = y2; z = z2;
  x2 = x; y2 = y * cx + z * sx; z2 = -y * sx + z * cx;
  return { x: x2, y: y2, z: z2 };
}

/** Build a closed-loop circular sketch profile in the plane (x_dir, y_dir)
 *  centered on origin. Two semicircle arcs — matches the convention in the
 *  system prompt (sketches must be closed and have ≥2 segments). */
function circleProfileSegments(radius: number): unknown[] {
  return [
    { type: "Arc", start: { x: radius, y: 0 }, end: { x: -radius, y: 0 }, center: { x: 0, y: 0 }, ccw: false },
    { type: "Arc", start: { x: -radius, y: 0 }, end: { x: radius, y: 0 }, center: { x: 0, y: 0 }, ccw: false },
  ];
}

/** Derive two unit vectors perpendicular to `dir` to form a sketch plane.
 *  `dir` need not be unit-length. Returns null if `dir` is zero. */
function perpendicularBasis(dir: Vec3): { xDir: Vec3; yDir: Vec3 } | null {
  const len = Math.hypot(dir.x, dir.y, dir.z);
  if (len < 1e-9) return null;
  const d = { x: dir.x / len, y: dir.y / len, z: dir.z / len };
  // Pick a reference axis that's not near-parallel to d.
  const ref: Vec3 = Math.abs(d.z) < 0.9 ? { x: 0, y: 0, z: 1 } : { x: 1, y: 0, z: 0 };
  const yDir = vec3Normalize(vec3Cross(d, ref));
  const xDir = vec3Normalize(vec3Cross(yDir, d));
  return { xDir, yDir };
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

type Vec2Lite = { x: number; y: number };
type SketchSegLite = {
  type: "Line" | "Arc";
  start: Vec2Lite;
  end: Vec2Lite;
  center?: Vec2Lite;
  ccw?: boolean;
};

/** Validate a sketch forms a closed loop with matched segment endpoints.
 *  Returns an error result or null if valid. */
function validateSketch(segments: unknown[]): ExecutionResult | null {
  if (!Array.isArray(segments) || segments.length === 0) {
    return { status: "error", result: "sketch must have at least one segment" };
  }
  if (segments.length < 2) {
    return {
      status: "error",
      result: "sketch must have at least 2 segments to form a closed loop. A single arc is not a closed profile — add a line or another arc to close it.",
    };
  }

  const segs = segments as SketchSegLite[];
  const eps = 1e-3; // 1 micron
  const matches = (a: Vec2Lite, b: Vec2Lite) =>
    Math.abs(a.x - b.x) < eps && Math.abs(a.y - b.y) < eps;

  for (let i = 0; i < segs.length; i++) {
    const curr = segs[i]!;
    const next = segs[(i + 1) % segs.length]!;
    if (!curr.start || !curr.end || !next.start) {
      return { status: "error", result: `segment ${i} missing start/end points` };
    }
    if (!matches(curr.end, next.start)) {
      return {
        status: "error",
        result: `sketch is not closed: segment ${i} ends at (${curr.end.x}, ${curr.end.y}) but segment ${(i + 1) % segs.length} starts at (${next.start.x}, ${next.start.y}). Each segment's end must match the next segment's start.`,
      };
    }
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
  // Prefer the Rust planner when the kernel exposes plan_chat_tool.
  // Falls through to the TS path when the binding isn't loaded yet,
  // when Rust returns an outcome shape we haven't wired to a docstore
  // method, or when anything in the dispatch trips. The TS fallback is
  // the authoritative path for anything Rust doesn't yet cover
  // (sketched extrudes, booleans, transforms, etc).
  const wasmResult = tryExecuteViaWasm(tool, args, docStore);
  if (wasmResult) {
    wasmResult.duration = performance.now() - t0;
    return wasmResult;
  }
  const result = executeCrudInner(tool, args, docStore, uiStore);
  result.duration = performance.now() - t0;
  return result;
}

/**
 * Ask the Rust planner what should happen, then dispatch the returned
 * `ToolOutcome` through the existing CRDT-backed docstore methods.
 * Returns `null` when wasm isn't available or when the planner's
 * response doesn't fit into one of the four outcome shapes we know how
 * to apply — both cases fall through to the legacy TS `executeCrudInner`.
 */
function tryExecuteViaWasm(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult | null {
  // Skip reads — the TS path already handles them cheaply without any
  // mutation, and the planner would just return a no-outcome
  // PlannedResponse we'd still have to translate.
  if (tool === "read") return null;
  // Skip set_material when bulk inputs are present — the Rust planner only
  // knows the single-part shape and would reject `part_ids` / `selector`
  // with an error before TS got a chance.
  if (
    tool === "set_material" &&
    (args.selector !== undefined || Array.isArray(args.part_ids))
  ) {
    return null;
  }
  // Skip tools the Rust planner doesn't know about. Without this, an AI
  // call like `circular_pattern(...)` short-circuits with the planner's
  // "unknown tool" error before the TS dispatch can route it. Keep this
  // list in sync with the top-level tools handled in `executeCrudInner`
  // that aren't in the Rust planner's match.
  const TS_ONLY_TOOLS = new Set([
    "tube",
    "polyline_tube",
    "linear_pattern",
    "circular_pattern",
    "mirror",
    "inspect_part",
    "place",
    "describe_scene",
    "batch",
  ]);
  if (TS_ONLY_TOOLS.has(tool)) return null;

  const docJson = JSON.stringify(docStore.document);
  const planned: PlannedResponse | null = commandRegistry.planCrud(
    tool,
    args,
    docJson,
  );
  if (!planned) return null;
  if (planned.status === "error") {
    return { status: "error", result: planned.result };
  }
  if (!planned.outcome) return null;

  try {
    return dispatchOutcome(planned.result, planned.outcome, docStore);
  } catch (e) {
    console.warn("[executors] wasm dispatch failed, falling back to TS:", e);
    return null;
  }
}

/** Route a planned `ToolOutcome` through the closest CRDT-aware
 *  docstore method. Returns an `ExecutionResult` with the ids the
 *  engine assigned, or `null` to fall through to the legacy TS path.
 *
 *  `add_feature` deliberately falls through: the Rust planner emits
 *  a `vcad_ir::CsgOp`-shaped payload, but the web's CRDT engine
 *  (`WasmDocumentEngine::add_feature`) parses its input as
 *  `vcad_app::feature::FeatureInput` — a different, web-specific schema
 *  with flat primitive fields (`size_x` / `size_y` / `size_z`), string
 *  stable IDs, boolean-kind inner enum, and JSON-serialized sketches.
 *  The two shapes don't round-trip, and the wasm engine silently
 *  ignores parse failures, so routing `add_feature` through here
 *  would falsely report "success" on a no-op mutation. Until the Rust
 *  planner emits `FeatureInput`-shaped payloads (or the TUI moves onto
 *  the shared `DocumentApi` pipeline), the TS legacy path in
 *  `executeCrudInner` remains the authority for creation on the web.
 *
 *  The three non-create outcomes (`remove_part`, `set_part_material`,
 *  `update_params`) are dispatched here because they hit higher-level
 *  docstore methods that take plain part-id / key / value arguments
 *  rather than the FeatureInput schema. */
function dispatchOutcome(
  plannedResult: string,
  outcome: ToolOutcome,
  docStore: DocStore,
): ExecutionResult | null {
  switch (outcome.kind) {
    case "add_feature": {
      // See doc comment above — fall through to TS until Rust emits FeatureInput.
      return null;
    }
    case "remove_part": {
      docStore.removePart(outcome.part_id);
      return { status: "success", result: plannedResult };
    }
    case "set_part_material": {
      docStore.setPartMaterial(outcome.part_id, outcome.material);
      return { status: "success", result: plannedResult, partId: outcome.part_id };
    }
    case "update_params": {
      for (const [key, value] of Object.entries(outcome.params)) {
        if (key === "type") continue;
        docStore.setFeatureParam(outcome.node_id, key, value as never);
      }
      return { status: "success", result: plannedResult, nodeId: outcome.node_id };
    }
  }
}

function executeCrudInner(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  switch (tool) {
    case "create":
      return executeCreateWithName(
        args.type as string,
        args.params as Record<string, unknown>,
        args.parent_part_id as string | undefined,
        args.name as string | undefined,
        docStore,
        uiStore,
      );
    case "read":
      return executeRead(args.part_id as string | undefined, docStore);
    case "update":
      return executeUpdate(
        args.node_id as string,
        args.params as Record<string, unknown>,
        docStore,
      );
    case "delete":
      return executeDelete(args.part_id as string, docStore, uiStore);
    case "set_material":
      return executeSetMaterial(args, docStore);
    case "inspect_part":
      return executeInspectPart(args.part_id as string, docStore);
    case "place":
      return executePlace(args, docStore);
    case "describe_scene":
      return executeDescribeScene(args, docStore);
    case "tube":
      return executeCreateWithName(
        tool,
        args as Record<string, unknown>,
        undefined,
        args.name as string | undefined,
        docStore,
        uiStore,
      );
    case "polyline_tube":
      // polyline_tube names each segment individually inside executeCreate,
      // so don't let executeCreateWithName rename the last segment on top.
      return executeCreateWithName(
        tool,
        args as Record<string, unknown>,
        undefined,
        undefined,
        docStore,
        uiStore,
      );
    case "linear_pattern":
    case "circular_pattern":
      // Pattern primitives are advertised as top-level tools in
      // static-schemas.ts; route them through executeCreate so the AI can
      // call either `circular_pattern(...)` or `create(type:"circular_pattern",
      // params:...)` and get the same result. Without this case, the AI
      // gets `Unknown tool: circular_pattern` and falls back to manually
      // unrolling the pattern (e.g. 16 separate spoke tubes).
      return executeCreateWithName(
        tool,
        args as Record<string, unknown>,
        undefined,
        args.name as string | undefined,
        docStore,
        uiStore,
      );
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

/** Resolve a part-id selector to a concrete list of ids. Selectors let the
 *  AI assign a material to many parts in one call (e.g. all spokes, all
 *  parts whose name starts with "Frame") instead of issuing N separate
 *  `set_material` calls — which is what produced the 18-call material run
 *  in the bicycle transcript. */
function resolveSelector(
  selector: unknown,
  docStore: DocStore,
): string[] {
  if (!selector || typeof selector !== "object") return [];
  const sel = selector as { by?: string; value?: string };
  if (!sel.by || sel.value == null) return [];
  const value = String(sel.value).toLowerCase();
  const ids: string[] = [];
  for (const p of docStore.parts) {
    let match = false;
    switch (sel.by) {
      case "kind":
        match = p.kind?.toLowerCase() === value;
        break;
      case "name_prefix":
        match = !!p.name && p.name.toLowerCase().startsWith(value);
        break;
      case "name_contains":
        match = !!p.name && p.name.toLowerCase().includes(value);
        break;
      case "name_equals":
        match = !!p.name && p.name.toLowerCase() === value;
        break;
    }
    if (match) ids.push(p.id);
  }
  return ids;
}

function executeSetMaterial(
  args: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult {
  const materialKey = args.material as string | undefined;
  if (!materialKey) return { status: "error", result: "set_material requires material key" };

  // Resolve the target list. Three input shapes are accepted:
  //   { part_id: "p1", material: "..." }              — one part
  //   { part_ids: ["p1","p2","p3"], material: "..." } — explicit batch
  //   { selector: { by, value }, material: "..." }    — match by kind/name
  // Falling back to single-part keeps the old call sites working.
  let targetIds: string[] = [];
  const partIds = args.part_ids as string[] | undefined;
  const partId = args.part_id as string | undefined;
  const selector = args.selector;
  if (Array.isArray(partIds) && partIds.length > 0) {
    targetIds = partIds.slice();
  } else if (partId) {
    targetIds = [partId];
  } else if (selector) {
    targetIds = resolveSelector(selector, docStore);
    if (targetIds.length === 0) {
      return {
        status: "error",
        result: structuredError(
          "SELECTOR_EMPTY",
          "set_material: selector matched zero parts",
          { selector, suggestion: "Inspect the scene snapshot to confirm the selector value (case-insensitive)." },
        ),
      };
    }
  } else {
    return { status: "error", result: "set_material requires one of: part_id, part_ids[], or selector{by,value}" };
  }

  const succeeded: string[] = [];
  const failed: Array<{ id: string; reason: string }> = [];
  for (const id of targetIds) {
    const err = validatePartId(id, docStore, "set_material part_id");
    if (err) {
      failed.push({ id, reason: typeof err.result === "string" ? err.result : "invalid id" });
      continue;
    }
    try {
      docStore.setPartMaterial(id, materialKey);
      succeeded.push(id);
    } catch (e) {
      failed.push({ id, reason: e instanceof Error ? e.message : "set_material failed" });
    }
  }

  return {
    status: failed.length > 0 && succeeded.length === 0 ? "error" : "success",
    result: JSON.stringify({
      ok: failed.length === 0,
      applied: "set_material",
      material: materialKey,
      part_ids: succeeded,
      failed: failed.length > 0 ? failed : undefined,
    }),
    partId: succeeded[0],
    display: {
      summary: [
        text("⬤ Material"),
        ...(targetIds.length === 1 ? [text(" "), link(targetIds[0]!, docStore)] : [text(` ×${succeeded.length}`)]),
        text(` = ${materialKey}`),
      ],
      fields: [{ label: "material", value: materialKey }],
      affectedPartIds: succeeded,
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

/** Wrap executeCreate with an optional post-rename that keeps display segments in sync. */
function executeCreateWithName(
  type: string,
  params: Record<string, unknown>,
  parentPartId: string | undefined,
  name: string | undefined,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const result = executeCreate(type, params, parentPartId, docStore, uiStore);
  if (result.status === "success" && name && result.partId) {
    applyName(docStore, result.partId, name);
    // Patch any partLink segments in the display that reference the newly named part
    if (result.display?.summary) {
      for (const seg of result.display.summary) {
        if (seg.type === "partLink" && seg.partId === result.partId) {
          seg.name = name;
        }
      }
    }
  }
  return result;
}

/** Tool-name aliases — Claude occasionally hallucinates extra underscores in
 * snake_case (`sketch_2_d` instead of `sketch_2d`). Normalize at the boundary
 * so legitimate prompts don't fail just because the model picked the wrong
 * shape of underscore. */
const CREATE_TYPE_ALIASES: Record<string, string> = {
  sketch_2_d: "sketch_2d",
  text_2_d: "text_2d",
};

function executeCreate(
  type: string,
  params: Record<string, unknown>,
  parentPartId: string | undefined,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  type = CREATE_TYPE_ALIASES[type] ?? type;
  try {
    switch (type) {
      case "cube":
      case "cylinder":
      case "sphere": {
        const partId = docStore.addPrimitive(type as "cube" | "cylinder" | "sphere");
        // addPrimitive returns "" when the CRDT engine is null/freed — don't
        // schedule a follow-up param update against a bogus id, it would crash
        // when the deferred callback hits `_crdtEngine!` on the same dead engine.
        if (!partId) {
          return { status: "error", result: "engine not ready" };
        }
        if (params && Object.keys(params).length > 0) {
          // Defer param update to next microtask to avoid WASM re-entrant borrow.
          // addPrimitive's set() triggers subscribers synchronously — calling
          // setFeatureParam in the same tick would re-enter the CRDT engine.
          const capitalizedType = type.charAt(0).toUpperCase() + type.slice(1);
          setTimeout(() => {
            docStore.updatePrimitiveOp(partId, { type: capitalizedType, ...params });
          }, 0);
        }
        uiStore.select(partId);

        const fields: Array<{ label: string; value: string }> = [];
        if (type === "cube" && params.size) {
          const s = params.size as { x: number; y: number; z: number };
          fields.push({ label: "size", value: `${s.x}×${s.y}×${s.z} mm` });
        }
        if (type === "cylinder") {
          if (params.radius != null) fields.push({ label: "radius", value: `${params.radius} mm` });
          if (params.height != null) fields.push({ label: "height", value: `${params.height} mm` });
        }
        if (type === "sphere" && params.radius != null) {
          fields.push({ label: "radius", value: `${params.radius} mm` });
        }

        const sizeSuffix = fields.length > 0 ? ` ${fields.map((f) => f.value).join(", ")}` : "";
        const capitalized = type.charAt(0).toUpperCase() + type.slice(1);

        return {
          status: "success",
          result: snapshotJson(partId, docStore, { created: type }),
          partId,
          display: {
            summary: [text(`+ ${capitalized}${sizeSuffix} `), link(partId, docStore)],
            fields,
            affectedPartIds: [partId],
          },
        };
      }

      case "cone":
        return { status: "error", result: "Cone primitive not yet supported via addPrimitive" };

      case "tube": {
        // High-level primitive: a cylindrical pipe between two world-space
        // points. Desugars to a sweep of a circular sketch along a straight
        // segment. Gives the AI a one-call "draw me a pipe from A to B with
        // radius r" — no perpendicular-basis math, no tube-length
        // calculation. Caller supplies `start`, `end`, optional `radius`
        // (default 5mm) and optional `arc_segments` (default 16).
        const start = params.start as Vec3 | undefined;
        const end = params.end as Vec3 | undefined;
        if (!start || !end) return { status: "error", result: "tube requires start and end points" };
        const radius = (params.radius as number | undefined) ?? 5;
        if (radius <= 0) return { status: "error", result: "tube radius must be positive" };
        const arcSegments = (params.arc_segments as number | undefined) ?? 16;
        const dir = { x: end.x - start.x, y: end.y - start.y, z: end.z - start.z };
        const length = Math.hypot(dir.x, dir.y, dir.z);
        if (length < 1e-6) return { status: "error", result: "tube start and end are coincident" };
        const basis = perpendicularBasis(dir);
        if (!basis) return { status: "error", result: "tube direction is degenerate" };
        const normal = vec3Normalize(vec3Cross(basis.xDir, basis.yDir));
        const plane = { type: "face" as const, origin: start, xDir: basis.xDir, yDir: basis.yDir, normal };
        const path: PathCurve = { type: "Line", start, end };
        const partId = docStore.addSweep(
          plane,
          start,
          circleProfileSegments(radius) as never[],
          path,
          {},
        );
        if (!partId) return { status: "error", result: "tube creation failed" };
        // Bump arc count by updating the sweep op's arc_segments param. Best-effort.
        const info = docStore.partIndex.get(partId) as { sweepNodeId?: number | string } | undefined;
        if (info?.sweepNodeId !== undefined) {
          try {
            docStore.setFeatureParam(partId, "arc_segments", { F64: arcSegments } as never);
          } catch {
            // non-fatal
          }
        }
        uiStore.select(partId);
        return {
          status: "success",
          result: snapshotJson(partId, docStore, {
            created: "tube",
            radius,
            length,
          }),
          partId,
          display: {
            summary: [
              text(`∥ Tube r=${radius}mm, L=${length.toFixed(1)}mm → `),
              link(partId, docStore),
            ],
            fields: [
              { label: "start", value: `(${start.x}, ${start.y}, ${start.z})` },
              { label: "end", value: `(${end.x}, ${end.y}, ${end.z})` },
              { label: "radius", value: `${radius} mm` },
              { label: "length", value: `${length.toFixed(2)} mm` },
            ],
            affectedPartIds: [partId],
          },
        };
      }

      case "polyline_tube": {
        // Chain of tubes through a sequence of points. Each segment gets its
        // own part (kept separate so later edits target individual segments).
        // Typical use: bike frames, piping, cable runs — cases where authors
        // want to specify joints as points instead of computing each tube's
        // direction vector and length by hand.
        const points = params.points as Vec3[] | undefined;
        if (!points || points.length < 2) {
          return { status: "error", result: "polyline_tube requires points[] with at least 2 entries" };
        }
        const radius = (params.radius as number | undefined) ?? 5;
        const nameBase = (params.name as string | undefined) ?? "Tube";
        const createdIds: string[] = [];
        for (let i = 0; i < points.length - 1; i++) {
          const segRes = executeCreate(
            "tube",
            { start: points[i], end: points[i + 1], radius, arc_segments: params.arc_segments },
            undefined,
            docStore,
            uiStore,
          );
          if (segRes.status !== "success" || !segRes.partId) {
            // Roll back: delete anything we already created to keep the doc clean.
            for (const id of createdIds) {
              try { docStore.removePart(id); } catch { /* ignore */ }
            }
            return { status: "error", result: `polyline_tube: segment ${i} failed — ${segRes.result}` };
          }
          createdIds.push(segRes.partId);
          if (nameBase && typeof docStore.renamePart === "function") {
            try {
              docStore.renamePart(segRes.partId, `${nameBase} ${i + 1}`);
            } catch {
              // non-fatal
            }
          }
        }
        return {
          status: "success",
          result: `Created ${createdIds.length} tube segments: ${createdIds.join(", ")}`,
          partId: createdIds[createdIds.length - 1]!,
          display: {
            summary: [
              text(`∥∥ Polyline tube (${createdIds.length} segs, r=${radius}mm)`),
            ],
            fields: [
              { label: "segments", value: `${createdIds.length}` },
              { label: "points", value: `${points.length}` },
              { label: "radius", value: `${radius} mm` },
            ],
            affectedPartIds: createdIds,
          },
        };
      }

      case "linear_pattern": {
        // Repeat a child part along a direction. Vastly preferable to the AI
        // manually creating N copies because the kernel evaluates the pattern
        // as one node (cheap re-eval) and edits to the source propagate to
        // every instance.
        const child = params.child as string;
        const direction = params.direction as Vec3 | undefined;
        const count = params.count as number | undefined;
        const spacing = params.spacing as number | undefined;
        if (!child) return { status: "error", result: "linear_pattern requires child part_id" };
        if (!direction) return { status: "error", result: "linear_pattern requires direction (Vec3)" };
        if (!count || count < 1) return { status: "error", result: "linear_pattern requires count ≥ 1" };
        if (spacing == null) return { status: "error", result: "linear_pattern requires spacing" };
        const err = validatePartId(child, docStore, "linear_pattern child");
        if (err) return err;
        const partId = docStore.addLinearPattern(child, direction, count, spacing);
        if (!partId) return { status: "error", result: "linear_pattern failed" };
        uiStore.select(partId);
        return {
          status: "success",
          result: snapshotJson(partId, docStore, {
            created: "linear_pattern",
            child,
            count,
            spacing,
            direction,
          }),
          partId,
          display: {
            summary: [
              text(`▦ Linear pattern ×${count} → `),
              link(partId, docStore),
            ],
            fields: [
              { label: "child", value: child },
              { label: "direction", value: `(${direction.x}, ${direction.y}, ${direction.z})` },
              { label: "count", value: `${count}` },
              { label: "spacing", value: `${spacing} mm` },
            ],
            affectedPartIds: [partId],
          },
        };
      }

      case "circular_pattern": {
        // Repeat a child part around an axis. Use this for spokes, bolt
        // circles, fan blades — any radial array. One node, one eval cost,
        // identical instances.
        const child = params.child as string;
        const axisOrigin = params.axis_origin as Vec3 | undefined;
        const axisDir = params.axis_dir as Vec3 | undefined;
        const count = params.count as number | undefined;
        const angleDeg = params.angle_deg as number | undefined;
        if (!child) return { status: "error", result: "circular_pattern requires child part_id" };
        if (!axisOrigin) return { status: "error", result: "circular_pattern requires axis_origin (Vec3)" };
        if (!axisDir) return { status: "error", result: "circular_pattern requires axis_dir (Vec3)" };
        if (!count || count < 1) return { status: "error", result: "circular_pattern requires count ≥ 1" };
        if (angleDeg == null) return { status: "error", result: "circular_pattern requires angle_deg" };
        const err = validatePartId(child, docStore, "circular_pattern child");
        if (err) return err;
        const partId = docStore.addCircularPattern(child, axisOrigin, axisDir, count, angleDeg);
        if (!partId) return { status: "error", result: "circular_pattern failed" };
        uiStore.select(partId);
        return {
          status: "success",
          result: snapshotJson(partId, docStore, {
            created: "circular_pattern",
            child,
            count,
            angle_deg: angleDeg,
            axis_origin: axisOrigin,
            axis_dir: axisDir,
          }),
          partId,
          display: {
            summary: [
              text(`◯ Circular pattern ×${count} → `),
              link(partId, docStore),
            ],
            fields: [
              { label: "child", value: child },
              { label: "axis_origin", value: `(${axisOrigin.x}, ${axisOrigin.y}, ${axisOrigin.z})` },
              { label: "axis_dir", value: `(${axisDir.x}, ${axisDir.y}, ${axisDir.z})` },
              { label: "count", value: `${count}` },
              { label: "angle_deg", value: `${angleDeg}°` },
            ],
            affectedPartIds: [partId],
          },
        };
      }

      case "mirror": {
        const child = params.child as string;
        const plane = params.plane as "XY" | "XZ" | "YZ" | undefined;
        if (!child) return { status: "error", result: "mirror requires child part_id" };
        if (plane !== "XY" && plane !== "XZ" && plane !== "YZ") {
          return { status: "error", result: "mirror requires plane: 'XY' | 'XZ' | 'YZ'" };
        }
        const err = validatePartId(child, docStore, "mirror child");
        if (err) return err;
        const partId = docStore.addMirror(child, plane);
        if (!partId) return { status: "error", result: "mirror failed" };
        uiStore.select(partId);
        return {
          status: "success",
          result: snapshotJson(partId, docStore, {
            created: "mirror",
            child,
            plane,
          }),
          partId,
          display: {
            summary: [
              text(`⇋ Mirror ${plane} → `),
              link(partId, docStore),
            ],
            fields: [
              { label: "child", value: child },
              { label: "plane", value: plane },
            ],
            affectedPartIds: [partId],
          },
        };
      }

      case "translate": {
        const child = params.child as string;
        const offset = params.offset as { x: number; y: number; z: number };
        if (!child || !offset) return { status: "error", result: "translate requires child and offset" };
        const err = validatePartId(child, docStore, "translate child");
        if (err) return err;
        docStore.setTranslation(child, offset);
        return {
          status: "success",
          result: snapshotJson(child, docStore, {
            applied: "translate",
            offset,
          }),
          partId: child,
          display: {
            summary: [
              text("↦ Translate "),
              link(child, docStore),
              text(` by (${offset.x}, ${offset.y}, ${offset.z})`),
            ],
            fields: [{ label: "offset", value: `(${offset.x}, ${offset.y}, ${offset.z}) mm` }],
            affectedPartIds: [child],
          },
        };
      }
      case "rotate": {
        const child = params.child as string;
        const angles = params.angles as { x: number; y: number; z: number };
        if (!child || !angles) return { status: "error", result: "rotate requires child and angles" };
        const err = validatePartId(child, docStore, "rotate child");
        if (err) return err;

        // Pivot semantics:
        //   omitted / "center"  → rotate around the part's current world bbox center
        //   "origin"            → rotate around world origin (legacy behavior)
        //   {x,y,z}             → rotate around the given world-space point
        // The underlying CRDT engine rotates child geometry around the origin
        // of the part's local frame, then applies the outer Translate. To
        // simulate rotating around an arbitrary world pivot P we compute a
        // compensating translation:
        //     T_new = P - R_new · (R_old⁻¹ · (P - T_old))
        // which lands the world point `P` at the same place after the
        // rotation swap. This makes "rotate in place" Just Work for primitives,
        // extrudes, sweeps, booleans, etc. without touching Rust.
        const rawPivot = params.pivot as Vec3 | "center" | "origin" | undefined;
        let pivot: Vec3 | null = null;
        if (rawPivot === "origin") {
          pivot = null;
        } else if (rawPivot && typeof rawPivot === "object" && "x" in rawPivot && "y" in rawPivot && "z" in rawPivot) {
          pivot = { x: (rawPivot as Vec3).x, y: (rawPivot as Vec3).y, z: (rawPivot as Vec3).z };
        } else {
          // Default ("center"): use the part's current world bbox center.
          const bbox = computePartWorldBbox(child, docStore);
          if (bbox) pivot = bboxCenter(bbox);
        }

        if (pivot) {
          const tOld = getCurrentOffset(child, docStore);
          const rOld = getCurrentAngles(child, docStore);
          // Local-frame vector from the part's origin to the pivot, at the
          // moment we captured it. Undoing R_old takes world → local-before-R.
          const pivotLocal = inverseRotateVec3(
            { x: pivot.x - tOld.x, y: pivot.y - tOld.y, z: pivot.z - tOld.z },
            rOld,
          );
          const pivotAfter = rotateVec3(pivotLocal, angles);
          const tNew = {
            x: pivot.x - pivotAfter.x,
            y: pivot.y - pivotAfter.y,
            z: pivot.z - pivotAfter.z,
          };
          docStore.setRotation(child, angles);
          docStore.setTranslation(child, tNew);
        } else {
          docStore.setRotation(child, angles);
        }
        const pivotLabel =
          rawPivot === "origin"
            ? " around origin"
            : pivot
              ? ` around (${pivot.x.toFixed(1)}, ${pivot.y.toFixed(1)}, ${pivot.z.toFixed(1)})`
              : "";
        return {
          status: "success",
          result: snapshotJson(child, docStore, {
            applied: "rotate",
            angles,
            pivot: pivot ?? "origin",
          }),
          partId: child,
          display: {
            summary: [
              text("↻ Rotate "),
              link(child, docStore),
              text(` by (${angles.x}°, ${angles.y}°, ${angles.z}°)${pivotLabel}`),
            ],
            fields: [
              { label: "angles", value: `(${angles.x}°, ${angles.y}°, ${angles.z}°)` },
              ...(pivot
                ? [{ label: "pivot", value: `(${pivot.x.toFixed(1)}, ${pivot.y.toFixed(1)}, ${pivot.z.toFixed(1)})` }]
                : [{ label: "pivot", value: "origin" }]),
            ],
            affectedPartIds: [child],
          },
        };
      }
      case "scale": {
        const child = params.child as string;
        const factor = params.factor as { x: number; y: number; z: number };
        if (!child || !factor) return { status: "error", result: "scale requires child and factor" };
        const err = validatePartId(child, docStore, "scale child");
        if (err) return err;
        docStore.setScale(child, factor);
        return {
          status: "success",
          result: snapshotJson(child, docStore, {
            applied: "scale",
            factor,
          }),
          partId: child,
          display: {
            summary: [
              text("⇱ Scale "),
              link(child, docStore),
              text(` by (${factor.x}, ${factor.y}, ${factor.z})`),
            ],
            fields: [{ label: "factor", value: `(${factor.x}, ${factor.y}, ${factor.z})` }],
            affectedPartIds: [child],
          },
        };
      }

      case "union":
      case "difference":
      case "intersection": {
        let left = params.left as string;
        let right = params.right as string;
        if (!left || !right) {
          const selectedIds = Array.from(uiStore.selectedPartIds);
          if (selectedIds.length !== 2) {
            return { status: "error", result: "Boolean requires left and right part IDs, or exactly 2 parts selected" };
          }
          left = selectedIds[0]!;
          right = selectedIds[1]!;
        }
        const lerr = validatePartId(left, docStore, "boolean left");
        if (lerr) return lerr;
        const rerr = validatePartId(right, docStore, "boolean right");
        if (rerr) return rerr;
        const resultId = docStore.applyBoolean(type, left, right);
        if (!resultId) return { status: "error", result: `${type} failed` };
        const verb = type === "union" ? "Join" : type === "difference" ? "Cut" : "Intersect";
        const icon = type === "union" ? "⊕" : type === "difference" ? "⊖" : "⊗";
        return {
          status: "success",
          result: `Applied ${type} → new part id: ${resultId}`,
          partId: resultId,
          display: {
            summary: [
              text(`${icon} ${verb} `),
              link(left, docStore),
              text(" with "),
              link(right, docStore),
              text(" → "),
              link(resultId, docStore),
            ],
            fields: [{ label: "operation", value: type }],
            affectedPartIds: [left, right, resultId],
          },
        };
      }

      case "fillet": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "fillet requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "fillet target");
        if (err) return err;
        const id = docStore.addFillet(target, params.radius as number);
        if (!id) return { status: "error", result: "Fillet failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.radius}mm fillet to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌒ Fillet `),
              link(target, docStore),
              text(` r=${params.radius}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "radius", value: `${params.radius} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "chamfer": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "chamfer requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "chamfer target");
        if (err) return err;
        const id = docStore.addChamfer(target, params.distance as number);
        if (!id) return { status: "error", result: "Chamfer failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.distance}mm chamfer to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌐ Chamfer `),
              link(target, docStore),
              text(` d=${params.distance}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "distance", value: `${params.distance} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "shell": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "shell requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "shell target");
        if (err) return err;
        const id = docStore.addShell(target, params.thickness as number);
        if (!id) return { status: "error", result: "Shell failed — target may not be a solid" };
        return {
          status: "success",
          result: `Shelled ${target} with ${params.thickness}mm walls → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`□ Shell `),
              link(target, docStore),
              text(` t=${params.thickness}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "thickness", value: `${params.thickness} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }

      case "extrude": {
        const sketch = params.sketch;
        if (!sketch) return { status: "error", result: "extrude requires a sketch parameter" };
        const direction = params.direction as { x: number; y: number; z: number };
        if (!direction) return { status: "error", result: "extrude requires direction" };

        if (typeof sketch === "object") {
          const s = sketch as {
            origin: { x: number; y: number; z: number };
            x_dir: { x: number; y: number; z: number };
            y_dir: { x: number; y: number; z: number };
            segments: unknown[];
          };
          const sketchErr = validateSketch(s.segments);
          if (sketchErr) return sketchErr;
          const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
          const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };
          const partId = docStore.addExtrude(
            plane, s.origin, s.segments as never[], direction,
            { twist_angle: params.twist_angle as number | undefined, scale_end: params.scale_end as number | undefined },
          );
          if (!partId) {
            return { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
          }
          const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
          return {
            status: "success",
            result: `Extruded sketch → new part id: ${partId}`,
            partId,
            display: {
              summary: [
                text(`▲ Extrude sketch (${s.segments.length} segs, ${depth.toFixed(1)}mm) → `),
                link(partId, docStore),
              ],
              fields: [
                { label: "segments", value: `${s.segments.length}` },
                { label: "depth", value: `${depth.toFixed(2)} mm` },
                { label: "origin", value: `(${s.origin.x}, ${s.origin.y}, ${s.origin.z})` },
              ],
              affectedPartIds: [partId],
            },
          };
        }
        return { status: "error", result: "Extrude from existing sketch node not yet supported" };
      }

      case "revolve": {
        const sketch = params.sketch;
        if (!sketch || typeof sketch !== "object") return { status: "error", result: "revolve requires an inline sketch" };
        const s = sketch as {
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        };
        const axisOrigin = params.axis_origin as { x: number; y: number; z: number };
        const axisDir = params.axis_dir as { x: number; y: number; z: number };
        const sketchErr = validateSketch(s.segments);
        if (sketchErr) return sketchErr;
        const angleDeg = params.angle_deg as number;
        const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
        const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };
        const partId = docStore.addRevolve(plane, s.origin, s.segments as never[], axisOrigin, axisDir, angleDeg);
        return partId
          ? { status: "success", result: `Revolved sketch → new part id: ${partId}`, partId }
          : { status: "error", result: "Revolve failed" };
      }

      case "sweep": {
        const sketch = params.sketch;
        if (!sketch || typeof sketch !== "object") return { status: "error", result: "sweep requires an inline sketch" };
        const s = sketch as {
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        };
        const sketchErr = validateSketch(s.segments);
        if (sketchErr) return sketchErr;
        const path = params.path as Record<string, unknown>;
        const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
        const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };

        // Polyline paths are a vcad-TS convenience: the Rust kernel only
        // understands Line/Helix, so we desugar {type:"Polyline", points:[]}
        // into N separate Line sweeps sharing the same profile. Keeping them
        // as distinct parts (instead of unioning) avoids the cost and
        // robustness risk of N-1 boolean operations for common cases like
        // pipe runs and frame-tube-chains where the separation doesn't
        // matter visually.
        if (path && (path as { type?: string }).type === "Polyline") {
          const points = (path as { points?: Vec3[] }).points;
          if (!points || points.length < 2) {
            return { status: "error", result: "sweep Polyline requires points[] with ≥2 entries" };
          }
          const ids: string[] = [];
          for (let i = 0; i < points.length - 1; i++) {
            const segPath: PathCurve = { type: "Line", start: points[i]!, end: points[i + 1]! };
            const id = docStore.addSweep(plane, s.origin, s.segments as never[], segPath, {
              twist_angle: params.twist_angle as number | undefined,
              scale_start: params.scale_start as number | undefined,
              scale_end: params.scale_end as number | undefined,
            });
            if (!id) {
              for (const x of ids) { try { docStore.removePart(x); } catch { /* ignore */ } }
              return { status: "error", result: `sweep Polyline segment ${i} failed` };
            }
            ids.push(id);
          }
          return {
            status: "success",
            result: `Swept polyline (${ids.length} segments): ${ids.join(", ")}`,
            partId: ids[ids.length - 1]!,
            display: {
              summary: [text(`〰 Polyline sweep (${ids.length} segs)`)],
              fields: [{ label: "segments", value: `${ids.length}` }],
              affectedPartIds: ids,
            },
          };
        }

        const partId = docStore.addSweep(plane, s.origin, s.segments as never[], path as never, {
          twist_angle: params.twist_angle as number | undefined,
          scale_start: params.scale_start as number | undefined,
          scale_end: params.scale_end as number | undefined,
        });
        return partId
          ? { status: "success", result: `Swept sketch → new part id: ${partId}`, partId }
          : { status: "error", result: "Sweep failed" };
      }

      case "loft": {
        const sketches = params.sketches as Array<{
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        }>;
        if (!sketches || sketches.length < 2) return { status: "error", result: "loft requires at least 2 sketch profiles" };
        const profiles = sketches.map((s) => {
          const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
          return {
            plane: { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal },
            origin: s.origin,
            segments: s.segments as never[],
          };
        });
        const partId = docStore.addLoft(profiles, { closed: params.closed as boolean | undefined });
        return partId
          ? { status: "success", result: `Lofted ${sketches.length} profiles`, partId }
          : { status: "error", result: "Loft failed" };
      }

      case "sketch_2d":
        return { status: "success", result: "Sketch2D is typically used inline with extrude/revolve. Use create(type: 'extrude', ...) with an inline sketch instead." };

      default:
        return { status: "error", result: `Unknown create type: ${type}` };
    }
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Create failed" };
  }
}

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

function executeUpdate(
  nodeId: string,
  params: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult {
  try {
    let targetPartId: string | undefined;
    for (const part of docStore.parts) {
      if (docStore.document.nodes[nodeId]) {
        targetPartId = part.id;
        break;
      }
    }

    if (!targetPartId) {
      return { status: "error", result: `Node ${nodeId} not found in any part` };
    }

    for (const [key, value] of Object.entries(params)) {
      let crdtValue: unknown;
      if (typeof value === "number") {
        crdtValue = { F64: value };
      } else if (typeof value === "boolean") {
        crdtValue = { Bool: value };
      } else if (typeof value === "string") {
        crdtValue = { String: value };
      } else if (typeof value === "object" && value !== null && "x" in value && "y" in value && "z" in value) {
        const v = value as { x: number; y: number; z: number };
        crdtValue = { Vec3: [v.x, v.y, v.z] };
      } else {
        continue;
      }

      docStore.setFeatureParam(targetPartId, key, crdtValue as never);
    }

    return {
      status: "success",
      result: `Updated node ${nodeId}: ${Object.keys(params).join(", ")}`,
      partId: targetPartId,
      nodeId,
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Update failed" };
  }
}

function executeDelete(
  partId: string,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  try {
    const part = docStore.partIndex.get(partId);
    const name = part?.name ?? partId.slice(-4);
    docStore.removePart(partId);
    uiStore.clearSelection();
    return {
      status: "success",
      result: `Deleted part ${partId}`,
      display: {
        summary: [
          text("✕ Delete "),
          { type: "partLink", partId, name },
        ],
        fields: [{ label: "part id", value: partId }],
        affectedPartIds: [partId],
      },
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Delete failed" };
  }
}
