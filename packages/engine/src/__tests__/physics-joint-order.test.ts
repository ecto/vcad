import { describe, expect, it, beforeAll } from "vitest";
import type { Document } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getKernelWasm } from "../wasm-singleton.js";
import { PhysicsEnv, isPhysicsAvailable } from "../physics.js";
import { solveForwardKinematics } from "../kinematics.js";

beforeAll(async () => {
  await getKernelWasm();
});

/**
 * Regression doc for the joint-ordering bug: PhysicsWorld::joint_ids() used
 * to iterate a HashMap, so observation slots could silently permute against
 * doc.joints for multi-joint assemblies. This chain declares its `joints`
 * array in REVERSE of the kinematic (BFS-from-ground) order, with a distinct
 * angle per joint, so any permutation is detectable.
 */
function threeJointChainDoc(): Document {
  const doc = createDocument();
  const names = ["base", "link1", "link2", "link3"];
  names.forEach((name, i) => {
    doc.nodes[String(i + 1)] = {
      id: i + 1,
      name,
      op: { type: "Cube", size: { x: 20, y: 20, z: 100 } },
    };
  });
  doc.partDefs = Object.fromEntries(
    names.map((name, i) => [name, { id: name, root: i + 1 }]),
  );
  doc.instances = names.map((name) => ({
    id: `${name}_inst`,
    partDefId: name,
    transform: {
      translation: { x: 0, y: 0, z: 0 },
      rotation: { x: 0, y: 0, z: 0 },
      scale: { x: 1, y: 1, z: 1 },
    },
  }));
  const revolute = (id: string, parent: string, child: string, state: number) => ({
    id,
    parentInstanceId: `${parent}_inst`,
    childInstanceId: `${child}_inst`,
    parentAnchor: { x: 0, y: 0, z: 50 },
    childAnchor: { x: 0, y: 0, z: -50 },
    kind: { type: "Revolute" as const, axis: { x: 0, y: 1, z: 0 } },
    state,
  });
  // Leaf-most joint first: doc order is the opposite of BFS discovery order.
  doc.joints = [
    revolute("joint3", "link2", "link3", 30),
    revolute("joint2", "link1", "link2", 20),
    revolute("joint1", "base", "link1", 10),
  ];
  doc.groundInstanceId = "base_inst";
  return doc;
}

describe("physics joint observation ordering", () => {
  it("observation slots map onto the right doc joints and instance transforms", async () => {
    if (!(await isPhysicsAvailable())) return;

    const doc = threeJointChainDoc();
    const env = await PhysicsEnv.create(doc, {
      endEffectorIds: ["link3_inst"],
    });
    try {
      // The kernel must expose its joint order, and it must be doc order.
      expect(env.jointIds).toEqual(["joint3", "joint2", "joint1"]);

      // Each observation slot carries the state of jointIds[i]: 30/20/10,
      // not any permutation.
      const obs = env.observe();
      expect(obs.joint_positions.length).toBe(3);
      expect(obs.joint_positions[0]).toBeCloseTo(30, 4);
      expect(obs.joint_positions[1]).toBeCloseTo(20, 4);
      expect(obs.joint_positions[2]).toBeCloseTo(10, 4);

      // End-to-end: write the observation back into a zeroed clone by id
      // (the record_simulation / get_sim_replay mapping) and check every
      // instance lands on the same world transform FK gives the original
      // doc. A permuted mapping bends the wrong joints and diverges.
      const clone: Document = JSON.parse(JSON.stringify(doc));
      for (const j of clone.joints!) j.state = 0;
      const byId = new Map(clone.joints!.map((j) => [j.id, j]));
      env.jointIds!.forEach((id, i) => {
        byId.get(id)!.state = obs.joint_positions[i]!;
      });

      const expected = solveForwardKinematics(doc);
      const actual = solveForwardKinematics(clone);
      expect(actual.size).toBe(expected.size);
      for (const [instId, want] of expected) {
        const got = actual.get(instId);
        expect(got, `instance ${instId} missing from FK result`).toBeDefined();
        for (const axis of ["x", "y", "z"] as const) {
          expect(got!.translation[axis]).toBeCloseTo(want.translation[axis], 4);
          expect(got!.rotation[axis]).toBeCloseTo(want.rotation[axis], 4);
        }
      }
    } finally {
      env.close();
    }
  });
});
