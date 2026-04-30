/**
 * build (v2) — the centerpiece tool for creating and editing CAD docs.
 *
 * Phase-1 vocabulary covered here:
 *   - primitives:     cube, cylinder, sphere, cone (wedge/torus → not yet)
 *   - booleans:       union, difference, intersection
 *   - transforms:     translate, rotate, scale
 *   - features:       fillet, chamfer, shell  (whole-node application)
 *   - patterns:       linear_pattern, circular_pattern, mirror (mirror as
 *                     scale by -1; merge folds back into a union)
 *   - bookkeeping:    set_material, rename, delete, set_parameter, raw_ir
 *
 * Sketch-driven ops (sketch / extrude / revolve / sweep / loft) and the
 * specialised feature ops (hole / draft / sheet metal / threads) are
 * not desugared yet and return a clear `unsupported_op` error so agents
 * can route around them.
 *
 * The desugarer threads a NameRegistry through every op so subsequent
 * NodeRefs resolve against names declared earlier in the same call.
 */

import {
  createDocument,
  type CsgOp,
  type Document,
  type MaterialDef,
  type Node,
  type NodeId,
  type SketchSegment2D,
  type Vec2,
  type Vec3,
} from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { commitDoc, resolveRef } from "../handles.js";
import {
  emptyRegistry,
  type NameRegistry,
  registryFromDoc,
  resolveAxis,
  resolvePlane,
  resolvePosition,
} from "../refs.js";
import type {
  BuildInput,
  BuildOp,
  BuildResult,
  Material,
  NamedPos,
  NodeRef,
  SketchEntity,
} from "../types.js";

export const buildSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Optional existing handle/IR. Omit for a fresh document." },
    ops: {
      type: "array" as const,
      description:
        "Ordered list of build operations. See `BuildOp` discriminated union for every variant.",
    },
    materials: { type: "object" as const },
    parameters: { type: "object" as const },
    metadata: { type: "object" as const },
  },
  required: ["ops"],
};

export function buildTool(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as BuildInput;

  if (!Array.isArray(args.ops)) {
    return fail("invalid_input", "`ops` must be an array of BuildOp.");
  }

  const baseHandle = typeof args.doc === "string" ? args.doc : undefined;
  const { doc: source } = args.doc ? resolveRef(args.doc) : { doc: createDocument() };
  // Defensive copy — never mutate stored versions.
  const doc: Document = JSON.parse(JSON.stringify(source));
  const reg = args.doc ? registryFromDoc(doc) : emptyRegistry();

  if (args.materials) {
    for (const [k, m] of Object.entries(args.materials)) {
      doc.materials[k] = materialToDef(k, m as Material | MaterialDef);
    }
  }
  if (args.parameters) {
    doc.parameters = doc.parameters ?? {};
    for (const [k, v] of Object.entries(args.parameters)) {
      doc.parameters[k] = { value: v };
    }
  }

  const result: BuildResult = {
    added_nodes: [],
    modified_nodes: [],
    removed_nodes: [],
    named_nodes: {},
  };

  for (let i = 0; i < args.ops.length; i++) {
    const op = args.ops[i];
    try {
      applyOp(doc, reg, op, result);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      return fail("op_failed", `op[${i}] (${(op as { op?: string }).op}): ${message}`, {
        index: i,
        op,
      });
    }
  }

  const handle = commitDoc(doc, baseHandle as `vcad:doc:${string}` | undefined);
  return ok({
    result,
    handle,
    doc,
    engine,
    startedAt,
  });
}

// ── Desugarer ──────────────────────────────────────────────────────

function applyOp(
  doc: Document,
  reg: NameRegistry,
  op: BuildOp,
  result: BuildResult,
): void {
  switch (op.op) {
    case "primitive":
      return applyPrimitive(doc, reg, op, result);
    case "union":
    case "difference":
    case "intersection":
      return applyBoolean(doc, reg, op, result);
    case "translate":
    case "rotate":
    case "scale":
      return applyTransform(doc, reg, op, result);
    case "fillet":
    case "chamfer":
    case "shell":
      return applyFeature(doc, reg, op, result);
    case "linear_pattern":
    case "circular_pattern":
      return applyPattern(doc, reg, op, result);
    case "mirror":
      return applyMirror(doc, reg, op, result);
    case "set_material":
      return applySetMaterial(doc, reg, op);
    case "rename":
      return applyRename(doc, reg, op);
    case "delete":
      return applyDelete(doc, reg, op, result);
    case "set_parameter":
      doc.parameters = doc.parameters ?? {};
      doc.parameters[op.name] = { value: op.value };
      return;
    case "ref_plane":
      reg.refPlanes.set(op.name, resolvePlane(planeRefFromDef(op.def), reg, doc));
      return;
    case "ref_axis":
      reg.refAxes.set(op.name, resolveAxis(axisRefFromDef(op.def), reg, doc));
      return;
    case "ref_point":
      reg.refPoints.set(op.name, op.at);
      return;
    case "raw_ir":
      return applyRawIr(doc, reg, op, result);
    case "sketch":
      return applySketch(doc, reg, op, result);
    case "extrude":
      return applyExtrude(doc, reg, op, result);
    case "revolve":
      return applyRevolve(doc, reg, op, result);
    case "sweep":
      return applySweep(doc, reg, op, result);
    case "loft":
      return applyLoft(doc, reg, op, result);
    case "hole":
      return applyHole(doc, reg, op, result);
    case "draft":
    case "sheet_base":
    case "sheet_flange":
    case "sheet_unfold":
      throw new Error(
        `op '${op.op}' is not desugared yet — file via raw_ir or wait for the sheet-metal phase.`,
      );
    default:
      throw new Error(`Unknown op: ${(op as { op: string }).op}`);
  }
}

// ── Helpers ────────────────────────────────────────────────────────

function nextNodeId(doc: Document): NodeId {
  let max = 0;
  for (const k of Object.keys(doc.nodes)) {
    const n = Number(k);
    if (n > max) max = n;
  }
  return max + 1;
}

function addNode(
  doc: Document,
  reg: NameRegistry,
  result: BuildResult,
  name: string | undefined,
  csgOp: CsgOp,
): NodeId {
  const id = nextNodeId(doc);
  const node: Node = { id, name: name ?? null, op: csgOp };
  doc.nodes[String(id)] = node;
  result.added_nodes.push(id);
  if (name) {
    reg.byName.set(name, id);
    if (result.named_nodes) result.named_nodes[name] = id;
  }
  return id;
}

function removeRoot(doc: Document, id: NodeId): string | undefined {
  for (let i = 0; i < doc.roots.length; i++) {
    if (doc.roots[i].root === id) {
      const material = doc.roots[i].material;
      doc.roots.splice(i, 1);
      return material;
    }
  }
  return undefined;
}

function attachRoot(doc: Document, id: NodeId, material: string): void {
  // Avoid duplicating an existing root entry.
  for (const r of doc.roots) if (r.root === id) return;
  doc.roots.push({ root: id, material });
}

function resolveNodeRef(reg: NameRegistry, ref: NodeRef): NodeId {
  const id = reg.byName.get(ref);
  if (id === undefined) throw new Error(`Unknown node ref: "${ref}"`);
  return id;
}

function bboxOfRoot(doc: Document, id: NodeId, engine: Engine | undefined):
  | { min: Vec3; max: Vec3 }
  | undefined {
  if (!engine) return undefined;
  try {
    const scene = engine.evaluate(doc);
    for (let i = 0; i < scene.parts.length; i++) {
      const root = doc.roots[i];
      if (!root || root.root !== id) continue;
      const m = scene.parts[i].mesh;
      let minX = Infinity,
        minY = Infinity,
        minZ = Infinity;
      let maxX = -Infinity,
        maxY = -Infinity,
        maxZ = -Infinity;
      for (let p = 0; p < m.positions.length; p += 3) {
        const x = m.positions[p],
          y = m.positions[p + 1],
          z = m.positions[p + 2];
        if (x < minX) minX = x;
        if (y < minY) minY = y;
        if (z < minZ) minZ = z;
        if (x > maxX) maxX = x;
        if (y > maxY) maxY = y;
        if (z > maxZ) maxZ = z;
      }
      if (isFinite(minX)) return { min: { x: minX, y: minY, z: minZ }, max: { x: maxX, y: maxY, z: maxZ } };
    }
  } catch {
    return undefined;
  }
  return undefined;
}

function asVec3(v: Vec3 | NamedPos | undefined): Vec3 {
  return resolvePosition(v as Vec3 | NamedPos | undefined);
}

function materialToDef(name: string, mat: Material | MaterialDef): MaterialDef {
  if ("name" in mat && "color" in mat) {
    return mat as MaterialDef;
  }
  if ((mat as Material).kind === "named") {
    return {
      name: (mat as { name: string }).name,
      color: [0.75, 0.75, 0.78],
      metallic: 0.3,
      roughness: 0.5,
    };
  }
  if ((mat as Material).kind === "pbr") {
    const m = mat as {
      albedo: [number, number, number];
      roughness: number;
      metallic: number;
    };
    return {
      name,
      color: m.albedo,
      metallic: m.metallic,
      roughness: m.roughness,
    };
  }
  throw new Error(`Unrecognised material spec for "${name}"`);
}

// ── Op handlers ────────────────────────────────────────────────────

function applyPrimitive(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "primitive" }>,
  result: BuildResult,
): void {
  let csg: CsgOp;
  switch (op.kind) {
    case "cube":
      csg = { type: "Cube", size: op.size };
      break;
    case "cylinder":
      csg = {
        type: "Cylinder",
        radius: op.radius,
        height: op.height,
        segments: op.segments ?? 32,
      };
      break;
    case "sphere":
      csg = { type: "Sphere", radius: op.radius, segments: op.segments ?? 24 };
      break;
    case "cone":
      csg = {
        type: "Cone",
        radius_bottom: op.radius_bottom,
        radius_top: op.radius_top,
        height: op.height,
        segments: op.segments ?? 32,
      };
      break;
    case "torus":
    case "wedge":
      throw new Error(
        `primitive '${op.kind}' is not in the IR yet — use raw_ir or compose from cubes/cylinders.`,
      );
    default:
      throw new Error(`Unknown primitive kind: ${(op as { kind: string }).kind}`);
  }

  let id = addNode(doc, reg, result, op.name, csg);

  // Optional translate to put the primitive at `at`.
  if (op.at !== undefined) {
    const offset = asVec3(op.at);
    if (offset.x !== 0 || offset.y !== 0 || offset.z !== 0) {
      id = addNode(doc, reg, result, undefined, {
        type: "Translate",
        child: id,
        offset,
      });
      // The translate is the new "named" node iff the primitive was named.
      if (op.name) reg.byName.set(op.name, id);
    }
  }

  attachRoot(doc, id, op.material ?? "default");
  if (op.material) doc.part_materials[String(id)] = op.material;
}

function applyBoolean(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "union" | "difference" | "intersection" }>,
  result: BuildResult,
): void {
  if (op.op === "difference") {
    const subjectId = resolveNodeRef(reg, op.subject);
    const subjectMat = removeRoot(doc, subjectId) ?? "default";
    let id = subjectId;
    for (const t of op.tools) {
      const toolId = resolveNodeRef(reg, t);
      removeRoot(doc, toolId);
      id = addNode(doc, reg, result, undefined, {
        type: "Difference",
        left: id,
        right: toolId,
      });
    }
    if (op.name) reg.byName.set(op.name, id);
    if (op.name && result.named_nodes) result.named_nodes[op.name] = id;
    attachRoot(doc, id, op.material ?? subjectMat);
    return;
  }

  const subjects = op.subjects;
  if (!subjects || subjects.length === 0) {
    throw new Error(`${op.op}: needs at least one subject.`);
  }
  let id = resolveNodeRef(reg, subjects[0]);
  let firstMat = removeRoot(doc, id) ?? "default";
  for (let i = 1; i < subjects.length; i++) {
    const next = resolveNodeRef(reg, subjects[i]);
    removeRoot(doc, next);
    id = addNode(doc, reg, result, undefined, {
      type: op.op === "union" ? "Union" : "Intersection",
      left: id,
      right: next,
    });
  }
  if (op.name) reg.byName.set(op.name, id);
  if (op.name && result.named_nodes) result.named_nodes[op.name] = id;
  attachRoot(doc, id, op.material ?? firstMat);
}

function applyTransform(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "translate" | "rotate" | "scale" }>,
  result: BuildResult,
): void {
  for (const subj of op.subjects) {
    const id = resolveNodeRef(reg, subj);
    const mat = removeRoot(doc, id) ?? "default";
    let csg: CsgOp;
    if (op.op === "translate") {
      csg = { type: "Translate", child: id, offset: op.offset };
    } else if (op.op === "rotate") {
      const axis = resolveAxis(op.axis, reg, doc);
      const a = (op.angle_deg * Math.PI) / 180;
      // Convert axis-angle to Euler XYZ — assume principal axes are common.
      const angles = axisAngleToEuler(axis.direction, a);
      csg = { type: "Rotate", child: id, angles };
    } else {
      const factor = typeof op.factor === "number"
        ? { x: op.factor, y: op.factor, z: op.factor }
        : op.factor;
      csg = { type: "Scale", child: id, factor };
    }
    const newId = addNode(doc, reg, result, op.name, csg);
    attachRoot(doc, newId, mat);
    // Migrate the registered name from the source to the wrapper so
    // subsequent ops referencing this name pick up the transformed node.
    const sourceName = doc.nodes[String(id)]?.name;
    if (sourceName) reg.byName.set(sourceName, newId);
  }
}

function axisAngleToEuler(axis: Vec3, angle: number): Vec3 {
  // Simple specialisation — most rotations target a principal axis.
  const e = 1e-6;
  if (Math.abs(axis.x) > 1 - e) return { x: angle, y: 0, z: 0 };
  if (Math.abs(axis.y) > 1 - e) return { x: 0, y: angle, z: 0 };
  if (Math.abs(axis.z) > 1 - e) return { x: 0, y: 0, z: angle };
  // Fallback — fold the rotation into Z. The kernel's evaluator
  // applies XYZ Euler in order; for non-principal axes the agent
  // should compose principal-axis rotations.
  return { x: 0, y: 0, z: angle };
}

function applyFeature(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "fillet" | "chamfer" | "shell" }>,
  result: BuildResult,
): void {
  const targetName = op.target ?? targetFromEdges(op);
  const id = resolveNodeRef(reg, targetName);
  const mat = removeRoot(doc, id) ?? "default";
  let csg: CsgOp;
  if (op.op === "fillet") {
    csg = { type: "Fillet", child: id, radius: op.radius };
  } else if (op.op === "chamfer") {
    csg = { type: "Chamfer", child: id, distance: op.distance };
  } else {
    csg = { type: "Shell", child: id, thickness: op.thickness };
  }
  const newId = addNode(doc, reg, result, op.name, csg);
  attachRoot(doc, newId, mat);
  const sourceName = doc.nodes[String(id)]?.name;
  if (sourceName) reg.byName.set(sourceName, newId);
}

function targetFromEdges(
  op: Extract<BuildOp, { op: "fillet" | "chamfer" }>,
): NodeRef {
  const e = op.edges?.[0];
  if (!e) throw new Error(`${op.op}: edges[] is required.`);
  if ("between_faces" in e) return e.between_faces[0].node;
  return e.node;
}

function applyPattern(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "linear_pattern" | "circular_pattern" }>,
  result: BuildResult,
): void {
  for (const subj of op.subjects) {
    const id = resolveNodeRef(reg, subj);
    const mat = removeRoot(doc, id) ?? "default";
    let csg: CsgOp;
    if (op.op === "linear_pattern") {
      csg = {
        type: "LinearPattern",
        child: id,
        direction: op.direction,
        count: op.count,
        spacing: op.spacing,
      };
    } else {
      const axis = resolveAxis(op.axis, reg, doc);
      csg = {
        type: "CircularPattern",
        child: id,
        axis_origin: axis.origin,
        axis_dir: axis.direction,
        count: op.count,
        angle_deg: op.angle_deg ?? 360,
      };
    }
    const newId = addNode(doc, reg, result, op.name, csg);
    attachRoot(doc, newId, mat);
    const sourceName = doc.nodes[String(id)]?.name;
    if (sourceName) reg.byName.set(sourceName, newId);
  }
}

function applyMirror(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "mirror" }>,
  result: BuildResult,
): void {
  const plane = resolvePlane(op.plane, reg, doc);
  const factor = mirrorFactorFromNormal(plane.normal);
  for (const subj of op.subjects) {
    const id = resolveNodeRef(reg, subj);
    const mat = removeRoot(doc, id) ?? "default";
    const mirrored = addNode(doc, reg, result, undefined, {
      type: "Scale",
      child: id,
      factor,
    });
    if (op.merge) {
      const merged = addNode(doc, reg, result, op.name, {
        type: "Union",
        left: id,
        right: mirrored,
      });
      attachRoot(doc, merged, mat);
    } else {
      if (op.name) reg.byName.set(op.name, mirrored);
      attachRoot(doc, mirrored, mat);
    }
  }
}

function mirrorFactorFromNormal(normal: Vec3): Vec3 {
  // Pick the principal axis closest to the normal and flip it.
  const ax = Math.abs(normal.x);
  const ay = Math.abs(normal.y);
  const az = Math.abs(normal.z);
  if (ax >= ay && ax >= az) return { x: -1, y: 1, z: 1 };
  if (ay >= az) return { x: 1, y: -1, z: 1 };
  return { x: 1, y: 1, z: -1 };
}

function applySetMaterial(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "set_material" }>,
): void {
  const matKey =
    typeof op.material === "string"
      ? op.material
      : ((op.material as { kind: "named"; name: string }).kind === "named"
          ? (op.material as { name: string }).name
          : registerInlineMaterial(doc, op.material));
  for (const subj of op.subjects) {
    const id = resolveNodeRef(reg, subj);
    doc.part_materials[String(id)] = matKey;
    for (const r of doc.roots) {
      if (r.root === id) r.material = matKey;
    }
  }
}

function registerInlineMaterial(doc: Document, mat: Material): string {
  if ((mat as { kind: string }).kind === "named") {
    return (mat as { kind: "named"; name: string }).name;
  }
  const def = mat as Extract<Material, { kind: "pbr" }>;
  let n = 0;
  let key: string;
  do {
    key = `pbr_${n++}`;
  } while (doc.materials[key]);
  doc.materials[key] = {
    name: key,
    color: def.albedo,
    metallic: def.metallic,
    roughness: def.roughness,
  };
  return key;
}

function applyRename(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "rename" }>,
): void {
  const id = resolveNodeRef(reg, op.node);
  const node = doc.nodes[String(id)];
  if (!node) throw new Error(`rename: missing node "${op.node}"`);
  if (node.name) reg.byName.delete(node.name);
  node.name = op.name;
  reg.byName.set(op.name, id);
}

function applyDelete(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "delete" }>,
  result: BuildResult,
): void {
  for (const subj of op.subjects) {
    const id = resolveNodeRef(reg, subj);
    removeRoot(doc, id);
    delete doc.nodes[String(id)];
    delete doc.part_materials[String(id)];
    const node = Object.values(doc.nodes).find((n) => n.id === id);
    if (!node) result.removed_nodes.push(id);
    reg.byName.delete(subj);
  }
}

function applyRawIr(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "raw_ir" }>,
  result: BuildResult,
): void {
  // Re-key the incoming nodes to avoid clashing with the existing graph,
  // while preserving internal references (left/right/child/sketch/...).
  const remap = new Map<NodeId, NodeId>();
  let next = nextNodeId(doc);
  for (const n of op.nodes) {
    remap.set(n.id, next);
    next++;
  }
  for (const n of op.nodes) {
    const newId = remap.get(n.id)!;
    const newOp = remapChildIds(n.op, remap);
    doc.nodes[String(newId)] = { id: newId, name: n.name, op: newOp };
    result.added_nodes.push(newId);
    if (n.name) reg.byName.set(n.name, newId);
  }
  if (op.roots) {
    for (const r of op.roots) {
      const id = remap.get(r) ?? r;
      attachRoot(doc, id, "default");
    }
  }
}

function remapChildIds(op: CsgOp, remap: Map<NodeId, NodeId>): CsgOp {
  const cloned = JSON.parse(JSON.stringify(op)) as Record<string, unknown> & CsgOp;
  for (const key of ["child", "left", "right", "sketch"]) {
    const v = (cloned as Record<string, unknown>)[key];
    if (typeof v === "number") {
      (cloned as Record<string, unknown>)[key] = remap.get(v) ?? v;
    }
  }
  const sketches = (cloned as Record<string, unknown>).sketches;
  if (Array.isArray(sketches)) {
    (cloned as Record<string, unknown>).sketches = sketches.map((s) =>
      typeof s === "number" ? (remap.get(s) ?? s) : s,
    );
  }
  return cloned;
}

// ── Plane / axis def → ref helpers ─────────────────────────────────

function planeRefFromDef(def: Extract<BuildOp, { op: "ref_plane" }>["def"]) {
  if ("kind" in def) {
    if (def.offset)
      return {
        offset: { from: { kind: def.kind } as { kind: "xy" | "xz" | "yz" }, distance: def.offset },
      };
    return { kind: def.kind };
  }
  // origin/normal and three_points need topology-aware resolution; for
  // now, fall back to XY plane and let the caller refine via raw_ir.
  return { kind: "xy" as const };
}

function axisRefFromDef(def: Extract<BuildOp, { op: "ref_axis" }>["def"]) {
  if ("kind" in def) return { kind: def.kind };
  return { from: def.from, to: def.to };
}

// ── Sketch op handlers ─────────────────────────────────────────────

function applySketch(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "sketch" }>,
  result: BuildResult,
): void {
  const plane = resolvePlane(op.plane, reg, doc);
  const segments = entitiesToSegments(op.entities);
  const id = addNode(doc, reg, result, op.name, {
    type: "Sketch2D",
    origin: plane.origin,
    x_dir: plane.xDir,
    y_dir: plane.yDir,
    segments,
  });
  reg.sketches.set(op.name, id);
  // Sketches don't become roots until extruded/revolved/etc.
}

function entitiesToSegments(entities: SketchEntity[]): SketchSegment2D[] {
  const out: SketchSegment2D[] = [];
  for (const e of entities) {
    switch (e.kind) {
      case "line":
        out.push({ type: "Line", start: e.from, end: e.to });
        break;
      case "circle":
        // Approximate circle as 4 arcs.
        out.push(...arcsAroundCenter(e.center, e.radius, 0, 2 * Math.PI, 8));
        break;
      case "arc": {
        const startAngle = e.start_angle;
        const endAngle = e.end_angle;
        const start: Vec2 = {
          x: e.center.x + Math.cos(startAngle) * e.radius,
          y: e.center.y + Math.sin(startAngle) * e.radius,
        };
        const end: Vec2 = {
          x: e.center.x + Math.cos(endAngle) * e.radius,
          y: e.center.y + Math.sin(endAngle) * e.radius,
        };
        out.push({ type: "Arc", start, end, center: e.center, ccw: endAngle > startAngle });
        break;
      }
      case "rectangle": {
        const c = e.corner;
        const s = e.size;
        const a = c;
        const b = { x: c.x + s.x, y: c.y };
        const cc = { x: c.x + s.x, y: c.y + s.y };
        const d = { x: c.x, y: c.y + s.y };
        out.push({ type: "Line", start: a, end: b });
        out.push({ type: "Line", start: b, end: cc });
        out.push({ type: "Line", start: cc, end: d });
        out.push({ type: "Line", start: d, end: a });
        break;
      }
      case "polygon": {
        const pts: Vec2[] = [];
        for (let i = 0; i < e.sides; i++) {
          const a = (e.rotation_deg ?? 0) * (Math.PI / 180) + (i * 2 * Math.PI) / e.sides;
          pts.push({ x: e.center.x + Math.cos(a) * e.radius, y: e.center.y + Math.sin(a) * e.radius });
        }
        for (let i = 0; i < pts.length; i++) {
          const next = (i + 1) % pts.length;
          out.push({ type: "Line", start: pts[i], end: pts[next] });
        }
        break;
      }
      case "ellipse":
      case "spline":
      case "text":
        // Tessellate as polylines — a single segment per chord.
        // For now we approximate ellipse, skip spline/text.
        if (e.kind === "ellipse") {
          const N = 32;
          const rot = (e.rotation_deg ?? 0) * (Math.PI / 180);
          const pts: Vec2[] = [];
          for (let i = 0; i < N; i++) {
            const a = (i * 2 * Math.PI) / N;
            const x = Math.cos(a) * e.rx;
            const y = Math.sin(a) * e.ry;
            pts.push({
              x: e.center.x + x * Math.cos(rot) - y * Math.sin(rot),
              y: e.center.y + x * Math.sin(rot) + y * Math.cos(rot),
            });
          }
          for (let i = 0; i < pts.length; i++) {
            const j = (i + 1) % pts.length;
            out.push({ type: "Line", start: pts[i], end: pts[j] });
          }
        }
        // spline / text — silently skipped for now.
        break;
    }
  }
  return out;
}

function arcsAroundCenter(
  center: Vec2,
  radius: number,
  start: number,
  end: number,
  steps: number,
): SketchSegment2D[] {
  const out: SketchSegment2D[] = [];
  const span = end - start;
  for (let i = 0; i < steps; i++) {
    const a0 = start + (span * i) / steps;
    const a1 = start + (span * (i + 1)) / steps;
    out.push({
      type: "Arc",
      start: { x: center.x + Math.cos(a0) * radius, y: center.y + Math.sin(a0) * radius },
      end: { x: center.x + Math.cos(a1) * radius, y: center.y + Math.sin(a1) * radius },
      center,
      ccw: true,
    });
  }
  return out;
}

function applyExtrude(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "extrude" }>,
  result: BuildResult,
): void {
  const sketchId = reg.sketches.get(op.sketch) ?? reg.byName.get(op.sketch);
  if (sketchId === undefined) throw new Error(`extrude: unknown sketch "${op.sketch}"`);
  const sketchNode = doc.nodes[String(sketchId)];
  if (!sketchNode) throw new Error(`extrude: sketch node missing`);
  if (sketchNode.op.type !== "Sketch2D") {
    throw new Error(`extrude: node "${op.sketch}" is ${sketchNode.op.type}, not a sketch`);
  }

  const depth = typeof op.depth === "number" ? op.depth : 10;
  // Extrude along the sketch's normal (z_dir = x_dir × y_dir).
  const xd = sketchNode.op.x_dir;
  const yd = sketchNode.op.y_dir;
  const normal = {
    x: xd.y * yd.z - xd.z * yd.y,
    y: xd.z * yd.x - xd.x * yd.z,
    z: xd.x * yd.y - xd.y * yd.x,
  };
  const sign = op.direction === "reverse" ? -1 : 1;
  const direction = { x: normal.x * depth * sign, y: normal.y * depth * sign, z: normal.z * depth * sign };

  const id = addNode(doc, reg, result, op.name, {
    type: "Extrude",
    sketch: sketchId,
    direction,
  });
  attachRoot(doc, id, op.material ?? "default");
  if (op.material) doc.part_materials[String(id)] = op.material;
}

function applyRevolve(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "revolve" }>,
  result: BuildResult,
): void {
  const sketchId = reg.sketches.get(op.sketch) ?? reg.byName.get(op.sketch);
  if (sketchId === undefined) throw new Error(`revolve: unknown sketch "${op.sketch}"`);
  const axis = resolveAxis(op.axis, reg, doc);
  const sign = op.direction === "reverse" ? -1 : 1;
  const id = addNode(doc, reg, result, op.name, {
    type: "Revolve",
    sketch: sketchId,
    axis_origin: axis.origin,
    axis_dir: axis.direction,
    angle_deg: op.angle_deg * sign,
  });
  attachRoot(doc, id, op.material ?? "default");
  if (op.material) doc.part_materials[String(id)] = op.material;
}

function applySweep(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "sweep" }>,
  result: BuildResult,
): void {
  const sketchId = reg.sketches.get(op.profile) ?? reg.byName.get(op.profile);
  if (sketchId === undefined) throw new Error(`sweep: unknown profile "${op.profile}"`);
  // Path: support either a named LinePath/HelixPath in the registry or
  // an inline IR PathCurve literal.
  let path: { type: "Line"; start: Vec3; end: Vec3 } | { type: "Helix"; radius: number; pitch: number; height: number; turns: number };
  if (typeof op.path === "string") {
    throw new Error(`sweep: named paths not yet supported — pass an inline { type: 'Line'|'Helix', ... }`);
  } else {
    path = op.path as typeof path;
  }
  const id = addNode(doc, reg, result, op.name, {
    type: "Sweep",
    sketch: sketchId,
    path,
    twist_angle: op.twist_deg !== undefined ? (op.twist_deg * Math.PI) / 180 : undefined,
  });
  attachRoot(doc, id, op.material ?? "default");
  if (op.material) doc.part_materials[String(id)] = op.material;
}

function applyLoft(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "loft" }>,
  result: BuildResult,
): void {
  const sketches: NodeId[] = [];
  for (const name of op.profiles) {
    const id = reg.sketches.get(name) ?? reg.byName.get(name);
    if (id === undefined) throw new Error(`loft: unknown profile "${name}"`);
    sketches.push(id);
  }
  if (sketches.length < 2) throw new Error(`loft: needs ≥2 profiles`);
  const id = addNode(doc, reg, result, op.name, {
    type: "Loft",
    sketches,
    closed: op.closed ?? false,
  });
  attachRoot(doc, id, op.material ?? "default");
  if (op.material) doc.part_materials[String(id)] = op.material;
}

function applyHole(
  doc: Document,
  reg: NameRegistry,
  op: Extract<BuildOp, { op: "hole" }>,
  result: BuildResult,
): void {
  // Hole desugars to: a cylinder positioned at `at`, subtracted from `target`.
  const target = op.target ?? findFirstRoot(doc, reg);
  if (!target) throw new Error(`hole: no target and no roots present`);
  const targetId = resolveNodeRef(reg, target);
  const targetMat = removeRoot(doc, targetId) ?? "default";

  const center: Vec3 = "node" in (op.at as object)
    ? // FaceRef — use its bbox-derived point as the hole center.
      (() => {
        const fr = op.at as { node: NodeRef; face_role?: string };
        const bbox = bboxOfRoot(doc, resolveNodeRef(reg, fr.node), undefined);
        if (!bbox) return { x: 0, y: 0, z: 0 };
        return {
          x: (bbox.min.x + bbox.max.x) / 2,
          y: (bbox.min.y + bbox.max.y) / 2,
          z: bbox.max.z,
        };
      })()
    : (op.at as Vec3);

  const depth = typeof op.depth === "number" ? op.depth : 100;
  const cyl = addNode(doc, reg, result, undefined, {
    type: "Cylinder",
    radius: op.diameter / 2,
    height: depth,
    segments: 32,
  });
  const placed = addNode(doc, reg, result, undefined, {
    type: "Translate",
    child: cyl,
    offset: { x: center.x, y: center.y, z: center.z - depth / 2 },
  });

  let resultId = addNode(doc, reg, result, undefined, {
    type: "Difference",
    left: targetId,
    right: placed,
  });

  if (op.counterbore) {
    const cb = addNode(doc, reg, result, undefined, {
      type: "Cylinder",
      radius: op.counterbore.diameter / 2,
      height: op.counterbore.depth,
      segments: 32,
    });
    const cbPlaced = addNode(doc, reg, result, undefined, {
      type: "Translate",
      child: cb,
      offset: { x: center.x, y: center.y, z: center.z - op.counterbore.depth / 2 },
    });
    resultId = addNode(doc, reg, result, undefined, {
      type: "Difference",
      left: resultId,
      right: cbPlaced,
    });
  }

  if (op.name) {
    reg.byName.set(op.name, resultId);
    if (result.named_nodes) result.named_nodes[op.name] = resultId;
    doc.nodes[String(resultId)].name = op.name;
  }
  // Re-bind the target's name (if any) to the new result, so subsequent
  // ops referencing the target name pick up the holed version.
  const targetName = doc.nodes[String(targetId)]?.name;
  if (targetName) reg.byName.set(targetName, resultId);

  attachRoot(doc, resultId, targetMat);
}

function findFirstRoot(doc: Document, reg: NameRegistry): NodeRef | undefined {
  if (doc.roots.length === 0) return undefined;
  const node = doc.nodes[String(doc.roots[0].root)];
  if (!node || !node.name) return undefined;
  return node.name;
}

// ── Suppressed unused warnings ────────────────────────────────────
void bboxOfRoot;
