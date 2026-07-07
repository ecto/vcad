import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { checkClearance } from "../tools/clearance.js";
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

/**
 * Rotor/stator fixture (the PCB-motor field report): a rotor cylinder spinning
 * inside a ring stator. With `rotorRadius` 5 and a 6 mm stator bore the design
 * air gap is 1.0 mm. 128 segments keeps tessellation chord error ≪ the gap.
 */
function rotorStatorDocument(rotorRadius: number): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  // Rotor: shorter than the stator so an interference case has a clean
  // penetration depth (coincident end faces would degenerate to depth 0).
  const rotorCyl = add("rotor-solid", {
    type: "Cylinder",
    radius: rotorRadius,
    height: 8,
    segments: 128,
  });
  const rotor = add("rotor", {
    type: "Translate",
    child: rotorCyl,
    offset: { x: 0, y: 0, z: 1 },
  });

  const statorOuter = add("stator-outer", {
    type: "Cylinder",
    radius: 10,
    height: 10,
    segments: 128,
  });
  const statorBoreCyl = add("stator-bore-solid", {
    type: "Cylinder",
    radius: 6,
    height: 12,
    segments: 128,
  });
  const statorBore = add("stator-bore", {
    type: "Translate",
    child: statorBoreCyl,
    offset: { x: 0, y: 0, z: -1 },
  });
  const stator = add("stator", { type: "Difference", left: statorOuter, right: statorBore });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [
      { root: rotor, material: "steel" },
      { root: stator, material: "aluminum" },
    ],
  } as unknown as Document;
}

function openRotorStator(rotorRadius: number): string {
  const opened = out(openDocument({ initial: rotorStatorDocument(rotorRadius) }));
  return opened.document_id as string;
}

describe("check_clearance", () => {
  it("measures a 1.0mm design air gap and passes", async () => {
    const docId = openRotorStator(5.0);
    const res = out(
      await checkClearance(
        { document_id: docId, group_a: ["rotor"], group_b: ["stator"], min_mm: 0.9 },
        engine,
      ),
    );
    expect(res.pass).toBe(true);
    expect(res.intersecting).toBe(false);
    expect(Math.abs(res.measured_mm - 1.0)).toBeLessThan(0.02);
    expect(res.worst_pair.a.name).toBe("rotor");
    expect(res.worst_pair.b.name).toBe("stator");
    expect(res.pairs_checked).toBe(1);
  });

  it("fails with the measured value when the gap shrinks below the requirement", async () => {
    const docId = openRotorStator(5.6); // 0.4 mm gap
    const res = out(
      await checkClearance(
        { document_id: docId, group_a: ["rotor"], group_b: ["stator"], min_mm: 0.65 },
        engine,
      ),
    );
    expect(res.pass).toBe(false);
    expect(Math.abs(res.measured_mm - 0.4)).toBeLessThan(0.02);
  });

  it("reports negative distance (penetration depth) for intersecting parts", async () => {
    const docId = openRotorStator(7.0); // rotor pierces the stator ring
    const res = out(
      await checkClearance(
        { document_id: docId, group_a: ["rotor"], group_b: ["stator"], min_mm: 0.1 },
        engine,
      ),
    );
    expect(res.pass).toBe(false);
    expect(res.intersecting).toBe(true);
    expect(res.measured_mm).toBeLessThan(0);
    expect(Math.abs(res.measured_mm + 1.0)).toBeLessThan(0.05);
  });

  it("resolves parts by id as well as name", async () => {
    const docId = openRotorStator(5.0);
    const doc = getSession(docId);
    const [rotorRoot, statorRoot] = doc.roots.map((r) => String(r.root));
    const res = out(
      await checkClearance(
        { document_id: docId, group_a: [rotorRoot], group_b: [statorRoot], min_mm: 0.9 },
        engine,
      ),
    );
    expect(res.pass).toBe(true);
    expect(res.worst_pair.a.id).toBe(rotorRoot);
  });

  it("errors on unknown parts, listing what is available", async () => {
    const docId = openRotorStator(5.0);
    const res = await checkClearance(
      { document_id: docId, group_a: ["flux-capacitor"], group_b: ["stator"], min_mm: 1 },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("flux-capacitor");
    expect(res.content[0].text).toContain("rotor");
  });

  it("rejects a part appearing in both groups", async () => {
    const docId = openRotorStator(5.0);
    const res = await checkClearance(
      { document_id: docId, group_a: ["rotor"], group_b: ["rotor", "stator"], min_mm: 1 },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("both groups");
  });

  it("persists a labeled assertion on the document", async () => {
    const docId = openRotorStator(5.0);
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["rotor"],
          group_b: ["stator"],
          min_mm: 0.9,
          label: "air-gap",
        },
        engine,
      ),
    );
    expect(res.spec_saved).toBe(true);
    const doc = getSession(docId);
    expect(doc.clearance_specs).toHaveLength(1);
    expect(doc.clearance_specs![0].label).toBe("air-gap");
    expect(doc.clearance_specs![0].min_mm).toBe(0.9);
    // Persisted by resolved part id, so the spec survives renames.
    expect(doc.clearance_specs![0].group_a).toEqual([String(doc.roots[0].root)]);

    // Re-running the same label updates in place instead of duplicating.
    out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["rotor"],
          group_b: ["stator"],
          min_mm: 0.95,
          label: "air-gap",
        },
        engine,
      ),
    );
    expect(getSession(docId).clearance_specs).toHaveLength(1);
    expect(getSession(docId).clearance_specs![0].min_mm).toBe(0.95);
  });
});

describe("clearance receipts (build_receipt / verify_receipt)", () => {
  async function labeledSession(rotorRadius: number, minMm = 0.9): Promise<string> {
    const docId = openRotorStator(rotorRadius);
    const res = await checkClearance(
      {
        document_id: docId,
        group_a: ["rotor"],
        group_b: ["stator"],
        min_mm: minMm,
        label: "air-gap",
      },
      engine,
    );
    expect(res.isError).toBeUndefined();
    return docId;
  }

  it("build_receipt emits a mech.clearance claim for a CAD-only document", async () => {
    const docId = await labeledSession(5.0);
    const built = out(await buildReceipt({ document_id: docId }, engine));
    expect(built.unified.schema).toBe("vcad.receipt/1");
    const claim = built.unified.claims.find(
      (c: { id: string }) => c.id === "mech.clearance.air-gap",
    );
    expect(claim).toBeDefined();
    expect(claim.verdict).toBe("pass");
    expect(claim.predicted.value).toBe(0.9);
    expect(Math.abs(claim.measured.value - 1.0)).toBeLessThan(0.02);
    expect(built.unified_summary.overall).toBe("pass");
    // details carries the typed ClearanceClaim for re-verification
    const typed = JSON.parse(claim.details);
    expect(typed.holds).toBe(true);
    expect(typed.group_a).toHaveLength(1);
  });

  it("build_receipt fails the ledger when a persisted clearance is violated", async () => {
    const docId = await labeledSession(5.6, 0.65); // 0.4 mm measured vs 0.65 required
    const built = out(await buildReceipt({ document_id: docId }, engine));
    const claim = built.unified.claims.find(
      (c: { id: string }) => c.id === "mech.clearance.air-gap",
    );
    expect(claim.verdict).toBe("fail");
    expect(Math.abs(claim.measured.value - 0.4)).toBeLessThan(0.02);
    expect(built.unified_summary.overall).toBe("fail");
  });

  it("re-verifies Holds on unchanged geometry, Stale on drift, Violated on regression", async () => {
    const docId = await labeledSession(5.0, 0.9);
    const built = out(await buildReceipt({ document_id: docId }, engine));

    // Pass the whole build payload back, as an agent would.
    const holds = out(await verifyReceipt({ document_id: docId, receipt: built }, engine));
    expect(holds.status).toBe("Holds");
    expect(holds.clearance.checks[0]).toMatchObject({ label: "air-gap", status: "Holds" });

    // Nudge the rotor: still clears 0.9 mm, but the measurement moved.
    const doc = getSession(docId);
    const rotorSolid = Object.values(doc.nodes).find((n) => n.name === "rotor-solid")!;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (rotorSolid.op as any).radius = 5.05;
    const stale = out(await verifyReceipt({ document_id: docId, receipt: built }, engine));
    expect(stale.status).toBe("Stale");
    expect(Math.abs(stale.clearance.checks[0].measured_mm - 0.95)).toBeLessThan(0.02);

    // Grow the rotor past the requirement: the stored receipt is violated.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (rotorSolid.op as any).radius = 5.6;
    const violated = out(await verifyReceipt({ document_id: docId, receipt: built }, engine));
    expect(violated.status).toBe("Violated");
    expect(Math.abs(violated.clearance.checks[0].measured_mm - 0.4)).toBeLessThan(0.02);
    expect(violated.clearance.checks[0].reason).toContain("below the required");
  });

  it("re-verifies Violated (fail-closed) when an asserted part disappears", async () => {
    const docId = await labeledSession(5.0);
    const built = out(await buildReceipt({ document_id: docId }, engine));
    const doc = getSession(docId);
    doc.roots = doc.roots.slice(0, 1); // drop the stator
    const res = out(await verifyReceipt({ document_id: docId, receipt: built }, engine));
    expect(res.status).toBe("Violated");
    expect(res.clearance.checks[0].reason).toBeDefined();
  });

  it("build_receipt emits an unverifiable claim (fail-closed) for unmeasurable specs", async () => {
    const docId = await labeledSession(5.0);
    const doc = getSession(docId);
    doc.roots = doc.roots.slice(0, 1); // stator gone before the receipt is built
    const built = out(await buildReceipt({ document_id: docId }, engine));
    const claim = built.unified.claims.find(
      (c: { id: string }) => c.id === "mech.clearance.air-gap",
    );
    expect(claim.verdict).toBe("unverifiable");
    expect(built.unified_summary.overall).toBe("unverifiable");
  });

  it("build_receipt still errors on a document with neither PCB nor specs", async () => {
    const docId = openRotorStator(5.0);
    const res = await buildReceipt({ document_id: docId }, engine);
    expect(res.isError).toBe(true);
  });
});
