/**
 * `joint_state` — posing an assembly through the read/export tools.
 *
 * The contract this pins is the one that used to be done by hand: posing the
 * kinematic master via `joint_state` must produce exactly the geometry of a
 * hand-placed "stance snapshot" document, so the second document stops being
 * necessary. Plus the fail-closed rules: unknown joint keys error, and
 * out-of-limit states clamp with a warning instead of rendering a machine
 * that cannot exist.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { exportCad } from "../tools/export.js";
import { computeInspection, inspectCad } from "../tools/inspect.js";
import { applyJointState, PoseError } from "../tools/pose.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
  process.env.VCAD_MCP_EXPORT_DIR = mkdtempSync(join(tmpdir(), "vcad-pose-"));
});

/** Two parts, one revolute joint about +Y with ±90° limits. */
const ASSEMBLY_LOON = `
[assembly
  #[[part "base" [cylinder 40.0 30.0] "steel"]
    [part "arm" [cube 80.0 20.0 20.0] "aluminum"]]
  #[[instance "base-inst" "base" 0.0 0.0 0.0]
    [instance "arm-inst" "arm" 0.0 0.0 30.0]]
  #[[revolute-joint "shoulder" 0.0 1.0 0.0 -90.0 90.0
      "base-inst" 0.0 0.0 25.0
      "arm-inst" 0.0 0.0 0.0]]
  "base-inst"]
`;

function assemblyDoc(): Document {
  const doc = engine.evalVcadSource(ASSEMBLY_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

function jointId(doc: Document): string {
  const j = doc.joints?.[0];
  if (!j) throw new Error("fixture lost its joint");
  return j.id;
}

const bboxOf = (doc: Document) => computeInspection(doc, engine).bounding_box;

describe("joint_state", () => {
  it("poses the assembly exactly like a hand-placed stance snapshot", () => {
    const doc = assemblyDoc();
    const id = jointId(doc);

    // Posed via FK: the arm swings 40° about +Y at the shoulder anchor.
    const { doc: posed, pose } = applyJointState(doc, { [id]: 40 });
    expect(pose?.applied[id]).toBe(40);

    // The hand-built equivalent: no joints at all, the arm pre-rotated and
    // placed at the anchor by hand — exactly the duplicate model this
    // feature removes. world = parent_anchor - R(40°,+Y) * child_anchor,
    // and the child anchor is the origin, so the translation is the anchor.
    const hand = structuredClone(doc);
    delete hand.joints;
    const arm = hand.instances?.find((i) => i.id === "arm-inst");
    if (!arm) throw new Error("fixture lost its instance");
    arm.transform = {
      translation: { x: 0, y: 0, z: 25 },
      rotation: { x: 0, y: 40, z: 0 },
      scale: { x: 1, y: 1, z: 1 },
    };

    // The FK readout and the hand placement agree...
    const fk = pose!.transforms["arm-inst"]!;
    expect(fk.translation).toEqual({ x: 0, y: 0, z: 25 });
    expect(fk.rotation.x).toBeCloseTo(0, 9);
    expect(fk.rotation.y).toBeCloseTo(40, 9);
    expect(fk.rotation.z).toBeCloseTo(0, 9);

    // ...and so does the geometry both documents evaluate to.
    const a = bboxOf(posed);
    const b = bboxOf(hand);
    for (const corner of ["min", "max"] as const) {
      for (const axis of ["x", "y", "z"] as const) {
        expect(a[corner][axis]).toBeCloseTo(b[corner][axis], 6);
      }
    }
  });

  it("changes the measured geometry (a pose is not a no-op)", () => {
    const doc = assemblyDoc();
    const zero = bboxOf(doc);
    const { doc: posed } = applyJointState(doc, { [jointId(doc)]: 40 });
    expect(bboxOf(posed)).not.toEqual(zero);
  });

  it("leaves the input document untouched", () => {
    const doc = assemblyDoc();
    const before = JSON.stringify(doc);
    applyJointState(doc, { [jointId(doc)]: 40 });
    expect(JSON.stringify(doc)).toBe(before);
  });

  it("is a no-op when omitted", () => {
    const doc = assemblyDoc();
    const { doc: same, pose } = applyJointState(doc, undefined);
    expect(pose).toBeUndefined();
    expect(same).toBe(doc);
  });

  it("resolves a joint by name as well as by id", () => {
    const doc = assemblyDoc();
    const name = doc.joints?.[0]?.name;
    if (!name) return; // fixture carries no name — nothing to pin
    const { pose } = applyJointState(doc, { [name]: 25 });
    expect(pose?.applied[jointId(doc)]).toBe(25);
  });

  it("clamps an out-of-limits state and says so", () => {
    const doc = assemblyDoc();
    const id = jointId(doc);
    const { pose } = applyJointState(doc, { [id]: 200 });
    expect(pose?.applied[id]).toBe(90);
    expect(pose?.warnings?.join(" ")).toMatch(/clamped to 90/);
  });

  it("errors on an unknown joint key instead of silently ignoring it", () => {
    const doc = assemblyDoc();
    expect(() => applyJointState(doc, { elbow: 10 })).toThrow(PoseError);
    expect(() => applyJointState(doc, { elbow: 10 })).toThrow(/matches no joint/);
  });

  it("errors when the document has no joints to pose", () => {
    const doc = assemblyDoc();
    delete doc.joints;
    expect(() => applyJointState(doc, { shoulder: 10 })).toThrow(/no joints/);
  });

  it("errors on a non-numeric state", () => {
    const doc = assemblyDoc();
    expect(() => applyJointState(doc, { [jointId(doc)]: "up" })).toThrow(
      /not a finite number/,
    );
  });

  it("flows through inspect_cad and export_cad", () => {
    const doc = assemblyDoc();
    const id = jointId(doc);

    const inspected = JSON.parse(
      inspectCad({ document: doc, joint_state: { [id]: 40 } }, engine).content[0]!
        .text,
    ) as Record<string, unknown>;
    expect((inspected.pose as { applied: Record<string, number> }).applied[id]).toBe(40);
    expect(inspected.bounding_box).not.toEqual(bboxOf(doc));

    const exported = JSON.parse(
      exportCad(
        { document: doc, filename: "posed.stl", joint_state: { [id]: 40 } },
        engine,
      ).content[0]!.text,
    ) as Record<string, unknown>;
    expect((exported.pose as { applied: Record<string, number> }).applied[id]).toBe(40);
  });
});
