/**
 * Sheet-metal engine glue. Detects a sheet-metal op chain at a scene root,
 * serialises it into the wire format the Rust kernel's
 * `evaluateSheetMetalChain` accepts, and parses the response into the
 * shape consumers expect.
 *
 * No geometric computation lives here — the kernel owns it. The TS side is
 * a thin transport: walk the IR, call WASM, hand the result to the UI.
 */

import type { CsgOp, Node, NodeId, SheetMetalDirection } from "@vcad/ir";
import type { TriangleMesh } from "./mesh.js";

/** Per-bend summary the property panel reads. Mirrors `BendSummaryDto`. */
export interface SheetMetalBendSummary {
  parent: number;
  child: number;
  /** Bend angle in radians. */
  angle_rad: number;
  /** Inside bend radius (mm). */
  radius: number;
  direction: SheetMetalDirection;
  k_factor: number;
  /** Provenance label (e.g. `"builtin:Al-soft/R1.00t1.00"`, `"manual"`). */
  k_factor_source: string | null;
  /** `θ · (R + K · t)` (mm). */
  allowance_mm: number;
}

export interface SheetMetalModelSummary {
  thickness: number;
  panel_count: number;
  bend_count: number;
  bends: SheetMetalBendSummary[];
}

export interface SheetMetalFlatCrease {
  /** `[[x0, y0], [x1, y1]]` in global flat 2D coords. */
  line: [[number, number], [number, number]];
  angle: number;
  radius: number;
  k_factor: number;
  k_factor_source: string | null;
  direction: SheetMetalDirection;
  bend_id: number;
}

export interface SheetMetalFlatPattern {
  thickness: number;
  /** Outlines (CCW) in global flat 2D coords, one entry per panel. */
  panel_outlines_2d: [number, number][][];
  /** Hole loops per panel. */
  panel_holes_2d: [number, number][][][];
  creases: SheetMetalFlatCrease[];
  area_mm2: number;
  /** `[min_x, min_y, max_x, max_y]`. */
  bbox: [number, number, number, number];
}

/** Everything the engine attaches to an `EvaluatedPart.sheetMetal`. */
export interface SheetMetalRendered {
  flatPattern: SheetMetalFlatPattern;
  model: SheetMetalModelSummary;
}

interface RawResult {
  mesh: { positions: number[]; indices: number[]; normals: number[] };
  flat_pattern: SheetMetalFlatPattern;
  model: SheetMetalModelSummary;
  error: string | null;
}

interface ChainOpBase {
  type: "BaseFlangeRect";
  width: number;
  depth: number;
  thickness: number;
  material: string;
}

interface ChainOpEdge {
  type: "EdgeFlange";
  panelId: number;
  edgeIndex: number;
  length: number;
  angle: number;
  radius: number;
  direction: SheetMetalDirection;
  manualK?: number;
}

type ChainOp = ChainOpBase | ChainOpEdge;

/**
 * Walk back from `rootId` through any chain of sheet-metal ops, returning
 * the chain in base-to-tip order. Returns `null` if the root isn't a
 * sheet-metal op.
 */
export function buildSheetMetalChain(
  rootId: NodeId,
  nodes: Record<string, Node>,
): ChainOp[] | null {
  const root = nodes[String(rootId)];
  if (!root) return null;
  if (
    root.op.type !== "SheetMetalBaseFlangeRect" &&
    root.op.type !== "SheetMetalEdgeFlange"
  ) {
    return null;
  }

  // Walk tip-to-base, then reverse.
  const tipToBase: ChainOp[] = [];
  let material: string | null = null;
  let cursor: Node | undefined = root;
  while (cursor) {
    const op: CsgOp = cursor.op;
    if (op.type === "SheetMetalBaseFlangeRect") {
      tipToBase.push({
        type: "BaseFlangeRect",
        width: op.width,
        depth: op.depth,
        thickness: op.thickness,
        material: op.material,
      });
      material = op.material;
      break;
    } else if (op.type === "SheetMetalEdgeFlange") {
      tipToBase.push({
        type: "EdgeFlange",
        panelId: op.panel_id,
        edgeIndex: op.edge_index,
        length: op.length,
        angle: op.angle,
        radius: op.radius,
        direction: op.direction,
        manualK: op.manual_k,
      });
      cursor = nodes[String(op.parent)];
    } else {
      return null;
    }
  }
  if (material === null) return null;
  return tipToBase.reverse();
}

/** Kernel binding signature — just the function we need. */
interface SheetMetalKernel {
  evaluateSheetMetalChain?(chainJson: string): string;
}

/**
 * Evaluate a sheet-metal chain through the kernel and return the rendered
 * mesh + the data attached to `EvaluatedPart.sheetMetal`. Throws on kernel
 * error so the caller can surface it as a per-root failure.
 */
export function evaluateSheetMetalChain(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
): { mesh: TriangleMesh; sheetMetal: SheetMetalRendered } {
  if (!kernel.evaluateSheetMetalChain) {
    throw new Error(
      "kernel.evaluateSheetMetalChain not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.evaluateSheetMetalChain(JSON.stringify(chain));
  const parsed = JSON.parse(json) as RawResult;
  if (parsed.error) {
    throw new Error(`sheet-metal eval: ${parsed.error}`);
  }
  const mesh: TriangleMesh = {
    positions: new Float32Array(parsed.mesh.positions),
    indices: new Uint32Array(parsed.mesh.indices),
    normals:
      parsed.mesh.normals.length > 0
        ? new Float32Array(parsed.mesh.normals)
        : undefined,
  };
  return {
    mesh,
    sheetMetal: {
      flatPattern: parsed.flat_pattern,
      model: parsed.model,
    },
  };
}
