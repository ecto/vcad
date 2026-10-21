/**
 * `check_joint_range` on a small authored mechanism — the same fixture as
 * `crates/vcad-kernel-assembly/tests/joint_range.rs`, so the TS tool and the
 * Rust CLI are held to the same numbers.
 *
 * A 50 mm arm (plus a fixed tip) pivots about Z on a base. A wall stands on
 * the +Y side, so swinging positive runs into it and negative is free. A hub
 * coaxial with the pivot overlaps the arm root at every angle — a deliberate
 * zero-pose overlap the sweep must not report.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { checkJointRangeTool, sideAngles } from "../tools/joint-range.js";
import { documents, openDocument } from "../tools/session.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});
beforeEach(() => {
  documents.clear();
});

const ARM_LOON = `
[assembly
  #[[part "base" [cube 100.0 100.0 10.0] "steel"]
    [part "hub"  [translate 0.0 5.0 30.0 [cylinder-n 8.0 10.0 64]] "steel"]
    [part "wall" [translate -20.0 30.0 20.0 [cube 80.0 10.0 30.0]] "steel"]
    [part "arm"  [cube 50.0 10.0 10.0] "aluminum"]
    [part "tip"  [translate 50.0 0.0 0.0 [cube 5.0 10.0 10.0]] "aluminum"]]
  #[[instance "base_i" "base" -50.0 -50.0 0.0]
    [instance "hub_i"  "hub"  0.0 0.0 0.0]
    [instance "wall_i" "wall" 0.0 0.0 0.0]
    [instance "arm_i"  "arm"  0.0 0.0 0.0]
    [instance "tip_i"  "tip"  0.0 0.0 0.0]]
  #[[fixed-joint "fix_hub" "base_i" 50.0 50.0 0.0 "hub_i" 0.0 0.0 0.0]
    [fixed-joint "fix_wall" "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]
    [revolute-joint "swing" 0.0 0.0 1.0 -90.0 90.0 "base_i" 50.0 55.0 30.0 "arm_i" 0.0 0.0 0.0]
    [fixed-joint "fix_tip" "arm_i" 0.0 0.0 0.0 "tip_i" 0.0 0.0 0.0]]
  "base_i"]
`;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

function openArm(src = ARM_LOON): string {
  const doc = engine.evalVcadSource(src);
  return out(openDocument({ initial: doc })).document_id as string;
}

describe("check_joint_range", () => {
  it("finds the wall on the positive side only, ignoring the hub baseline", () => {
    const id = openArm();
    const res = out(checkJointRangeTool({ document_id: id }, engine));
    expect(res.joints).toHaveLength(1);
    const r = res.joints[0];
    expect(r.name).toBe("swing");
    expect(r.moving).toEqual(["arm_i", "tip_i"]);
    expect(r.baseline_overlap_mm3).toBeGreaterThan(100);

    expect(r.negative.first_collision_deg).toBeNull();
    expect(r.negative.free_to_deg).toBe(-90);

    const hit = r.positive.first_collision_deg;
    expect(hit).toBeGreaterThanOrEqual(15);
    expect(hit).toBeLessThanOrEqual(35);
    expect(r.positive.free_to_deg).toBe(hit - 5);
    expect(r.positive.hits[0].stationary).toBe("wall_i");
    expect(["arm_i", "tip_i"]).toContain(r.positive.hits[0].moving);
  });

  it("chain scope ignores geometry hung off another joint", () => {
    const src = ARM_LOON.replace(
      `[fixed-joint "fix_wall" "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]`,
      `[revolute-joint "wall_hinge" 1.0 0.0 0.0 -10.0 10.0 "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]`,
    );
    const id = openArm(src);
    const chain = out(checkJointRangeTool({ document_id: id, joints: ["swing"], scope: "chain" }, engine));
    expect(chain.joints[0].positive.first_collision_deg).toBeNull();
    const all = out(checkJointRangeTool({ document_id: id, joints: ["swing"] }, engine));
    expect(all.joints[0].positive.first_collision_deg).not.toBeNull();
  });

  it("rejects unknown and non-revolute joints", () => {
    const id = openArm();
    const bad = checkJointRangeTool({ document_id: id, joints: ["nope"] }, engine);
    expect(bad.isError).toBe(true);
    expect(bad.content[0].text).toContain("swing");
    expect(checkJointRangeTool({ document_id: id, joints: ["fix_tip"] }, engine).isError).toBe(true);
  });

  it("rotating about a coaxial motor can is not a collision", () => {
    const id = openArm(`
[assembly
  #[[part "motor" [rotate -90.0 0.0 0.0 [translate 0 0 -32.0 [cylinder-n 44.0 64.0 96]]] "steel"]
    [part "neck"  [translate 5.0 -54.128 39.0 [translate 0 0 -7.1 [cylinder-n 41.25 14.2 96]]] "steel"]]
  #[[instance "motor_i" "motor" 0.0 0.0 0.0]
    [instance "neck_i"  "neck"  0.0 0.0 0.0]]
  #[[revolute-joint "pitch" 0.0 1.0 0.0 -90.0 90.0 "motor_i" 0.0 0.0 0.0 "neck_i" 0.0 0.0 0.0]]
  "motor_i"]`);
    const r = out(checkJointRangeTool({ document_id: id }, engine)).joints[0];
    expect(r.baseline_overlap_mm3).toBeGreaterThan(1000);
    expect(r.negative.first_collision_deg).toBeNull();
    expect(r.positive.first_collision_deg).toBeNull();
  });

  it("samples every step plus the limit itself", () => {
    expect(sideAngles(12, 5)).toEqual([5, 10, 12]);
    expect(sideAngles(-10, 5)).toEqual([-5, -10]);
    expect(sideAngles(0, 5)).toEqual([]);
  });
});
