import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
import { createHash } from "node:crypto";
import { Engine } from "@vcad/engine";
import { documents } from "../tools/session.js";
import { quoteManufacturing } from "../tools/order.js";
import { sheetMetalCreate } from "../tools/sheet-metal.js";
import { storeArtifact, clearArtifacts } from "../tools/artifact-store.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import { kerfFetch, setKerfFetch } from "../fabricate/kerf/client.js";
import { intentHash } from "../fabricate/kerf/intent-hash.js";
import type { ConfiguratorIntent } from "../fabricate/kerf/contract.js";

/**
 * The kerf rail (Wave 0, SendCutSend quote-only): with KERF_URL configured and
 * a fab bundle bound, quote_manufacturing routes sheet metal through the kerf
 * adapter and surfaces the fab's OWN displayed price (pricing_basis "quoted",
 * suppressing the generic estimator); an unreachable rail degrades gracefully
 * back to the vcad estimate. Plus the intent-hash discipline invariants the
 * whole rail hangs off.
 */

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const out = (r: { content: Array<{ text: string }> }): any =>
  JSON.parse(r.content[0].text);

/** Canned kerf quote-job response (shape per fabricate/kerf/contract.ts). */
const CANNED_JOB = {
  job_id: "job_test_1",
  state: "DELIVERED",
  quote: {
    quote_id: "vq_test_1",
    vendor: "sendcutsend",
    intent_hash: "cafe".repeat(16),
    pricing_basis: "quoted",
    unit_price: { currency: "USD", amount_minor: 279 },
    total: { currency: "USD", amount_minor: 558 },
    lead_time_days: 0,
    evidence: ["ev_1", "ev_2"],
    notes: ["SCS displayed price"],
  },
  intent_hash: "cafe".repeat(16),
  live_url: null,
  evidence: { items: 2, claims: [] },
};

function square(size: number) {
  return [
    { x: 0, y: 0 },
    { x: size, y: 0 },
    { x: size, y: size },
    { x: 0, y: size },
  ];
}

/** The exact DXF bytes the fixture artifact holds — the wire contract test
 *  asserts these round-trip into `bytes_base64` hash-verified. */
const FIXTURE_DXF = "0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";

/** A sheet-metal session (base flange ⇒ material/thickness derivable — an
 *  SCS-native aluminum gauge: 3.175 mm = .125" = ALU-125) plus a bound DXF
 *  fab artifact — the two preconditions of the kerf intent path. */
function sheetMetalFixture(over: {
  material?: string;
  thickness?: number;
  files?: Array<{ name: string; content: string }>;
} = {}): { documentId: string; artifactId: string } {
  const created = out(
    sheetMetalCreate(
      {
        outline: square(50),
        thickness: over.thickness ?? 3.175,
        material: over.material ?? "al-soft",
      },
      engine,
    ),
  );
  const handle = storeArtifact(
    over.files ?? [{ name: "flat-pattern.dxf", content: FIXTURE_DXF }],
  );
  return { documentId: created.document_id as string, artifactId: handle.artifact_id };
}

/** Install a recording kerf fetch that answers CANNED_JOB. */
function recordKerf(): Array<{ url: string; body: unknown }> {
  const requests: Array<{ url: string; body: unknown }> = [];
  setKerfFetch((async (url: unknown, init?: { body?: unknown }) => {
    requests.push({
      url: String(url),
      body: init?.body ? JSON.parse(String(init.body)) : null,
    });
    return { ok: true, status: 200, json: async () => CANNED_JOB };
  }) as unknown as typeof fetch);
  return requests;
}

describe("kerf rail — quoted pricing basis + graceful degrade", () => {
  const restoreFetch = kerfFetch;
  beforeEach(() => {
    documents.clear();
    clearArtifacts();
    process.env.KERF_URL = "http://kerf.test";
  });
  afterEach(() => {
    delete process.env.KERF_URL;
    delete process.env.KERF_QUOTE_MODE;
    setKerfFetch(restoreFetch);
  });

  it("LIVE mode surfaces the fab's displayed price as 'quoted', with bytes + vendor-native config on the wire", async () => {
    process.env.KERF_QUOTE_MODE = "live";
    const requests = recordKerf();

    const { documentId, artifactId } = sheetMetalFixture();
    const res = await quoteManufacturing(
      {
        document_id: documentId,
        process: "sheet_metal",
        quantity: 2,
        fab_artifact_id: artifactId,
      },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBeFalsy();
    const quote = out(res);

    // The kerf adapter quoted — the fab's own price, basis "quoted".
    const scs = quote.fab_options.find((o: { fab: string }) => o.fab === "sendcutsend");
    expect(scs).toBeDefined();
    expect(scs.pricing_basis).toBe("quoted");
    expect(scs.notes).toContain("kerf job job_test_1");
    // A contracted fab quoted ⇒ the generic ballpark must not compete.
    expect(
      quote.fab_options.some((o: { fab: string }) => o.fab === "vcad_estimate"),
    ).toBe(false);
    expect(quote.pricing_basis).toBe("quoted");
    expect(quote.recommended_fab).toBe("sendcutsend");

    // Quote-is-only-meaningful-with-its-intent-hash: persisted + surfaced.
    expect(quote.kerf_intent_hash).toMatch(/^[0-9a-f]{64}$/);
    expect(quote.kerf_job_id).toBe("job_test_1");

    // The adapter sent the SAME intent order.ts hashed: one identity, no drift.
    expect(requests).toHaveLength(1);
    expect(requests[0].url).toBe("http://kerf.test/api/quote");
    const sent = requests[0].body as { vendor: string; mode: string; intent: ConfiguratorIntent };
    expect(sent.vendor).toBe("sendcutsend");
    expect(sent.mode).toBe("live");
    expect(intentHash(sent.intent)).toBe(quote.kerf_intent_hash);
    // Quote intents can never fund an order (kerf canary discipline).
    expect(sent.intent.budget_cap.amount_minor).toBe(0);

    // Wire contract: exactly ONE file, carrying bytes_base64 that hash-match
    // the manifest sha256 (kerf's posted-intent API 400s without the bytes).
    expect(sent.intent.files).toHaveLength(1);
    const file = sent.intent.files[0];
    expect(file.bytes_base64).toBeDefined();
    expect(Buffer.from(file.bytes_base64!, "base64").toString("utf8")).toBe(FIXTURE_DXF);
    expect(
      createHash("sha256").update(Buffer.from(file.bytes_base64!, "base64")).digest("hex"),
    ).toBe(file.sha256);
    // intentHash ignores the bytes (sha256s only): stripping bytes_base64
    // must not move the hash.
    const { bytes_base64: _b, ...bare } = file;
    expect(
      intentHash({ ...sent.intent, files: [bare] } as ConfiguratorIntent),
    ).toBe(quote.kerf_intent_hash);

    // Config vocabulary: exactly the pointers the SCS playbook dereferences,
    // with vendor-native values (canary-intent fixture vocabulary).
    expect(sent.intent.config).toEqual({
      units: "MM",
      material_category: "Metals",
      material_family: "Aluminum",
      material: "5052 H32",
      thickness: "ALU-125",
      thickness_label: '.125" (3.2 MM)',
    });
  });

  it("scripted (default) mode downgrades to pricing_basis 'estimate' with the rehearsal note", async () => {
    recordKerf(); // KERF_QUOTE_MODE unset → scripted

    const { documentId, artifactId } = sheetMetalFixture();
    const quote = out(
      await quoteManufacturing(
        {
          document_id: documentId,
          process: "sheet_metal",
          quantity: 2,
          fab_artifact_id: artifactId,
        },
        engine,
        new InMemoryFabricateStore(),
        null,
      ),
    );

    // The scripted run is a rehearsal of the rail (fixture price regardless
    // of geometry) — it must NEVER present as the fab's own displayed price.
    const scs = quote.fab_options.find((o: { fab: string }) => o.fab === "sendcutsend");
    expect(scs).toBeDefined();
    expect(scs.pricing_basis).toBe("estimate");
    expect(scs.notes).toContain("kerf scripted rehearsal — not a vendor-displayed price");
    expect(quote.pricing_basis).toBe("estimate");
    // The intent binding still records what was rehearsed…
    expect(quote.kerf_intent_hash).toMatch(/^[0-9a-f]{64}$/);
    // …and the CONTRACTED_FABS suppression still applies (fab-key based).
    expect(
      quote.fab_options.some((o: { fab: string }) => o.fab === "vcad_estimate"),
    ).toBe(false);
  });

  it("fails closed when no vendor-native config derives (steel ⇒ no kerf request at all)", async () => {
    const requests = recordKerf();

    const { documentId, artifactId } = sheetMetalFixture({
      material: "steel-mild",
      thickness: 2.7,
    });
    const quote = out(
      await quoteManufacturing(
        {
          document_id: documentId,
          process: "sheet_metal",
          quantity: 2,
          fab_artifact_id: artifactId,
        },
        engine,
        new InMemoryFabricateStore(),
        null,
      ),
    );
    expect(requests).toHaveLength(0); // the rail was never called
    expect(
      quote.fab_options.some((o: { fab: string }) => o.fab === "sendcutsend"),
    ).toBe(false);
    expect(quote.kerf_intent_hash).toBeUndefined();
    expect(quote.note).toContain("kerf vendor quote skipped");
    expect(quote.note).toContain("vendor-native");
  });

  it("fails closed on multi-DXF bundles (the vendor playbook uploads a single file)", async () => {
    const requests = recordKerf();

    const { documentId, artifactId } = sheetMetalFixture({
      files: [
        { name: "part-a.dxf", content: FIXTURE_DXF },
        { name: "part-b.dxf", content: FIXTURE_DXF + "\n" },
      ],
    });
    const quote = out(
      await quoteManufacturing(
        {
          document_id: documentId,
          process: "sheet_metal",
          quantity: 1,
          fab_artifact_id: artifactId,
        },
        engine,
        new InMemoryFabricateStore(),
        null,
      ),
    );
    expect(requests).toHaveLength(0);
    expect(quote.kerf_intent_hash).toBeUndefined();
    expect(quote.note).toContain("multi-DXF orders not yet kerf-quotable");
  });

  it("degrades to the vcad estimate when the rail is unreachable (never fails the quote)", async () => {
    setKerfFetch((async () => {
      throw new Error("ECONNREFUSED");
    }) as unknown as typeof fetch);

    const { documentId, artifactId } = sheetMetalFixture();
    const res = await quoteManufacturing(
      {
        document_id: documentId,
        process: "sheet_metal",
        quantity: 2,
        fab_artifact_id: artifactId,
      },
      engine,
      new InMemoryFabricateStore(),
      null,
    );
    expect(res.isError).toBeFalsy();
    const quote = out(res);
    expect(
      quote.fab_options.some((o: { fab: string }) => o.fab === "sendcutsend"),
    ).toBe(false);
    const generic = quote.fab_options.find(
      (o: { fab: string }) => o.fab === "vcad_estimate",
    );
    expect(generic).toBeDefined();
    expect(generic.pricing_basis).toBe("estimate");
    // No vendor quote ⇒ no intent binding on the quote.
    expect(quote.kerf_intent_hash).toBeUndefined();
  });
});

describe("kerf intent-hash discipline (what would be manufactured, nothing else)", () => {
  const base: ConfiguratorIntent = {
    kind: "configurator",
    vendor: "sendcutsend",
    process: "sheet_metal",
    files: [{ name: "flat.dxf", bytes: 128, sha256: "ab".repeat(32) }],
    config: { material: "5052", thickness: "ALU-125", thickness_label: "3.175 mm" },
    quantity: 2,
    idempotency_key: "vq_original",
    ship_to: {
      name: "vcad quote",
      line1: "548 Market St",
      city: "San Francisco",
      region: "CA",
      postal_code: "94104",
      country: "US",
    },
    budget_cap: { currency: "USD", amount_minor: 0 },
  };

  it("is independent of object key order (canonical JSON)", () => {
    const reordered: ConfiguratorIntent = {
      budget_cap: base.budget_cap,
      ship_to: base.ship_to,
      idempotency_key: base.idempotency_key,
      quantity: base.quantity,
      // config keys inserted in reverse order
      config: { thickness_label: "3.175 mm", thickness: "ALU-125", material: "5052" },
      files: [{ sha256: "ab".repeat(32), bytes: 128, name: "flat.dxf" }],
      process: base.process,
      vendor: base.vendor,
      kind: "configurator",
    };
    expect(intentHash(reordered)).toBe(intentHash(base));
  });

  it("moves when quantity, config, or file bytes change (quote is dead ⇒ re-quote)", () => {
    expect(intentHash({ ...base, quantity: 3 })).not.toBe(intentHash(base));
    expect(
      intentHash({ ...base, config: { ...base.config, material: "6061" } }),
    ).not.toBe(intentHash(base));
    expect(
      intentHash({
        ...base,
        files: [{ ...base.files[0], sha256: "cd".repeat(32) }],
      }),
    ).not.toBe(intentHash(base));
  });

  it("ignores idempotency_key, ship_to, budget_cap, deadline, and file names", () => {
    expect(intentHash({ ...base, idempotency_key: "vq_retry_99" })).toBe(intentHash(base));
    expect(
      intentHash({
        ...base,
        ship_to: { ...base.ship_to, line1: "1 Infinite Loop", city: "Cupertino" },
      }),
    ).toBe(intentHash(base));
    expect(
      intentHash({ ...base, budget_cap: { currency: "USD", amount_minor: 99_999 } }),
    ).toBe(intentHash(base));
    expect(intentHash({ ...base, deadline: "2027-01-01T00:00:00Z" })).toBe(intentHash(base));
    expect(
      intentHash({ ...base, files: [{ ...base.files[0], name: "renamed.dxf", bytes: 999 }] }),
    ).toBe(intentHash(base));
  });
});
