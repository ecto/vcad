/**
 * vcad Fabricate MCP tools — Phase 0 (the spine, zero money).
 *
 *   quote_manufacturing — measure a live design, run light DFM, fan out to fab
 *                         adapters, return a margin-inclusive quote, and persist
 *                         it + a QUOTED order. NO money moves.
 *   get_order_status    — read an order's lifecycle row.
 *   list_orders         — list the caller's orders.
 *
 * place_order / authorize_spend / wallet tools (the money plane) are Phase 1,
 * gated on the three critical fixes (DB-backed revocable authz, idempotent
 * outbox worker, atomic debit RPC). Nothing here can spend.
 */

import { randomUUID, createHash } from "node:crypto";
import { estimateCost, type Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import type { AuthUser } from "../oauth.js";
import { getSession } from "./session.js";
import { measureDocument } from "../fabricate/geometry.js";
import { FulfillmentBroker } from "../fabricate/broker.js";
import { toDfmProcess, catalogMaterial } from "../fabricate/process-map.js";
import { ownerId, type FabricateStore } from "../fabricate/store.js";
import { buildFabHandoff } from "../fabricate/handoff.js";
import { captureEvent } from "../telemetry.js";
import { resolveArtifactRef } from "./artifact-store.js";
import { behavior, type ToolDef } from "./tool-def.js";
import {
  PROCESSES,
  type DfmSummary,
  type FabArtifactRef,
  type GeometryMetrics,
  type Order,
  type OrderState,
  type Process,
  type Quote,
} from "../fabricate/types.js";
import { okPretty as ok, err, type ToolResult } from "./tool-result.js";

const QUOTE_TTL_MS = 24 * 60 * 60 * 1000; // 24h. Phase 1 shortens this for duty volatility.

// ── quote_manufacturing ────────────────────────────────────────────────────

export const quoteManufacturingSchema = {
  type: "object" as const,
  properties: {
    ir: {
      type: "object" as const,
      description:
        "Inline Document IR to quote directly — a STATELESS alternative to document_id. Needs no open_document, holds no session, and is immune to the serverless cold-start/instance issue, so it's the robust choice for one-shot or parallel quotes. Provide this OR document_id.",
    },
    document_id: {
      type: "string" as const,
      description: "Session id from open_document. Provide this OR `ir`.",
    },
    process: {
      type: "string" as const,
      enum: [...PROCESSES],
      description: "Manufacturing process: pcb | cnc | 3dprint | sheet_metal | cast_metal.",
    },
    quantity: { type: "integer" as const, minimum: 1, description: "Number of parts. Default 1." },
    material: { type: "string" as const, description: "Optional material (e.g. aluminum, stainless, resin)." },
    finish: { type: "string" as const, description: "Optional finish (e.g. anodized, sandblasted)." },
    ship_to: {
      type: "object" as const,
      description: "Optional ship-to address (stored on the order; not validated in Phase 0).",
    },
    layers: { type: "integer" as const, minimum: 1, description: "PCB only: copper layer count (default 2)." },
    board_area_mm2: {
      type: "number" as const,
      description: "PCB only: board area override when geometry can't recover it.",
    },
    fab_artifact_id: {
      type: "string" as const,
      description:
        "Optional artifact id (or artifact_url) of a fab bundle returned by " +
        "export_gerber / export_cad. Binds those files to the order WITHOUT " +
        "re-sending them through model context — the files stay in the artifact " +
        "store and are fetched at fab-submission time. The manifest's per-file " +
        "sha256 is recorded so the order is traceable to the exact bytes.",
    },
  },
  required: ["process", "quantity"],
};

/** Lightweight Phase-0 DFM. Deep checks (dfm_check / run_drc /
 *  sheet_metal_check) are wired into this path in Phase 1. */
function quoteDfm(process: Process, metrics: GeometryMetrics): DfmSummary {
  const violations: string[] = [];
  if (process !== "pcb" && !metrics.ok) {
    violations.push(
      "No solid geometry could be measured from the document — quote is a floor estimate only.",
    );
  }
  return { checked: true, passed: violations.length === 0, violations };
}

function docHash(ir: unknown): string {
  return createHash("sha256").update(JSON.stringify(ir)).digest("hex").slice(0, 16);
}

const toUsd = (minor: number): number => Math.round(minor) / 100;

export async function quoteManufacturing(
  input: unknown,
  engine: Engine,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const inlineIr =
    args.ir && typeof args.ir === "object" ? (args.ir as Document) : null;
  const process = String(args.process ?? "") as Process;
  const quantity = Math.max(1, Math.round(Number(args.quantity ?? 1)));

  if (!PROCESSES.includes(process)) {
    return err(`Unknown process "${process}". Use one of: ${PROCESSES.join(", ")}.`);
  }
  if (!inlineIr && !documentId) {
    return err(
      "Provide either `ir` (inline Document — stateless, serverless-safe) or `document_id` (an open session).",
    );
  }

  // Optional fab-bundle handle: bind the export to the order by reference, so
  // the files never re-enter model context. Resolved up front so a stale/expired
  // handle fails the quote cleanly rather than silently dropping the binding.
  const fabHandle = typeof args.fab_artifact_id === "string" ? args.fab_artifact_id : "";
  let fabArtifact: FabArtifactRef | null = null;
  if (fabHandle) {
    const ref = resolveArtifactRef(fabHandle);
    if (!ref) {
      return err(
        `Unknown or expired fab artifact "${fabHandle}". Re-run export_gerber / export_cad and pass the artifact_id it returns.`,
      );
    }
    fabArtifact = ref;
  }

  // Stateless when `ir` is supplied: no session lookup, so it's immune to the
  // serverless cold-start/cross-instance session loss and safe to call in
  // parallel. Otherwise resolve from the live session (throws if absent).
  const ir = inlineIr ?? getSession(documentId);
  const hash = docHash(ir);
  // Traceability ref for the persisted quote/order when there's no session id.
  const documentRef = documentId || `inline:${hash}`;
  const metrics = measureDocument(ir, engine);
  const dfm = quoteDfm(process, metrics);

  // Price via the SHARED kernel cost model (estimateCost / vcad-kernel-cost) —
  // the same estimator the app's Build → QuotePanel uses — so an agent's quote
  // agrees with the in-app quote. PCB has no kernel model (maps to null) and
  // keeps the JLCPCB area/layers estimate. Mirror the app's call (no qty) for
  // exact per-unit agreement, then scale to the requested quantity.
  const requestedMaterial = typeof args.material === "string" ? args.material : undefined;
  const dfmProcess = toDfmProcess(process);
  const materialCatalog = dfmProcess ? catalogMaterial(process, requestedMaterial) : undefined;
  let baseCostMinor: number | undefined;
  if (dfmProcess && materialCatalog && metrics.ok && metrics.volume_mm3 > 0) {
    try {
      const est = await estimateCost({
        process: dfmProcess,
        material: materialCatalog,
        partVolumeMm3: metrics.volume_mm3,
      });
      if (est && est.total_usd > 0) {
        baseCostMinor = Math.round(est.total_usd * 100) * quantity;
      }
    } catch {
      // estimateCost unavailable (e.g. unknown material) — adapters fall back
      // to their local coefficient estimate.
    }
  }

  const broker = new FulfillmentBroker();
  const result = await broker.quote({
    process,
    quantity,
    material: requestedMaterial,
    finish: typeof args.finish === "string" ? args.finish : undefined,
    metrics,
    layers: typeof args.layers === "number" ? args.layers : undefined,
    boardAreaMm2: typeof args.board_area_mm2 === "number" ? args.board_area_mm2 : undefined,
    baseCostMinor,
    materialCatalog,
  });

  if (!result.recommended) {
    return err(`No fab adapter could quote process "${process}".`);
  }

  const now = new Date();
  const expiresAt = new Date(now.getTime() + QUOTE_TTL_MS).toISOString();
  const quoteId = randomUUID();
  const orderId = randomUUID();
  const owner = ownerId(user);

  const quote: Quote = {
    quote_id: quoteId,
    document_id: documentRef,
    doc_hash: hash,
    process,
    material: typeof args.material === "string" ? args.material : null,
    quantity,
    dfm,
    fab_options: result.options,
    landed_cost: result.landed_cost,
    total_amount_minor: result.recommended.total_minor,
    currency: "USD",
    margin_hidden: true,
    expires_at: expiresAt,
    created_at: now.toISOString(),
  };

  const order: Order = {
    order_id: orderId,
    document_id: documentRef,
    quote_id: quoteId,
    state: "QUOTED",
    fab: result.recommended.fab,
    fab_order_ref: null,
    amount_total_minor: result.recommended.total_minor,
    currency: "USD",
    ship_to: (args.ship_to as unknown) ?? null,
    events: [{ state: "QUOTED", at: now.toISOString(), note: "quote_manufacturing" }],
    fab_artifact: fabArtifact,
    created_at: now.toISOString(),
    updated_at: now.toISOString(),
  };

  // Best-effort persistence — a store outage must never fail a quote.
  try {
    await store.saveQuote(quote, { fab_cost_minor: result.fab_cost_minor, margin_minor: result.margin_minor }, owner);
    await store.saveOrder(order, result.fab_cost_minor, owner);
  } catch {
    /* in-memory never throws; cloud logs internally */
  }

  // Interim rail for processes with no signed fab partner: hand the agent a
  // structured path to finish the order on a fab's own instant-quote site.
  const fabHandoff = buildFabHandoff(process, { hasArtifact: fabArtifact != null });
  if (fabHandoff) {
    // Aggregate BD telemetry — how much order-ready demand the handoff rail
    // generates (counts + totals only; no argument values or IR).
    captureEvent("fab_handoff_generated", {
      process,
      quantity,
      material: quote.material ?? "unspecified",
      total_usd: toUsd(quote.total_amount_minor),
      has_fab_artifact: fabArtifact != null,
      dfm_passed: dfm.passed,
    });
  }

  // Agent-facing payload: margin-inclusive prices only; fab cost / margin never appear.
  return ok({
    quote_id: quoteId,
    order_id: orderId,
    process,
    quantity,
    material: quote.material,
    material_catalog: materialCatalog ?? null,
    cost_model:
      baseCostMinor != null
        ? "vcad kernel cost model (consistent with the in-app Build quote)"
        : "adapter-local estimate",
    geometry: metrics.ok
      ? {
          parts: metrics.parts,
          volume_mm3: Math.round(metrics.volume_mm3),
          max_dim_mm: Math.round(metrics.max_dim_mm),
          footprint_mm2: Math.round(metrics.footprint_mm2),
        }
      : { measured: false },
    dfm,
    fab_options: result.options.map((o) => ({
      fab: o.fab,
      fab_label: o.fab_label,
      region: o.region,
      unit_price_usd: toUsd(o.unit_price_minor),
      total_usd: toUsd(o.total_minor),
      lead_time_days: o.lead_time_days,
      in_spec: o.in_spec,
      pricing_basis: o.pricing_basis,
      orderable: o.orderable,
      notes: o.notes,
    })),
    recommended_fab: result.recommended.fab,
    total_amount_usd: toUsd(quote.total_amount_minor),
    total_amount_minor: quote.total_amount_minor,
    currency: "USD",
    landed_cost: {
      shipping_usd: toUsd(result.landed_cost.shipping_minor),
      duty_usd: toUsd(result.landed_cost.duty_minor),
      basis: result.landed_cost.basis,
    },
    margin_hidden: true,
    orderable: result.recommended.orderable,
    ...(fabHandoff ? { fab_handoff: fabHandoff } : {}),
    expires_at: expiresAt,
    fab_artifact: fabArtifact
      ? {
          artifact_id: fabArtifact.artifact_id,
          artifact_url: fabArtifact.artifact_url,
          bytes: fabArtifact.bytes,
          files: fabArtifact.manifest.length,
        }
      : null,
    note:
      "Phase 0: quote-only. Prices are local ESTIMATES (no binding fab quote yet) and ordering/payment ships in Phase 1. " +
      "An order row was created at state QUOTED — see it with get_order_status / list_orders." +
      (fabArtifact
        ? " Fab files are bound by reference (artifact_id) — they stay in the artifact store and never transit model context."
        : ""),
  });
}

// ── get_order_status ─────────────────────────────────────────────────────────

export const getOrderStatusSchema = {
  type: "object" as const,
  properties: {
    order_id: { type: "string" as const, description: "Order id from quote_manufacturing." },
  },
  required: ["order_id"],
};

export async function getOrderStatus(
  input: unknown,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const orderId = String(args.order_id ?? "");
  if (!orderId) return err("order_id is required.");

  const order = await store.getOrder(orderId, ownerId(user));
  if (!order) return err(`Unknown order_id: ${orderId}`);

  return ok({
    order_id: order.order_id,
    document_id: order.document_id,
    quote_id: order.quote_id,
    state: order.state,
    fab: order.fab,
    fab_order_ref: order.fab_order_ref,
    total_amount_usd: toUsd(order.amount_total_minor),
    currency: order.currency,
    tracking: null,
    fab_artifact: order.fab_artifact ?? null,
    events: order.events,
  });
}

// ── list_orders ───────────────────────────────────────────────────────────────

export const listOrdersSchema = {
  type: "object" as const,
  properties: {
    status: { type: "string" as const, description: "Optional order state filter (e.g. QUOTED)." },
    limit: { type: "integer" as const, minimum: 1, maximum: 200, description: "Max rows (default 50)." },
  },
  required: [],
};

export async function listOrders(
  input: unknown,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const status = typeof args.status === "string" ? (args.status as OrderState) : undefined;
  const limit = typeof args.limit === "number" ? args.limit : undefined;

  const orders = await store.listOrders(ownerId(user), { status, limit });
  return ok({
    count: orders.length,
    orders: orders.map((o) => ({
      order_id: o.order_id,
      document_id: o.document_id,
      state: o.state,
      fab: o.fab,
      total_amount_usd: toUsd(o.amount_total_minor),
      currency: o.currency,
      created_at: o.created_at,
    })),
  });
}

export const toolDefs: ToolDef[] = [
  {
    name: "quote_manufacturing",
    pack: "fabricate",
    description:
      "Quote manufacturing a part: measures the design, runs light DFM, and returns margin-inclusive price options per fab (pcb/cnc/3dprint/sheet_metal/cast_metal). Pass `ir` (inline Document — stateless, no open_document needed, serverless-safe, parallel-safe) OR a `document_id` from an open session. Persists a quote + a QUOTED order. Phase 0 is quote-only — prices are estimates and ordering/payment ship next; no money moves. For sheet_metal the result includes `fab_handoff`: curated US instant-quote shops (SendCutSend/OSH Cut/Fabworks), the exact file recipe (DXF via sheet_metal_unfold or folded STEP via export_cad), and what to enter at upload — everything needed to finish the order on the fab's site today.",
    inputSchema: quoteManufacturingSchema,
    handler: (a, c) => quoteManufacturing(a, c.engine, c.fabricateStore, c.user),
    behavior: behavior({}),
  },
  {
    name: "get_order_status",
    pack: "fabricate",
    description:
      "Return the lifecycle row for a Fabricate order (state, fab, totals, event timeline). Read-only.",
    inputSchema: getOrderStatusSchema,
    handler: (a, c) => getOrderStatus(a, c.fabricateStore, c.user),
    behavior: behavior({}),
  },
  {
    name: "list_orders",
    pack: "fabricate",
    description:
      "List the caller's Fabricate orders, newest first. Optional status filter and limit. Read-only.",
    inputSchema: listOrdersSchema,
    handler: (a, c) => listOrders(a, c.fabricateStore, c.user),
    behavior: behavior({}),
  },
];
