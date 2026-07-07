/**
 * vcad project-level BOM tools — Phase 0 (estimates only, no money).
 *
 *   bom_create    — start a BOM (optionally seeded with a full line list in
 *                   one call, the stateless-friendly path), attached to a
 *                   session document or standalone.
 *   bom_add_line  — append one line: 'manufactured' (a part vcad quoted or a
 *                   fab artifact — links persisted quotes/orders from
 *                   quote_manufacturing by id) or 'cots' (an off-the-shelf
 *                   part — links the mechanical catalog by id).
 *   bom_export    — render the BOM as markdown, CSV, or JSON with subtotals,
 *                   a landed-cost shipping estimate (reusing the Fabricate
 *                   shipping model), and a DesignReceipt cost claim.
 *
 * Money is integer MINOR units (USD cents) internally, mirroring the
 * fabricate module; tool inputs/outputs use USD. Every price is an ESTIMATE
 * (Phase-0 quotes, catalog price bands, or caller-supplied numbers) and every
 * export says so — the same honesty rule as quote_manufacturing.
 *
 * BOMs live in a module-global in-memory store scoped by owner (like the
 * in-memory fabricate store): they survive across calls in one process but
 * not across serverless instances — the one-call `bom_create` bulk path
 * exists for exactly that reason.
 */

import { randomUUID } from "node:crypto";
import type { ReceiptClaim } from "@vcad/ir";
import type { AuthUser } from "../oauth.js";
import { ownerId, type FabricateStore } from "../fabricate/store.js";
import { estimateLandedCost } from "../fabricate/pricing.js";
import { PROCESSES } from "../fabricate/types.js";
import { mechCatalog, type MechPart } from "./mech-parts.js";

type ToolResult = { content: Array<{ type: "text"; text: string }>; isError?: boolean };

function ok(payload: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}
function err(message: string): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify({ error: message }) }], isError: true };
}

const PRICING_NOTE =
  "All prices are ESTIMATES (Phase-0 quote estimates, catalog price bands, or caller-supplied numbers) — not binding quotes.";

/** Where a line's price came from. Everything is an estimate; this records which kind. */
export type BomPricingBasis =
  | "quote_estimate" // linked quote_manufacturing quote/order
  | "catalog_estimate" // mechanical-catalog price band midpoint
  | "manual_estimate" // caller-supplied price
  | "unpriced"; // no price — excluded from totals, flagged in exports

/** A custom-manufactured part (PCB, sheet metal, 3D print, CNC, casting). */
export interface BomLineManufactured {
  kind: "manufactured";
  line_id: string;
  name: string;
  process: string;
  vendor: string | null;
  qty: number;
  unit_price_minor: number | null;
  total_minor: number | null;
  pricing_basis: BomPricingBasis;
  /** Link to a persisted quote_manufacturing quote (prices auto-fill from it). */
  quote_id: string | null;
  /** Link to a persisted quote_manufacturing order. */
  order_id: string | null;
  /** The design this line manufactures (session id or inline:<hash> ref). */
  document_id: string | null;
  /** Fab artifact path or artifact id (gerbers/STEP/STL), recorded verbatim. */
  artifact: string | null;
  material: string | null;
  notes: string | null;
}

/** An off-the-shelf part (bearing, shaft, screws, magnets, ESC, …). */
export interface BomLineCots {
  kind: "cots";
  line_id: string;
  name: string;
  spec: string | null;
  example_pn: string | null;
  /** Link to a `search_mechanical_parts` catalog entry (spec/price auto-fill). */
  catalog_id: string | null;
  vendor: string | null;
  qty: number;
  unit_price_minor: number | null;
  total_minor: number | null;
  pricing_basis: BomPricingBasis;
  notes: string | null;
}

export type BomLine = BomLineManufactured | BomLineCots;

/** A project bill of materials. */
export interface Bom {
  bom_id: string;
  title: string;
  document_id: string | null;
  assembly_notes: string[];
  lines: BomLine[];
  currency: "USD";
  created_at: string;
  updated_at: string;
}

/** Totals with the landed-cost shipping estimate. */
export interface BomTotals {
  manufactured_subtotal_minor: number;
  cots_subtotal_minor: number;
  shipping_estimate_minor: number;
  shipping_basis: string;
  grand_total_minor: number;
  unpriced_lines: number;
  currency: "USD";
}

// ── In-memory store (module-global, owner-scoped — mirrors fabricate) ────────

const memBoms = new Map<string, Bom>();
const bomKey = (owner: string, id: string): string => `${owner}::${id}`;

/** Test-only: drop all BOMs. */
export function clearBoms(): void {
  memBoms.clear();
}

function getBom(owner: string, bomId: string): Bom | null {
  return memBoms.get(bomKey(owner, bomId)) ?? null;
}

// ── money helpers ────────────────────────────────────────────────────────────

const toUsd = (minor: number): number => Math.round(minor) / 100;
const usdToMinor = (usd: number): number => Math.round(usd * 100);
const fmtUsd = (minor: number | null): string =>
  minor === null ? "—" : `$${(Math.round(minor) / 100).toFixed(2)}`;

// ── totals ───────────────────────────────────────────────────────────────────

/**
 * Sum the BOM. Shipping reuses the Fabricate landed-cost model: one flat
 * domestic-estimate shipment per distinct vendor — but ONLY for lines not
 * priced from a persisted quote, because quote_manufacturing unit prices are
 * already margin-inclusive AND landed (double-counting guard). Lines without
 * a vendor pool into one "unspecified" shipment.
 */
export function computeTotals(bom: Bom): BomTotals {
  let manufactured = 0;
  let cots = 0;
  let unpriced = 0;
  const shipVendors = new Set<string>();

  for (const line of bom.lines) {
    if (line.total_minor === null) {
      unpriced += 1;
    } else if (line.kind === "manufactured") {
      manufactured += line.total_minor;
    } else {
      cots += line.total_minor;
    }
    if (line.pricing_basis !== "quote_estimate") {
      shipVendors.add((line.vendor ?? "unspecified").trim().toLowerCase() || "unspecified");
    }
  }

  const perShipment = estimateLandedCost({ region: "US", supportsDdp: false }).shipping_minor;
  const shipping = shipVendors.size * perShipment;
  return {
    manufactured_subtotal_minor: manufactured,
    cots_subtotal_minor: cots,
    shipping_estimate_minor: shipping,
    shipping_basis: `flat domestic estimate (${fmtUsd(perShipment)}) per distinct vendor (${shipVendors.size}); quote-linked lines excluded — their prices are already landed`,
    grand_total_minor: manufactured + cots + shipping,
    unpriced_lines: unpriced,
    currency: "USD",
  };
}

// ── DesignReceipt cost claim ─────────────────────────────────────────────────

const BOM_ORACLE = { id: "vcad-mcp/bom", version: "1" };

/**
 * The BOM's claim for the unified DesignReceipt (schema vcad.receipt/1):
 * the oracle re-sums the line math, so a consistent BOM is a `pass` with the
 * grand total as the measured value. Like `pcb.sourcing.snapshot`, it is
 * informational — cost never gates a design verdict — and the estimate basis
 * is always spelled out in `details`. A BOM with no priced lines is
 * `unverifiable` (no evidence is not a clean total), never a silent zero.
 */
export function bomCostClaim(bom: Bom, totals: BomTotals): ReceiptClaim {
  const pricedLines = bom.lines.length - totals.unpriced_lines;
  const base = {
    id: "bom.cost.total",
    domain: "bom",
    description: `estimated landed cost of the project BOM (${bom.lines.length} line(s))`,
    oracle: BOM_ORACLE,
    ...(bom.document_id ? { subject: `document:${bom.document_id}` } : {}),
  };
  if (bom.lines.length === 0 || pricedLines === 0) {
    return {
      ...base,
      verdict: "unverifiable",
      details:
        bom.lines.length === 0
          ? "BOM has no lines — nothing to sum."
          : `all ${bom.lines.length} line(s) are unpriced — no cost evidence.`,
    };
  }
  return {
    ...base,
    verdict: "pass",
    measured: { value: toUsd(totals.grand_total_minor), unit: "USD" },
    details:
      `Line totals re-summed (${pricedLines} priced` +
      (totals.unpriced_lines > 0 ? `, ${totals.unpriced_lines} unpriced excluded` : "") +
      `) + shipping estimate ${fmtUsd(totals.shipping_estimate_minor)}. ${PRICING_NOTE} Informational — cost never gates a design verdict.`,
  };
}

// ── line construction ────────────────────────────────────────────────────────

const str = (v: unknown): string | null =>
  typeof v === "string" && v.trim() ? v.trim() : null;

function posNumber(v: unknown, fallback: number): number {
  const n = Number(v);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

/** Build one line from tool args, resolving quote/order/catalog links. */
async function buildLine(
  args: Record<string, unknown>,
  store: FabricateStore,
  owner: string,
): Promise<BomLine | { error: string }> {
  const kind = String(args.kind ?? "");
  if (kind !== "manufactured" && kind !== "cots") {
    return { error: `Line kind must be 'manufactured' or 'cots', got "${kind}".` };
  }
  const lineId = randomUUID().slice(0, 8);
  const explicitUnitUsd =
    typeof args.unit_price_usd === "number" && Number.isFinite(args.unit_price_usd)
      ? (args.unit_price_usd as number)
      : null;

  if (kind === "manufactured") {
    const name = str(args.name);
    if (!name) return { error: "Manufactured lines need a `name`." };

    let process = str(args.process);
    let vendor = str(args.vendor);
    let qty = posNumber(args.qty, 0);
    let unitMinor = explicitUnitUsd !== null ? usdToMinor(explicitUnitUsd) : null;
    let basis: BomPricingBasis = unitMinor !== null ? "manual_estimate" : "unpriced";
    let material = str(args.material);
    let documentId = str(args.document_id);
    const quoteId = str(args.quote_id);
    const orderId = str(args.order_id);

    if (quoteId) {
      const quote = await store.getQuote(quoteId, owner);
      if (!quote) {
        return {
          error: `Unknown quote_id "${quoteId}" (quotes are owner-scoped and, on serverless without a durable store, per-instance). Re-run quote_manufacturing or pass prices explicitly.`,
        };
      }
      process = process ?? quote.process;
      material = material ?? quote.material;
      documentId = documentId ?? quote.document_id;
      if (qty <= 0) qty = quote.quantity;
      if (unitMinor === null && quote.quantity > 0) {
        unitMinor = Math.round(quote.total_amount_minor / quote.quantity);
        basis = "quote_estimate";
      }
      if (!vendor) {
        // The recommended option is the one whose total the quote adopted.
        const rec =
          quote.fab_options.find((o) => o.total_minor === quote.total_amount_minor) ??
          quote.fab_options[0];
        vendor = rec ? rec.fab_label : null;
      }
    } else if (orderId) {
      const order = await store.getOrder(orderId, owner);
      if (!order) {
        return {
          error: `Unknown order_id "${orderId}". Re-run quote_manufacturing or pass prices explicitly.`,
        };
      }
      vendor = vendor ?? order.fab;
      documentId = documentId ?? order.document_id;
      if (qty <= 0) qty = 1;
      if (unitMinor === null) {
        unitMinor = Math.round(order.amount_total_minor / qty);
        basis = "quote_estimate";
      }
    }

    if (qty <= 0) qty = 1;
    if (!process) {
      return {
        error: `Manufactured lines need a \`process\` (${PROCESSES.join(" | ")}, or a free-form label) — or a \`quote_id\` to pull it from.`,
      };
    }
    return {
      kind: "manufactured",
      line_id: lineId,
      name,
      process,
      vendor,
      qty,
      unit_price_minor: unitMinor,
      total_minor: unitMinor === null ? null : Math.round(unitMinor * qty),
      pricing_basis: basis,
      quote_id: quoteId,
      order_id: orderId,
      document_id: documentId,
      artifact: str(args.artifact),
      material,
      notes: str(args.notes),
    };
  }

  // COTS
  let name = str(args.name) ?? str(args.description);
  let spec = str(args.spec);
  let examplePn = str(args.example_pn);
  let unitMinor = explicitUnitUsd !== null ? usdToMinor(explicitUnitUsd) : null;
  let basis: BomPricingBasis = unitMinor !== null ? "manual_estimate" : "unpriced";
  const catalogId = str(args.catalog_id);

  if (catalogId) {
    const entry = mechCatalog().find((p) => p.id === catalogId);
    if (!entry) {
      return {
        error: `Unknown catalog_id "${catalogId}" — use an id from search_mechanical_parts.`,
      };
    }
    name = name ?? entry.name;
    spec = spec ?? specString(entry);
    examplePn = examplePn ?? entry.example_pn;
    if (unitMinor === null) {
      // Midpoint of the catalog price band — an estimate of an estimate,
      // flagged as catalog_estimate.
      unitMinor = Math.round(((entry.price_band_usd[0] + entry.price_band_usd[1]) / 2) * 100);
      basis = "catalog_estimate";
    }
  }
  if (!name) return { error: "COTS lines need a `name`/`description` (or a `catalog_id`)." };

  const qty = posNumber(args.qty, 1);
  return {
    kind: "cots",
    line_id: lineId,
    name,
    spec,
    example_pn: examplePn,
    catalog_id: catalogId,
    vendor: str(args.vendor),
    qty,
    unit_price_minor: unitMinor,
    total_minor: unitMinor === null ? null : Math.round(unitMinor * qty),
    pricing_basis: basis,
    notes: str(args.notes),
  };
}

/** Compact one-line spec from a catalog entry's spec bag (primitives only). */
function specString(entry: MechPart): string {
  const parts: string[] = [];
  for (const [k, v] of Object.entries(entry.spec)) {
    if (Array.isArray(v)) {
      if (k === "lengths_mm") parts.push(`lengths ${v.join("/")} mm`);
      else if (k === "br_mT") parts.push(`Br ${v.join("-")} mT`);
      continue;
    }
    if (typeof v === "number") {
      parts.push(k.endsWith("_mm") ? `${k.slice(0, -3)} ${v} mm` : `${k} ${v}`);
    } else if (typeof v === "string") {
      parts.push(v);
    }
  }
  return parts.join(", ");
}

function lineSummary(line: BomLine): Record<string, unknown> {
  return {
    line_id: line.line_id,
    kind: line.kind,
    name: line.name,
    qty: line.qty,
    unit_price_usd: line.unit_price_minor === null ? null : toUsd(line.unit_price_minor),
    total_usd: line.total_minor === null ? null : toUsd(line.total_minor),
    pricing_basis: line.pricing_basis,
  };
}

function totalsSummary(totals: BomTotals): Record<string, unknown> {
  return {
    manufactured_subtotal_usd: toUsd(totals.manufactured_subtotal_minor),
    cots_subtotal_usd: toUsd(totals.cots_subtotal_minor),
    shipping_estimate_usd: toUsd(totals.shipping_estimate_minor),
    shipping_basis: totals.shipping_basis,
    grand_total_usd: toUsd(totals.grand_total_minor),
    unpriced_lines: totals.unpriced_lines,
    currency: totals.currency,
  };
}

// ── schemas ──────────────────────────────────────────────────────────────────

const lineProperties = {
  kind: {
    type: "string" as const,
    enum: ["manufactured", "cots"],
    description: "'manufactured' (a part vcad quotes/fabs) or 'cots' (off-the-shelf).",
  },
  name: { type: "string" as const, description: "Line name (part or item). For COTS, `description` is an accepted alias." },
  description: { type: "string" as const, description: "COTS alias for `name`." },
  qty: { type: "number" as const, description: "Quantity (> 0; default 1, or the linked quote's quantity)." },
  unit_price_usd: {
    type: "number" as const,
    description:
      "Estimated unit price in USD. Optional when a quote_id / catalog_id fills it; omit entirely for an unpriced line (excluded from totals, flagged in exports).",
  },
  vendor: { type: "string" as const, description: "Vendor / fab / store (e.g. 'jlcpcb', 'SendCutSend', 'Amazon')." },
  notes: { type: "string" as const, description: "Free-form line notes." },
  // manufactured-only
  process: {
    type: "string" as const,
    description: `Manufactured only: process (${PROCESSES.join(" | ")}, or a free-form label). Auto-filled from quote_id.`,
  },
  quote_id: {
    type: "string" as const,
    description:
      "Manufactured only: quote id from quote_manufacturing — auto-fills process, vendor, qty, and landed unit price.",
  },
  order_id: { type: "string" as const, description: "Manufactured only: order id from quote_manufacturing." },
  document_id: { type: "string" as const, description: "Manufactured only: the design this line manufactures." },
  artifact: {
    type: "string" as const,
    description: "Manufactured only: fab artifact path or artifact id (gerber zip, STEP, STL) — recorded verbatim.",
  },
  material: { type: "string" as const, description: "Manufactured only: material (auto-filled from quote_id)." },
  // cots-only
  spec: { type: "string" as const, description: "COTS only: spec string (e.g. '8x22x7 mm, ZZ shields')." },
  example_pn: { type: "string" as const, description: "COTS only: example part number (e.g. '608ZZ', 'ISO 4762 M3x10')." },
  catalog_id: {
    type: "string" as const,
    description:
      "COTS only: id from search_mechanical_parts — auto-fills name, spec, example PN, and a price-band-midpoint estimate.",
  },
};

export const bomCreateSchema = {
  type: "object" as const,
  properties: {
    title: { type: "string" as const, description: "BOM title (e.g. 'Axial-flux demo motor'). Default 'Bill of Materials'." },
    document_id: {
      type: "string" as const,
      description: "Optional session id (open_document) or document ref this BOM belongs to.",
    },
    assembly_notes: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Optional assembly/ordering notes rendered at the end of exports.",
    },
    lines: {
      type: "array" as const,
      items: { type: "object" as const, properties: lineProperties, required: ["kind"] },
      description:
        "Optional full line list to build the BOM in ONE call (recommended on serverless — BOMs are in-memory per instance, so one-shot creation is the robust path).",
    },
  },
  required: [],
};

export const bomAddLineSchema = {
  type: "object" as const,
  properties: {
    bom_id: { type: "string" as const, description: "BOM id from bom_create." },
    ...lineProperties,
  },
  required: ["bom_id", "kind"],
};

export const bomExportSchema = {
  type: "object" as const,
  properties: {
    bom_id: { type: "string" as const, description: "BOM id from bom_create." },
    format: {
      type: "string" as const,
      enum: ["markdown", "csv", "json"],
      description: "Output format (default markdown).",
    },
  },
  required: ["bom_id"],
};

// ── tool handlers ────────────────────────────────────────────────────────────

/** `bom_create` handler. */
export async function bomCreate(
  input: unknown,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const owner = ownerId(user);
  const now = new Date().toISOString();
  const bom: Bom = {
    bom_id: randomUUID(),
    title: str(args.title) ?? "Bill of Materials",
    document_id: str(args.document_id),
    assembly_notes: Array.isArray(args.assembly_notes)
      ? (args.assembly_notes as unknown[]).map(String).filter((s) => s.trim())
      : [],
    lines: [],
    currency: "USD",
    created_at: now,
    updated_at: now,
  };

  const lineInputs = Array.isArray(args.lines) ? (args.lines as unknown[]) : [];
  for (let i = 0; i < lineInputs.length; i++) {
    const built = await buildLine((lineInputs[i] ?? {}) as Record<string, unknown>, store, owner);
    if ("error" in built) return err(`lines[${i}]: ${built.error}`);
    bom.lines.push(built);
  }

  memBoms.set(bomKey(owner, bom.bom_id), bom);
  const totals = computeTotals(bom);
  return ok({
    bom_id: bom.bom_id,
    title: bom.title,
    document_id: bom.document_id,
    lines: bom.lines.map(lineSummary),
    totals: totalsSummary(totals),
    note:
      `${PRICING_NOTE} Add lines with bom_add_line, render with bom_export. ` +
      "BOMs are in-memory per server instance — on serverless, prefer passing the full `lines` list to bom_create in one call.",
  });
}

/** `bom_add_line` handler. */
export async function bomAddLine(
  input: unknown,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const owner = ownerId(user);
  const bomId = String(args.bom_id ?? "");
  const bom = getBom(owner, bomId);
  if (!bom) {
    return err(
      `Unknown bom_id "${bomId}". BOMs are in-memory per server instance — re-create with bom_create (pass the full \`lines\` list in one call on serverless).`,
    );
  }

  const built = await buildLine(args, store, owner);
  if ("error" in built) return err(built.error);
  bom.lines.push(built);
  bom.updated_at = new Date().toISOString();

  const totals = computeTotals(bom);
  return ok({
    bom_id: bom.bom_id,
    added: lineSummary(built),
    line_count: bom.lines.length,
    totals: totalsSummary(totals),
    note: PRICING_NOTE,
  });
}

/** `bom_export` handler. */
export function bomExport(input: unknown, user: AuthUser | null): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
  const owner = ownerId(user);
  const bomId = String(args.bom_id ?? "");
  const bom = getBom(owner, bomId);
  if (!bom) {
    return err(
      `Unknown bom_id "${bomId}". BOMs are in-memory per server instance — re-create with bom_create (pass the full \`lines\` list in one call on serverless).`,
    );
  }
  const format = String(args.format ?? "markdown");
  if (!["markdown", "csv", "json"].includes(format)) {
    return err(`Unknown format "${format}". Use markdown, csv, or json.`);
  }

  const totals = computeTotals(bom);
  const claim = bomCostClaim(bom, totals);
  const payload: Record<string, unknown> = {
    bom_id: bom.bom_id,
    title: bom.title,
    format,
    totals: totalsSummary(totals),
    receipt_claim: claim,
    note:
      `${PRICING_NOTE} \`receipt_claim\` is a vcad.receipt/1 claim (domain 'bom') — ` +
      "append it to a DesignReceipt's claims to carry the cost estimate alongside verification claims (informational; it never gates a design verdict).",
  };
  if (format === "json") {
    payload.bom = bom;
  } else {
    payload.rendered = format === "markdown" ? renderMarkdown(bom, totals) : renderCsv(bom);
  }
  return ok(payload);
}

// ── renderers ────────────────────────────────────────────────────────────────

/** Escape a value for a markdown table cell. */
const md = (v: string | number | null): string =>
  v === null ? "—" : String(v).replace(/\|/g, "\\|").replace(/\r?\n/g, " ");

/** Source cell for a manufactured line: quote/order refs + artifact path. */
function sourceCell(line: BomLineManufactured): string {
  const bits: string[] = [];
  if (line.quote_id) bits.push(`quote ${line.quote_id.slice(0, 8)}`);
  if (line.order_id) bits.push(`order ${line.order_id.slice(0, 8)}`);
  if (line.document_id) bits.push(`doc ${line.document_id}`);
  if (line.artifact) bits.push(line.artifact);
  return bits.length ? bits.join(" · ") : "—";
}

/** Render the BOM as the deliverable markdown document. */
export function renderMarkdown(bom: Bom, totals: BomTotals): string {
  const out: string[] = [];
  out.push(`# ${bom.title}`);
  out.push("");
  const meta = [`vcad BOM \`${bom.bom_id.slice(0, 8)}\``, `generated ${bom.updated_at.slice(0, 10)}`];
  if (bom.document_id) meta.push(`document \`${bom.document_id}\``);
  out.push(meta.join(" · "));
  out.push("");
  out.push(`> ${PRICING_NOTE}`);

  const manufactured = bom.lines.filter((l): l is BomLineManufactured => l.kind === "manufactured");
  const cots = bom.lines.filter((l): l is BomLineCots => l.kind === "cots");

  if (manufactured.length > 0) {
    out.push("");
    out.push("## Manufactured Parts");
    out.push("");
    out.push("| # | Part | Process | Material | Vendor | Qty | Unit | Total | Source |");
    out.push("|---|------|---------|----------|--------|----:|-----:|------:|--------|");
    manufactured.forEach((l, i) => {
      out.push(
        `| ${i + 1} | ${md(l.name)} | ${md(l.process)} | ${md(l.material)} | ${md(l.vendor)} | ${l.qty} | ${fmtUsd(l.unit_price_minor)} | ${fmtUsd(l.total_minor)} | ${md(sourceCell(l))} |`,
      );
    });
  }

  if (cots.length > 0) {
    out.push("");
    out.push("## COTS Parts");
    out.push("");
    out.push("| # | Item | Spec | Example PN | Vendor | Qty | Unit (est.) | Total (est.) |");
    out.push("|---|------|------|------------|--------|----:|------------:|-------------:|");
    cots.forEach((l, i) => {
      out.push(
        `| ${i + 1} | ${md(l.name)} | ${md(l.spec)} | ${md(l.example_pn)} | ${md(l.vendor)} | ${l.qty} | ${fmtUsd(l.unit_price_minor)} | ${fmtUsd(l.total_minor)} |`,
      );
    });
  }

  out.push("");
  out.push("## Totals");
  out.push("");
  out.push("| | |");
  out.push("|---|---:|");
  out.push(`| Manufactured subtotal | ${fmtUsd(totals.manufactured_subtotal_minor)} |`);
  out.push(`| COTS subtotal | ${fmtUsd(totals.cots_subtotal_minor)} |`);
  out.push(`| Shipping estimate | ${fmtUsd(totals.shipping_estimate_minor)} |`);
  out.push(`| **Estimated total** | **${fmtUsd(totals.grand_total_minor)}** |`);
  out.push("");
  out.push(`Shipping basis: ${totals.shipping_basis}.`);
  if (totals.unpriced_lines > 0) {
    out.push("");
    out.push(`**${totals.unpriced_lines} unpriced line(s) excluded from totals.**`);
  }

  if (bom.assembly_notes.length > 0) {
    out.push("");
    out.push("## Assembly Notes");
    out.push("");
    for (const note of bom.assembly_notes) out.push(`- ${note}`);
  }
  out.push("");
  return out.join("\n");
}

/** Escape one CSV field per RFC 4180. */
const csv = (v: string | number | null): string => {
  if (v === null) return "";
  const s = String(v);
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
};

/** Render the BOM as a flat CSV (one row per line, both kinds). */
export function renderCsv(bom: Bom): string {
  const rows: string[] = [
    "kind,name,process,material,spec,example_pn,vendor,qty,unit_price_usd,total_usd,pricing_basis,quote_id,order_id,document_id,artifact,notes",
  ];
  for (const l of bom.lines) {
    const m = l.kind === "manufactured" ? l : null;
    const c = l.kind === "cots" ? l : null;
    rows.push(
      [
        l.kind,
        csv(l.name),
        csv(m?.process ?? null),
        csv(m?.material ?? null),
        csv(c?.spec ?? null),
        csv(c?.example_pn ?? null),
        csv(l.vendor),
        l.qty,
        l.unit_price_minor === null ? "" : toUsd(l.unit_price_minor).toFixed(2),
        l.total_minor === null ? "" : toUsd(l.total_minor).toFixed(2),
        l.pricing_basis,
        csv(m?.quote_id ?? null),
        csv(m?.order_id ?? null),
        csv(m?.document_id ?? null),
        csv(m?.artifact ?? null),
        csv(l.notes),
      ].join(","),
    );
  }
  return rows.join("\r\n") + "\r\n";
}
