import { describe, it, expect } from "vitest";
import { createRobotEnv, gymStep, gymReset, gymClose } from "../tools/gym.js";
import { getSimReplay, getSimVersion } from "../tools/sim-replay.js";
import { documents } from "../tools/session.js";

/**
 * Golden-trajectory coverage for the inline viewer's replay tools (M1): the
 * gym ring buffer records every step, get_sim_replay serves the trajectory
 * plus per-step kernel-FK instance transforms, get_sim_version is the cheap
 * change token, and gym_reset truncates. Physics-gated like the existing gym
 * tests — the WASM build without `--features physics` skips cleanly.
 */

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

const json = (r: { content: Array<{ text: string }> }) => JSON.parse(r.content[0].text);

/** Physics availability gate (same pattern as the gym tests in tools.test.ts). */
async function createEnvOrSkip(): Promise<{ envId: string; numJoints: number } | null> {
  const result = await createRobotEnv({
    document: robotDoc,
    end_effector_ids: ["link1_inst"],
  });
  const info = json(result);
  if (info.error) return null;
  return { envId: info.env_id, numJoints: info.num_joints };
}

describe("sim replay (gym ring buffer → viewer playback)", () => {
  it("records a 5-step torque rollout with per-step FK transforms for every instance", async () => {
    documents.clear();
    const env = await createEnvOrSkip();
    if (!env) return; // physics unavailable in this WASM build

    for (let i = 0; i < 5; i++) {
      const step = gymStep({ env_id: env.envId, action_type: "torque", values: [0.5] });
      expect(step.isError ?? false).toBe(false);
    }

    const replay = json(await getSimReplay({ env_id: env.envId }));
    expect(replay.steps).toBe(5);
    expect(replay.total_steps).toBe(5);
    expect(replay.joint_trajectory).toHaveLength(5);
    for (const row of replay.joint_trajectory) {
      expect(row).toHaveLength(env.numJoints);
    }
    expect(replay.rewards).toHaveLength(5);
    expect(replay.dones).toHaveLength(5);
    expect(typeof replay.document_id).toBe("string");
    expect(replay.dt).toBeGreaterThan(0);
    expect(replay.substeps).toBeGreaterThan(0);

    // Per-step kernel FK: one transform record per step, one entry per instance.
    expect(replay.instance_transforms).toHaveLength(5);
    for (const row of replay.instance_transforms) {
      for (const instanceId of ["base_inst", "link1_inst"]) {
        expect(row[instanceId], `FK row carries ${instanceId}`).toBeDefined();
        // The viewer contract is plain [x, y, z] ARRAYS for every component —
        // the kernel's Vec3 {x,y,z} objects (or serde Maps) must have been
        // normalized server-side, or applySimFrame reads pa[0] === undefined
        // and every pose goes NaN. Regression-pinned here.
        for (const key of ["translation", "rotation", "scale"] as const) {
          const v = row[instanceId][key];
          expect(Array.isArray(v), `${instanceId}.${key} is an array`).toBe(true);
          expect(v).toHaveLength(3);
          for (const c of v) expect(typeof c).toBe("number");
        }
      }
    }

    gymClose({ env_id: env.envId });
  });

  it("version token advances with steps and gym_reset clears the rollout", async () => {
    documents.clear();
    const env = await createEnvOrSkip();
    if (!env) return;

    gymStep({ env_id: env.envId, action_type: "torque", values: [0.2] });
    const v1 = json(getSimVersion({ env_id: env.envId }));
    expect(v1.step_count).toBe(1);
    expect(typeof v1.version).toBe("string");

    gymStep({ env_id: env.envId, action_type: "torque", values: [0.2] });
    const v2 = json(getSimVersion({ env_id: env.envId }));
    expect(v2.step_count).toBe(2);
    expect(v2.version).not.toBe(v1.version);

    // A reset starts a fresh episode: the recorded rollout drops to zero.
    const reset = gymReset({ env_id: env.envId });
    expect(reset.isError ?? false).toBe(false);
    const replay = json(await getSimReplay({ env_id: env.envId }));
    expect(replay.steps).toBe(0);
    expect(replay.joint_trajectory).toHaveLength(0);
    expect(replay.instance_transforms).toHaveLength(0);
    const v3 = json(getSimVersion({ env_id: env.envId }));
    expect(v3.step_count).toBe(0);

    // Reset epoch keeps the token moving: an equal-length rollout AFTER the
    // reset must not collide with the pre-reset token at the same count —
    // otherwise the viewer would never re-fetch the new episode.
    expect(v3.reset_epoch).toBe(1);
    gymStep({ env_id: env.envId, action_type: "torque", values: [0.2] });
    gymStep({ env_id: env.envId, action_type: "torque", values: [0.2] });
    const v4 = json(getSimVersion({ env_id: env.envId }));
    expect(v4.step_count).toBe(2); // same count as v2, different episode
    expect(v4.version).not.toBe(v2.version);
    const replay2 = json(await getSimReplay({ env_id: env.envId }));
    expect(replay2.version).toBe(v4.version);
    expect(replay2.reset_epoch).toBe(1);

    gymClose({ env_id: env.envId });
  });

  it("errors on an unknown env_id (fail-closed, isError set)", async () => {
    const replay = await getSimReplay({ env_id: "sim_nope" });
    expect(replay.isError).toBe(true);
    expect(json(replay).error).toContain("Unknown env_id");

    const version = getSimVersion({ env_id: "sim_nope" });
    expect(version.isError).toBe(true);
    expect(json(version).error).toContain("Unknown env_id");
  });
});
