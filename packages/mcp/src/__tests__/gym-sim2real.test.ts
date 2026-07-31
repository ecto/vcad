/**
 * Sim2real gym surface: env config passthrough (domain randomization /
 * observation noise / termination) on create_robot_env, seeded gym_reset,
 * and the per-step `info` map + base state on gym_step results.
 *
 * The kernel-side behavior (reproducibility, latency, noise distribution,
 * termination logic) is pinned by Rust tests in
 * crates/vcad-kernel-physics/src/gym.rs — these tests pin the MCP wiring.
 *
 * The suite probes the loaded kernel WASM: builds predating the sim2real
 * bindings (resetSeeded / env config) skip the dependent tests instead of
 * failing, mirroring the graceful degradation the engine wrapper ships.
 */

import { describe, it, expect } from "vitest";
import {
  createRobotEnv,
  createRobotEnvSchema,
  gymReset,
  gymResetSchema,
  gymStep,
  gymClose,
} from "../tools/gym.js";

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

// Probe: does the loaded kernel WASM carry the sim2real bindings? A plain
// env is created and a seeded reset attempted; older builds fail closed with
// a "predates seeded resets" error from the engine wrapper.
const probeEnv = await createRobotEnv({
  document: robotDoc,
  end_effector_ids: ["link1_inst"],
});
const probeEnvId = probeEnv.isError ? null : json(probeEnv).env_id;
let sim2real = false;
try {
  sim2real =
    probeEnvId !== null && !gymReset({ env_id: probeEnvId, seed: 1 }).isError;
} finally {
  if (probeEnvId) gymClose({ env_id: probeEnvId });
}
if (!sim2real) {
  console.warn(
    "[gym-sim2real] loaded kernel WASM predates sim2real bindings — " +
      "skipping behavior tests (schema tests still run)",
  );
}

describe("gym sim2real schemas", () => {
  it("create_robot_env advertises the env config", () => {
    expect(createRobotEnvSchema.properties.config).toBeDefined();
    expect(createRobotEnvSchema.properties.config.description).toContain(
      "randomization",
    );
    expect(createRobotEnvSchema.required).not.toContain("config");
  });

  it("gym_reset advertises the optional seed", () => {
    expect(gymResetSchema.properties.seed).toBeDefined();
    expect(gymResetSchema.required).not.toContain("seed");
  });
});

describe.runIf(sim2real)("gym sim2real behavior", () => {
  const config = {
    randomization: {
      mass_scale: { min: 0.9, max: 1.1 },
      friction_scale: { min: 0.5, max: 2.0 },
      pd_gain_scale: { min: 0.8, max: 1.2 },
      action_latency_steps: [2, 8],
      joint_pos_perturb: 5.0,
    },
    observation_noise: { joint_pos_std: 0.1 },
    termination: { base_tilt_above_deg: 45 },
  };

  async function makeEnv() {
    const out = await createRobotEnv({
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
      config,
    });
    expect(out.isError).toBeUndefined();
    return json(out);
  }

  it("accepts and echoes the env config", async () => {
    const info = await makeEnv();
    expect(info.config).toEqual(config);
    gymClose({ env_id: info.env_id });
  });

  it("seeded resets reproduce randomized episodes", async () => {
    const { env_id } = await makeEnv();
    const a = json(gymReset({ env_id, seed: 42 }));
    const b = json(gymReset({ env_id, seed: 42 }));
    const c = json(gymReset({ env_id, seed: 43 }));
    expect(a.joint_positions).toEqual(b.joint_positions);
    expect(a.joint_positions).not.toEqual(c.joint_positions);
    gymClose({ env_id });
  });

  it("gym_step returns base state and the info map", async () => {
    const { env_id } = await makeEnv();
    gymReset({ env_id, seed: 1 });
    const result = json(
      gymStep({ env_id, action_type: "torque", values: [0.01] }),
    );
    expect(result.observation.base_pose).toHaveLength(7);
    expect(result.observation.base_velocity).toHaveLength(6);
    expect(result.info.step).toBe(1);
    expect(typeof result.info.terminated).toBe("boolean");
    expect(typeof result.info.truncated).toBe("boolean");
    expect(result.info.action_latency_substeps).toBeGreaterThanOrEqual(2);
    expect(result.info.action_latency_substeps).toBeLessThanOrEqual(8);
    expect(Array.isArray(result.info.joint_limit_violations)).toBe(true);
    expect(typeof result.info.base_height_m).toBe("number");
    expect(typeof result.info.base_tilt_deg).toBe("number");
    gymClose({ env_id });
  });
});
