/**
 * Sheet-metal engine glue. Detects a sheet-metal op chain at a scene root,
 * serialises it into the wire format the Rust kernel's
 * `evaluateSheetMetalChain` accepts, and parses the response into the
 * shape consumers expect.
 *
 * No geometric computation lives here — the kernel owns it. The TS side is
 * a thin transport: walk the IR, call WASM, hand the result to the UI.
 */

import type {
  CsgOp,
  Node,
  NodeId,
  SheetMetalDirection,
  SheetMetalHemKind,
} from "@vcad/ir";
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
  /** Estimated springback (radians) — `material.springback_per_radian × angle`. */
  springback_rad: number;
  /** Brake angle to form (radians) so the part springs back to `angle_rad`. */
  compensated_angle_rad: number;
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
/** Shop pricing rates for sheet-metal cost estimation. Mirrors the Rust
 *  `CostRates`. Field-tolerant deserialization on the kernel side: omitted
 *  keys fall back to {@link DEFAULT_COST_RATES}. */
export interface SheetMetalCostRates {
  currency: string;
  material_usd_per_kg: number;
  cut_usd_per_m: number;
  pierce_usd_each: number;
  bend_usd_each: number;
  setup_usd: number;
  markup_pct: number;
}

/** The kernel's `CostRates::generic()` — keep in sync with Rust. */
export const DEFAULT_COST_RATES: SheetMetalCostRates = {
  currency: "USD",
  material_usd_per_kg: 5.0,
  cut_usd_per_m: 1.2,
  pierce_usd_each: 0.1,
  bend_usd_each: 0.75,
  setup_usd: 25,
  markup_pct: 30,
};

/** A transparent line-itemed cost estimate. Mirrors `CostBreakdown`. */
export interface SheetMetalCostBreakdown {
  currency: string;
  quantity: number;
  material_each: number;
  cut_each: number;
  pierce_each: number;
  bend_each: number;
  setup_each: number;
  subtotal_each: number;
  markup_each: number;
  total_each: number;
  total_run: number;
  mass_kg_each: number;
  cut_length_m: number;
  pierces: number;
  bends: number;
}

/** Result of {@link costSheetMetalChain}. */
export interface SheetMetalCostResult {
  breakdown: SheetMetalCostBreakdown;
  rates: SheetMetalCostRates;
}

interface RawCostResult {
  breakdown: SheetMetalCostBreakdown | null;
  rates: SheetMetalCostRates | null;
  error: string | null;
}

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

interface ChainOpBasePolygon {
  type: "BaseFlangePolygon";
  outline: [number, number][];
  holes?: [number, number][][];
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

interface ChainOpHem {
  type: "Hem";
  panelId: number;
  edgeIndex: number;
  kind: SheetMetalHemKind;
  length: number;
  gap: number;
  direction: SheetMetalDirection;
}

interface ChainOpJog {
  type: "Jog";
  panelId: number;
  edgeIndex: number;
  offset: number;
  length: number;
  radius: number;
  direction: SheetMetalDirection;
}

type ChainOp =
  | ChainOpBase
  | ChainOpBasePolygon
  | ChainOpEdge
  | ChainOpHem
  | ChainOpJog;

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
    root.op.type !== "SheetMetalBaseFlangePolygon" &&
    root.op.type !== "SheetMetalEdgeFlange" &&
    root.op.type !== "SheetMetalHem" &&
    root.op.type !== "SheetMetalJog"
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
    } else if (op.type === "SheetMetalBaseFlangePolygon") {
      tipToBase.push({
        type: "BaseFlangePolygon",
        outline: op.outline.map((p) => [p.x, p.y]),
        holes: op.holes?.map((h) => h.map((p) => [p.x, p.y])),
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
    } else if (op.type === "SheetMetalHem") {
      tipToBase.push({
        type: "Hem",
        panelId: op.panel_id,
        edgeIndex: op.edge_index,
        kind: op.kind ?? "Closed",
        length: op.length,
        gap: op.gap ?? 0,
        direction: op.direction,
      });
      cursor = nodes[String(op.parent)];
    } else if (op.type === "SheetMetalJog") {
      tipToBase.push({
        type: "Jog",
        panelId: op.panel_id,
        edgeIndex: op.edge_index,
        offset: op.offset,
        length: op.length,
        radius: op.radius,
        direction: op.direction,
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
  costSheetMetal?(chainJson: string, ratesJson: string, quantity: number): string;
  sheetMetalSequence?(chainJson: string): string;
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

/** One step in a bend sequence. */
export interface SheetMetalBendStep {
  step: number;
  bend_id: number;
  parent_panel: number;
  child_panel: number;
  depth: number;
  angle_rad: number;
  radius_mm: number;
  compensated_angle_rad: number;
  hinge_length_mm: number;
  rationale: string;
}

interface RawSequenceResult {
  steps: SheetMetalBendStep[];
  error: string | null;
}

/** Compute a feasible bend sequence (outermost-first) for the chain. */
export function sheetMetalSequence(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
): SheetMetalBendStep[] {
  if (!kernel.sheetMetalSequence) {
    throw new Error(
      "kernel.sheetMetalSequence not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.sheetMetalSequence(JSON.stringify(chain));
  const parsed = JSON.parse(json) as RawSequenceResult;
  if (parsed.error) {
    throw new Error(`sheet-metal sequence: ${parsed.error}`);
  }
  return parsed.steps;
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

/**
 * Compute a transparent cost estimate for a sheet-metal chain.
 *
 * Pure query — no mesh evaluation. The kernel rebuilds the model and flat
 * pattern internally. `rates` is merged onto {@link DEFAULT_COST_RATES}
 * field-by-field, so a partial object is fine. `quantity` is clamped to
 * `>= 1`. Throws on kernel error.
 */
export function costSheetMetalChain(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
  rates?: SheetMetalCostRates,
  quantity = 1,
): SheetMetalCostResult {
  if (!kernel.costSheetMetal) {
    throw new Error(
      "kernel.costSheetMetal not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.costSheetMetal(
    JSON.stringify(chain),
    rates ? JSON.stringify(rates) : "",
    Math.max(1, Math.floor(quantity)),
  );
  const parsed = JSON.parse(json) as RawCostResult;
  if (parsed.error) {
    throw new Error(`sheet-metal cost: ${parsed.error}`);
  }
  if (!parsed.breakdown || !parsed.rates) {
    throw new Error("sheet-metal cost: kernel returned empty breakdown");
  }
  return { breakdown: parsed.breakdown, rates: parsed.rates };
}
