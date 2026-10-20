import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  PartStockClient,
  annotateBomStock,
  bomCsvToStockLines,
  checkPartStock,
  bomCheckStock,
  extCost,
  lookupOrderable,
  orderableParts,
  passiveKeyword,
  searchOrderable,
  setStockClient,
  unitPriceAt,
  type FetchLike,
} from "../tools/part-stock.js";
import { resolvePart, findAlternatives, searchElectronicParts } from "../tools/ecad.js";
import { bomCreate, clearBoms } from "../tools/bom.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";

// Recorded (trimmed) LCSC/JLCPCB responses — tests never touch the network.
const here = dirname(fileURLToPath(import.meta.url));
const FIX = JSON.parse(readFileSync(join(here, "fixtures", "part-stock.json"), "utf-8")) as {
  jlc: Record<string, unknown>;
  lcsc: Record<string, unknown>;
};

function fixtureFetch(log: string[] = []): FetchLike {
  return async (url, init) => {
    log.push(url);
    let body: unknown;
    if (url.includes("jlcpcb.com")) {
      const kw = JSON.parse(init?.body ?? "{}").keyword as string;
      body = FIX.jlc[kw] ?? { code: 200, data: { componentPageInfo: { list: [] } } };
    } else if (url.includes("lcsc.com")) {
      const code = decodeURIComponent(url.split("productCode=")[1]);
      body = FIX.lcsc[code] ?? { code: 200, result: null };
    } else {
      return { ok: false, status: 404, json: async () => ({}) };
    }
    return { ok: true, status: 200, json: async () => body };
  };
}

const fastClient = (fetch: FetchLike, extra: Record<string, unknown> = {}) =>
  new PartStockClient({ fetch, minIntervalMs: 0, sleep: async () => {}, ...extra });

const parse = (r: { content: Array<{ text: string }> }) => JSON.parse(r.content[0].text);

describe("orderable DB", () => {
  it("covers the limbic ICs with valid identities", () => {
    for (const mpn of ["TPS48110AQDGXRQ1", "STM32H723VGT6", "TCAN1462VDRQ1", "LAN8742A-CZ-TR", "BMI088", "LM5145RGYR"]) {
      const p = lookupOrderable(mpn);
      expect(p, mpn).not.toBeNull();
      expect(p!.lcsc).toMatch(/^C\d+$/);
    }
    for (const p of orderableParts()) {
      expect(["verified", "corrected", "flagged"]).toContain(p.verification.status);
      if (p.verification.status !== "verified") expect(p.verification.note, p.id).toBeTruthy();
    }
  });

  it("maps the BOM's wrong MPNs / LCSC numbers to the corrected part", () => {
    expect(lookupOrderable("TPS48111AQDGXRQ1")!.mpn).toBe("TPS48111LQDGXRQ1");
    expect(lookupOrderable("SS10P10-M3/86A")!.mpn).toBe("SS10PH10-M3/86A");
    expect(lookupOrderable("IPT012N08N5ATMA1")!.lcsc).toBe("C531199");
    // BOM noted C730194 for the H723 — that LCSC# is an STM32H747.
    expect(lookupOrderable("C730194")!.lcsc).toBe("C730142");
    expect(lookupOrderable("Molex 43045-0412")!.id).toBe("molex.43045-0412");
    expect(lookupOrderable("XT30PB(2+2)-M (AMASS, right angle)")!.id).toBe("amass.xt30pb22m");
  });

  it("token search finds parts by function", () => {
    const hits = searchOrderable("CAN FD transceiver");
    expect(hits[0].mpn).toBe("TCAN1462VDRQ1");
  });
});

describe("price helpers", () => {
  it("unitPriceAt picks the highest break at or below qty", () => {
    const b = [
      { qty: 1, unit_usd: 3 },
      { qty: 10, unit_usd: 2 },
      { qty: 100, unit_usd: 1 },
    ];
    expect(unitPriceAt(b, 5)).toBe(3);
    expect(unitPriceAt(b, 10)).toBe(2);
    expect(unitPriceAt(b, 500)).toBe(1);
    expect(unitPriceAt([], 5)).toBeNull();
  });

  it("passive spec lines become JLC keywords with a value guard", () => {
    expect(passiveKeyword("RES 10k 0603")).toEqual({ keyword: "10kΩ 0603", mustContain: ["10kΩ"] });
    expect(passiveKeyword("RES 49.9 0603")!.keyword).toBe("49.9Ω 0603");
    expect(passiveKeyword("CAP 100nF 25V 0603")).toEqual({ keyword: "100nF 25V 0603", mustContain: ["100nF", "25V"] });
    expect(passiveKeyword("TPS48110AQDGXRQ1")).toBeNull();
  });
});

describe("PartStockClient (recorded fixtures)", () => {
  it("returns LCSC + JLC stock, ladder, and library for a DB part", async () => {
    const c = fastClient(fixtureFetch());
    const q = await c.check("TPS48110AQDGXRQ1");
    expect(q.found).toBe(true);
    expect(q.lcsc).toBe("C17556513");
    expect(q.stock_lcsc).toBe(78);
    expect(q.stock_jlc).toBe(78);
    expect(q.jlc_library).toBe("extended");
    expect(q.price_breaks[0]).toEqual({ qty: 1, unit_usd: expect.any(Number) });
    expect(q.lifecycle).toBe("normal");
    expect(q.eol).toBe(false);
    expect(q.source).toBe("live");
  });

  it("caches: a repeat check makes no new requests", async () => {
    const log: string[] = [];
    const c = fastClient(fixtureFetch(log));
    await c.check("C7519");
    const n = log.length;
    const again = await c.check("C7519");
    expect(log.length).toBe(n);
    expect(again.source).toBe("cache");
    expect(again.min_order_qty).toBe(5);
  });

  it("expires the cache after the TTL", async () => {
    let t = 0;
    const log: string[] = [];
    const c = fastClient(fixtureFetch(log), { now: () => t, ttlMs: 1000 });
    await c.check("C7519");
    const n = log.length;
    t = 5000;
    await c.check("C7519");
    expect(log.length).toBe(2 * n);
  });

  it("rate-limits: requests are spaced by minIntervalMs", async () => {
    let t = 0;
    const waits: number[] = [];
    const c = new PartStockClient({
      fetch: fixtureFetch(),
      now: () => t,
      minIntervalMs: 400,
      sleep: async (ms) => {
        waits.push(ms);
        t += ms;
      },
    });
    await Promise.all([c.check("C7519"), c.check("C730142")]);
    expect(c.requests).toBe(4);
    expect(waits.length).toBe(3);
    expect(waits.every((w) => w === 400)).toBe(true);
  });

  it("times out a hung request and falls back to the offline DB", async () => {
    const hang: FetchLike = () => new Promise(() => {});
    const c = fastClient(hang, { timeoutMs: 20 });
    const q = await c.check("STM32H723VGT6");
    expect(q.source).toBe("offline");
    expect(q.lcsc).toBe("C730142");
    expect(q.stock_lcsc).toBeNull();
    expect(q.warnings.join(" ")).toMatch(/timeout/);
  });

  it("offline mode never fetches", async () => {
    const log: string[] = [];
    const c = new PartStockClient({ fetch: fixtureFetch(log), offline: true });
    const q = await c.check("BMI088");
    expect(log).toHaveLength(0);
    expect(q.source).toBe("offline");
    expect(q.found).toBe(true);
  });

  it("flags an LCSC# that belongs to a different part", async () => {
    const c = fastClient(fixtureFetch());
    const q = await c.check("C730194");
    expect(q.lcsc).toBe("C730142");
    expect(q.warnings.join(" ")).toMatch(/NOT this part's number/);
  });

  it("reports a missing part (no listing) with the DB's flag", async () => {
    const c = fastClient(fixtureFetch());
    const q = await c.check("EEH-ZA1J101P");
    expect(q.found).toBe(false);
    expect(q.warnings.join(" ")).toMatch(/DB flags this part/);
  });

  it("treats discontinued lifecycles as EOL but sold_out as a stock state", async () => {
    const withCycle = (cycle: string): FetchLike => {
      const base = fixtureFetch();
      return async (url, init) => {
        const r = await base(url, init);
        if (!url.includes("lcsc.com")) return r;
        const body = JSON.parse(JSON.stringify(await r.json()));
        if (body.result) body.result.productCycle = cycle;
        return { ok: true, status: 200, json: async () => body };
      };
    };
    expect((await fastClient(withCycle("discontinued")).check("C7519")).eol).toBe(true);
    expect((await fastClient(withCycle("sold_out")).check("C7519")).eol).toBe(false);
  });

  it("resolves a passive spec to a stocked basic part", async () => {
    const c = fastClient(fixtureFetch());
    const q = await c.check("RES 10k 0603", 4000);
    expect(q.lcsc).toBe("C25804");
    expect(q.jlc_library).toBe("basic");
  });
});

describe("BOM annotation", () => {
  const CSV =
    "kind,name,process,material,spec,example_pn,vendor,qty,unit_price_usd,total_usd,pricing_basis,quote_id,order_id,document_id,artifact,notes\n" +
    'cots,TPS48110-Q1 [U300],,,VSSOP-19,TPS48110AQDGXRQ1,LCSC,4,2.50,10.00,manual_estimate,,,,,"LCSC unverified; OC 20 A"\n' +
    "cots,STM32H723VGT6 [U701],,,LQFP-100,STM32H723VGT6,LCSC,1,8.00,8.00,manual_estimate,,,,,LCSC C730194 (unverified)\n" +
    "cots,TOLL FET,,,TOLL,IPT012N08N5ATMA1,LCSC,3,4.50,13.50,manual_estimate,,,,,\n" +
    "cots,100uF 63V,,,CP,EEH-ZA1J101P,LCSC,4,0.80,3.20,manual_estimate,,,,,\n" +
    "cots,10k [R1 R2],,,R_0603,RES 10k 0603,LCSC,2,0,0,manual_estimate,,,,,\n" +
    "cots,SWD,,,TC2030,Tag-Connect TC2030 (footprint only),LCSC,1,0,0,manual_estimate,,,,,\n" +
    "manufactured,PCB,pcb,,,,,1,22.00,22.00,manual_estimate,,,,,\n";

  it("parses bom_export CSV, skipping manufactured and footprint-only lines", () => {
    const lines = bomCsvToStockLines(CSV);
    expect(lines.map((l) => l.part)).toEqual([
      "TPS48110AQDGXRQ1",
      "STM32H723VGT6",
      "IPT012N08N5ATMA1",
      "EEH-ZA1J101P",
      "RES 10k 0603",
    ]);
    expect(lines[1].noted_lcsc).toBe("C730194");
  });

  it("annotates stock, costs both bases at each build qty, and flags problems", async () => {
    const r = await annotateBomStock(fastClient(fixtureFetch()), bomCsvToStockLines(CSV), [5, 100]);
    const by = (p: string) => r.lines.find((l) => l.part === p)!;
    expect(by("TPS48110AQDGXRQ1").flags).toContain("low_stock"); // 78 < 400
    expect(by("STM32H723VGT6").flags).toContain("lcsc_mismatch");
    expect(by("IPT012N08N5ATMA1").flags).toContain("db_corrected");
    expect(by("EEH-ZA1J101P").flags).toEqual(expect.arrayContaining(["missing", "db_flagged", "unpriced"]));
    expect(by("EEH-ZA1J101P").alternatives.length).toBeGreaterThan(0);
    // LCSC honours the 100-pc MOQ on the 0603 resistor; JLC assembly doesn't.
    const res = by("RES 10k 0603");
    expect(res.cost.lcsc["5"]!).toBeGreaterThan(res.cost.jlc_assembly["5"]!);
    const tps = by("TPS48110AQDGXRQ1");
    expect(tps.cost.lcsc["5"]).toBeCloseTo(20 * unitPriceAt(tps.quote.price_breaks, 20)!, 6);
    expect(r.totals.lcsc["5"].unpriced_lines).toBe(1);
    expect(r.totals.lcsc["100"].usd).toBeGreaterThan(r.totals.lcsc["5"].usd);
    expect(r.flagged.map((f) => f.part)).toContain("EEH-ZA1J101P");
  });

  it("extCost falls back to the other ladder", () => {
    const q = { price_breaks: [], jlc_price_breaks: [{ qty: 1, unit_usd: 2 }], min_order_qty: 50 } as never;
    expect(extCost(q, 3, "lcsc")).toBe(6);
  });
});

describe("MCP handlers", () => {
  beforeEach(() => setStockClient(fastClient(fixtureFetch())));
  afterEach(() => setStockClient(null));

  it("check_part_stock answers batches", async () => {
    const r = await checkPartStock({ queries: ["C7519", "TPS48110AQDGXRQ1"] });
    const j = parse(r);
    expect(j.count).toBe(2);
    expect(j.results[0].mpn).toBe("USBLC6-2SC6");
    expect(r.structuredContent).toBeTruthy();
  });

  it("check_part_stock rejects an empty call", async () => {
    expect((await checkPartStock({})).isError).toBe(true);
  });

  it("bom_check_stock reads a bom_create BOM by id", async () => {
    clearBoms();
    const created = parse(
      await bomCreate(
        {
          title: "limbic",
          lines: [{ kind: "cots", name: "ESD", example_pn: "USBLC6-2SC6", qty: 1, notes: "LCSC C7519 (unverified)" }],
        },
        new InMemoryFabricateStore(),
        null,
      ),
    );
    const r = parse(await bomCheckStock({ bom_id: created.bom_id, builds: [5] }, { user: null }));
    expect(r.lines).toHaveLength(1);
    expect(r.lines[0].quote.lcsc).toBe("C7519");
    expect(r.lines[0].flags).not.toContain("lcsc_mismatch");
    expect(r.totals.lcsc["5"].usd).toBeGreaterThan(0);
  });

  it("parts tools surface the orderable DB", async () => {
    const res = parse(await resolvePart({ query: "TPS48111AQDGXRQ1" }));
    expect(res.kind).toBe("orderable");
    expect(res.mpn).toBe("TPS48111LQDGXRQ1");
    const alts = parse(await findAlternatives({ query: "SS10PH10-M3/86A" }));
    expect(alts.orderable_alternatives[0].mpn).toBe("V10P10-M3/86A");
    const s = parse(await searchElectronicParts({ query: "BMI088" }));
    expect(s.orderable[0].mpn).toBe("BMI088");
  });
});
