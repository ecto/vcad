/**
 * IR document builder library for docs.
 *
 * Makes it practical to embed 3D viewers in every MDX page by reducing
 * 50-200 lines of raw IR to 1-10 lines of builder calls.
 *
 * Usage:
 *   import { doc, cube, cylinder, difference, translate } from "@/lib/ir-helpers";
 *   const plate = doc(difference(cube(100, 60, 5, "plate"), translate(cylinder(3, 10), 0, 0, -5)));
 */

import type {
  Document,
  Node,
  NodeId,
  CsgOp,
  MaterialDef,
  Vec2,
  Vec3,
  SketchSegment2D,
  PathCurve,
} from "@vcad/ir";

// ---------------------------------------------------------------------------
// Module-level context
// ---------------------------------------------------------------------------

let _nodes: Node[] = [];
let _nextId = 0;

function reset() {
  _nodes = [];
  _nextId = 0;
}

function push(op: CsgOp, name?: string): NodeId {
  const id = _nextId++;
  _nodes.push({ id, name: name ?? null, op });
  return id;
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/** Create a box at the origin extending to (sx, sy, sz). */
export function cube(sx: number, sy: number, sz: number, name?: string): NodeId {
  return push({ type: "Cube", size: { x: sx, y: sy, z: sz } }, name);
}

/** Create a centered box (translated so center is at origin). */
export function centeredCube(sx: number, sy: number, sz: number, name?: string): NodeId {
  const c = cube(sx, sy, sz, name);
  return translate(c, -sx / 2, -sy / 2, -sz / 2);
}

/** Create a cylinder along Z with given radius and height. */
export function cylinder(r: number, h: number, name?: string, segments = 0): NodeId {
  return push({ type: "Cylinder", radius: r, height: h, segments }, name);
}

/** Create a centered cylinder (translated so center is at origin). */
export function centeredCylinder(r: number, h: number, name?: string, segments = 0): NodeId {
  const c = cylinder(r, h, name, segments);
  return translate(c, 0, 0, -h / 2);
}

/** Create a sphere centered at the origin. */
export function sphere(r: number, name?: string, segments = 0): NodeId {
  return push({ type: "Sphere", radius: r, segments }, name);
}

/** Create a cone along Z. */
export function cone(rb: number, rt: number, h: number, name?: string, segments = 0): NodeId {
  return push({ type: "Cone", radius_bottom: rb, radius_top: rt, height: h, segments }, name);
}

// ---------------------------------------------------------------------------
// Booleans
// ---------------------------------------------------------------------------

export function union(a: NodeId, b: NodeId, name?: string): NodeId {
  return push({ type: "Union", left: a, right: b }, name);
}

export function difference(a: NodeId, b: NodeId, name?: string): NodeId {
  return push({ type: "Difference", left: a, right: b }, name);
}

export function intersection(a: NodeId, b: NodeId, name?: string): NodeId {
  return push({ type: "Intersection", left: a, right: b }, name);
}

// ---------------------------------------------------------------------------
// Transforms
// ---------------------------------------------------------------------------

export function translate(child: NodeId, x: number, y: number, z: number, name?: string): NodeId {
  return push({ type: "Translate", child, offset: { x, y, z } }, name);
}

export function rotate(child: NodeId, rx: number, ry: number, rz: number, name?: string): NodeId {
  return push({ type: "Rotate", child, angles: { x: rx, y: ry, z: rz } }, name);
}

export function scale(child: NodeId, sx: number, sy: number, sz: number, name?: string): NodeId {
  return push({ type: "Scale", child, factor: { x: sx, y: sy, z: sz } }, name);
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

export function linearPattern(
  child: NodeId,
  dir: { x: number; y: number; z: number },
  count: number,
  spacing?: number,
  name?: string,
): NodeId {
  const s = spacing ?? Math.sqrt(dir.x ** 2 + dir.y ** 2 + dir.z ** 2);
  return push(
    { type: "LinearPattern", child, direction: dir, count, spacing: s },
    name,
  );
}

export function circularPattern(
  child: NodeId,
  axis: { x: number; y: number; z: number },
  count: number,
  angleDeg = 360,
  name?: string,
): NodeId {
  return push(
    {
      type: "CircularPattern",
      child,
      axis_origin: { x: 0, y: 0, z: 0 },
      axis_dir: axis,
      count,
      angle_deg: angleDeg,
    },
    name,
  );
}

// ---------------------------------------------------------------------------
// Features
// ---------------------------------------------------------------------------

export function fillet(child: NodeId, r: number, name?: string): NodeId {
  return push({ type: "Fillet", child, radius: r }, name);
}

export function chamfer(child: NodeId, d: number, name?: string): NodeId {
  return push({ type: "Chamfer", child, distance: d }, name);
}

export function shell(child: NodeId, t: number, name?: string): NodeId {
  return push({ type: "Shell", child, thickness: t }, name);
}

// ---------------------------------------------------------------------------
// Sketch & Sweep
// ---------------------------------------------------------------------------

/** Create a 2D sketch on a plane. */
export function sketch(
  segments: SketchSegment2D[],
  origin: Vec3 = { x: 0, y: 0, z: 0 },
  xDir: Vec3 = { x: 1, y: 0, z: 0 },
  yDir: Vec3 = { x: 0, y: 1, z: 0 },
  name?: string,
): NodeId {
  return push(
    { type: "Sketch2D", origin, x_dir: xDir, y_dir: yDir, segments },
    name,
  );
}

/** Helper to create a line segment for sketches. */
export function line(x1: number, y1: number, x2: number, y2: number): SketchSegment2D {
  return { type: "Line", start: { x: x1, y: y1 }, end: { x: x2, y: y2 } };
}

/** Helper to create an arc segment for sketches. */
export function arc(
  x1: number, y1: number,
  x2: number, y2: number,
  cx: number, cy: number,
  ccw = true,
): SketchSegment2D {
  return {
    type: "Arc",
    start: { x: x1, y: y1 },
    end: { x: x2, y: y2 },
    center: { x: cx, y: cy },
    ccw,
  };
}

/** Create a closed rectangular sketch profile. */
export function rectSketch(
  w: number,
  h: number,
  origin: Vec3 = { x: 0, y: 0, z: 0 },
  name?: string,
): NodeId {
  const hw = w / 2;
  const hh = h / 2;
  return sketch(
    [
      line(-hw, -hh, hw, -hh),
      line(hw, -hh, hw, hh),
      line(hw, hh, -hw, hh),
      line(-hw, hh, -hw, -hh),
    ],
    origin,
    { x: 1, y: 0, z: 0 },
    { x: 0, y: 1, z: 0 },
    name,
  );
}

/** Create a closed circular sketch profile (approximated with arcs). */
export function circleSketch(
  r: number,
  origin: Vec3 = { x: 0, y: 0, z: 0 },
  name?: string,
): NodeId {
  return sketch(
    [
      arc(r, 0, -r, 0, 0, 0, true),
      arc(-r, 0, r, 0, 0, 0, true),
    ],
    origin,
    { x: 1, y: 0, z: 0 },
    { x: 0, y: 1, z: 0 },
    name,
  );
}

/** Extrude a sketch along a direction vector. */
export function extrude(
  sketchId: NodeId,
  dir: Vec3,
  name?: string,
): NodeId {
  return push({ type: "Extrude", sketch: sketchId, direction: dir }, name);
}

/** Revolve a sketch around an axis. */
export function revolve(
  sketchId: NodeId,
  axisOrigin: Vec3,
  axisDir: Vec3,
  angleDeg = 360,
  name?: string,
): NodeId {
  return push(
    {
      type: "Revolve",
      sketch: sketchId,
      axis_origin: axisOrigin,
      axis_dir: axisDir,
      angle_deg: angleDeg,
    },
    name,
  );
}

/** Sweep a sketch along a path. */
export function sweep(
  sketchId: NodeId,
  path: PathCurve,
  name?: string,
): NodeId {
  return push({ type: "Sweep", sketch: sketchId, path }, name);
}

/** Loft between multiple sketch profiles. */
export function loft(sketches: NodeId[], closed?: boolean, name?: string): NodeId {
  return push({ type: "Loft", sketches, closed }, name);
}

// ---------------------------------------------------------------------------
// Material presets
// ---------------------------------------------------------------------------

export const materials: Record<string, MaterialDef> = {
  aluminum: {
    name: "Aluminum",
    color: [0.9, 0.9, 0.92],
    metallic: 0.95,
    roughness: 0.3,
    density: 2.7,
  },
  steel: {
    name: "Steel",
    color: [0.6, 0.6, 0.65],
    metallic: 0.9,
    roughness: 0.4,
    density: 7.8,
  },
  plastic: {
    name: "Plastic",
    color: [0.2, 0.6, 0.9],
    metallic: 0.0,
    roughness: 0.5,
    density: 1.2,
  },
  copper: {
    name: "Copper",
    color: [0.95, 0.64, 0.37],
    metallic: 0.95,
    roughness: 0.25,
    density: 8.96,
  },
  brass: {
    name: "Brass",
    color: [0.88, 0.78, 0.5],
    metallic: 0.9,
    roughness: 0.3,
    density: 8.5,
  },
  rubber: {
    name: "Rubber",
    color: [0.15, 0.15, 0.15],
    metallic: 0.0,
    roughness: 0.9,
    density: 1.1,
  },
  wood: {
    name: "Wood",
    color: [0.55, 0.35, 0.17],
    metallic: 0.0,
    roughness: 0.7,
    density: 0.6,
  },
  abs: {
    name: "ABS Plastic",
    color: [0.15, 0.15, 0.15],
    metallic: 0.0,
    roughness: 0.6,
    density: 1.04,
  },
  white: {
    name: "White Plastic",
    color: [0.95, 0.95, 0.95],
    metallic: 0.0,
    roughness: 0.5,
    density: 1.2,
  },
  red: {
    name: "Red Plastic",
    color: [0.8, 0.15, 0.1],
    metallic: 0.0,
    roughness: 0.5,
    density: 1.2,
  },
  green: {
    name: "Green Plastic",
    color: [0.1, 0.7, 0.2],
    metallic: 0.0,
    roughness: 0.5,
    density: 1.2,
  },
};

// ---------------------------------------------------------------------------
// Document builder — drains the context
// ---------------------------------------------------------------------------

/**
 * Collect all registered nodes into a Document. Call this once per document.
 * Resets the module context after draining.
 */
export function doc(
  rootNode: NodeId,
  material: MaterialDef | string = "aluminum",
): Document {
  const mat: MaterialDef =
    typeof material === "string"
      ? (materials[material] ?? materials.aluminum!)
      : material;

  const matKey = mat.name.toLowerCase().replace(/\s+/g, "-");

  const nodes: Record<string, Node> = {};
  for (const node of _nodes) {
    nodes[node.id.toString()] = node;
  }

  const document: Document = {
    version: "0.1",
      mates: [],
    nodes,
    materials: { [matKey]: mat },
    part_materials: {},
    roots: [{ root: rootNode, material: matKey }],
  };

  reset();
  return document;
}

/**
 * Build a multi-root document (multiple visible parts with different materials).
 */
export function multiDoc(
  roots: Array<{ node: NodeId; material: MaterialDef | string }>,
): Document {
  const allMats: Record<string, MaterialDef> = {};
  const entries: Array<{ root: NodeId; material: string }> = [];

  for (const { node, material } of roots) {
    const mat: MaterialDef =
      typeof material === "string"
        ? (materials[material] ?? materials.aluminum!)
        : material;
    const matKey = mat.name.toLowerCase().replace(/\s+/g, "-");
    allMats[matKey] = mat;
    entries.push({ root: node, material: matKey });
  }

  const nodes: Record<string, Node> = {};
  for (const n of _nodes) {
    nodes[n.id.toString()] = n;
  }

  const document: Document = {
    version: "0.1",
      mates: [],
    nodes,
    materials: allMats,
    part_materials: {},
    roots: entries,
  };

  reset();
  return document;
}
