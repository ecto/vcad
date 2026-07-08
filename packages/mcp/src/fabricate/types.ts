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

import type { ConfiguratorIntent } from "./kerf/contract.js";

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

/**
 * How firm a price is (aligned with kerf's ACP-CM vocabulary):
 * "estimate" = vcad's own cost model, never gates money; "quoted" = the fab's
 * own displayed price via the kerf rail (may gate money where the cart
 * preserves price); "binding" = fab-committed.
 */
export type PricingBasis = "estimate" | "quoted" | "binding";

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
  /**
   * Which estimator produced `baseCostMinor`, so adapter notes stay truthful:
   * "kernel" = estimateCost / vcad-kernel-cost (the in-app Build quote);
   * "sheet_metal_laser" = the line-itemed laser model behind sheet_metal_cost
   * (pre-markup subtotal — the broker's MARGIN_RATE is the single margin layer).
   */
  baseCostModel?: "kernel" | "sheet_metal_laser";
  /** Resolved kernel-cost catalog material name (e.g. "Aluminum 6061"). */
  materialCatalog?: string;
  /**
   * kerf rail (sheet metal, Wave 0): the fully-built ConfiguratorIntent for a
   * vendor quote. Constructed by quote_manufacturing — order.ts owns intent
   * construction so the persisted kerf_intent_hash is computed over the exact
   * object the adapter sends. The kerf adapter forwards it whole; absent ⇒
   * the adapter returns null and the generic estimator covers.
   */
  kerfIntent?: { intent: ConfiguratorIntent };
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
  /** kerf intent hash the recommended vendor quote is bound to (kerf rail).
   *  Geometry/config/quantity edit ⇒ new hash ⇒ the vendor quote is dead. */
  kerf_intent_hash?: string | null;
  /** kerf quote-job id — the handle for job-state and evidence lookups. */
  kerf_job_id?: string | null;
  /** Pricing basis of the recommended option (agents read this to know
   *  whether the price is an estimate, a fab-displayed quote, or binding). */
  pricing_basis_best?: PricingBasis;
}

/** Server-only economics persisted alongside a quote, never returned. */
export interface QuoteEconomics {
  fab_cost_minor: number;
  margin_minor: number;
}

/** Lifecycle of a spend authorization (mirrors the migration-027 check). */
export type AuthorizationStatus =
  | "pending_human"
  | "authorized"
  | "consumed"
  | "revoked"
  | "expired";

/**
 * A DB-backed, revocable spend authorization (NOT a stateless JWT). The agent
 * PROPOSES one (status pending_human); a HUMAN approves it (→ authorized) out of
 * band via the web app — never through an MCP tool. Only then can place_order
 * consume it. Mirrors the spend_authorizations table.
 */
export interface SpendAuthorization {
  id: string;
  user_id: string;
  quote_id: string | null;
  kind: "one_time" | "standing";
  max_amount_minor: number;
  daily_cap_minor: number | null;
  process_allowlist: string[] | null;
  fab_allowlist: string[] | null;
  doc_hash: string | null;
  status: AuthorizationStatus;
  expires_at: string;
  created_at: string;
}

/** Result of an atomic wallet debit (mirrors the debit_wallet RPC jsonb). */
export interface DebitResult {
  ok: boolean;
  reason?: string;
  balance_minor?: number;
  idempotent?: boolean;
}

/**
 * A reference to a stored fab bundle (Gerbers, drill, P&P, BOM) in the artifact
 * store. Metadata only — bytes never travel through model context, only this
 * handle. The downstream fab-submission worker fetches the files from
 * artifact_url; the manifest's per-file sha256 lets it verify what it sends.
 */
export interface FabArtifactRef {
  artifact_id: string;
  artifact_url: string;
  bytes: number;
  manifest: Array<{ file: string; bytes: number; sha256: string }>;
}

/**
 * Design-receipt verdict recorded on an order by place_order's receipt gate
 * (M4). "holds" = every clearance claim re-verified at place time; "stale" /
 * "violated" never reach a PLACED order (the gate refuses; the refusal also
 * persists "violated" on the still-QUOTED order so the block survives
 * session non-residency — only a re-quote resets it); "unverified" =
 * the document carried no claims (or wasn't resident) — flagged, not blocked.
 */
export type OrderReceiptStatus = "holds" | "stale" | "violated" | "unverified";

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
  /** Fab files bound to this order, kept out of context (set when an artifact
   *  handle is passed to quote_manufacturing / place_order). */
  fab_artifact?: FabArtifactRef | null;
  /** Spend authorization proposed/consumed for this order (money plane). */
  authorization_id: string | null;
  /** Receipt-gate verdict recorded at place time (see OrderReceiptStatus). */
  receipt_status: OrderReceiptStatus | null;
  /** kerf intent hash the order's quote was bound to — the geometry-edit
   *  tripwire place_order names when it refuses on a doc_hash mismatch. */
  kerf_intent_hash: string | null;
  created_at: string;
  updated_at: string;
}
