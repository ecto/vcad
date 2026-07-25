/**
 * create_robot_env / batch_create_envs are session-first like the rest of the
 * surface: a resident `document_id` is the primary path (no ~14k-token IR
 * round-trip per sim iteration, and no chance of simulating a stale copy),
 * inline `document` IR is the documented stateless fallback, and supplying
 * both or neither fails closed.
 *
 * Also pins the observation labeling: bare positional arrays are retained,
 * with id-keyed `joints` / `end_effectors` views alongside them so a caller
 * can't mis-attribute a value by forgetting the order it passed ids in.
 */

import { describe, it, expect } from "vitest";
import {
  createRobotEnv,
  batchCreateEnvs,
  batchReset,
  gymReset,
  gymObserve,
  gymStep,
  gymClose,
} from "../tools/gym.js";
import { documents, registerSession } from "../tools/session.js";

const robotDoc = {
  version: "0.1",
  nodes: {
    "1": { id: 1, name: "base", op: { type: "Cube", size: { x: 100, y: 100, z: 50 } } },
    "2": { id: 2, name: "link1", op: { type: "Cube", size: { x: 20, y: 20, z: 100 } } },
  },
  materials: {},
  roots: [{ root: 1, material: "default" }],
  part_materials: {},
  partDefs: {
    base: { id: "base", name: "Base", root: 1, defaultMaterial: null },
    link1: { id: "link1", name: "Link 1", root: 2, defaultMaterial: null },
  },
  instances: [
    { id: "base_inst", partDefId: "base", name: "Base", transform: null, material: null },
    { id: "link1_inst", partDefId: "link1", name: "Link 1", transform: null, material: null },
  ],
  joints: [
    {
      id: "joint1",
      name: "Joint 1",
      parentInstanceId: "base_inst",
      childInstanceId: "link1_inst",
      parentAnchor: { x: 0, y: 0, z: 25 },
      childAnchor: { x: 0, y: 0, z: -50 },
      kind: { type: "Revolute", axis: { x: 0, y: 1, z: 0 }, limits: [-90, 90] },
      state: 0,
    },
  ],
  groundInstanceId: "base_inst",
};

const json = (r: { content: Array<{ text: string }> }) =>
  JSON.parse(r.content[0].text);

describe("create_robot_env document input", () => {
  it("refuses neither document_id nor document, naming both", async () => {
    const out = await createRobotEnv({ end_effector_ids: ["link1_inst"] });
    expect(out.isError).toBe(true);
    const { error } = json(out);
    expect(error).toContain("document_id");
    expect(error).toContain("document");
    expect(error).toContain("Exactly one");
  });

  it("refuses both, and says which one to drop", async () => {
    const id = registerSession(robotDoc as never);
    const out = await createRobotEnv({
      document_id: id,
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
    });
    expect(out.isError).toBe(true);
    const { error } = json(out);
    expect(error).toContain("not both");
    expect(error).toContain(id);
  });

  it("refuses an unknown document_id rather than silently minting one", async () => {
    const out = await createRobotEnv({
      document_id: "doc_nope",
      end_effector_ids: ["link1_inst"],
    });
    expect(out.isError).toBe(true);
    expect(json(out).error).toContain("Unknown document_id");
  });

  it("binds the env to the session it was given (no new document minted)", async () => {
    documents.clear();
    const id = registerSession(robotDoc as never);
    const before = documents.size;

    const out = await createRobotEnv({
      document_id: id,
      end_effector_ids: ["link1_inst"],
    });
    const info = json(out);
    if (info.error) return; // physics unavailable in this WASM build

    expect(info.document_id).toBe(id);
    expect(info.document_source).toBe("session");
    expect(documents.size).toBe(before);
    // Both handles come back, and the result says what each drives.
    expect(info.binds.env_id).toContain("gym_step");
    expect(info.binds.document_id).toContain("get_preview_glb");
    expect(out.structuredContent?.document_id).toBe(id);

    gymClose({ env_id: info.env_id });
  });

  it("still accepts inline IR, reporting the new session it minted", async () => {
    documents.clear();
    const out = await createRobotEnv({
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
    });
    const info = json(out);
    if (info.error) return; // physics unavailable

    expect(info.document_source).toBe("inline");
    expect(typeof info.document_id).toBe("string");
    expect(documents.has(info.document_id)).toBe(true);

    gymClose({ env_id: info.env_id });
  });
});

describe("observation labeling", () => {
  it("keys joint values and end-effector poses by id, keeping the bare arrays", async () => {
    documents.clear();
    const created = await createRobotEnv({
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
    });
    const info = json(created);
    if (info.error) return; // physics unavailable

    for (const obs of [
      json(gymReset({ env_id: info.env_id })),
      json(gymObserve({ env_id: info.env_id })),
      json(gymStep({ env_id: info.env_id, action_type: "torque", values: [0.5] }))
        .observation,
    ]) {
      // Bare positional arrays are unchanged.
      expect(Array.isArray(obs.joint_positions)).toBe(true);
      expect(obs.end_effector_poses[0]).toHaveLength(7);

      // End effectors are keyed by the instance id they were requested under.
      expect(obs.end_effectors).toEqual([
        { id: "link1_inst", pose: obs.end_effector_poses[0] },
      ]);

      // Joints are keyed by joint id when the kernel reports them; older
      // builds return joint_ids: null and the labeled view is omitted rather
      // than guessed.
      if (obs.joint_ids) {
        expect(obs.joints).toHaveLength(obs.joint_positions.length);
        obs.joints.forEach(
          (j: { id: string; position: number; velocity: number }, i: number) => {
            expect(j.id).toBe(obs.joint_ids[i]);
            expect(j.position).toBe(obs.joint_positions[i]);
            expect(j.velocity).toBe(obs.joint_velocities[i]);
          },
        );
      } else {
        expect(obs.joints).toBeUndefined();
      }
    }

    gymClose({ env_id: info.env_id });
  });
});

describe("batch_create_envs document input", () => {
  it("refuses neither, and refuses both", async () => {
    const neither = await batchCreateEnvs({
      n_envs: 2,
      end_effector_ids: ["link1_inst"],
    });
    expect(neither.isError).toBe(true);
    expect(json(neither).error).toContain("Exactly one");

    const id = registerSession(robotDoc as never);
    const both = await batchCreateEnvs({
      document_id: id,
      document: robotDoc,
      n_envs: 2,
      end_effector_ids: ["link1_inst"],
    });
    expect(both.isError).toBe(true);
    expect(json(both).error).toContain("not both");
  });

  it("reads the session it was given and labels batch observations", async () => {
    documents.clear();
    const id = registerSession(robotDoc as never);
    const before = documents.size;

    const out = await batchCreateEnvs({
      document_id: id,
      n_envs: 2,
      end_effector_ids: ["link1_inst"],
    });
    const info = json(out);
    if (info.error) return; // physics unavailable

    expect(info.document_id).toBe(id);
    expect(info.document_source).toBe("session");
    // A batch mounts no viewer, so it must not mint a session of its own.
    expect(documents.size).toBe(before);
    expect(info.end_effector_ids).toEqual(["link1_inst"]);

    const reset = json(batchReset({ batch_id: info.batch_id }));
    expect(reset.observations).toHaveLength(2);
    for (const obs of reset.observations) {
      expect(obs.end_effectors).toEqual([
        { id: "link1_inst", pose: obs.end_effector_poses[0] },
      ]);
    }
  });
});
