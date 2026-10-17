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
import { copperSortKey, innerCopperIndex, isCopperLayer } from "@vcad/core";
import type { LayerConfig } from "@/stores/electronics-store";
import { layerConfigFor } from "@/lib/pcb-layer-config";

// ---------------------------------------------------------------------------
// Layer Z offsets
// ---------------------------------------------------------------------------

/** Copper spans this Z band (FCu at +COPPER_Z_TOP, BCu at -COPPER_Z_TOP). */
const COPPER_Z_TOP = 0.8;

/**
 * Copper count assumed when the caller doesn't know the board's: 9 keeps
 * the historical 0.2 spacing (In1 0.6 … In6 -0.4, BCu -0.8); deeper layers
 * widen it so In7/In8 never collapse onto BCu.
 */
function defaultCopperCount(layer: string): number {
  return Math.max(9, innerCopperIndex(layer) + 2);
}

/** Z offset of a copper layer evenly spaced across `copperCount` layers. */
function copperZOffset(layer: string, copperCount: number): number {
  if (copperCount < 2) return COPPER_Z_TOP;
  const rank = layer === "BCu" ? copperCount - 1 : layer === "FCu" ? 0 : innerCopperIndex(layer);
  return COPPER_Z_TOP - (2 * COPPER_Z_TOP * rank) / (copperCount - 1);
}

const LAYER_Z: Record<string, number> = {
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
export function layerZOffset(layer: PcbLayer, explosion = 0, copperCount?: number): number {
  const base = isCopperLayer(layer)
    ? copperZOffset(layer, copperCount ?? defaultCopperCount(layer))
    : (LAYER_Z[layer] ?? 0);
  // When explosion > 0, exaggerate the Z separation
  return base * (1 + explosion * 4);
}

/** Z position in kernel space (board surface + layer offset). */
export function layerZ(
  layer: PcbLayer,
  boardThickness: number,
  explosion = 0,
  copperCount?: number,
): number {
  return boardThickness / 2 + layerZOffset(layer, explosion, copperCount);
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
  return layerConfigFor(layers, layer).color;
}

/** Copper layers without a stored config default to visible. */
export function isLayerVisible(layers: LayerConfig[], layer: string): boolean {
  return layerConfigFor(layers, layer).visible;
}

/** Copper layer index for consistent ordering (FCu first, BCu last). */
export function copperLayerIndex(layer: PcbLayer): number {
  return isCopperLayer(layer) ? copperSortKey(layer) : Number.MAX_SAFE_INTEGER;
}
