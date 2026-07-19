import { describe, expect, it, beforeAll } from "vitest";
import type { Document } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getKernelWasm } from "../wasm-singleton.js";
import {
  solveForwardKinematics,
  applyForwardKinematics,
} from "../kinematics.js";

beforeAll(async () => {
  await getKernelWasm();
});

/**
 * A minimal assembly: two cube instances, the first grounded, the second
 * attached by a revolute joint at 90°. Regression doc for the wrapper bug
 * where serde_wasm_bindgen's JS Map result was fed through
 * Object.entries() and always came back empty.
 */
function twoInstanceRevoluteDoc(): Document {
  const doc = createDocument();
  doc.nodes["1"] = {
    id: 1,
    name: "link",
    op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
  };
  doc.partDefs = { link: { id: "link", root: 1 } };
  doc.instances = [
    {
      id: "base",
      partDefId: "link",
      transform: {
        translation: { x: 0, y: 0, z: 0 },
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      },
    },
    {
      id: "arm",
      partDefId: "link",
      transform: {
        translation: { x: 10, y: 0, z: 0 },
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      },
    },
  ];
  doc.joints = [
    {
      id: "j1",
      parentInstanceId: "base",
      childInstanceId: "arm",
      parentAnchor: { x: 10, y: 0, z: 0 },
      childAnchor: { x: 0, y: 0, z: 0 },
      kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 } },
      state: 90,
    },
  ];
  doc.groundInstanceId = "base";
  return doc;
}

describe("solveForwardKinematics", () => {
  it("returns a non-empty Map for a 2-instance revolute assembly", () => {
    const result = solveForwardKinematics(twoInstanceRevoluteDoc());
    expect(result).toBeInstanceOf(Map);
    expect(result.size).toBe(2);

    const base = result.get("base");
    const arm = result.get("arm");
    expect(base).toBeDefined();
    expect(arm).toBeDefined();
    // Values are plain Transform3D objects, not WASM handles.
    expect(base!.translation).toEqual({ x: 0, y: 0, z: 0 });
    // The arm rotates 90° about Z around the joint anchor at (10,0,0).
    expect(arm!.rotation.z).toBeCloseTo(90);
    // The joint fully places the child: world = parent · joint(anchors, state).
    // The instance's own transform (also {10,0,0} here — the natural authoring
    // pattern) must NOT be composed on top, or the arm lands at {10,10,0}.
    expect(arm!.translation.x).toBeCloseTo(10);
    expect(arm!.translation.y).toBeCloseTo(0);
    expect(arm!.translation.z).toBeCloseTo(0);
  });

  it("applyForwardKinematics poses instances in place", () => {
    const doc = twoInstanceRevoluteDoc();
    applyForwardKinematics(doc);
    const arm = doc.instances!.find((i) => i.id === "arm")!;
    expect(arm.transform!.rotation.z).toBeCloseTo(90);
  });
});
