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
  SheetMetalEngraving,
  SheetMetalHemKind,
} from "@vcad/ir";
import type { TriangleMesh } from "./mesh.js";
import { findWrappedRoot, type TransformInfo } from "./transform-walk.js";

/** Whether `op` is any foundation-tier sheet-metal op (base flange, edge
 *  flange, hem, jog, or bend relief). The chain root and every detector key
 *  off this one list. */
function isSheetMetalOp(op: CsgOp): boolean {
  return (
    op.type === "SheetMetalBaseFlangeRect" ||
    op.type === "SheetMetalBaseFlangePolygon" ||
    op.type === "SheetMetalEdgeFlange" ||
    op.type === "SheetMetalHem" ||
    op.type === "SheetMetalJog" ||
    op.type === "SheetMetalBendRelief"
  );
}

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
  /** Merged cut profile (panels ∪ allowance strips): first ring is the CCW
   *  exterior, the rest are CW holes — what the DXF CUT layer carries.
   *  Empty when the flat pattern is empty or disconnected. */
  silhouette_2d: [number, number][][];
  /** Surface-marking (engrave) polylines in global flat 2D — open strokes
   *  for the viewer to draw; what the DXF ENGRAVE layer carries. */
  engravings_2d: [number, number][][];
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
  /** Die width (mm) used for min-flange checks, when known. */
  die_width_mm?: number;
  /** Required bend-relief notch width (mm), when the shop publishes one. */
  relief_width_mm?: number;
  /** Required bend-relief notch depth (mm), when the shop publishes one. */
  relief_depth_mm?: number;
  /** Fixed inside bend radius (mm) — set for catalog shops (e.g.
   *  SendCutSend) whose tooling forms exactly one radius per
   *  material/thickness. Bends at any other radius are flagged
   *  `BendRadiusNotFixed`. */
  fixed_bend_radius_mm?: number;
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

/** Wire shape of one engrave primitive — mirrors the kernel's `EngravingOp`. */
type ChainEngraving =
  | { type: "Polyline"; points: [number, number][] }
  | {
      type: "Text";
      text: string;
      x: number;
      y: number;
      height: number;
      angle?: number;
    };

interface ChainOpBase {
  type: "BaseFlangeRect";
  width: number;
  depth: number;
  thickness: number;
  material: string;
  /** Optional built-in shop catalog id (e.g. `"sendcutsend"`). */
  shopProfile?: string;
  /** Optional surface-marking primitives on the base flange. */
  engravings?: ChainEngraving[];
}

interface ChainOpBasePolygon {
  type: "BaseFlangePolygon";
  outline: [number, number][];
  holes?: [number, number][][];
  thickness: number;
  material: string;
  /** Optional built-in shop catalog id (e.g. `"sendcutsend"`). */
  shopProfile?: string;
  /** Optional surface-marking primitives on the base flange. */
  engravings?: ChainEngraving[];
}

interface ChainOpEdge {
  type: "EdgeFlange";
  panelId: number;
  edgeIndex: number;
  length: number;
  angle: number;
  /** Omitted → thickness, or the shop's fixed radius under a shop profile. */
  radius?: number;
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
  /** Omitted → thickness, or the shop's fixed radius under a shop profile. */
  radius?: number;
  direction: SheetMetalDirection;
}

interface ChainOpBendRelief {
  type: "BendRelief";
  /** Notch width (mm). Default `max(1.5·t, 1.0)`. */
  width?: number;
  /** Notch depth (mm). Default `R + t` or the shop's published depth. */
  depth?: number;
}

type ChainOp =
  | ChainOpBase
  | ChainOpBasePolygon
  | ChainOpEdge
  | ChainOpHem
  | ChainOpJog
  | ChainOpBendRelief;

/** Map IR engravings to the kernel chain's wire shape (Vec2 → pairs). */
function engravingsToChain(
  engravings: SheetMetalEngraving[] | undefined,
): { engravings?: ChainEngraving[] } {
  if (!engravings || engravings.length === 0) return {};
  return {
    engravings: engravings.map((e) =>
      e.type === "Polyline"
        ? {
            type: "Polyline",
            points: e.points.map((p) => [p.x, p.y] as [number, number]),
          }
        : {
            type: "Text",
            text: e.text,
            x: e.x,
            y: e.y,
            height: e.height,
            angle: e.angle,
          },
    ),
  };
}

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
  if (!isSheetMetalOp(root.op)) return null;

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
        ...(op.shop_profile !== undefined
          ? { shopProfile: op.shop_profile }
          : {}),
        ...engravingsToChain(op.engravings),
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
        ...(op.shop_profile !== undefined
          ? { shopProfile: op.shop_profile }
          : {}),
        ...engravingsToChain(op.engravings),
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
        ...(op.radius !== undefined ? { radius: op.radius } : {}),
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
        ...(op.radius !== undefined ? { radius: op.radius } : {}),
        direction: op.direction,
      });
      cursor = nodes[String(op.parent)];
    } else if (op.type === "SheetMetalBendRelief") {
      tipToBase.push({
        type: "BendRelief",
        ...(op.width !== undefined ? { width: op.width } : {}),
        ...(op.depth !== undefined ? { depth: op.depth } : {}),
      });
      cursor = nodes[String(op.parent)];
    } else {
      return null;
    }
  }
  if (material === null) return null;
  return tipToBase.reverse();
}

/**
 * Resolve a scene root to the tip of a sheet-metal op chain, walking through
 * any `Translate` / `Rotate` / `Scale` wrappers and accumulating their
 * placement. Returns `{ root, transform }` where `root` is the sheet-metal
 * node id (the chain tip, ready for {@link buildSheetMetalChain}) and
 * `transform` is the world placement to apply to the rendered body (identity
 * for a bare root). Returns `null` when no sheet-metal op is reached.
 *
 * Shares {@link findWrappedRoot} with `findEmbroideryPattern` /
 * `findImportedMesh`, so a positioned bracket — the common case, e.g.
 * `Translate(child: EdgeFlange)` — is recognized as sheet metal the same way a
 * bare root is.
 */
export function findSheetMetalChainRoot(
  rootId: NodeId,
  nodes: Record<string, Node>,
): { root: NodeId; transform: TransformInfo } | null {
  const hit = findWrappedRoot(rootId, nodes, (op) =>
    isSheetMetalOp(op) ? op : null,
  );
  return hit ? { root: hit.node, transform: hit.transform } : null;
}

/** Kernel binding signature — just the functions we need. */
interface SheetMetalKernel {
  evaluateSheetMetalChain?(chainJson: string): string;
  checkSheetMetal?(chainJson: string, shopJson: string): string;
  costSheetMetal?(chainJson: string, ratesJson: string, quantity: number): string;
  sheetMetalSequence?(chainJson: string): string;
  nestSheetMetalParts?(partsJson: string, paramsJson: string): string;
  getSheetMetalMaterials?(): string;
  getSheetMetalBendTable?(): string;
  getSheetMetalShopCatalog?(shopId: string): string;
  sheetMetalFoldedStep?(chainJson: string): string;
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
 * `shop` is either a {@link SheetMetalShopProfile} object, a built-in shop
 * catalog id string (e.g. `"sendcutsend"` — the kernel resolves it to the
 * catalog's per-material/thickness profile), or omitted. When omitted, the
 * kernel uses the chain's own shop profile if the base op carries one,
 * otherwise the generic shop. Throws on kernel error.
 */
export function checkSheetMetalManufacturability(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
  shop?: SheetMetalShopProfile | string,
): SheetMetalCheckResult {
  if (!kernel.checkSheetMetal) {
    throw new Error(
      "kernel.checkSheetMetal not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.checkSheetMetal(
    JSON.stringify(chain),
    shop !== undefined ? JSON.stringify(shop) : "",
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

/** One footprint to nest. */
export interface SheetMetalPartFootprint {
  name?: string;
  width_mm: number;
  height_mm: number;
  quantity: number;
}

/** Nesting parameters. */
export interface SheetMetalNestingParams {
  stock_width_mm: number;
  stock_height_mm: number;
  spacing_mm: number;
  edge_margin_mm: number;
  allow_rotation: boolean;
}

export const DEFAULT_NESTING_PARAMS: SheetMetalNestingParams = {
  stock_width_mm: 2438,
  stock_height_mm: 1219,
  spacing_mm: 3,
  edge_margin_mm: 5,
  allow_rotation: true,
};

/** A single placement returned by nesting. */
export interface SheetMetalPlacement {
  part_index: number;
  copy: number;
  sheet: number;
  x_mm: number;
  y_mm: number;
  width_mm: number;
  height_mm: number;
  rotated: boolean;
  name: string;
}

/** Result of {@link nestSheetMetalParts}. */
export interface SheetMetalNestingResult {
  placements: SheetMetalPlacement[];
  sheets_used: number;
  utilization_pct: number;
  used_area_mm2: number;
  stock_area_mm2: number;
  per_sheet_pct: number[];
  unplaceable: number[];
}

interface RawNestingResult {
  result: SheetMetalNestingResult | null;
  error: string | null;
}

/** Nest part footprints on stock sheets. */
export function nestSheetMetalParts(
  parts: SheetMetalPartFootprint[],
  kernel: SheetMetalKernel,
  params?: SheetMetalNestingParams,
): SheetMetalNestingResult {
  if (!kernel.nestSheetMetalParts) {
    throw new Error(
      "kernel.nestSheetMetalParts not available — rebuild @vcad/kernel-wasm",
    );
  }
  const json = kernel.nestSheetMetalParts(
    JSON.stringify(parts),
    params ? JSON.stringify(params) : "",
  );
  const parsed = JSON.parse(json) as RawNestingResult;
  if (parsed.error || !parsed.result) {
    throw new Error(`sheet-metal nest: ${parsed.error ?? "empty result"}`);
  }
  return parsed.result;
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

/** One thickness row of a shop catalog material. */
export interface SheetMetalShopCatalogRow {
  thickness_mm: number;
  k_factor: number;
  /** The shop's fixed inside bend radius for this material/thickness. */
  inside_radius_mm: number;
  die_width_mm: number;
  min_flange_formed_mm: number;
  min_flange_flat_mm: number;
  relief_depth_mm: number;
  corner_relief_clearance_mm: number;
  max_bend_length_mm: number;
}

/** One material entry of a shop catalog. */
export interface SheetMetalShopCatalogMaterial {
  /** Canonical material key (e.g. `"al-5052"`). */
  key: string;
  display: string;
  aliases: string[];
  rows: SheetMetalShopCatalogRow[];
}

/** A fab service's published bending catalog (e.g. SendCutSend). Mirrors
 *  the kernel's `ShopCatalog` JSON. */
export interface SheetMetalShopCatalog {
  id: string;
  name: string;
  /** Source URLs the data was transcribed from. */
  sources: string[];
  /** Retrieval date of the published data (YYYY-MM-DD). */
  retrieved: string;
  /** Free-form caveats. */
  notes: string[];
  /** Minimum flat part size for bending `[short, long]` (mm). */
  min_part_bend_mm: [number, number];
  /** Maximum flat part size for bending `[short, long]` (mm). */
  max_part_bend_mm: [number, number];
  /** Maximum bend angle the shop forms (degrees from flat). */
  max_bend_angle_deg: number;
  /** Published minimum relief width as a multiple of thickness. */
  relief_width_min_factor: number;
  materials: SheetMetalShopCatalogMaterial[];
}

/**
 * Read a built-in shop catalog (e.g. `"sendcutsend"`) — the fab service's
 * published per-material/thickness bending data: fixed inside radii,
 * K-factors, die widths, min flange sizes, and relief depths. Throws when
 * the id is unknown or the binding is missing.
 */
export function getSheetMetalShopCatalog(
  kernel: SheetMetalKernel,
  shopId: string,
): SheetMetalShopCatalog {
  if (!kernel.getSheetMetalShopCatalog) {
    throw new Error(
      "kernel.getSheetMetalShopCatalog not available — rebuild @vcad/kernel-wasm",
    );
  }
  const parsed = JSON.parse(kernel.getSheetMetalShopCatalog(shopId)) as
    | SheetMetalShopCatalog
    | { error: string };
  if ("error" in parsed && parsed.error) {
    throw new Error(`sheet-metal shop catalog: ${parsed.error}`);
  }
  return parsed as SheetMetalShopCatalog;
}

/**
 * Build the FOLDED sheet-metal solid for a chain and return it as a STEP
 * AP214 file (ASCII). The body carries true cylindrical bend faces sized by
 * the chain's radii/K (shop-profile resolved when the chain names one), so
 * fab services with a 3D pipeline (SendCutSend) auto-detect bends, angles,
 * and directions from the upload. Throws on kernel error — including
 * disconnected models and ≥170° folds (hems), which the folded-body
 * construction cannot represent.
 */
export function foldedSheetMetalStep(
  chain: ChainOp[],
  kernel: SheetMetalKernel,
): string {
  if (!kernel.sheetMetalFoldedStep) {
    throw new Error(
      "kernel.sheetMetalFoldedStep not available — rebuild @vcad/kernel-wasm",
    );
  }
  const parsed = JSON.parse(kernel.sheetMetalFoldedStep(JSON.stringify(chain))) as {
    step: string;
    error: string | null;
  };
  if (parsed.error) {
    throw new Error(`sheet-metal folded STEP: ${parsed.error}`);
  }
  if (!parsed.step) {
    throw new Error("sheet-metal folded STEP: kernel returned empty file");
  }
  return parsed.step;
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
