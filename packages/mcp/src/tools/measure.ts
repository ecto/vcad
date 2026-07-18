/**
 * Targeted measurement tools for MCP agents.
 *
 * `inspect_cad` is aggregate-only; these give an agent per-part detail and
 * part-pair distances without leaving the modeling loop:
 *
 *  - `inspect_part` — one part's world-space bbox, size, center, volume,
 *    center of mass, material, and the named anchors `place` accepts.
 *  - `describe_scene` — the same bbox/size/center snapshot for every part
 *    (or a chosen subset) in one call, so an agent needn't chain
 *    `inspect_part`.
 *  - `measure` — given two part ids, the minimum distance between them
 *    (0/negative = contact/overlap) plus each part's bbox; given one part
 *    id, that part's bbox, volume, and center of mass.
 *
 * `inspect_part` / `describe_scene` are the kernel chat surface's tools,
 * re-implemented here against evaluated tessellations and dispatched through
 * the registry surface (registry-dispatch.ts). `measure` is a standalone
 * ToolDef. All three measure from the evaluated triangle mesh, so accuracy is
 * tessellation-bound (increase segment counts for tighter numbers).
 */

import type { ClearanceResult, Engine, TriangleMesh } from "@vcad/engine";
import { transformMesh } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { computeMeshProperties, type BoundingBox } from "./inspect.js";
import { getSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** Round to micron precision so payloads don't carry float noise. */
const r6 = (v: number) => Math.round(v * 1e6) / 1e6;

const point = (p: { x: number; y: number; z: number }) => ({
  x: r6(p.x),
  y: r6(p.y),
  z: r6(p.z),
});

/** A part resolved to its evaluated (already-placed) mesh + material. */
interface ResolvedPart {
  id: string;
  name?: string;
  material?: string;
  mesh: TriangleMesh;
}

/** Evaluate a session document into measurable parts (id, name, material,
 *  placed mesh), skipping hidden roots and empty meshes — mirrors the
 *  clearance tool's part resolution. */
function evaluateParts(doc: Document, engine: Engine): ResolvedPart[] {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const partMaterials = (doc as { part_materials?: Record<string, string> })
    .part_materials;
  const out: ResolvedPart[] = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const root = visibleRoots[i];
    const rootId = String(root.root);
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    const node = doc.nodes[rootId];
    out.push({
      id: rootId,
      name: node?.name ?? undefined,
      material: root.material ?? partMaterials?.[rootId] ?? undefined,
      mesh,
    });
  }
  // Assembly instances: bake the FK world transform into the part-local mesh
  // so measurements report poses, not part-local geometry — the same
  // candidate population check_clearance uses. Without this, assembly-only
  // documents answered every measure/inspect_part query with "Available: none".
  for (const inst of scene.instances ?? []) {
    const mesh = inst.transform
      ? transformMesh(inst.mesh, {
          translate: inst.transform.translation,
          rotate: inst.transform.rotation,
          scale: inst.transform.scale,
        })
      : inst.mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({
      id: inst.instanceId,
      name: inst.name ?? undefined,
      mesh,
    });
  }
  return out;
}

/** Resolve a part by id first, then by exact name. */
function findPart(
  candidates: ResolvedPart[],
  wanted: string,
): ResolvedPart | undefined {
  return (
    candidates.find((c) => c.id === wanted) ??
    candidates.find((c) => c.name === wanted)
  );
}

/** Human-readable list of what a caller could have asked for. */
function availableList(candidates: ResolvedPart[]): string {
  return (
    candidates.map((c) => `${c.id}${c.name ? ` (${c.name})` : ""}`).join(", ") ||
    "none"
  );
}

/** The bbox payload (rounded min/max/center/size) all three tools emit. */
function bboxPayload(bbox: BoundingBox) {
  const center = {
    x: (bbox.min.x + bbox.max.x) / 2,
    y: (bbox.min.y + bbox.max.y) / 2,
    z: (bbox.min.z + bbox.max.z) / 2,
  };
  const size = {
    x: bbox.max.x - bbox.min.x,
    y: bbox.max.y - bbox.min.y,
    z: bbox.max.z - bbox.min.z,
  };
  return {
    min: point(bbox.min),
    max: point(bbox.max),
    center: point(center),
    size: point(size),
  };
}

/**
 * Named world-space AABB anchors, matching the app's `place` semantics so an
 * agent can pass any of these back to a future `place` and get the point it
 * saw here.
 */
function anchors(bbox: BoundingBox) {
  const c = {
    x: (bbox.min.x + bbox.max.x) / 2,
    y: (bbox.min.y + bbox.max.y) / 2,
    z: (bbox.min.z + bbox.max.z) / 2,
  };
  return {
    center: point(c),
    min: point(bbox.min),
    max: point(bbox.max),
    top: point({ x: c.x, y: c.y, z: bbox.max.z }),
    bottom: point({ x: c.x, y: c.y, z: bbox.min.z }),
    front: point({ x: c.x, y: bbox.min.y, z: c.z }),
    back: point({ x: c.x, y: bbox.max.y, z: c.z }),
    left: point({ x: bbox.min.x, y: c.y, z: c.z }),
    right: point({ x: bbox.max.x, y: c.y, z: c.z }),
  };
}

/** One part's world-space snapshot (bbox/center/size + material). */
function partSnapshot(part: ResolvedPart) {
  const props = computeMeshProperties(part.mesh);
  return {
    id: part.id,
    ...(part.name ? { name: part.name } : {}),
    material: part.material ?? null,
    bbox: bboxPayload(props.bbox),
  };
}

/** Thrown by the helpers below; carries a recovery hint for the MCP error. */
export class MeasureError extends Error {}

/**
 * `inspect_part` payload: one part's world-space bbox, size, center, volume,
 * center of mass, material, and named anchors. Tessellation-bound.
 */
export function inspectPartResult(
  doc: Document,
  engine: Engine,
  partId: string,
): Record<string, unknown> {
  if (!partId) throw new MeasureError("inspect_part requires `part_id`.");
  const candidates = evaluateParts(doc, engine);
  const part = findPart(candidates, partId);
  if (!part) {
    throw new MeasureError(
      `No part with id or name "${partId}". Available: ${availableList(candidates)}`,
    );
  }
  const props = computeMeshProperties(part.mesh);
  return {
    id: part.id,
    ...(part.name ? { name: part.name } : {}),
    material: part.material ?? null,
    bbox: bboxPayload(props.bbox),
    volume_mm3: r6(props.volume),
    center_of_mass: point(props.centroid),
    anchors: anchors(props.bbox),
    note: "Measured from the evaluated tessellation — accuracy is tessellation-bound.",
  };
}

/**
 * `describe_scene` payload: a bbox/center/size + material snapshot for every
 * part (or the requested subset), in one call.
 */
export function describeSceneResult(
  doc: Document,
  engine: Engine,
  partIds: string[] | undefined,
  limit: number | undefined,
): Record<string, unknown> {
  const candidates = evaluateParts(doc, engine);
  const cap = typeof limit === "number" && limit > 0 ? limit : 100;
  const wanted =
    partIds && partIds.length > 0
      ? partIds.map(String)
      : candidates.map((c) => c.id).slice(0, cap);
  const parts: Array<Record<string, unknown>> = [];
  const missing: string[] = [];
  for (const id of wanted) {
    const part = findPart(candidates, id);
    if (!part) missing.push(id);
    else parts.push(partSnapshot(part));
  }
  return {
    part_count: parts.length,
    parts,
    ...(missing.length > 0 ? { missing } : {}),
    note: "Bounding boxes are measured from evaluated tessellations — accuracy is tessellation-bound.",
  };
}

/**
 * `measure` payload. With two ids: the minimum distance between the parts
 * (0/negative reported as contact/overlap via `contact`/`intersecting`) plus
 * each part's bbox. With one id: that part's bbox, volume, and center of mass.
 */
export function measureResult(
  doc: Document,
  engine: Engine,
  partIds: string[],
): Record<string, unknown> {
  const candidates = evaluateParts(doc, engine);
  const resolved: ResolvedPart[] = [];
  const missing: string[] = [];
  for (const id of partIds.map(String)) {
    const part = findPart(candidates, id);
    if (!part) missing.push(id);
    else resolved.push(part);
  }
  if (missing.length > 0) {
    throw new MeasureError(
      `No part with id or name ${missing
        .map((m) => `"${m}"`)
        .join(", ")}. Available: ${availableList(candidates)}`,
    );
  }

  if (resolved.length === 1) {
    const part = resolved[0];
    const props = computeMeshProperties(part.mesh);
    return {
      mode: "part",
      part: {
        id: part.id,
        ...(part.name ? { name: part.name } : {}),
        material: part.material ?? null,
        bbox: bboxPayload(props.bbox),
        volume_mm3: r6(props.volume),
        center_of_mass: point(props.centroid),
      },
      note: "Measured from the evaluated tessellation — accuracy is tessellation-bound.",
    };
  }

  const [a, b] = resolved;
  if (a.id === b.id) {
    throw new MeasureError(
      `measure needs two distinct parts — "${a.id}" was given twice.`,
    );
  }
  let clearance: ClearanceResult;
  try {
    clearance = engine.meshClearance(a.mesh, b.mesh);
  } catch (e) {
    throw new MeasureError(e instanceof Error ? e.message : String(e));
  }
  const distance = r6(clearance.distance);
  return {
    mode: "pair",
    distance_mm: distance,
    contact: distance <= 0,
    intersecting: clearance.intersecting,
    closest_points: {
      a: clearance.pointA.map(r6) as [number, number, number],
      b: clearance.pointB.map(r6) as [number, number, number],
    },
    parts: {
      a: partSnapshot(a),
      b: partSnapshot(b),
    },
    note: "Distance is negative when the parts overlap (penetration depth). Measured from evaluated tessellations — accuracy is tessellation-bound.",
  };
}

export const measureSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document holding the parts to measure.",
    },
    part_ids: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "One or two part ids (or part names). One id → that part's bbox, volume, and center of mass. Two ids → the minimum distance between them (0/negative = contact/overlap) plus each part's bbox.",
    },
  },
  required: ["document_id", "part_ids"],
} as const;

/** `measure` MCP handler. */
export function measure(
  args: Record<string, unknown>,
  engine: Engine,
) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) {
    return err("Pass `document_id` (the open CAD session).");
  }
  const partIds = Array.isArray(args.part_ids)
    ? args.part_ids.map((x) => String(x))
    : undefined;
  if (!partIds || partIds.length < 1 || partIds.length > 2) {
    return err(
      "Pass `part_ids` as an array of one or two part ids (or part names).",
    );
  }
  let doc: Document;
  try {
    doc = getSession(documentId);
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }
  let payload: Record<string, unknown>;
  try {
    payload = measureResult(doc, engine, partIds);
  } catch (e) {
    if (e instanceof MeasureError) return err(e.message);
    throw e;
  }
  const body = { document_id: documentId, ...payload };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(body) }],
    structuredContent: { measure: body, document_id: documentId },
  };
}

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "measure",
    pack: null,
    description:
      "Measure geometry in an open session. Pass `part_ids` with two ids to get the minimum distance between those parts (0/negative = contact/overlap) plus each part's world-space bounding box; pass one id to get that part's bbox, volume, and center of mass. Complements `inspect_cad` (whole-document aggregate) and `check_clearance` (named, persisted air-gap assertions). Distances are measured from evaluated tessellations, so accuracy is tessellation-bound.",
    inputSchema: measureSchema,
    handler: (a, c) => measure(a, c.engine),
    behavior: behavior({}),
  },
];
