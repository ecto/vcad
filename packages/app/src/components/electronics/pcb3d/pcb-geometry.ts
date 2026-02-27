/**
 * Pure geometry utilities for 3D PCB rendering.
 *
 * PCB IR coordinates are 2D (x, y in mm). Board lies in kernel XY plane with Z-up normal.
 * After the -90deg X rotation group (same as ViewportContent):
 *   PCB X -> Three.js X
 *   PCB Y -> Three.js -Z
 *   Board normal -> Three.js +Y
 *
 * Ortho camera looks down -Y at the board.
 */

import { Matrix4, Vector3, Quaternion } from "three";
import type { Vec2, PcbLayer, Trace, Via, PadShape } from "@vcad/ir";
import type { LayerConfig } from "@/stores/electronics-store";

// ---------------------------------------------------------------------------
// Layer Z offsets
// ---------------------------------------------------------------------------

/** Copper layer ordering for Z offset computation. */
const COPPER_LAYER_ORDER: PcbLayer[] = [
  "FCu",
  "In1Cu",
  "In2Cu",
  "In3Cu",
  "In4Cu",
  "In5Cu",
  "In6Cu",
  "BCu",
];

const LAYER_Z: Record<string, number> = {
  FCu: 0.8,
  In1Cu: 0.6,
  In2Cu: 0.4,
  In3Cu: 0.2,
  In4Cu: 0.0,
  In5Cu: -0.2,
  In6Cu: -0.4,
  BCu: -0.8,
  FSilkS: 0.9,
  BSilkS: -0.9,
  FMask: 0.85,
  BMask: -0.85,
  EdgeCuts: 0,
  FCrtYd: 0.95,
  BCrtYd: -0.95,
  FFab: 0.92,
  BFab: -0.92,
};

/** Z offset for a layer in kernel Z-up space. */
export function layerZOffset(layer: PcbLayer, explosion = 0): number {
  const base = LAYER_Z[layer] ?? 0;
  // When explosion > 0, exaggerate the Z separation
  return base * (1 + explosion * 4);
}

/** Z position in kernel space (board surface + layer offset). */
export function layerZ(layer: PcbLayer, boardThickness: number, explosion = 0): number {
  return boardThickness / 2 + layerZOffset(layer, explosion);
}

// ---------------------------------------------------------------------------
// Trace instance matrix
// ---------------------------------------------------------------------------

const _pos = new Vector3();
const _scale = new Vector3();
const _quat = new Quaternion();

/** Build a 4x4 transform matrix for a trace segment (unit box [0,0,0]->[1,1,1] scaled to trace). */
export function buildTraceMatrix(
  trace: Trace,
  boardThickness: number,
  explosion: number,
  out: Matrix4,
): Matrix4 {
  const dx = trace.end.x - trace.start.x;
  const dy = trace.end.y - trace.start.y;
  const length = Math.sqrt(dx * dx + dy * dy);
  if (length < 1e-6) {
    out.identity();
    return out;
  }

  const angle = Math.atan2(dy, dx);
  const z = layerZ(trace.layer, boardThickness, explosion);
  const cx = (trace.start.x + trace.end.x) / 2;
  const cy = (trace.start.y + trace.end.y) / 2;

  // In kernel Z-up space: trace lies in XY plane at height z
  _pos.set(cx, cy, z);
  _scale.set(length, trace.width, 0.035); // thin slab
  _quat.setFromAxisAngle(new Vector3(0, 0, 1), angle);

  out.compose(_pos, _quat, _scale);
  return out;
}

// ---------------------------------------------------------------------------
// Pad geometry helpers
// ---------------------------------------------------------------------------

export type PadShapeType = "Circle" | "Rect" | "Oval" | "RoundRect" | "Custom";

export function padShapeType(shape: PadShape): PadShapeType {
  return shape.type;
}

/** Pad radius (half the max dimension) for hit testing. */
export function padRadius(shape: PadShape): number {
  switch (shape.type) {
    case "Circle":
      return shape.diameter / 2;
    case "Rect":
    case "RoundRect":
      return Math.max(shape.width, shape.height) / 2;
    case "Oval":
      return Math.max(shape.width, shape.height) / 2;
    case "Custom":
      return 1; // fallback
  }
}

/** Pad dimensions [width, height] for scaling a unit geometry. */
export function padDimensions(shape: PadShape): [number, number] {
  switch (shape.type) {
    case "Circle":
      return [shape.diameter, shape.diameter];
    case "Rect":
    case "RoundRect":
    case "Oval":
      return [shape.width, shape.height];
    case "Custom":
      return [2, 2];
  }
}

// ---------------------------------------------------------------------------
// Via geometry
// ---------------------------------------------------------------------------

export function viaOuterRadius(via: Via): number {
  return via.diameter / 2;
}

export function viaDrillRadius(via: Via): number {
  return via.drill / 2;
}

// ---------------------------------------------------------------------------
// Coordinate conversion
// ---------------------------------------------------------------------------

/**
 * Convert a raycasted board-plane intersection to PCB coordinates + grid snap.
 * The board plane is at Z=boardSurfaceZ in kernel space.
 * Input point is in kernel Z-up space (inside the rotation group).
 */
export function worldToPcb(
  point: Vector3,
  gridSize: number,
  snapToGrid: boolean,
): Vec2 {
  let x = point.x;
  let y = point.y;
  if (snapToGrid && gridSize > 0) {
    x = Math.round(x / gridSize) * gridSize;
    y = Math.round(y / gridSize) * gridSize;
  }
  return { x, y };
}

// ---------------------------------------------------------------------------
// Layer color helpers
// ---------------------------------------------------------------------------

export function getLayerColor(layers: LayerConfig[], layer: string): string {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.color ?? "#888";
}

export function isLayerVisible(layers: LayerConfig[], layer: string): boolean {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.visible ?? false;
}

/** Copper layer index for consistent ordering. */
export function copperLayerIndex(layer: PcbLayer): number {
  const idx = COPPER_LAYER_ORDER.indexOf(layer);
  return idx >= 0 ? idx : 99;
}
