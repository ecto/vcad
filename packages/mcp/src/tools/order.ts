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
import { buildKerfSheetMetalIntent, KERF_VENDOR } from "../fabricate/adapters/kerf.js";
import { intentHash } from "../fabricate/kerf/intent-hash.js";
import type { ConfiguratorIntent, FileRef } from "../fabricate/kerf/contract.js";
import { captureEvent } from "../telemetry.js";
import { getArtifactFile, resolveArtifactRef } from "./artifact-store.js";
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

/** sha256(JSON)[0..16] — the design fingerprint persisted on quotes and
 *  re-checked by place_order's geometry gate (exported so the gate hashes
 *  with the exact same function, never a near-copy). */
export function docHash(ir: unknown): string {
  return createHash("sha256").update(JSON.stringify(ir)).digest("hex").slice(0, 16);
}

/** Pull material + thickness (mm) off the document's sheet-metal base flange,
 *  mirroring what sheet_metal_create writes (tools/sheet-metal.ts). Null when
 *  the document has no sheet-metal chain. */
function sheetMetalParams(ir: Document): { material: string; thicknessMm: number } | null {
  for (const node of Object.values(ir.nodes ?? {})) {
    const op = node.op;
    if (
      op.type === "SheetMetalBaseFlangeRect" ||
      op.type === "SheetMetalBaseFlangePolygon"
    ) {
      if (op.thickness > 0) return { material: op.material, thicknessMm: op.thickness };
    }
  }
  return null;
}

/**
 * vcad/registry material names → SendCutSend's vendor-native alloy labels
 * (the exact link text the SCS configurator's alloy step shows — recorded
 * 2026-07-07 in kerf's sendcutsend manifest: 2024 T3, 5052 H32, 6061 T6,
 * 7075 T6, MIC-6). Only aluminum maps today; anything unmapped fails closed
 * (no kerfIntent) rather than guessing a label the playbook can't select.
 */
const SCS_ALLOY_LABELS: Record<string, string> = {
  // Soft/bendable aluminum → 5052 H32 (SCS's default bendable sheet alloy).
  "5052": "5052 H32",
  "5052 h32": "5052 H32",
  "5052-h32": "5052 H32",
  "al-soft": "5052 H32",
  aluminum: "5052 H32",
  aluminium: "5052 H32",
  al: "5052 H32",
  // Hard aluminum → 6061 T6 (flat-only at SCS — no bends).
  "6061": "6061 T6",
  "6061 t6": "6061 T6",
  "6061-t6": "6061 T6",
  "al-hard": "6061 T6",
  "2024": "2024 T3",
  "2024 t3": "2024 T3",
  "2024-t3": "2024 T3",
  "7075": "7075 T6",
  "7075 t6": "7075 T6",
  "7075-t6": "7075 T6",
};

/**
 * SendCutSend aluminum thickness → the (radio value code, display label)
 * pair the kerf SCS quote playbook dereferences: `/config/thickness` must
 * equal the checked radio's stable VALUE code ("ALU-125") and
 * `/config/thickness_label` is the visible tile label the select step
 * clicks (`.125" (3.2 MM)`). Derivation is confident only for inch-native
 * mils in the recorded ALU-040..ALU-500 range (label format verified
 * against kerf's recorded pairs: ALU-040 '.040" (1.0 MM)', ALU-125
 * '.125" (3.2 MM)', ALU-250 '.250" (6.3 MM)'); anything else returns null
 * and the whole kerfIntent is omitted — fail-closed, never a silently
 * mis-selected thickness.
 */
function scsAluminumThickness(
  thicknessMm: number,
): { code: string; label: string } | null {
  const mils = (thicknessMm / 25.4) * 1000;
  const rounded = Math.round(mils);
  if (Math.abs(mils - rounded) > 0.25) return null;
  if (rounded < 40 || rounded > 500) return null; // recorded SCS ALU range
  const mmLabel = (Math.round(thicknessMm * 10) / 10).toFixed(1);
  return {
    code: `ALU-${rounded}`,
    label: `.${String(rounded).padStart(3, "0")}" (${mmLabel} MM)`,
  };
}

/**
 * Full vendor-native SendCutSend sheet-metal config — EXACTLY the pointers
 * kerf's SCS quote playbook dereferences (playbooks/quote.json):
 * `/config/units`, `/config/material_category`, `/config/material_family`,
 * `/config/material`, `/config/thickness`, `/config/thickness_label` (the
 * playbook's resolveValueRef THROWS on a missing pointer, so a partial
 * config kills the run). Returns null when any value can't be derived
 * vendor-natively — the caller then omits the kerfIntent entirely.
 */
export function scsSheetConfig(
  material: string,
  thicknessMm: number,
): Record<string, string | number | boolean> | null {
  const alloy = SCS_ALLOY_LABELS[material.trim().toLowerCase()];
  if (!alloy) return null;
  const th = scsAluminumThickness(thicknessMm);
  if (!th) return null;
  return {
    units: "MM", // vcad documents are always millimeters
    material_category: "Metals",
    material_family: "Aluminum",
    material: alloy,
    thickness: th.code,
    thickness_label: th.label,
  };
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
  let baseCostModel: "kernel" | "sheet_metal_laser" | undefined;

  // Sheet metal with a sheet-metal chain in the document: price via the SAME
  // line-itemed laser model sheet_metal_cost uses (material + cut + pierce +
  // bends + setup amortized over qty), so the two tools can never drift apart.
  // The fab cost is the PRE-markup subtotal: the laser model's markup_pct and
  // the broker's MARGIN_RATE describe the same economic layer, and stacking
  // both would double-margin the quote. Expected relation to sheet_metal_cost:
  //   quote_unit ≈ total_each × (1 + MARGIN_RATE) / (1 + markup_pct/100)
  //              + shipping / qty
  // (see the consistency test in __tests__/fabricate.test.ts).
  if (process === "sheet_metal") {
    try {
      const sm = engine.costSheetMetal(ir, undefined, quantity);
      if (sm) {
        baseCostMinor = Math.round(sm.breakdown.subtotal_each * quantity * 100);
        baseCostModel = "sheet_metal_laser";
      }
    } catch {
      // No usable sheet chain — fall through to the volume-based estimate.
    }
  }

  if (baseCostMinor == null && dfmProcess && materialCatalog && metrics.ok && metrics.volume_mm3 > 0) {
    try {
      const est = await estimateCost({
        process: dfmProcess,
        material: materialCatalog,
        partVolumeMm3: metrics.volume_mm3,
        // Sheet metal: the WASM binding reinterprets stockVolumeMm3 as the
        // blank AREA (mm²); pass the real footprint so blank area / thickness
        // are true instead of the (2 × volume, 0.5 mm) fallback.
        ...(process === "sheet_metal" && metrics.footprint_mm2 > 0
          ? { stockVolumeMm3: metrics.footprint_mm2 }
          : {}),
      });
      if (est && est.total_usd > 0) {
        if (process === "sheet_metal") {
          // Setup is one-time per run — amortize it instead of paying it on
          // every unit (the dominant term for small flat parts).
          const perUnitUsd = Math.max(0, est.total_usd - est.setup_cost_usd);
          baseCostMinor =
            Math.round(perUnitUsd * 100) * quantity +
            Math.round(est.setup_cost_usd * 100);
        } else {
          baseCostMinor = Math.round(est.total_usd * 100) * quantity;
        }
        baseCostModel = "kernel";
      }
    } catch {
      // estimateCost unavailable (e.g. unknown material) — adapters fall back
      // to their local coefficient estimate.
    }
  }

  // ── kerf rail (Wave 0, sheet metal): with a fab bundle bound, build the
  // ConfiguratorIntent HERE — order.ts owns intent construction so the
  // persisted kerf_intent_hash is computed over the exact object the adapter
  // sends — and thread it through the broker to the SendCutSend-via-kerf
  // adapter. kerf's posted-intent API requires the actual DXF bytes inline
  // (`bytes_base64` per file, hash-checked at the door), so the single DXF's
  // bytes are read from the artifact store, re-hashed against the manifest
  // sha256, and attached as a WIRE-ONLY field — the intent hash stays over
  // sha256s alone (see intent-hash.ts). Every underivable input fails closed:
  // no intent, a note, and the generic estimator covers.
  let kerfIntent: ConfiguratorIntent | null = null;
  let kerfSkipNote: string | null = null;
  if (process === "sheet_metal" && fabArtifact) {
    const dxfEntries = fabArtifact.manifest.filter((m) =>
      m.file.toLowerCase().endsWith(".dxf"),
    );
    const sheet = sheetMetalParams(ir);
    if (dxfEntries.length === 0) {
      kerfSkipNote =
        "kerf vendor quote skipped: the bound fab artifact has no .dxf files (export the flat pattern via sheet_metal_unfold).";
    } else if (dxfEntries.length > 1) {
      // The SCS playbook uploads only /files/0 — pricing one file of a
      // multi-part bundle and presenting it as the whole order would
      // misprice, so multi-DXF intents are refused outright.
      kerfSkipNote =
        "kerf vendor quote skipped: multi-DXF orders not yet kerf-quotable (the vendor playbook uploads a single file; quoting only the first DXF would misprice the bundle).";
    } else if (!sheet) {
      kerfSkipNote =
        "kerf vendor quote skipped: no sheet-metal base flange in the document to derive material/thickness config from.";
    } else {
      const material = requestedMaterial ?? sheet.material ?? "5052";
      const config = scsSheetConfig(material, sheet.thicknessMm);
      if (!config) {
        kerfSkipNote =
          `kerf vendor quote skipped: no vendor-native SendCutSend config for material "${material}" at ${sheet.thicknessMm} mm — ` +
          "fail-closed rather than guessing a configurator selection (aluminum at inch-native gauges derives today).";
      } else {
        const entry = dxfEntries[0];
        const stored = getArtifactFile(fabArtifact.artifact_id, entry.file);
        const actualSha = stored
          ? createHash("sha256").update(stored.buf).digest("hex")
          : null;
        if (!stored || actualSha !== entry.sha256) {
          // Never send bytes that don't hash to the manifest's pin — kerf's
          // upload-hash oracle would (rightly) refuse, and a mismatch here
          // means the artifact store no longer holds what was quoted.
          console.error(
            `[quote_manufacturing] kerf intent skipped: artifact ${fabArtifact.artifact_id} file "${entry.file}" ` +
              (stored
                ? `sha256 mismatch (manifest ${entry.sha256}, bytes ${actualSha})`
                : "bytes unavailable in the artifact store"),
          );
          kerfSkipNote =
            "kerf vendor quote skipped: the fab artifact's DXF bytes are unavailable or do not match the manifest sha256 — re-run the export and re-quote.";
        } else {
          const files: FileRef[] = [
            {
              name: entry.file,
              bytes: entry.bytes,
              sha256: entry.sha256,
              media_type: "image/vnd.dxf",
              // Wire-only: kerf strips this at the door after hash-checking;
              // it never participates in intentHash.
              bytes_base64: stored.buf.toString("base64"),
            },
          ];
          kerfIntent = buildKerfSheetMetalIntent({ files, config, quantity });
        }
      }
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
    baseCostModel,
    materialCatalog,
    ...(kerfIntent ? { kerfIntent: { intent: kerfIntent } } : {}),
  });

  if (!result.recommended) {
    return err(`No fab adapter could quote process "${process}".`);
  }

  // kerf provenance: when the recommendation came from the kerf rail, bind
  // the quote to its intent hash (kerf discipline: geometry/config/quantity
  // edit ⇒ new hash ⇒ the vendor quote is dead) and record the quote-job id
  // for job-state/evidence lookups.
  let kerfIntentHash: string | null = null;
  let kerfJobId: string | null = null;
  if (kerfIntent && result.recommended.fab === KERF_VENDOR) {
    kerfIntentHash = intentHash(kerfIntent);
    const jobNote = result.recommended.notes.find((n) => n.startsWith("kerf job "));
    kerfJobId = jobNote ? jobNote.slice("kerf job ".length) : null;
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
    kerf_intent_hash: kerfIntentHash,
    kerf_job_id: kerfJobId,
    pricing_basis_best: result.recommended.pricing_basis,
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
    authorization_id: null,
    receipt_status: null,
    kerf_intent_hash: kerfIntentHash,
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
  const quoteResult = ok({
    quote_id: quoteId,
    order_id: orderId,
    process,
    quantity,
    material: quote.material,
    material_catalog: materialCatalog ?? null,
    cost_model:
      baseCostModel === "sheet_metal_laser"
        ? "sheet-metal laser cost model (same line items as sheet_metal_cost; vcad margin applied on top)"
        : baseCostMinor != null
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
    // Pricing basis of the recommended option — agents branch on this:
    // "estimate" never gates money; "quoted" is the fab's own displayed price
    // (kerf rail); "binding" is fab-committed.
    pricing_basis: result.recommended.pricing_basis,
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
    ...(kerfIntentHash
      ? { kerf_intent_hash: kerfIntentHash, ...(kerfJobId ? { kerf_job_id: kerfJobId } : {}) }
      : {}),
    note:
      (result.recommended.pricing_basis === "quoted"
        ? "The recommended price is the fab's OWN displayed quote (via the kerf rail), bound to kerf_intent_hash — any geometry/config/quantity edit kills it (re-quote). Ordering/payment still ships separately. "
        : "Phase 0: quote-only. Prices are local ESTIMATES (no binding fab quote yet) and ordering/payment ships in Phase 1. ") +
      "An order row was created at state QUOTED — see it with get_order_status / list_orders." +
      (fabArtifact
        ? " Fab files are bound by reference (artifact_id) — they stay in the artifact store and never transit model context."
        : "") +
      (kerfSkipNote ? ` ${kerfSkipNote}` : ""),
  });
  // quote_manufacturing mounts the viewer (behavior.mount) but isn't a
  // geometry tool, so the dispatch never attaches a preview handle — surface
  // the session id in structuredContent so the mounted order dock knows which
  // document to render and poll. Inline-IR quotes have no live session and
  // stay handle-less (the dock can't bind a session that doesn't exist).
  if (documentId) {
    quoteResult.structuredContent = { document_id: documentId };
  }
  return quoteResult;
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
      "Quote manufacturing a part: measures the design, runs light DFM, and returns margin-inclusive price options per fab (pcb/cnc/3dprint/sheet_metal/cast_metal). Pass `ir` (inline Document — stateless, no open_document needed, serverless-safe, parallel-safe) OR a `document_id` from an open session. Persists a quote + a QUOTED order. Phase 0 is quote-only — prices are estimates and ordering/payment ship next; no money moves. For sheet_metal the result includes `fab_handoff`: curated US instant-quote shops (SendCutSend/OSH Cut/Fabworks), the exact file recipe (DXF via sheet_metal_unfold or folded STEP via export_cad), and what to enter at upload — everything needed to finish the order on the fab's site today. Mounts the inline viewer's order dock, so the quote and its order lifecycle render alongside the model.",
    inputSchema: quoteManufacturingSchema,
    handler: (a, c) => quoteManufacturing(a, c.engine, c.fabricateStore, c.user),
    behavior: behavior({ mount: true }),
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
