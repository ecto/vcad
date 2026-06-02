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
  /** Material key (e.g. `"al-soft"`); empty when unspecified. */
  material: string;
  panel_count: number;
  bend_count: number;
  bends: SheetMetalBendSummary[];
}

/** Sheet-metal material from the kernel's built-in registry. */
export interface SheetMetalMaterial {
  name: string;
  display_name: string;
  min_r_over_t: number;
  yield_mpa: number;
  modulus_gpa: number;
  density_kg_m3: number;
  springback_per_radian: number;
}

/** One row of the kernel's built-in bend table. */
export interface SheetMetalBendTableRow {
  material: string;
  thickness_mm: number;
  radius_mm: number;
  k_factor: number;
}

export interface SheetMetalBendTable {
  id: string;
  rows: SheetMetalBendTableRow[];
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

/** A manufacturability finding. Mirrors `ViolationDto`. */
export interface SheetMetalViolation {
  /** Stable rule id, e.g. `"sheet.bend_radius"`. */
  rule: string;
  /** `"Error"` or `"Warning"`. */
  severity: "Error" | "Warning";
  /** One-line human-readable summary. */
  message: string;
  /** Kind-tagged structured detail (for camera-fly / fix actions). */
  detail: { kind: string } & Record<string, unknown>;
}

/**
 * A shop's manufacturing capabilities. Mirrors the Rust `ShopProfile`;
 * drives every DFM rule. Deserialization on the kernel side is
 * field-tolerant, so a partial object still loads (missing keys fall back
 * to {@link DEFAULT_SHOP_PROFILE}).
 */
export interface SheetMetalShopProfile {
  /** Human-readable name (e.g. `"Acme Machining"`). */
  name: string;
  /** Maximum bend length the press brake can form (mm). */
  max_bend_length_mm: number;
  /** Minimum inside-radius / thickness ratio `(R/t)_min`. */
  min_bend_radius_ratio: number;
  /** Minimum formable flange height (mm). */
  min_flange_height_mm: number;
  /** Minimum punched-hole-to-bend-line distance (mm). */
  min_hole_to_bend_mm: number;
  /** Minimum flat between two parallel bends on a panel (mm). */
  min_distance_between_bends_mm: number;
}

/** The kernel's `ShopProfile::generic()` — keep in sync with Rust. */
export const DEFAULT_SHOP_PROFILE: SheetMetalShopProfile = {
  name: "Generic shop",
  max_bend_length_mm: 3000,
  min_bend_radius_ratio: 1,
  min_flange_height_mm: 5,
  min_hole_to_bend_mm: 3,
  min_distance_between_bends_mm: 6,
};

/** Result of {@link checkSheetMetalManufacturability}. */
export interface SheetMetalCheckResult {
  violations: SheetMetalViolation[];
  /** Profile the kernel actually checked against (post field-merge). */
  shop: SheetMetalShopProfile;
}

interface RawCheckResult {
  violations: SheetMetalViolation[];
  shop: SheetMetalShopProfile | null;
  error: string | null;
}

/** Everything the engine attaches to an `EvaluatedPart.sheetMetal`. */
export interface SheetMetalRendered {
  flatPattern: SheetMetalFlatPattern;
  model: SheetMetalModelSummary;
  /** Layered DXF (CUT / BEND_UP / BEND_DOWN) of the flat pattern. */
  dxf: string;
  /** DFM findings vs. the generic shop profile; empty when shop-ready. */
  violations: SheetMetalViolation[];
}

interface RawResult {
  mesh: { positions: number[]; indices: number[]; normals: number[] };
  flat_pattern: SheetMetalFlatPattern;
  model: SheetMetalModelSummary;
  dxf: string;
  violations: SheetMetalViolation[];
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

/** Kernel binding signature — just the functions we need. */
interface SheetMetalKernel {
  evaluateSheetMetalChain?(chainJson: string): string;
  checkSheetMetal?(chainJson: string, shopJson: string): string;
  getSheetMetalMaterials?(): string;
  getSheetMetalBendTable?(): string;
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
      dxf: parsed.dxf,
      violations: parsed.violations,
    },
  };
}

/**
 * Re-run manufacturability for a chain against a specific shop profile.
 *
 * This is the tunable counterpart to the generic-shop violations that ride
 * on {@link SheetMetalRendered}: the DFM inspector calls it with the user's
 * saved shop so the findings reflect their real brake / die / grain rules.
 * Pure query — no mesh, no document re-eval.
 *
 * Pass `shop` omitted/undefined for the generic shop. Throws on kernel
 * error.
 */
export function checkSheetMetalManufacturability(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
  shop?: SheetMetalShopProfile,
): SheetMetalCheckResult {
  if (!kernel.checkSheetMetal) {
    throw new Error(
      "kernel.checkSheetMetal not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.checkSheetMetal(
    JSON.stringify(chain),
    shop ? JSON.stringify(shop) : "",
  );
  const parsed = JSON.parse(json) as RawCheckResult;
  if (parsed.error) {
    throw new Error(`sheet-metal check: ${parsed.error}`);
  }
  return {
    violations: parsed.violations,
    shop: parsed.shop ?? DEFAULT_SHOP_PROFILE,
  };
}

/** Read the kernel's built-in materials registry. */
export function getSheetMetalMaterials(
  kernel: SheetMetalKernel,
): SheetMetalMaterial[] {
  if (!kernel.getSheetMetalMaterials) return [];
  try {
    return JSON.parse(kernel.getSheetMetalMaterials()) as SheetMetalMaterial[];
  } catch {
    return [];
  }
}

/** Read the kernel's built-in bend table. */
export function getSheetMetalBendTable(
  kernel: SheetMetalKernel,
): SheetMetalBendTable {
  if (!kernel.getSheetMetalBendTable) return { id: "", rows: [] };
  try {
    return JSON.parse(kernel.getSheetMetalBendTable()) as SheetMetalBendTable;
  } catch {
    return { id: "", rows: [] };
  }
}
