import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { openDocument, documents } from "../tools/session.js";
import {
  quoteManufacturing,
  getOrderStatus,
  listOrders,
} from "../tools/order.js";
import { buildFabHandoff } from "../fabricate/handoff.js";
import { telemetryConfig, flushTelemetry } from "../telemetry.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import { storeArtifact, clearArtifacts } from "../tools/artifact-store.js";
import { FulfillmentBroker } from "../fabricate/broker.js";
import { applyMargin, MARGIN_RATE, estimateLandedCost } from "../fabricate/pricing.js";
import { digitalMetalAdapter } from "../fabricate/adapters/digitalmetal.js";
import { catalogMaterial } from "../fabricate/process-map.js";
import { sheetMetalCreate, sheetMetalCost } from "../tools/sheet-metal.js";
import type { GeometryMetrics } from "../fabricate/types.js";

function metrics(over: Partial<GeometryMetrics> = {}): GeometryMetrics {
  return {
    ok: true,
    parts: 1,
    volume_mm3: 1000,
    surface_area_mm2: 600,
    footprint_mm2: 100,
    max_dim_mm: 10,
    bbox: { min: [0, 0, 0], max: [10, 10, 10] },
    ...over,
  };
}

function cubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": { id: 1, name: "blank", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  } as unknown as Document;
}

describe("fabricate pricing + broker (no engine)", () => {
  it("applies the cost-plus margin", () => {
    expect(applyMargin(1000)).toBe(Math.round(1000 * (1 + MARGIN_RATE)));
    expect(applyMargin(1000)).toBe(1250);
  });

  it("quotes cast_metal, keeps fab cost server-only, and is not orderable in Phase 0", async () => {
    const broker = new FulfillmentBroker();
    const r = await broker.quote({ process: "cast_metal", quantity: 2, metrics: metrics() });

    expect(r.recommended).not.toBeNull();
    expect(r.options.length).toBeGreaterThan(0);
    // Server-only economics are returned to the BROKER caller (the tool), but
    // every customer-facing option is margin-inclusive and not orderable yet.
    expect(r.fab_cost_minor).toBeGreaterThan(0);
    expect(r.margin_minor).toBeGreaterThan(0);
    for (const o of r.options) {
      expect(o.total_minor).toBeGreaterThan(0);
      expect(o.pricing_basis).toBe("estimate");
      expect(o.orderable).toBe(false); // Phase 0: estimate, never orderable
    }
    // Marked-up total is strictly above the raw fab cost.
    expect(r.recommended!.total_minor).toBeGreaterThan(r.fab_cost_minor);
  });

  it("flags an oversize cast_metal part as out of spec", async () => {
    const q = await digitalMetalAdapter.quote({
      process: "cast_metal",
      quantity: 1,
      metrics: metrics({ max_dim_mm: 400 }), // > 350 mm envelope
    });
    expect(q).not.toBeNull();
    expect(q!.in_spec).toBe(false);
  });

  it("PCB quote uses board area + layers and stays in spec", async () => {
    const broker = new FulfillmentBroker();
    const r = await broker.quote({
      process: "pcb",
      quantity: 5,
      metrics: metrics({ ok: false, parts: 0, footprint_mm2: 0, max_dim_mm: 0, bbox: null }),
      boardAreaMm2: 2500, // 50x50mm
      layers: 4,
    });
    const jlc = r.options.find((o) => o.fab === "jlcpcb");
    expect(jlc).toBeDefined();
    expect(jlc!.in_spec).toBe(true);
    expect(jlc!.total_minor).toBeGreaterThan(0);
  });

  it("CNC has no contracted fab → only a non-orderable estimate", async () => {
    const broker = new FulfillmentBroker();
    const r = await broker.quote({ process: "cnc", quantity: 1, metrics: metrics() });
    expect(r.options.length).toBeGreaterThan(0);
    expect(r.options.every((o) => o.orderable === false)).toBe(true);
    expect(r.options.some((o) => o.fab === "vcad_estimate")).toBe(true);
  });

  it("uses the shared kernel estimate when provided, so MCP agrees with the app", async () => {
    const broker = new FulfillmentBroker();
    const base = 10000; // $100 total fab cost from the shared estimator
    const r = await broker.quote({
      process: "cast_metal",
      quantity: 1,
      metrics: metrics(),
      baseCostMinor: base,
    });
    expect(r.recommended).not.toBeNull();
    // The adapter used the shared estimate, NOT its local coefficients.
    expect(r.fab_cost_minor).toBe(base);
    // Customer total = marked-up base + landed (Digital Metal = US, $8, no duty).
    expect(r.recommended!.fab).toBe("digitalmetal");
    expect(r.recommended!.total_minor).toBe(applyMargin(base) + 800);
  });

  it("drops the generic estimator when a contracted fab serves the process", async () => {
    const broker = new FulfillmentBroker();

    // PCB: JLCPCB is contracted → the non-orderable generic must not appear or
    // out-rank it (regression: it used to undercut JLCPCB on a 4-layer board).
    const pcb = await broker.quote({
      process: "pcb",
      quantity: 10,
      metrics: metrics({ ok: false, parts: 0, footprint_mm2: 0, max_dim_mm: 0, bbox: null }),
      boardAreaMm2: 10000,
      layers: 4,
    });
    expect(pcb.options.some((o) => o.fab === "vcad_estimate")).toBe(false);
    expect(pcb.recommended?.fab).toBe("jlcpcb");

    // cast_metal: Digital Metal contracted → generic dropped.
    const cast = await broker.quote({
      process: "cast_metal",
      quantity: 1,
      metrics: metrics(),
      baseCostMinor: 5000,
    });
    expect(cast.options.every((o) => o.fab !== "vcad_estimate")).toBe(true);

    // CNC: no contracted fab → the generic estimator remains (as the fallback).
    const cnc = await broker.quote({ process: "cnc", quantity: 1, metrics: metrics() });
    expect(cnc.options.some((o) => o.fab === "vcad_estimate")).toBe(true);
  });
});

describe("fabricate quote loop (engine)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });
  beforeEach(() => {
    documents.clear();
    clearArtifacts();
  });

  it("binds a fab artifact handle to the quote/order without re-sending bytes", async () => {
    const handle = storeArtifact([
      { name: "top.gbr", content: "G04 top secret copper*" },
      { name: "out.drl", content: "M48\n" },
    ]);
    const store = new InMemoryFabricateStore();
    const res = await quoteManufacturing(
      {
        ir: cubeDoc(),
        process: "cnc",
        quantity: 5,
        material: "aluminum",
        fab_artifact_id: handle.artifact_id,
      },
      engine,
      store,
      null,
    );
    expect(res.isError).toBeFalsy();
    const quote = JSON.parse(res.content[0].text);
    expect(quote.fab_artifact.artifact_id).toBe(handle.artifact_id);
    expect(quote.fab_artifact.files).toBe(2);
    // The fab bytes never enter the tool result.
    expect(res.content[0].text).not.toContain("G04 top secret");

    // The binding is persisted on the order and visible via get_order_status.
    const statusRes = await getOrderStatus({ order_id: quote.order_id }, store, null);
    const status = JSON.parse(statusRes.content[0].text);
    expect(status.fab_artifact.artifact_id).toBe(handle.artifact_id);
    expect(status.fab_artifact.manifest).toHaveLength(2);
  });

  it("rejects an unknown fab artifact handle", async () => {
    const res = await quoteManufacturing(
      { ir: cubeDoc(), process: "cnc", quantity: 1, fab_artifact_id: "art_does_not_exist" },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("Unknown or expired fab artifact");
  });

  it("quotes a real cube, hides fab cost, persists a QUOTED order, and reads it back", async () => {
    const open = openDocument({ initial: cubeDoc() });
    const { document_id } = JSON.parse(open.content[0].text);

    const store = new InMemoryFabricateStore();
    const quoteRes = await quoteManufacturing(
      { document_id, process: "cast_metal", quantity: 2, material: "stainless" },
      engine,
      store,
      null,
    );
    expect(quoteRes.isError).toBeFalsy();
    const quote = JSON.parse(quoteRes.content[0].text);

    // Measured real geometry off the cube.
    expect(quote.geometry.parts).toBe(1);
    expect(quote.geometry.volume_mm3).toBeGreaterThan(0);
    expect(quote.quote_id).toBeTruthy();
    expect(quote.order_id).toBeTruthy();
    expect(quote.total_amount_usd).toBeGreaterThan(0);
    expect(quote.fab_options.length).toBeGreaterThan(0);
    expect(quote.margin_hidden).toBe(true);

    // Consistency: a volume-based process is priced by the SAME kernel cost
    // model the app's Build quote uses (not ad-hoc coefficients).
    expect(quote.material_catalog).toBeTruthy();
    expect(quote.cost_model).toContain("kernel");

    // The marked-up cost / margin must NEVER leak into the agent-facing result.
    expect(quoteRes.content[0].text).not.toContain("fab_cost");
    expect(quoteRes.content[0].text).not.toContain("margin_minor");

    // The order was persisted at QUOTED and is readable + listable.
    const statusRes = await getOrderStatus({ order_id: quote.order_id }, store, null);
    const status = JSON.parse(statusRes.content[0].text);
    expect(status.state).toBe("QUOTED");
    expect(status.events[0].state).toBe("QUOTED");
    expect(status.tracking).toBeNull();

    const listRes = await listOrders({ status: "QUOTED" }, store, null);
    const list = JSON.parse(listRes.content[0].text);
    expect(list.orders.some((o: { order_id: string }) => o.order_id === quote.order_id)).toBe(true);
  });

  it("rejects an unknown process", async () => {
    const open = openDocument({ initial: cubeDoc() });
    const { document_id } = JSON.parse(open.content[0].text);
    const res = await quoteManufacturing(
      { document_id, process: "frobnicate", quantity: 1 },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBe(true);
  });

  it("quotes from inline IR with no open_document (stateless, serverless-safe)", async () => {
    // The serverless fix: no session is created, so this can't hit the
    // cross-instance "Unknown document_id" failure and is safe to parallelize.
    const res = await quoteManufacturing(
      { ir: cubeDoc(), process: "cnc", quantity: 1, material: "aluminum" },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBeFalsy();
    const quote = JSON.parse(res.content[0].text);
    expect(quote.geometry.parts).toBe(1);
    expect(quote.total_amount_usd).toBeGreaterThan(0);
    expect(quote.cost_model).toContain("kernel");
    expect(quote.quote_id).toBeTruthy();
    expect(quote.order_id).toBeTruthy();
  });

  it("requires either ir or document_id", async () => {
    const res = await quoteManufacturing(
      { process: "cnc", quantity: 1 },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBe(true);
  });
});

describe("sheet_metal quote ↔ sheet_metal_cost consistency (engine)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });
  beforeEach(() => {
    documents.clear();
  });

  function circle(cx: number, cy: number, r: number, n: number) {
    return Array.from({ length: n }, (_, i) => {
      const a = (2 * Math.PI * i) / n;
      return { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
    });
  }

  /** The reference part: a Ø58 × 2.7 mm mild-steel disc with 5 Ø5 holes —
   *  the shape SendCutSend prices at ~$12–18/ea at qty 2. */
  function discSession(): string {
    const created = JSON.parse(
      sheetMetalCreate(
        {
          outline: circle(0, 0, 29, 64),
          // Hole loops are CW (outline is CCW) per the create schema.
          holes: [0, 1, 2, 3, 4].map((i) =>
            circle(
              20 * Math.cos((2 * Math.PI * i) / 5),
              20 * Math.sin((2 * Math.PI * i) / 5),
              2.5,
              16,
            ).reverse(),
          ),
          thickness: 2.7,
          material: "steel-mild",
        },
        engine,
      ).content[0].text,
    );
    return created.document_id as string;
  }

  it("aliases sheet-metal material names onto the kernel cost catalog", () => {
    // Regression: "mild steel" used to silently fall back to Aluminum 6061.
    expect(catalogMaterial("sheet_metal", "mild steel")).toBe("Steel 1018");
    expect(catalogMaterial("sheet_metal", "steel-mild")).toBe("Steel 1018");
    expect(catalogMaterial("sheet_metal", "al-soft")).toBe("Aluminum 6061");
  });

  it("quotes a flat disc within 25% of sheet_metal_cost's total_each", async () => {
    const qty = 2;
    const documentId = discSession();

    const smc = JSON.parse(
      sheetMetalCost({ document_id: documentId, quantity: qty }, engine).content[0].text,
    );
    const totalEach: number = smc.breakdown.total_each;
    expect(totalEach).toBeGreaterThan(5); // sanity: a real laser-model price

    const res = await quoteManufacturing(
      { document_id: documentId, process: "sheet_metal", quantity: qty, material: "mild steel" },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBeFalsy();
    const quote = JSON.parse(res.content[0].text);

    // Same costing code path, surfaced in the result.
    expect(quote.cost_model).toContain("sheet_metal_cost");

    // THE consistency gate: quote_manufacturing is margin-inclusive (25%
    // MARGIN_RATE + domestic shipping folded into the landed unit price), so
    // it sits ABOVE sheet_metal_cost's shop price — but never 3x it again.
    // Both models price from the same pre-markup subtotal, so the unit price
    // stays within ~25% of sheet_metal_cost's total_each.
    const unit: number = quote.total_amount_usd / qty;
    expect(unit).toBeGreaterThan(totalEach * 0.75);
    expect(unit).toBeLessThan(totalEach * 1.25);

    // Margin handling stays EXPLICIT — the delta decomposes exactly into the
    // broker margin on the pre-markup subtotal plus flat domestic shipping:
    //   total = round(subtotal_each × qty × 100) × (1 + MARGIN_RATE) + shipping
    const shipping = estimateLandedCost({ region: "n/a", supportsDdp: false });
    expect(shipping.basis).toBe("domestic_estimate");
    const expectedMinor =
      applyMargin(Math.round(smc.breakdown.subtotal_each * qty * 100)) +
      shipping.shipping_minor;
    expect(quote.total_amount_minor).toBe(expectedMinor);
  });

  it("amortizes setup in the fallback path for flat solids quoted as sheet_metal", async () => {
    // A plain solid disc (no sheet-metal chain) exercises the estimateCost
    // fallback. Setup is one-time per run: per-unit price must drop sharply
    // with quantity (it used to be flat — setup was charged on every part).
    const solidDisc = {
      version: "0.1",
      nodes: {
        "1": {
          id: 1,
          name: "disc",
          op: { type: "Cylinder", radius: 29, height: 2.7, segments: 0 },
        },
      },
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
    } as unknown as Document;

    const store = new InMemoryFabricateStore();
    const at = async (qty: number) => {
      const res = await quoteManufacturing(
        { ir: solidDisc, process: "sheet_metal", quantity: qty, material: "mild steel" },
        engine,
        store,
        null,
      );
      expect(res.isError).toBeFalsy();
      const quote = JSON.parse(res.content[0].text);
      expect(quote.cost_model).toContain("kernel");
      expect(quote.material_catalog).toBe("Steel 1018");
      return quote.total_amount_usd / qty;
    };

    const unit1 = await at(1);
    const unit10 = await at(10);
    expect(unit10).toBeLessThan(unit1 / 2);
  });
});

describe("fab handoff (sheet metal interim rail)", () => {
  it("builds a sheet-metal handoff with curated shops and no ordering claim", () => {
    const h = buildFabHandoff("sheet_metal", { hasArtifact: false });
    expect(h).not.toBeNull();
    expect(h!.orderable_via_vcad).toBe(false);
    const ids = h!.shops.map((s) => s.id);
    expect(ids).toEqual(["sendcutsend", "oshcut", "fabworks"]);
    // SendCutSend is the only shop whose tooling the kernel encodes today.
    expect(h!.shops.find((s) => s.id === "sendcutsend")!.shop_profile).toBe("sendcutsend");
    expect(h!.shops.find((s) => s.id === "oshcut")!.shop_profile).toBeNull();
    // Without a bound artifact the recipe explains how to produce the files.
    expect(h!.file_recipe.join(" ")).toContain("sheet_metal_unfold");
  });

  it("shortens the recipe when fab files are already bound", () => {
    const h = buildFabHandoff("sheet_metal", { hasArtifact: true });
    expect(h!.file_recipe).toHaveLength(1);
    expect(h!.file_recipe[0]).toContain("already bound");
  });

  it("returns null for processes with a real or absent ordering path", () => {
    expect(buildFabHandoff("cnc", { hasArtifact: false })).toBeNull();
    expect(buildFabHandoff("pcb", { hasArtifact: false })).toBeNull();
    expect(buildFabHandoff("cast_metal", { hasArtifact: false })).toBeNull();
  });
});

describe("fab handoff in quote_manufacturing (engine)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("attaches fab_handoff to sheet_metal quotes and emits the BD event", async () => {
    const fetchMock = vi.fn(async () => ({ ok: true, status: 200, statusText: "OK" }));
    vi.stubGlobal("fetch", fetchMock);
    telemetryConfig.apiKey = "phc_test_key";
    try {
      const res = await quoteManufacturing(
        { ir: cubeDoc(), process: "sheet_metal", quantity: 3, material: "aluminum" },
        engine,
        new InMemoryFabricateStore(),
        null,
      );
      expect(res.isError).toBeFalsy();
      const quote = JSON.parse(res.content[0].text);
      expect(quote.fab_handoff).toBeDefined();
      expect(quote.fab_handoff.orderable_via_vcad).toBe(false);
      expect(quote.fab_handoff.shops).toHaveLength(3);

      await flushTelemetry();
      const handoffCall = fetchMock.mock.calls
        .map(([, init]) => JSON.parse((init as { body: string }).body))
        .find((p) => p.event === "fab_handoff_generated");
      expect(handoffCall).toBeDefined();
      expect(handoffCall.properties.process).toBe("sheet_metal");
      expect(handoffCall.properties.quantity).toBe(3);
      expect(handoffCall.properties.total_usd).toBeGreaterThan(0);
      // Aggregates only — no IR or shop payloads in the event.
      expect(handoffCall.properties.ir).toBeUndefined();
    } finally {
      telemetryConfig.apiKey = "";
      vi.unstubAllGlobals();
    }
  });

  it("omits fab_handoff for non-sheet-metal quotes", async () => {
    const res = await quoteManufacturing(
      { ir: cubeDoc(), process: "cnc", quantity: 1, material: "aluminum" },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    const quote = JSON.parse(res.content[0].text);
    expect(quote.fab_handoff).toBeUndefined();
  });
});
