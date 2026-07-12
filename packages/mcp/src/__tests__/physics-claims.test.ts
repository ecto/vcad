/**
 * Persisted physics specs: predict_physics + label stores a PhysicsSpec on the
 * document, build_receipt re-solves it as physics.static.<label>.* claims, and
 * verify_receipt re-verifies those claims as Holds / Stale / Violated —
 * mirroring the mech.clearance persistence loop. Fail-closed throughout: an
 * unresolvable part or a solve that cannot run is Violated/unverifiable, never
 * a silent pass.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { DesignReceipt, Document, ReceiptClaim } from "@vcad/ir";
import { predictPhysicsTool } from "../tools/physics.js";
import { buildReceipt, verifyReceipt } from "../tools/ecad.js";
import { documents, getSession, openDocument } from "../tools/session.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

/** An 80×10×10 mm aluminum cantilever beam as a single Cube part. */
function beamDocument(length: number): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "beam",
        op: { type: "Cube", size: { x: length, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "aluminum" }],
  } as unknown as Document;
}

function openBeam(length: number): string {
  const opened = out(openDocument({ initial: beamDocument(length) }));
  return opened.document_id as string;
}

/** Tip load + fixed root for a beam of the given length. */
const beamCase = (length: number, extra: Record<string, unknown>) => ({
  part: "beam",
  loads: [
    {
      region: { min: [length, 0, 0], max: [length, 10, 10] },
      force: [0, 0, -100],
    },
  ],
  supports: [{ region: { min: [0, 0, 0], max: [0, 10, 10] } }],
  ...extra,
});

describe("predict_physics label persistence", () => {
  it("persists the spec on the document and reports spec_saved", () => {
    const docId = openBeam(80);
    const res = out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    expect(res.spec_saved).toBe(true);
    expect(res.label).toBe("tip-load");
    const doc = getSession(docId);
    expect(doc.physics_specs).toHaveLength(1);
    const spec = doc.physics_specs![0];
    expect(spec.label).toBe("tip-load");
    expect(spec.part).toBe("1"); // resolved root id, survives renames
    expect(spec.fidelity).toBe("predict");
    expect(spec.max_displacement_mm).toBe(0.5);
  });

  it("upserts by label instead of duplicating", () => {
    const docId = openBeam(80);
    const args = { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) };
    out(predictPhysicsTool(args, engine));
    out(
      predictPhysicsTool(
        { ...args, max_displacement_mm: 0.4, fidelity: "verify" },
        engine,
      ),
    );
    const specs = getSession(docId).physics_specs!;
    expect(specs).toHaveLength(1);
    expect(specs[0].max_displacement_mm).toBe(0.4);
    expect(specs[0].fidelity).toBe("verify");
  });

  it("rejects a label without limits or without a document", () => {
    const docId = openBeam(80);
    expect(() =>
      predictPhysicsTool({ document_id: docId, ...beamCase(80, { label: "x" }) }, engine),
    ).toThrow(/at least one limit/);
    expect(() =>
      predictPhysicsTool(
        {
          label: "x",
          max_displacement_mm: 1,
          domain_box: { min: [0, 0, 0], max: [10, 10, 10] },
          loads: [{ region: { min: [10, 0, 0], max: [10, 10, 10] }, force: [0, 0, -1] }],
          supports: [{ region: { min: [0, 0, 0], max: [0, 10, 10] } }],
        },
        engine,
      ),
    ).toThrow(/requires `document_id`/);
  });
});

describe("build_receipt with physics specs", () => {
  async function buildFor(docId: string): Promise<DesignReceipt> {
    const res = out(await buildReceipt({ document_id: docId }, engine));
    expect(res.unified).toBeTruthy();
    return res.unified as DesignReceipt;
  }

  it("emits physics.static claims at the stored fidelity/basis", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        {
          document_id: docId,
          ...beamCase(80, {
            label: "tip-load",
            max_displacement_mm: 0.5,
            max_von_mises_mpa: 100,
          }),
        },
        engine,
      ),
    );
    const unified = await buildFor(docId);
    const phys = unified.claims.filter((c: ReceiptClaim) =>
      c.id.startsWith("physics.static."),
    );
    expect(phys.map((c) => c.id).sort()).toEqual([
      "physics.static.tip-load.displacement",
      "physics.static.tip-load.stress",
    ]);
    for (const c of phys) {
      expect(c.verdict).toBe("pass");
      expect(c.basis).toBe("predicted");
      expect(c.details).toBeTruthy(); // re-runnable payload
    }
  });

  it("fail-closed: a spec whose part vanished yields unverifiable claims", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    const doc = getSession(docId);
    doc.roots = [];
    const unified = await buildFor(docId);
    const phys = unified.claims.filter((c: ReceiptClaim) =>
      c.id.startsWith("physics.static."),
    );
    expect(phys).toHaveLength(1);
    expect(phys[0].verdict).toBe("unverifiable");
  });
});

describe("verify_receipt physics claims", () => {
  async function receiptFor(docId: string): Promise<DesignReceipt> {
    const res = out(await buildReceipt({ document_id: docId }, engine));
    return res.unified as DesignReceipt;
  }

  it("Holds on unchanged geometry", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    const unified = await receiptFor(docId);
    const res = out(await verifyReceipt({ document_id: docId, receipt: unified }, engine));
    expect(res.status).toBe("Holds");
    expect(res.physics.checks).toHaveLength(1);
    expect(res.physics.checks[0].status).toBe("Holds");
  });

  it("Stale when geometry changed but the limit still holds", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    const unified = await receiptFor(docId);
    // Thicken the beam: stiffer, deflection drops — limit still holds, and
    // the stored load region at the x=80 tip still touches the structure.
    const doc = getSession(docId);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (doc.nodes["1"] as any).op.size.z = 12;
    const res = out(await verifyReceipt({ document_id: docId, receipt: unified }, engine));
    expect(res.status).toBe("Stale");
    expect(res.physics.checks[0].status).toBe("Stale");
    expect(res.physics.checks[0].measured).toBeLessThan(res.physics.checks[0].stored);
  });

  it("Violated when the re-solve exceeds the limit", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    const unified = await receiptFor(docId);
    // Lengthen the beam far enough that tip deflection blows the 0.5 mm limit.
    const doc = getSession(docId);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (doc.nodes["1"] as any).op.size.x = 160;
    const res = out(await verifyReceipt({ document_id: docId, receipt: unified }, engine));
    expect(res.status).toBe("Violated");
    expect(res.physics.checks[0].status).toBe("Violated");
  });

  it("Violated (fail-closed) when the part can no longer be resolved", async () => {
    const docId = openBeam(80);
    out(
      predictPhysicsTool(
        { document_id: docId, ...beamCase(80, { label: "tip-load", max_displacement_mm: 0.5 }) },
        engine,
      ),
    );
    const unified = await receiptFor(docId);
    getSession(docId).roots = [];
    const res = out(await verifyReceipt({ document_id: docId, receipt: unified }, engine));
    expect(res.status).toBe("Violated");
    expect(res.physics.checks[0].reason).toMatch(/not found|empty/i);
  });

  it("Violated when a stored claim has no re-runnable payload", async () => {
    const docId = openBeam(80);
    const receipt: DesignReceipt = {
      schema: "vcad.receipt/1",
      claims: [
        {
          id: "physics.static.tip-load.displacement",
          domain: "mechanical",
          description: "tampered",
          oracle: { id: "vcad-kernel-topopt/static-fea", version: "0.9.4" },
          verdict: "pass",
        },
      ],
    };
    const res = out(await verifyReceipt({ document_id: docId, receipt }, engine));
    expect(res.status).toBe("Violated");
    expect(res.physics.checks[0].reason).toMatch(/no re-verifiable payload/);
  });
});
