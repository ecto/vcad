/**
 * Reference resolution.
 *
 * Translates the human-stable NodeRef / FaceRef / EdgeRef / AxisRef /
 * PlaneRef vocabulary into concrete IR data the kernel can act on.
 *
 * NodeRefs use a name registry (`names → NodeId`) maintained on the
 * desugar context. Faces and edges are addressed by role + bbox role
 * heuristics — works for primitives and simple booleans without forcing
 * the agent to pin opaque ids that will shift after the next edit.
 */

import type { Document, Node, NodeId, Vec3 } from "@vcad/ir";
import type {
  AxisRef,
  EdgeRef,
  FaceRef,
  NamedPos,
  NodeRef,
  PlaneRef,
} from "./types.js";

/** Mutable bookkeeping the desugarer threads through every op. */
export interface NameRegistry {
  /** name → NodeId. The "canonical" id of a chain — usually the last op. */
  byName: Map<string, NodeId>;
  /** name → reserved planes/axes/points (not part of the geometry graph). */
  refPlanes: Map<string, { origin: Vec3; normal: Vec3; xDir: Vec3; yDir: Vec3 }>;
  refAxes: Map<string, { origin: Vec3; direction: Vec3 }>;
  refPoints: Map<string, Vec3>;
  /** name → canonical sketch node id (for extrude/revolve/sweep/loft inputs). */
  sketches: Map<string, NodeId>;
}

export function emptyRegistry(): NameRegistry {
  return {
    byName: new Map(),
    refPlanes: new Map(),
    refAxes: new Map(),
    refPoints: new Map(),
    sketches: new Map(),
  };
}

/** Seed a registry from a doc — picks up names already on existing nodes. */
export function registryFromDoc(doc: Document): NameRegistry {
  const reg = emptyRegistry();
  for (const [idStr, node] of Object.entries(doc.nodes)) {
    if (node.name) {
      reg.byName.set(node.name, Number(idStr));
      if (node.op.type === "Sketch2D" || node.op.type === "Text2D") {
        reg.sketches.set(node.name, Number(idStr));
      }
    }
  }
  return reg;
}

/** Resolve a NodeRef to a NodeId using the registry. */
export function resolveNode(ref: NodeRef, reg: NameRegistry): NodeId {
  const id = reg.byName.get(ref);
  if (id === undefined) {
    throw new Error(`Unknown node reference: "${ref}"`);
  }
  return id;
}

export function tryResolveNode(ref: NodeRef, reg: NameRegistry): NodeId | undefined {
  return reg.byName.get(ref);
}

/** Resolve a sketch reference. Sketches must be ref'd by name. */
export function resolveSketch(name: string, reg: NameRegistry, doc: Document): NodeId {
  const id = reg.sketches.get(name) ?? reg.byName.get(name);
  if (id === undefined) {
    throw new Error(`Unknown sketch: "${name}"`);
  }
  const node = doc.nodes[String(id)];
  if (!node) throw new Error(`Sketch node "${name}" missing from doc`);
  if (node.op.type !== "Sketch2D" && node.op.type !== "Text2D") {
    throw new Error(`Node "${name}" is ${node.op.type}, not a sketch`);
  }
  return id;
}

/** Resolve a 3D position from a literal Vec3 or a NamedPos shorthand. */
export function resolvePosition(
  pos: Vec3 | NamedPos | undefined,
  bbox?: { min: Vec3; max: Vec3 },
): Vec3 {
  if (pos === undefined) return { x: 0, y: 0, z: 0 };
  if (typeof pos === "string") {
    if (!bbox) {
      // Without context, named positions reduce to the origin.
      return { x: 0, y: 0, z: 0 };
    }
    const cx = (bbox.min.x + bbox.max.x) / 2;
    const cy = (bbox.min.y + bbox.max.y) / 2;
    const cz = (bbox.min.z + bbox.max.z) / 2;
    if (pos === "center") return { x: cx, y: cy, z: cz };
    if (pos === "top-center") return { x: cx, y: cy, z: bbox.max.z };
    if (pos === "bottom-center") return { x: cx, y: cy, z: bbox.min.z };
  }
  if (typeof pos === "object" && "x" in pos && "y" in pos && !("z" in pos && typeof (pos as Vec3).z === "number" && !isNaN((pos as Vec3).z))) {
    // Could be a Vec3 with NaN z, or a NamedPos object — fall through
    // to the explicit Vec3 path below.
  }
  if (typeof pos === "object" && "x" in pos) {
    const obj = pos as { x: number | string; y: number | string; z?: number | string };
    if (
      typeof obj.x === "number" &&
      typeof obj.y === "number" &&
      (obj.z === undefined || typeof obj.z === "number")
    ) {
      return { x: obj.x, y: obj.y, z: obj.z ?? 0 };
    }
    // Percent shorthand requires bbox.
    if (!bbox) throw new Error("Percent positions require a bounding box context");
    const span = (axis: keyof Vec3): number =>
      bbox.max[axis] - bbox.min[axis];
    const resolveAxis = (v: number | string | undefined, axis: keyof Vec3): number => {
      if (v === undefined) return bbox.min[axis];
      if (typeof v === "number") return v;
      const m = /^(-?\d+(?:\.\d+)?)%$/.exec(v);
      if (!m) throw new Error(`Bad position component: ${v}`);
      return bbox.min[axis] + (Number(m[1]) / 100) * span(axis);
    };
    return {
      x: resolveAxis(obj.x, "x"),
      y: resolveAxis(obj.y, "y"),
      z: resolveAxis(obj.z, "z"),
    };
  }
  throw new Error(`Unrecognised position: ${JSON.stringify(pos)}`);
}

/** Resolve an AxisRef to {origin, direction}. */
export function resolveAxis(
  ref: AxisRef,
  reg: NameRegistry,
  doc: Document,
): { origin: Vec3; direction: Vec3 } {
  if ("kind" in ref) {
    const dir = unitAxis(ref.kind);
    return { origin: { x: 0, y: 0, z: 0 }, direction: dir };
  }
  if ("axis_named" in ref) {
    const axis = reg.refAxes.get(ref.axis_named);
    if (!axis) throw new Error(`Unknown ref_axis: "${ref.axis_named}"`);
    return axis;
  }
  if ("from" in ref) {
    return {
      origin: ref.from,
      direction: normalize(sub(ref.to, ref.from)),
    };
  }
  if ("axis_of" in ref) {
    const id = resolveNode(ref.node, reg);
    const node = doc.nodes[String(id)];
    if (!node) throw new Error(`Unknown node "${ref.node}"`);
    if (ref.axis_of === "cylinder" || ref.axis_of === "cone") {
      // Both Cylinder and Cone are oriented along Z in our IR.
      return { origin: { x: 0, y: 0, z: 0 }, direction: { x: 0, y: 0, z: 1 } };
    }
  }
  throw new Error(`Unrecognised axis ref: ${JSON.stringify(ref)}`);
}

/** Resolve a PlaneRef to {origin, normal, xDir, yDir} suitable for sketches. */
export function resolvePlane(
  ref: PlaneRef,
  reg: NameRegistry,
  doc: Document,
): { origin: Vec3; normal: Vec3; xDir: Vec3; yDir: Vec3 } {
  if ("kind" in ref) {
    if (ref.kind === "xy") {
      return planeFromBasis({ x: 0, y: 0, z: 0 }, { x: 1, y: 0, z: 0 }, { x: 0, y: 1, z: 0 });
    }
    if (ref.kind === "xz") {
      return planeFromBasis({ x: 0, y: 0, z: 0 }, { x: 1, y: 0, z: 0 }, { x: 0, y: 0, z: 1 });
    }
    if (ref.kind === "yz") {
      return planeFromBasis({ x: 0, y: 0, z: 0 }, { x: 0, y: 1, z: 0 }, { x: 0, y: 0, z: 1 });
    }
  }
  if ("plane_named" in ref) {
    const p = reg.refPlanes.get(ref.plane_named);
    if (!p) throw new Error(`Unknown ref_plane: "${ref.plane_named}"`);
    return p;
  }
  if ("offset" in ref) {
    const base = resolvePlane(ref.offset.from, reg, doc);
    return {
      ...base,
      origin: add(base.origin, scale(base.normal, ref.offset.distance)),
    };
  }
  if ("face_named" in ref) {
    // Face-based sketch planes need topology lookup we don't have here.
    // Fall back to XY for now and surface a warning at the caller.
    return planeFromBasis({ x: 0, y: 0, z: 0 }, { x: 1, y: 0, z: 0 }, { x: 0, y: 1, z: 0 });
  }
  throw new Error(`Unrecognised plane ref: ${JSON.stringify(ref)}`);
}

/**
 * Resolve a FaceRef to a {normal, point-on-face} hint that the kernel can
 * use. Without persistent topology ids we use bbox-role heuristics: the
 * agent says "top face of plate" and we project that against the node's
 * evaluated bounding box.
 */
export function resolveFace(
  ref: FaceRef,
  reg: NameRegistry,
  doc: Document,
  bboxes: Map<NodeId, { min: Vec3; max: Vec3 }>,
): { node: NodeId; normal: Vec3; point: Vec3 } {
  const id = resolveNode(ref.node, reg);
  const bbox = bboxes.get(id);
  if (!bbox) {
    // Fall back to top.
    return {
      node: id,
      normal: { x: 0, y: 0, z: 1 },
      point: { x: 0, y: 0, z: 0 },
    };
  }
  const cx = (bbox.min.x + bbox.max.x) / 2;
  const cy = (bbox.min.y + bbox.max.y) / 2;
  const cz = (bbox.min.z + bbox.max.z) / 2;
  const role = "face_role" in ref ? ref.face_role : undefined;
  switch (role) {
    case "top":
      return { node: id, normal: { x: 0, y: 0, z: 1 }, point: { x: cx, y: cy, z: bbox.max.z } };
    case "bottom":
      return { node: id, normal: { x: 0, y: 0, z: -1 }, point: { x: cx, y: cy, z: bbox.min.z } };
    case "front":
      return { node: id, normal: { x: 0, y: -1, z: 0 }, point: { x: cx, y: bbox.min.y, z: cz } };
    case "back":
      return { node: id, normal: { x: 0, y: 1, z: 0 }, point: { x: cx, y: bbox.max.y, z: cz } };
    case "left":
      return { node: id, normal: { x: -1, y: 0, z: 0 }, point: { x: bbox.min.x, y: cy, z: cz } };
    case "right":
      return { node: id, normal: { x: 1, y: 0, z: 0 }, point: { x: bbox.max.x, y: cy, z: cz } };
  }
  if ("face_at" in ref) {
    return { node: id, normal: { x: 0, y: 0, z: 1 }, point: ref.face_at };
  }
  if ("face_normal" in ref) {
    return { node: id, normal: normalize(ref.face_normal), point: { x: cx, y: cy, z: cz } };
  }
  // Default to top of bbox.
  return { node: id, normal: { x: 0, y: 0, z: 1 }, point: { x: cx, y: cy, z: bbox.max.z } };
}

/**
 * Resolve an EdgeRef. Without persistent topology we map roles to bbox
 * heuristics; downstream ops apply the operation to the whole node when
 * we can't pin a specific edge (graceful degradation).
 */
export function resolveEdges(
  ref: EdgeRef,
  reg: NameRegistry,
): { node: NodeId; role?: string; index?: number } {
  if ("between_faces" in ref) {
    // Use the first face's node — kernel finds shared edges from there.
    const faceA = ref.between_faces[0];
    return { node: resolveNode(faceA.node, reg), role: "between" };
  }
  const id = resolveNode(ref.node, reg);
  if ("edges_role" in ref) return { node: id, role: ref.edges_role };
  if ("edge" in ref) return { node: id, index: ref.edge };
  return { node: id };
}

// ── Vec3 helpers ────────────────────────────────────────────────────

function add(a: Vec3, b: Vec3): Vec3 {
  return { x: a.x + b.x, y: a.y + b.y, z: a.z + b.z };
}
function sub(a: Vec3, b: Vec3): Vec3 {
  return { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z };
}
function scale(v: Vec3, s: number): Vec3 {
  return { x: v.x * s, y: v.y * s, z: v.z * s };
}
function cross(a: Vec3, b: Vec3): Vec3 {
  return {
    x: a.y * b.z - a.z * b.y,
    y: a.z * b.x - a.x * b.z,
    z: a.x * b.y - a.y * b.x,
  };
}
function normalize(v: Vec3): Vec3 {
  const len = Math.hypot(v.x, v.y, v.z) || 1;
  return { x: v.x / len, y: v.y / len, z: v.z / len };
}
function unitAxis(axis: "x" | "y" | "z"): Vec3 {
  return axis === "x"
    ? { x: 1, y: 0, z: 0 }
    : axis === "y"
      ? { x: 0, y: 1, z: 0 }
      : { x: 0, y: 0, z: 1 };
}
function planeFromBasis(origin: Vec3, xDir: Vec3, yDir: Vec3) {
  const x = normalize(xDir);
  const y = normalize(yDir);
  const normal = normalize(cross(x, y));
  return { origin, normal, xDir: x, yDir: y };
}

/** Convenience: index every node's bbox so face/edge heuristics work. */
export function bboxesFromScene(parts: { mesh: { positions: number[] | Float32Array }; rootNode?: number }[]): Map<NodeId, { min: Vec3; max: Vec3 }> {
  const map = new Map<NodeId, { min: Vec3; max: Vec3 }>();
  for (const part of parts) {
    const id = part.rootNode;
    if (id === undefined) continue;
    let minX = Infinity,
      minY = Infinity,
      minZ = Infinity;
    let maxX = -Infinity,
      maxY = -Infinity,
      maxZ = -Infinity;
    const pos = part.mesh.positions;
    for (let i = 0; i < pos.length; i += 3) {
      if (pos[i] < minX) minX = pos[i];
      if (pos[i + 1] < minY) minY = pos[i + 1];
      if (pos[i + 2] < minZ) minZ = pos[i + 2];
      if (pos[i] > maxX) maxX = pos[i];
      if (pos[i + 1] > maxY) maxY = pos[i + 1];
      if (pos[i + 2] > maxZ) maxZ = pos[i + 2];
    }
    if (isFinite(minX)) {
      map.set(id, {
        min: { x: minX, y: minY, z: minZ },
        max: { x: maxX, y: maxY, z: maxZ },
      });
    }
  }
  return map;
}

/** Read a Node by name (helper used by ops that need to mutate in place). */
export function nodeByName(reg: NameRegistry, doc: Document, name: NodeRef): Node {
  const id = resolveNode(name, reg);
  const node = doc.nodes[String(id)];
  if (!node) throw new Error(`Node "${name}" missing from doc`);
  return node;
}
