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

/**
 * The four-bar-knee field report, minimized: a crank arm swinging about the
 * origin past a fixed post. Authored at 90° the arm points away from the post
 * and clears by a wide margin; at 0° it swings straight through it. Modelling
 * one pose is exactly the trap — the authored pose is the pose that works.
 */
function swingArmDocument(opts?: { limits?: [number, number] }): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  // Arm: 30 mm bar extending along +X from the pivot at the origin.
  const arm = add("arm-solid", { type: "Cube", size: { x: 30, y: 4, z: 4 } });
  // Post: a fixed obstacle straddling the arm's swept path at 0°.
  const postSolid = add("post-solid", { type: "Cube", size: { x: 6, y: 6, z: 10 } });
  const post = add("post", {
    type: "Translate",
    child: postSolid,
    offset: { x: 20, y: -3, z: -3 },
  });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [],
    partDefs: {
      arm: { id: "arm", name: "arm", root: arm },
      post: { id: "post", name: "post", root: post },
    },
    instances: [
      { id: "post-1", partDefId: "post", name: "post" },
      { id: "arm-1", partDefId: "arm", name: "arm" },
    ],
    joints: [
      {
        id: "shoulder",
        name: "shoulder",
        parentInstanceId: null,
        childInstanceId: "arm-1",
        parentAnchor: { x: 0, y: 0, z: 0 },
        childAnchor: { x: 0, y: 0, z: 0 },
        kind: {
          type: "Revolute",
          axis: { x: 0, y: 0, z: 1 },
          ...(opts?.limits ? { limits: opts.limits } : {}),
        },
        state: 90, // authored at the one angle that clears
      },
    ],
  } as unknown as Document;
}

function openSwingArm(): string {
  return out(openDocument({ initial: swingArmDocument() })).document_id as string;
}

describe("check_clearance sweep", () => {
  it("clears in the authored pose", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance(
        { document_id: docId, group_a: ["arm-1"], group_b: ["post-1"], min_mm: 1 },
        engine,
      ),
    );
    expect(res.pass).toBe(true);
    expect(res.poses_checked).toBeUndefined();
  });

  it("finds the collision the authored pose hides, and names the pose", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9 }],
        },
        engine,
      ),
    );
    expect(res.pass).toBe(false);
    expect(res.intersecting).toBe(true);
    // The kernel reports a zero penetration depth for two boxes crossing
    // face-to-face; `intersecting` — not the depth — is the load-bearing flag.
    expect(res.measured_mm).toBeLessThanOrEqual(0);
    expect(res.poses_checked).toBe(10);
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 0 }]);
  });

  it("restores joint states — a sweep asks a question, it does not pose the model", async () => {
    const docId = openSwingArm();
    await checkClearance(
      {
        document_id: docId,
        group_a: ["arm-1"],
        group_b: ["post-1"],
        min_mm: 1,
        sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 4 }],
      },
      engine,
    );
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("rejects an unknown joint and an oversized grid instead of guessing", async () => {
    const docId = openSwingArm();
    const unknown = await checkClearance(
      {
        document_id: docId,
        group_a: ["arm-1"],
        group_b: ["post-1"],
        min_mm: 1,
        sweep: [{ joint: "elbow", from: 0, to: 90, steps: 4 }],
      },
      engine,
    );
    expect(unknown.isError).toBe(true);

    const huge = await checkClearance(
      {
        document_id: docId,
        group_a: ["arm-1"],
        group_b: ["post-1"],
        min_mm: 1,
        sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9999 }],
      },
      engine,
    );
    expect(huge.isError).toBe(true);
  });

  it("persists a swept spec and re-verifies over the same range of motion", async () => {
    const docId = openSwingArm();
    const first = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          label: "knee-swing",
          sweep: [{ joint: "shoulder", from: 40, to: 90, steps: 5 }],
        },
        engine,
      ),
    );
    expect(first.pass).toBe(true);
    expect(first.spec_saved).toBe(true);

    const built = out(await buildReceipt({ document_id: docId }, engine));
    const claim = built.unified.claims.find(
      (c: { id: string }) => c.id === "mech.clearance.knee-swing",
    );
    expect(claim.verdict).toBe("pass");
    const details = JSON.parse(claim.details);
    expect(details.sweep).toHaveLength(1);
    expect(details.poses_checked).toBe(6);

    // Widen the travel on the stored spec: the swept assertion now reaches the
    // post, and re-verification must catch it rather than re-check one pose.
    const doc = getSession(docId);
    doc.clearance_specs![0].sweep = [{ joint: "shoulder", from: 0, to: 90, steps: 9 }];
    const rebuilt = out(await buildReceipt({ document_id: docId }, engine));
    const res = out(await verifyReceipt({ document_id: docId, receipt: rebuilt }, engine));
    expect(res.status).toBe("Violated");
    expect(res.clearance.checks[0].poses_checked).toBe(10);
  });

  it("returns the per-pose margin curve on request, and never persists it", async () => {
    const docId = openSwingArm();
    const plain = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          sweep: [{ joint: "shoulder", from: 30, to: 90, steps: 6 }],
        },
        engine,
      ),
    );
    expect(plain.samples).toBeUndefined();

    const withSamples = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          label: "knee-swing",
          include_samples: true,
          sweep: [{ joint: "shoulder", from: 30, to: 90, steps: 6 }],
        },
        engine,
      ),
    );
    expect(withSamples.samples).toHaveLength(7);
    expect(withSamples.samples[0].pose).toEqual([{ joint: "shoulder", state: 30 }]);
    expect(withSamples.measured_mm).toBeCloseTo(withSamples.samples[0].distance_mm, 6);
    // The spec is the assertion, not the curve.
    const spec = getSession(docId).clearance_specs![0];
    expect(spec.label).toBe("knee-swing");
    expect((spec as unknown as Record<string, unknown>).samples).toBeUndefined();

    const built = out(await buildReceipt({ document_id: docId }, engine));
    const claim = built.unified.claims.find(
      (c: { id: string }) => c.id === "mech.clearance.knee-swing",
    );
    expect(JSON.parse(claim.details).samples).toBeUndefined();
  });

  it("omits samples above the 256-pose cap and says so", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          include_samples: true,
          sweep: [{ joint: "shoulder", from: 30, to: 90, steps: 300 }],
        },
        engine,
      ),
    );
    expect(res.poses_checked).toBe(301);
    expect(res.samples).toBeUndefined();
    expect(res.samples_omitted).toBe(true);
    expect(res.samples_note).toContain("256");
  });

  it("warns instead of failing when the sweep exceeds a joint's declared limits", async () => {
    const docId = out(
      openDocument({ initial: swingArmDocument({ limits: [30, 90] }) }),
    ).document_id as string;
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9 }],
        },
        engine,
      ),
    );
    expect(res.sweep_warnings).toHaveLength(1);
    expect(res.sweep_warnings[0]).toContain("[30, 90]");
    // Reported, not clamped: the unreachable pose still drives the verdict.
    expect(res.pass).toBe(false);
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 0 }]);
  });
});

describe("check_clearance audit", () => {
  it("finds an interpenetrating pair with no pair named", async () => {
    const docId = openRotorStator(7.0);
    const res = out(await checkClearance({ document_id: docId }, engine));
    expect(res.mode).toBe("audit");
    expect(res.pass).toBe(false);
    expect(res.findings).toHaveLength(1);
    expect(res.findings[0].verdict).toBe("intersecting");
    expect(res.findings[0].distance_mm).toBeLessThan(0);
    expect(res.parts_checked).toBe(2);
    expect(res.pairs_total).toBe(1);
  });

  it("passes a document whose parts all clear", async () => {
    const docId = openRotorStator(5.0);
    const res = out(await checkClearance({ document_id: docId }, engine));
    expect(res.pass).toBe(true);
    expect(res.findings).toEqual([]);
  });

  it("honors min_mm as a proximity threshold, not just interpenetration", async () => {
    const docId = openRotorStator(5.0); // 1.0 mm gap
    const res = out(await checkClearance({ document_id: docId, min_mm: 2 }, engine));
    expect(res.pass).toBe(false);
    expect(res.findings).toHaveLength(1);
  });

  it("whitelists intended contact via ignore_pairs, and reports entries that resolve to nothing", async () => {
    const docId = openRotorStator(7.0);
    const ignored = out(
      await checkClearance({ document_id: docId, ignore_pairs: [["rotor", "stator"]] }, engine),
    );
    expect(ignored.pass).toBe(true);
    expect(ignored.pairs_ignored).toBe(1);

    const bogus = out(
      await checkClearance({ document_id: docId, ignore_pairs: [["rotor", "flywheel"]] }, engine),
    );
    expect(bogus.pass).toBe(false);
    expect(bogus.unresolved_ignores).toEqual(["flywheel"]);
  });

  it("audits the whole range of motion when swept", async () => {
    const docId = openSwingArm();
    const still = out(await checkClearance({ document_id: docId }, engine));
    expect(still.pass).toBe(true);

    const swept = out(
      await checkClearance(
        { document_id: docId, sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9 }] },
        engine,
      ),
    );
    expect(swept.pass).toBe(false);
    expect(swept.poses_checked).toBe(10);
    expect(swept.findings[0].worst_pose).toEqual([{ joint: "shoulder", state: 0 }]);
  });

  it("rejects a half-specified pair rather than silently auditing everything", async () => {
    const docId = openRotorStator(5.0);
    const res = await checkClearance({ document_id: docId, group_a: ["rotor"] }, engine);
    expect(res.isError).toBe(true);
  });
});

describe("check_clearance joint_state × sweep", () => {
  it("measures at the requested pose without touching the session", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          joint_state: { shoulder: 0 },
        },
        engine,
      ),
    );
    expect(res.pass).toBe(false);
    expect(res.intersecting).toBe(true);
    expect(res.pose.applied.shoulder).toBe(0);
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("refuses label + joint_state — a spec captured at an ad-hoc pose cannot re-verify", async () => {
    const docId = openSwingArm();
    const res = await checkClearance(
      {
        document_id: docId,
        group_a: ["arm-1"],
        group_b: ["post-1"],
        min_mm: 1,
        label: "arm-swing",
        joint_state: { shoulder: 0 },
      },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(getSession(docId).clearance_specs ?? []).toHaveLength(0);
  });

  it("sweeps on top of a joint_state pose and restores the clone, not the session", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance(
        {
          document_id: docId,
          group_a: ["arm-1"],
          group_b: ["post-1"],
          min_mm: 1,
          joint_state: { shoulder: 45 },
          sweep: [{ joint: "shoulder", from: 60, to: 90, steps: 3 }],
        },
        engine,
      ),
    );
    // The sweep drives the same joint, so it wins over the base pose — and the
    // whole 60°–90° arc clears.
    expect(res.pass).toBe(true);
    expect(res.poses_checked).toBe(4);
    expect(res.pose.applied.shoulder).toBe(45);
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("audit mode honors joint_state too", async () => {
    const docId = openSwingArm();
    const res = out(
      await checkClearance({ document_id: docId, joint_state: { shoulder: 0 } }, engine),
    );
    expect(res.mode).toBe("audit");
    expect(res.pass).toBe(false);
    expect(res.findings[0].verdict).toBe("intersecting");
    expect(res.pose.applied.shoulder).toBe(0);
  });
});
