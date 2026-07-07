import { describe, it, expect, beforeEach } from "vitest";
import {
  bomCreate,
  bomAddLine,
  bomExport,
  computeTotals,
  bomCostClaim,
  clearBoms,
  type Bom,
} from "../tools/bom.js";
import {
  searchMechanicalParts,
  searchMechCatalog,
  mechCatalog,
} from "../tools/mech-parts.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import type { FabOption, Quote } from "../fabricate/types.js";
import type { AuthUser } from "../oauth.js";

const text = (r: { content: Array<{ text: string }> }) => r.content[0].text;
const json = (r: { content: Array<{ text: string }> }) => JSON.parse(text(r));

function fabOption(over: Partial<FabOption> = {}): FabOption {
  return {
    fab: "jlcpcb",
    fab_label: "JLCPCB",
    region: "CN",
    unit_price_minor: 420,
    total_minor: 2100,
    lead_time_days: 7,
    in_spec: true,
    pricing_basis: "estimate",
    supports_ddp: true,
    orderable: false,
    notes: [],
    ...over,
  };
}

function makeQuote(over: Partial<Quote> = {}): Quote {
  const now = new Date();
  return {
    quote_id: "q-stator",
    document_id: "doc-stator",
    doc_hash: "abc123",
    process: "pcb",
    material: "FR-4",
    quantity: 5,
    dfm: { checked: true, passed: true, violations: [] },
    fab_options: [fabOption()],
    landed_cost: { shipping_minor: 2500, duty_minor: 0, basis: "ddp_estimate" },
    total_amount_minor: 2100,
    currency: "USD",
    margin_hidden: true,
    expires_at: new Date(now.getTime() + 86_400_000).toISOString(),
    created_at: now.toISOString(),
    ...over,
  };
}

// ── mechanical catalog search ────────────────────────────────────────────────

describe("search_mechanical_parts — catalog search", () => {
  it("loads a non-empty catalog with unique ids and sane price bands", () => {
    const parts = mechCatalog();
    expect(parts.length).toBeGreaterThan(30);
    const ids = new Set(parts.map((p) => p.id));
    expect(ids.size).toBe(parts.length);
    for (const p of parts) {
      expect(p.price_band_usd[0]).toBeGreaterThan(0);
      expect(p.price_band_usd[1]).toBeGreaterThanOrEqual(p.price_band_usd[0]);
    }
  });

  it("finds bearings by type + bore dimension", () => {
    const hits = searchMechCatalog({ type: "bearing", bore_mm: 8 });
    expect(hits.length).toBeGreaterThanOrEqual(2); // 608ZZ, 608-2RS, 688ZZ
    expect(hits.every((p) => p.type === "bearing")).toBe(true);
    expect(hits.every((p) => p.spec.bore_mm === 8)).toBe(true);
    expect(hits.map((p) => p.id)).toContain("bearing.608zz");
    expect(hits.map((p) => p.id)).toContain("bearing.688zz");
  });

  it("narrows by a second dimension (bore 8 + od 16 → thin 688 only)", () => {
    const hits = searchMechCatalog({ type: "bearing", bore_mm: 8, od_mm: 16 });
    expect(hits.map((p) => p.id)).toEqual(["bearing.688zz"]);
  });

  it("applies the dimension tolerance", () => {
    // 7.9 within default ±0.25 of an 8 mm bore
    expect(searchMechCatalog({ type: "bearing", bore_mm: 7.9 }).length).toBeGreaterThan(0);
    // 7.0 is not
    expect(searchMechCatalog({ type: "bearing", bore_mm: 7.0 })).toHaveLength(0);
    // …unless the tolerance is widened
    expect(
      searchMechCatalog({ type: "bearing", bore_mm: 7.0, tolerance_mm: 1.5 }).length,
    ).toBeGreaterThan(0);
  });

  it("matches screws by thread + available length", () => {
    const hits = searchMechCatalog({ type: "screw", thread: "M3", length_mm: 10 });
    expect(hits.length).toBeGreaterThan(0);
    for (const p of hits) {
      expect(String(p.spec.thread).toUpperCase()).toBe("M3");
      expect(p.spec.lengths_mm).toContain(10);
    }
    // A length nothing stocks matches nothing.
    expect(searchMechCatalog({ type: "screw", thread: "M3", length_mm: 11 })).toHaveLength(0);
  });

  it("normalizes thread case ('m3' == 'M3')", () => {
    const hits = searchMechCatalog({ type: "standoff", thread: "m3" });
    expect(hits.length).toBeGreaterThan(0);
  });

  it("free-text query matches synonyms and ranks name hits", () => {
    const hits = searchMechCatalog({ query: "skate bearing" });
    expect(hits.length).toBeGreaterThan(0);
    expect(hits[0].id).toBe("bearing.608zz");
  });

  it("finds ferrite magnets by grade text + dimensions", () => {
    const hits = searchMechCatalog({ type: "magnet", query: "Y30", od_mm: 12, thickness_mm: 3 });
    expect(hits.map((p) => p.id)).toEqual(["magnet.ferrite-disc-12x3-y30"]);
  });

  it("tool handler flags prices as estimates and hints on empty results", () => {
    const found = json(searchMechanicalParts({ type: "bearing", bore_mm: 8 }));
    expect(found.count).toBeGreaterThan(0);
    expect(found.results[0].price_band_usd.basis).toBe("estimate");
    expect(found.pricing_note).toContain("ESTIMATE");

    const empty = json(searchMechanicalParts({ type: "bearing", bore_mm: 99 }));
    expect(empty.count).toBe(0);
    expect(empty.hint).toBeTruthy();
  });

  it("rejects an unknown type", () => {
    const res = searchMechanicalParts({ type: "sprocket" });
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("Unknown type");
  });
});

// ── BOM math ─────────────────────────────────────────────────────────────────

describe("BOM tools — creation, line math, totals", () => {
  beforeEach(() => clearBoms());

  it("bom_create builds a full BOM in one call and sums it", async () => {
    const store = new InMemoryFabricateStore();
    const res = await bomCreate(
      {
        title: "Demo motor",
        document_id: "doc-motor",
        assembly_notes: ["Press bearings before gluing magnets."],
        lines: [
          {
            kind: "manufactured",
            name: "Rotor plate",
            process: "sheet_metal",
            vendor: "SendCutSend",
            qty: 2,
            unit_price_usd: 9.5,
            artifact: "fab/rotor-plate.step",
          },
          { kind: "cots", catalog_id: "bearing.608zz", qty: 2, vendor: "Amazon" },
          { kind: "cots", name: "30A ESC", spec: "3S, BLHeli", qty: 1, unit_price_usd: 12 },
        ],
      },
      store,
      null,
    );
    expect(res.isError).toBeUndefined();
    const out = json(res);
    expect(out.bom_id).toBeTruthy();
    expect(out.lines).toHaveLength(3);
    // manufactured: 2 × $9.50 = $19.00
    expect(out.totals.manufactured_subtotal_usd).toBe(19);
    // 608ZZ band midpoint = (0.3+1.2)/2 = 0.75 → 2 × $0.75 + $12
    expect(out.totals.cots_subtotal_usd).toBeCloseTo(13.5, 2);
    // three distinct vendor groups (sendcutsend, amazon, unspecified) × $8
    expect(out.totals.shipping_estimate_usd).toBe(24);
    expect(out.totals.grand_total_usd).toBeCloseTo(19 + 13.5 + 24, 2);
    expect(out.note).toContain("ESTIMATE");
  });

  it("bom_add_line links a persisted quote and inherits its landed pricing", async () => {
    const store = new InMemoryFabricateStore();
    await store.saveQuote(makeQuote(), { fab_cost_minor: 1680, margin_minor: 420 }, "local");

    const created = json(await bomCreate({ title: "t" }, store, null));
    const res = await bomAddLine(
      { bom_id: created.bom_id, kind: "manufactured", name: "Stator PCB", quote_id: "q-stator" },
      store,
      null,
    );
    expect(res.isError).toBeUndefined();
    const out = json(res);
    // qty from quote (5), unit = 2100/5 = 420 minor = $4.20
    expect(out.added.qty).toBe(5);
    expect(out.added.unit_price_usd).toBe(4.2);
    expect(out.added.total_usd).toBe(21);
    expect(out.added.pricing_basis).toBe("quote_estimate");
    // quote prices are already landed → no shipping line for this vendor
    expect(out.totals.shipping_estimate_usd).toBe(0);

    const exported = json(bomExport({ bom_id: created.bom_id, format: "json" }, null));
    const line = exported.bom.lines[0];
    expect(line.process).toBe("pcb");
    expect(line.material).toBe("FR-4");
    expect(line.vendor).toBe("JLCPCB");
    expect(line.document_id).toBe("doc-stator");
  });

  it("quotes are owner-scoped: another user's quote_id does not resolve", async () => {
    const store = new InMemoryFabricateStore();
    const alice: AuthUser = { sub: "u-alice", email: "a@x.y" };
    await store.saveQuote(makeQuote({ quote_id: "q-priv" }), { fab_cost_minor: 1, margin_minor: 1 }, "u-alice");

    const created = json(await bomCreate({}, store, null)); // anonymous "local"
    const res = await bomAddLine(
      { bom_id: created.bom_id, kind: "manufactured", name: "x", quote_id: "q-priv" },
      store,
      null,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("Unknown quote_id");

    // …while the owner can link it.
    const own = json(await bomCreate({}, store, alice));
    const ok = await bomAddLine(
      { bom_id: own.bom_id, kind: "manufactured", name: "x", quote_id: "q-priv" },
      store,
      alice,
    );
    expect(ok.isError).toBeUndefined();
  });

  it("BOMs are owner-scoped", async () => {
    const store = new InMemoryFabricateStore();
    const alice: AuthUser = { sub: "u-alice2", email: "a@x.y" };
    const created = json(await bomCreate({ title: "private" }, store, alice));
    const res = bomExport({ bom_id: created.bom_id }, null);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("Unknown bom_id");
  });

  it("unpriced lines are excluded from totals and counted", async () => {
    const store = new InMemoryFabricateStore();
    const out = json(
      await bomCreate(
        {
          lines: [
            { kind: "cots", name: "Mystery shaft", qty: 1 },
            { kind: "cots", name: "M3 screws", qty: 10, unit_price_usd: 0.1 },
          ],
        },
        store,
        null,
      ),
    );
    expect(out.totals.cots_subtotal_usd).toBe(1);
    expect(out.totals.unpriced_lines).toBe(1);
  });

  it("shipping groups by distinct vendor, case-insensitively", async () => {
    const store = new InMemoryFabricateStore();
    const out = json(
      await bomCreate(
        {
          lines: [
            { kind: "cots", name: "a", vendor: "Amazon", qty: 1, unit_price_usd: 1 },
            { kind: "cots", name: "b", vendor: "amazon", qty: 1, unit_price_usd: 1 },
            { kind: "cots", name: "c", vendor: "McMaster", qty: 1, unit_price_usd: 1 },
          ],
        },
        store,
        null,
      ),
    );
    // two distinct vendors × $8 flat domestic estimate
    expect(out.totals.shipping_estimate_usd).toBe(16);
  });

  it("validates line inputs with specific errors", async () => {
    const store = new InMemoryFabricateStore();
    const created = json(await bomCreate({}, store, null));

    const badKind = await bomAddLine({ bom_id: created.bom_id, kind: "widget" }, store, null);
    expect(badKind.isError).toBe(true);

    const noProcess = await bomAddLine(
      { bom_id: created.bom_id, kind: "manufactured", name: "x" },
      store,
      null,
    );
    expect(noProcess.isError).toBe(true);
    expect(text(noProcess)).toContain("process");

    const badCatalog = await bomAddLine(
      { bom_id: created.bom_id, kind: "cots", catalog_id: "bearing.nope" },
      store,
      null,
    );
    expect(badCatalog.isError).toBe(true);
    expect(text(badCatalog)).toContain("catalog_id");

    const badBom = await bomAddLine({ bom_id: "nope", kind: "cots", name: "x" }, store, null);
    expect(badBom.isError).toBe(true);

    // Bulk create fails loudly, naming the bad line.
    const bulk = await bomCreate({ lines: [{ kind: "cots" }] }, store, null);
    expect(bulk.isError).toBe(true);
    expect(text(bulk)).toContain("lines[0]");
  });
});

// ── export formats + receipt claim ───────────────────────────────────────────

describe("bom_export — formats and receipt claim", () => {
  beforeEach(() => clearBoms());

  async function motorBom(store: InMemoryFabricateStore): Promise<string> {
    await store.saveQuote(makeQuote(), { fab_cost_minor: 1680, margin_minor: 420 }, "local");
    const created = json(
      await bomCreate(
        {
          title: "Axial-flux demo motor",
          document_id: "doc-motor",
          assembly_notes: ["Glue magnets N/S alternating.", "Torque M3 to 0.5 Nm."],
          lines: [
            { kind: "manufactured", name: "Stator PCB", quote_id: "q-stator", artifact: "fab/gerbers.zip" },
            {
              kind: "manufactured",
              name: "Rotor plate, mild steel",
              process: "sheet_metal",
              vendor: "SendCutSend",
              qty: 2,
              unit_price_usd: 9.5,
              artifact: "fab/rotor.step",
            },
            { kind: "cots", catalog_id: "bearing.688zz", qty: 2, vendor: "Amazon" },
            { kind: "cots", name: 'Shaft, 8mm x 100mm, "ground"', spec: "h6, hardened", qty: 1, unit_price_usd: 4 },
          ],
        },
        store,
        null,
      ),
    );
    return created.bom_id;
  }

  it("renders the markdown deliverable", async () => {
    const store = new InMemoryFabricateStore();
    const bomId = await motorBom(store);
    const out = json(bomExport({ bom_id: bomId, format: "markdown" }, null));
    const mdDoc: string = out.rendered;

    expect(mdDoc).toContain("# Axial-flux demo motor");
    expect(mdDoc).toContain("## Manufactured Parts");
    expect(mdDoc).toContain("## COTS Parts");
    expect(mdDoc).toContain("## Totals");
    expect(mdDoc).toContain("## Assembly Notes");
    expect(mdDoc).toContain("- Glue magnets N/S alternating.");
    // quote-linked line shows landed price and its sources
    expect(mdDoc).toContain("$4.20");
    expect(mdDoc).toContain("quote q-stator");
    expect(mdDoc).toContain("fab/gerbers.zip");
    // COTS from catalog carries example PN + spec
    expect(mdDoc).toContain("688ZZ");
    // estimates disclaimer up top
    expect(mdDoc).toContain("ESTIMATES");
    // totals line
    expect(mdDoc).toContain("**Estimated total**");
  });

  it("renders RFC-4180 CSV with escaping", async () => {
    const store = new InMemoryFabricateStore();
    const bomId = await motorBom(store);
    const out = json(bomExport({ bom_id: bomId, format: "csv" }, null));
    const csvDoc: string = out.rendered;
    const rows = csvDoc.trim().split("\r\n");

    expect(rows[0]).toBe(
      "kind,name,process,material,spec,example_pn,vendor,qty,unit_price_usd,total_usd,pricing_basis,quote_id,order_id,document_id,artifact,notes",
    );
    expect(rows).toHaveLength(5); // header + 4 lines
    // comma + quotes in the shaft name → quoted with doubled quotes
    expect(csvDoc).toContain('"Shaft, 8mm x 100mm, ""ground"""');
    expect(rows[1]).toContain("quote_estimate");
  });

  it("json format returns the full BOM object", async () => {
    const store = new InMemoryFabricateStore();
    const bomId = await motorBom(store);
    const out = json(bomExport({ bom_id: bomId, format: "json" }, null));
    expect(out.bom.lines).toHaveLength(4);
    expect(out.bom.title).toBe("Axial-flux demo motor");
    expect(out.rendered).toBeUndefined();
  });

  it("emits a vcad.receipt/1 cost claim with the grand total", async () => {
    const store = new InMemoryFabricateStore();
    const bomId = await motorBom(store);
    const out = json(bomExport({ bom_id: bomId }, null));
    const claim = out.receipt_claim;

    expect(claim.id).toBe("bom.cost.total");
    expect(claim.domain).toBe("bom");
    expect(claim.verdict).toBe("pass");
    expect(claim.subject).toBe("document:doc-motor");
    expect(claim.oracle.id).toBe("vcad-mcp/bom");
    expect(claim.measured.unit).toBe("USD");
    expect(claim.measured.value).toBe(out.totals.grand_total_usd);
    expect(claim.details).toContain("ESTIMATE");
    expect(claim.details).toContain("never gates");
  });

  it("cost claim is fail-closed: empty or fully-unpriced BOMs are unverifiable", () => {
    const empty: Bom = {
      bom_id: "b1",
      title: "t",
      document_id: null,
      assembly_notes: [],
      lines: [],
      currency: "USD",
      created_at: "2026-07-07T00:00:00Z",
      updated_at: "2026-07-07T00:00:00Z",
    };
    expect(bomCostClaim(empty, computeTotals(empty)).verdict).toBe("unverifiable");

    const unpriced: Bom = {
      ...empty,
      lines: [
        {
          kind: "cots",
          line_id: "l1",
          name: "mystery",
          spec: null,
          example_pn: null,
          catalog_id: null,
          vendor: null,
          qty: 1,
          unit_price_minor: null,
          total_minor: null,
          pricing_basis: "unpriced",
          notes: null,
        },
      ],
    };
    const claim = bomCostClaim(unpriced, computeTotals(unpriced));
    expect(claim.verdict).toBe("unverifiable");
    expect(claim.details).toContain("unpriced");
  });

  it("rejects an unknown export format", async () => {
    const store = new InMemoryFabricateStore();
    const created = json(await bomCreate({}, store, null));
    const res = bomExport({ bom_id: created.bom_id, format: "xml" }, null);
    expect(res.isError).toBe(true);
  });
});
