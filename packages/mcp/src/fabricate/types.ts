/**
 * vcad Fabricate — shared types for the design-to-doorstep ordering layer.
 *
 * Phase 0 (the spine): quote a custom-manufactured part from a live session,
 * persist the quote + a QUOTED order. No money moves.
 *
 * Money is integer MINOR units (USD cents) everywhere — never floats. The
 * per-fab cost is server-only and never crosses into a tool result; the agent
 * sees only the margin-inclusive total.
 */

/** Manufacturing processes vcad can quote. */
export type Process = "pcb" | "cnc" | "3dprint" | "sheet_metal" | "cast_metal";

export const PROCESSES: readonly Process[] = [
  "pcb",
  "cnc",
  "3dprint",
  "sheet_metal",
  "cast_metal",
];

/** Full order lifecycle (mirrors the orders.state check constraint). */
export type OrderState =
  | "DRAFT"
  | "QUOTED"
  | "EXPIRED"
  | "AUTHORIZED"
  | "PENDING_PAYMENT"
  | "PAYMENT_FAILED"
  | "PAID"
  | "SUBMITTED"
  | "SUBMIT_FAILED"
  | "RECONCILING"
  | "IN_PRODUCTION"
  | "SHIPPED"
  | "DELIVERED"
  | "CANCELED"
  | "CANCELED_BY_FAB"
  | "REFUNDED";

/** Whether a price is a vcad local estimate or a binding fab quote. */
export type PricingBasis = "estimate" | "binding";

/** Normalized request a broker hands to each adapter. */
export interface QuoteRequest {
  process: Process;
  material?: string;
  quantity: number;
  finish?: string;
  /** Geometry measured from the live session document. */
  metrics: GeometryMetrics;
  /** PCB-only overrides (geometry can't always recover these in Phase 0). */
  layers?: number;
  boardAreaMm2?: number;
  /**
   * Total fab cost (minor units) from the SHARED kernel cost model
   * (estimateCost / vcad-kernel-cost) — the same estimator the app's Build
   * quote uses, so adapters agree with the in-app QuotePanel. Undefined for PCB
   * (no kernel model) or when the estimate is unavailable; adapters then fall
   * back to their local coefficient estimate.
   */
  baseCostMinor?: number;
  /** Resolved kernel-cost catalog material name (e.g. "Aluminum 6061"). */
  materialCatalog?: string;
}

/** Lean geometry summary used by the cost models. */
export interface GeometryMetrics {
  ok: boolean;
  parts: number;
  volume_mm3: number;
  surface_area_mm2: number;
  /** Bounding-box footprint (x * y) — a board-area proxy for PCBs. */
  footprint_mm2: number;
  /** Largest bounding-box dimension — the build-envelope gate. */
  max_dim_mm: number;
  bbox: { min: [number, number, number]; max: [number, number, number] } | null;
}

/** Raw, pre-margin quote from a single manufacturer adapter. */
export interface AdapterQuote {
  /** Per-fab cost in minor units BEFORE vcad margin — server-only. */
  fab_cost_minor: number;
  lead_time_days: number;
  in_spec: boolean;
  pricing_basis: PricingBasis;
  /** Human-readable notes (DFM flags, envelope, min-order, etc.). */
  notes: string[];
}

/** A manufacturer adapter: quote-only in Phase 0 (no createOrder yet). */
export interface ManufacturerAdapter {
  key: string;
  label: string;
  region: string;
  processes: readonly Process[];
  supportsDdp: boolean;
  /** Returns null if this adapter can't serve the request at all. */
  quote(req: QuoteRequest): Promise<AdapterQuote | null>;
}

/** A margin-INCLUSIVE option as shown to the agent. Fab cost is never here. */
export interface FabOption {
  fab: string;
  fab_label: string;
  region: string;
  /** Margin-inclusive, landed, per-unit price (minor units). */
  unit_price_minor: number;
  /** Margin-inclusive, landed, total for the requested quantity (minor units). */
  total_minor: number;
  lead_time_days: number;
  in_spec: boolean;
  pricing_basis: PricingBasis;
  supports_ddp: boolean;
  /** True when this option is orderable today (real contracted fab + binding). */
  orderable: boolean;
  notes: string[];
}

/** Landed-cost breakdown surfaced before the agent commits. */
export interface LandedCost {
  shipping_minor: number;
  duty_minor: number;
  /** "ddp_estimate" → fab is importer-of-record and bears duty volatility. */
  basis: string;
}

/** A lightweight DFM summary attached to a quote. */
export interface DfmSummary {
  checked: boolean;
  passed: boolean;
  violations: string[];
}

/** The persisted quote (and the shape returned to the agent, minus internals). */
export interface Quote {
  quote_id: string;
  document_id: string;
  doc_hash: string | null;
  process: Process;
  material: string | null;
  quantity: number;
  dfm: DfmSummary;
  fab_options: FabOption[];
  landed_cost: LandedCost;
  /** Recommended (cheapest in-spec orderable, else cheapest in-spec) total. */
  total_amount_minor: number;
  currency: string;
  /** Always true — fab cost / margin are never in the returned object. */
  margin_hidden: true;
  expires_at: string;
  created_at: string;
}

/** Server-only economics persisted alongside a quote, never returned. */
export interface QuoteEconomics {
  fab_cost_minor: number;
  margin_minor: number;
}

/** The persisted order lifecycle row. */
export interface Order {
  order_id: string;
  document_id: string;
  quote_id: string;
  state: OrderState;
  fab: string | null;
  fab_order_ref: string | null;
  amount_total_minor: number;
  currency: string;
  ship_to: unknown | null;
  events: Array<{ state: OrderState; at: string; note?: string }>;
  created_at: string;
  updated_at: string;
}
