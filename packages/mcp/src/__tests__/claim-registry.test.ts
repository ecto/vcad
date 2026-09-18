/**
 * The claim-family registry, end to end through the real server dispatch.
 *
 * The gap this closes: vcad had a dozen-odd Rust claim families, each able to
 * translate itself into unified receipt claims, and `build_receipt` could
 * reach exactly three of them (PCB, mechanical clearance, design
 * constraints). No `design_claims` in the repo had ever reached a receipt.
 *
 * So the load-bearing assertions here are about the *ladder surviving the
 * whole journey* — kernel oracle → deposit on the document → registry →
 * unified receipt — rather than about any call returning something:
 *
 * - a passing job's arithmetic claims read `pass` on a **verified** basis,
 *   and its claims about metal read `pass` only on a **predicted** one, so
 *   the receipt rolls up `provisional` and never `pass`;
 * - a measurement inside tolerance closes a prediction onto a **measured**
 *   basis, one outside it `fail`s the claim, and the compensation claim that
 *   supersedes it is itself only predicted again;
 * - editing the job turns its claims `unverifiable` (Stale), not `pass`;
 * - a document with no deposits produces exactly the receipt it did before
 *   any of this existed.
 *
 * Like cam.test.ts, these need a kernel WASM carrying the CAM bindings and
 * the registry; on a stale local artifact the suite skips rather than
 * failing.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents, getSession } from "../tools/session.js";

interface ToolCallResult {
  content: Array<{ type: string; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

type Json = Record<string, unknown>;

interface Claim {
  id: string;
  domain: string;
  verdict: string;
  basis?: string;
  subject?: string;
  details?: string;
  measured?: { value: unknown; unit?: string };
  predicted?: { value: unknown; unit?: string };
}

const engine = await Engine.init();
const kernel = (engine as unknown as { kernel: Json }).kernel;
const ready =
  typeof kernel.camJob === "function" &&
  typeof kernel.receiptClaimsFor === "function";
if (!ready) {
  console.warn(
    "[claim-registry.test] the loaded kernel WASM predates the CAM bindings or the claim registry — skipping. Rebuild with `node packages/kernel-wasm/scripts/build.mjs`.",
  );
}

async function connect() {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client };
}

function bodyOf(result: ToolCallResult): Json {
  const merged: Json = {};
  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        Object.assign(merged, parsed);
      }
    } catch {
      // prose block
    }
  }
  return merged;
}

async function call(
  client: Client,
  name: string,
  args: Json,
): Promise<ToolCallResult> {
  return (await client.callTool({ name, arguments: args })) as ToolCallResult;
}

async function ok(client: Client, name: string, args: Json): Promise<Json> {
  const res = await call(client, name, args);
  expect(res.isError, `${name}: ${JSON.stringify(res.content)}`).toBeFalsy();
  return bodyOf(res);
}

/** The same 80 × 50 × 6 plate cam.test.ts machines, in the same shape, so the
 *  two suites are talking about the same part. */
const PLATE = `
[let plate [cube 80 50 6]]
[let bore [translate 40 25 -1 [cylinder-n 8 8 96]]]
[let hole-a [translate 12 12 -1 [cylinder-n 2.5 8 64]]]
[let hole-b [translate 68 38 -1 [cylinder-n 2.5 8 64]]]
[difference [union hole-b [union hole-a bore]] plate]
`;

/** Two blocks 10 mm apart: a document the clearance oracle can measure
 *  across, and one that deposits no claim reports. */
const TWO_BLOCKS = `
[cube 10 10 10]
[translate 20 0 0 [cube 10 10 10]]
`;

const D3 = {
  number: 1,
  kind: "flat_end_mill",
  diameter: 3.0,
  flutes: 2,
  flute_length: 12.0,
  centre_cutting: true,
};

function plateJob(documentId: string): Json {
  return {
    document_id: documentId,
    name: "plate",
    stock: { thickness: 6.0, margin: 3.0, spoilboard: 6.0 },
    machine: { name: "Anolex Ultra 2", spindle: "dial" },
    tools: [D3],
    operations: [
      {
        name: "bore",
        tool: 1,
        kind: "contour_inside",
        contour: "hole:0",
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        bottom_allowance: 0.3,
      },
      {
        name: "profile",
        tool: 1,
        kind: "contour_outside",
        contour: "outer",
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        tabs: 3,
        tab_width: 5.0,
        tab_height: 1.0,
        bottom_allowance: 0.3,
        lead_in: false,
      },
    ],
  };
}

/** The 20 T module-1.0 planet the first milestone is measured against. */
function planetGear(documentId: string): Json {
  return {
    document_id: documentId,
    gear: { module: 1.0, teeth: 20, backlash_thinning: 0.03 },
    cutter_diameter: 1.0,
    pin_diameter: 1.4,
    contours: false,
  };
}

async function makeDoc(client: Client, source = PLATE): Promise<string> {
  const created = await ok(client, "create_cad_loon", { source });
  const id = created.document_id as string;
  expect(typeof id).toBe("string");
  return id;
}

/** The unified receipt's claims, from build_receipt. */
async function receiptClaims(client: Client, documentId: string): Promise<Claim[]> {
  const body = await ok(client, "build_receipt", { document_id: documentId });
  const unified = body.unified as { claims?: Claim[] } | undefined;
  expect(unified, `no unified receipt in ${JSON.stringify(body).slice(0, 400)}`).toBeTruthy();
  return unified?.claims ?? [];
}

function find(claims: Claim[], id: string): Claim {
  const c = claims.find((x) => x.id === id);
  expect(
    c,
    `no claim ${id} among [${claims.map((x) => x.id).join(", ")}]`,
  ).toBeTruthy();
  return c as Claim;
}

/** The unified receipt's basis-aware rollup, as receipt-unified computes it. */
function rollup(claims: Claim[]): string {
  if (claims.some((c) => c.verdict === "fail")) return "fail";
  if (claims.length === 0 || claims.some((c) => c.verdict === "unverifiable")) {
    return "unverifiable";
  }
  return claims.some((c) => (c.basis ?? "verified") === "predicted")
    ? "provisional"
    : "pass";
}

describe.skipIf(!ready)("the claim-family registry", () => {
  beforeEach(() => documents.clear());

  it("lists the families this kernel build actually carries", () => {
    const families = engine.receiptFamilies<{
      families?: Array<{ schema: string; domain: string; stale_aware: boolean }>;
    }>().families;
    expect(Array.isArray(families)).toBe(true);
    const schemas = (families ?? []).map((f) => f.schema);
    // CAM is the family this work package exists for; the others prove the
    // registry is a registry rather than a CAM special case.
    expect(schemas).toContain("vcad.cam-claims/1");
    expect(schemas).toContain("vcad.thermal-claims/1");
    expect(schemas).toContain("vcad.tolerance-claims/1");
    const cam = (families ?? []).find((f) => f.schema === "vcad.cam-claims/1");
    expect(cam?.domain).toBe("cam");
    expect(cam?.stale_aware, "CAM claims name the inputs they rest on").toBe(true);

    // A schema nobody registered is an error, never an empty claim list: an
    // empty list would let a receipt roll up as if the family had been run.
    const miss = engine.receiptClaimsFor<{ error?: string }>(
      "vcad.astrology-claims/1",
      "{}",
    );
    expect(typeof miss.error).toBe("string");
    expect(miss.error).toContain("vcad.cam-claims/1");
  });

  it("carries a posted job's claims into the receipt, arithmetic verified and metal only predicted", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);

    const job = await ok(client, "cam_job", plateJob(doc));
    expect(job.blocked, JSON.stringify(job.blocked_by)).toBe(false);
    // The job says where its claims went, so an agent can find them again.
    const deposited = job.claims as Json | undefined;
    expect(deposited?.schema).toBe("vcad.cam-claims/1");
    expect(deposited?.document_id).toBe(doc);

    const claims = await receiptClaims(client, doc);
    const cam = claims.filter((c) => c.domain === "cam");
    expect(cam.length, "the receipt carries the CAM family").toBeGreaterThan(5);

    // Arithmetic on the program: it passes on verified evidence, and no
    // measurement could make it truer.
    const gouge = find(claims, "cam.job.no_gouge");
    expect(gouge.verdict).toBe("pass");
    expect(gouge.basis ?? "verified").toBe("verified");
    expect(find(claims, "cam.job.depth").verdict).toBe("pass");
    expect(find(claims, "cam.job.rapids_safe").verdict).toBe("pass");

    // A statement about metal nobody has cut: it passes its own arithmetic
    // and is marked predicted, which is what keeps the receipt honest.
    const tabs = find(claims, "cam.job.tabs_hold");
    expect(tabs.verdict).toBe("pass");
    expect(tabs.basis).toBe("predicted");

    // The mutation check the whole ladder rests on, at the MCP level: a
    // predicted claim must never let a receipt read `pass`.
    expect(rollup(cam)).not.toBe("pass");
    expect(["provisional", "unverifiable"]).toContain(rollup(cam));
    expect(
      rollup(cam.filter((c) => c.verdict !== "unverifiable")),
      "with the unverifiable claims set aside, what is left is provisional — not pass",
    ).toBe("provisional");
  });

  it("closes a gear's over-pins prediction with a reading, and violates it when the part is wrong", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);

    const gear = await ok(client, "cam_gear", planetGear(doc));
    const nominal = gear.over_pins_mm as number;
    expect(nominal).toBeGreaterThan(20);
    const subject = (gear.claims as Json).subject as string;
    expect(subject).toBe("20T-m1");

    // Before any measurement: predicted, so the receipt is provisional.
    const before = await receiptClaims(client, doc);
    const pins = find(before, "cam.gear.over_pins");
    expect(pins.verdict).toBe("pass");
    expect(pins.basis).toBe("predicted");

    // A reading 5 µm off nominal, allowed 20 µm: inside the band.
    const closed = await ok(client, "record_measurement", {
      document_id: doc,
      claim: "gear.over_pins",
      subject,
      value: Number((nominal + 0.005).toFixed(4)),
      unit: "mm",
      pin_diameter: 1.4,
      tolerance: 0.02,
      instrument: "Ø1.4 gauge pins, Mitutoyo 293-340",
    });
    expect((closed.bound as Json).status).toBe("Holds");
    expect((closed.bound as Json).basis).toBe("Measured");

    const after = await receiptClaims(client, doc);
    const bound = find(after, "cam.gear.over_pins");
    expect(bound.verdict).toBe("pass");
    expect(bound.basis, "a closed prediction stands on the part, not the model").toBe(
      "measured",
    );

    // The compensation for the NEXT part rides alongside, saying what it
    // supersedes — and is itself only predicted again, because a
    // compensation is a plan until the second part is measured too.
    const next = find(after, "cam.gear.over_pins.compensated");
    expect(next.basis).toBe("predicted");
    expect(next.details ?? "").toContain('"supersedes":"gear.over_pins"');
    expect(
      (closed.derived as unknown[]).length,
      "one cutter compensation per reading",
    ).toBe(1);
    const comp = (closed.derived as Json[])[0];
    expect(Number(comp.thickness_error)).toBeGreaterThan(0);
    expect(
      Number(comp.tool_normal_offset),
      "teeth 5 µm fat send the cutter into the metal",
    ).toBeLessThan(0);
  });

  it("violates the claim when the reading is outside the band", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);
    const gear = await ok(client, "cam_gear", planetGear(doc));
    const nominal = gear.over_pins_mm as number;

    const out = await ok(client, "record_measurement", {
      document_id: doc,
      claim: "gear.over_pins",
      // 0.2 mm over nominal against a 0.02 mm band: the teeth are fat.
      value: Number((nominal + 0.2).toFixed(4)),
      unit: "mm",
      pin_diameter: 1.4,
      tolerance: 0.02,
      instrument: "Ø1.4 gauge pins",
    });
    expect((out.bound as Json).status).toBe("Violated");

    const claims = await receiptClaims(client, doc);
    expect(find(claims, "cam.gear.over_pins").verdict).toBe("fail");
    // One failing claim fails the whole receipt, on any basis.
    expect(rollup(claims.filter((c) => c.domain === "cam"))).toBe("fail");
  });

  it("refuses a measurement aimed at arithmetic, in the wrong unit, or with nothing to decide by", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);
    await ok(client, "cam_job", plateJob(doc));

    // `job.no_gouge` is a property of the program. A caliper cannot close it,
    // and recording one would launder arithmetic as physical evidence.
    const arithmetic = await call(client, "record_measurement", {
      document_id: doc,
      claim: "job.no_gouge",
      value: 0.0,
      unit: "mm",
      pin_diameter: 1.4,
      tolerance: 0.02,
      instrument: "calipers",
    });
    expect(arithmetic.isError).toBe(true);
    expect(JSON.stringify(arithmetic.content)).toMatch(/arithmetic/i);

    const gearDoc = await makeDoc(client);
    const gear = await ok(client, "cam_gear", planetGear(gearDoc));
    const nominal = gear.over_pins_mm as number;

    // Wrong unit: an inch reading against a mm claim is a scrapped part.
    const wrongUnit = await call(client, "record_measurement", {
      document_id: gearDoc,
      claim: "gear.over_pins",
      value: nominal / 25.4,
      unit: "in",
      pin_diameter: 1.4,
      tolerance: 0.001,
      instrument: "calipers",
    });
    expect(wrongUnit.isError).toBe(true);
    expect(JSON.stringify(wrongUnit.content)).toContain("mm");

    // No tolerance: nothing to decide Holds-or-Violated by.
    const noBand = await call(client, "record_measurement", {
      document_id: gearDoc,
      claim: "gear.over_pins",
      value: nominal,
      unit: "mm",
      pin_diameter: 1.4,
      instrument: "calipers",
    });
    expect(noBand.isError).toBe(true);

    // No instrument: a reading nobody can trace is not evidence.
    const noProvenance = await call(client, "record_measurement", {
      document_id: gearDoc,
      claim: "gear.over_pins",
      value: nominal,
      unit: "mm",
      pin_diameter: 1.4,
      tolerance: 0.02,
    });
    expect(noProvenance.isError).toBe(true);

    // The claim is still Provisional after all four refusals — a refused
    // measurement must not have half-closed anything.
    const claims = await receiptClaims(client, gearDoc);
    expect(find(claims, "cam.gear.over_pins").basis).toBe("predicted");
  });

  it("turns claims Stale when the job they rest on is edited", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);
    await ok(client, "cam_job", plateJob(doc));

    const before = await receiptClaims(client, doc);
    expect(find(before, "cam.job.no_gouge").verdict).toBe("pass");

    // Edit the program the claims rest on — one coordinate, in the deposit's
    // own inputs, which is exactly what re-posting an edited job does to the
    // document. Nothing re-runs the oracle; the point is that the receipt
    // stops vouching for a program nobody holds any more.
    const session = getSession(doc) as unknown as {
      claim_reports: Array<{ inputs?: Record<string, string> }>;
    };
    const inputs = session.claim_reports[0].inputs as Record<string, string>;
    const original = inputs.program;
    expect(typeof original).toBe("string");
    inputs.program = original.replace("G21", "G21 (edited)");
    expect(inputs.program).not.toBe(original);

    const after = await receiptClaims(client, doc);
    const gouge = find(after, "cam.job.no_gouge");
    expect(
      gouge.verdict,
      "a claim about an edited program is Stale — which is unverifiable, never a pass",
    ).toBe("unverifiable");
    expect(gouge.details ?? "").toContain("program");
    // Every claim resting on the program went with it; nothing certified a
    // job that no longer exists.
    const cam = after.filter((c) => c.domain === "cam");
    expect(cam.every((c) => c.verdict === "unverifiable")).toBe(true);
    expect(rollup(cam)).toBe("unverifiable");
  });

  it("changes nothing for a document that deposited no reports", async () => {
    const { client } = await connect();
    const doc = await makeDoc(client);

    // No PCB, no clearance specs, no constraints, no claim reports: the same
    // refusal this tool has always given, naming what to do about it.
    const empty = await call(client, "build_receipt", { document_id: doc });
    expect(empty.isError).toBe(true);
    expect(JSON.stringify(empty.content)).toContain("nothing to certify");

    // A clearance assertion alone still produces exactly the mechanical-only
    // receipt it did before the registry existed — one claim, no cam.* ones.
    const pair = await makeDoc(client, TWO_BLOCKS);
    const roots = (getSession(pair) as unknown as { roots: Array<{ root: unknown }> })
      .roots;
    expect(roots.length, "the clearance fixture has two parts").toBe(2);
    await ok(client, "check_clearance", {
      document_id: pair,
      group_a: [String(roots[0].root)],
      group_b: [String(roots[1].root)],
      min_mm: 5.0,
      label: "gap",
    });
    const claims = await receiptClaims(client, pair);
    expect(claims.length).toBeGreaterThan(0);
    expect(claims.every((c) => c.domain !== "cam")).toBe(true);
    expect(claims.some((c) => c.id.startsWith("mech.clearance"))).toBe(true);
  });
});
