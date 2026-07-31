/**
 * TypeScript wrapper for the WASM physics simulation.
 *
 * Provides a clean async API for initializing and running physics simulations
 * of robot assemblies with phyz.
 */

import type { Document } from "@vcad/ir";
import type { PhysicsSim as WasmPhysicsSim } from "@vcad/kernel-wasm";

/**
 * Observation from the physics simulation.
 *
 * Joint vectors are indexed by {@link PhysicsEnv.jointIds} order — the
 * document's `joints` array order.
 */
export interface PhysicsObservation {
  /** Joint positions (degrees for revolute, mm for prismatic) */
  joint_positions: number[];
  /** Joint velocities (deg/s or mm/s) */
  joint_velocities: number[];
  /** End effector poses as [x, y, z, qw, qx, qy, qz][] */
  end_effector_poses: Array<[number, number, number, number, number, number, number]>;
  /**
   * Base pose as [x, y, z, qw, qx, qy, qz] (config `base_instance_id`,
   * defaulting to the ground instance). Absent on kernel builds predating
   * base-state observations.
   */
  base_pose?: [number, number, number, number, number, number, number];
  /** Base velocity as [vx, vy, vz, wx, wy, wz] (m/s, rad/s, world frame). */
  base_velocity?: [number, number, number, number, number, number];
}

/** Per-step diagnostics from the kernel — reward inputs for the client. */
export interface PhysicsStepInfo {
  /** Steps since the last reset. */
  step: number;
  /** Episode ended by hitting max_steps. */
  truncated: boolean;
  /** Episode ended by a termination condition. */
  terminated: boolean;
  /** Which condition fired ("base_height", "base_tilt", "joint_limit", …). */
  termination_reason?: string | null;
  /** Base origin height (meters), when a base is known. */
  base_height_m?: number | null;
  /** Base tilt from upright (degrees), when a base is known. */
  base_tilt_deg?: number | null;
  /** Joint ids currently at/past a limit. */
  joint_limit_violations: string[];
  /** This episode's sampled actuator latency in physics substeps. */
  action_latency_substeps: number;
}

/** Result from stepping the simulation */
export interface PhysicsStepResult {
  observation: PhysicsObservation;
  reward: number;
  done: boolean;
  /** Absent on kernel builds predating the info map. */
  info?: PhysicsStepInfo;
}

/** Inclusive [min, max] sampling range for domain randomization. */
export interface PhysicsRange {
  min: number;
  max: number;
}

/** Seeded domain randomization applied on every reset. */
export interface PhysicsDomainRandomization {
  /** Per-link multiplicative mass scale (e.g. {min: 0.9, max: 1.1}). */
  mass_scale?: PhysicsRange;
  /** Per-joint scale on dry friction loss + viscous damping. */
  friction_scale?: PhysicsRange;
  /** Global scale on PD motor gains, sampled once per episode. */
  pd_gain_scale?: PhysicsRange;
  /** Actuator latency in physics substeps, uniform integer [min, max]. */
  action_latency_steps?: [number, number];
  /** Uniform ± initial joint position perturbation (deg / mm). */
  joint_pos_perturb?: number;
  /** Uniform ± initial joint velocity perturbation (deg/s / mm/s). */
  joint_vel_perturb?: number;
}

/** Gaussian observation noise (std-devs; zero/absent = none). */
export interface PhysicsObservationNoise {
  joint_pos_std?: number;
  joint_vel_std?: number;
  base_pos_std?: number;
  base_rot_std?: number;
  base_vel_std?: number;
}

/** Configurable termination conditions. */
export interface PhysicsTerminationConfig {
  /** Terminate when base z drops below this (meters). */
  base_height_below?: number;
  /** Terminate when base tilt exceeds this (degrees from upright). */
  base_tilt_above_deg?: number;
  /** Terminate when any joint reaches a limit. */
  terminate_on_joint_limit?: boolean;
}

/** Env configuration: randomization, noise, termination, base instance. */
export interface PhysicsEnvConfig {
  randomization?: PhysicsDomainRandomization;
  observation_noise?: PhysicsObservationNoise;
  termination?: PhysicsTerminationConfig;
  /** Instance id used for base pose/velocity (default: ground instance). */
  base_instance_id?: string;
}

/** Action types for controlling joints */
export type ActionType = "torque" | "position" | "velocity";

/** Ground-plane contact configuration for a physics environment. */
export interface PhysicsGroundOptions {
  /** Whether ground contact is active (default: true) */
  enabled?: boolean;
  /** Ground plane height in meters — the plane is z = height (default: 0) */
  height?: number;
  /** Coulomb friction coefficient of the ground (default: 0.8) */
  friction?: number;
  /** Restitution: 0 = inelastic rest, 1 = elastic bounce (default: 0) */
  restitution?: number;
}

/** Options for creating a physics environment */
export interface PhysicsEnvOptions {
  /** Instance IDs to track as end effectors */
  endEffectorIds: string[];
  /** Simulation timestep in seconds (default: 1/240) */
  dt?: number;
  /** Number of physics substeps per step (default: 4) */
  substeps?: number;
  /** Maximum episode length (default: 1000) */
  maxSteps?: number;
  /**
   * Domain randomization / observation noise / termination config. Requires
   * a kernel WASM build that supports it (create() throws otherwise rather
   * than silently dropping the config).
   */
  config?: PhysicsEnvConfig;

  /**
   * Ground-plane contact. Defaults to enabled at z = 0 with friction 0.8.
   * A kernel WASM predating ground contact ignores these extra constructor
   * arguments and runs contact-free, as before.
   */
  ground?: PhysicsGroundOptions;
}

/**
 * Recursively convert a Map (from serde_wasm_bindgen) to a plain object.
 *
 * serde_wasm_bindgen returns Maps for objects by default, which don't
 * serialize properly with JSON.stringify. This converts them to plain objects.
 */
function mapToObject(value: unknown): unknown {
  if (value instanceof Map) {
    const obj: Record<string, unknown> = {};
    for (const [k, v] of value.entries()) {
      obj[k] = mapToObject(v);
    }
    return obj;
  }
  if (Array.isArray(value)) {
    return value.map(mapToObject);
  }
  return value;
}

/** Resolve the kernel WASM module through the shared singleton. */
async function ensureWasmLoaded(): Promise<typeof import("@vcad/kernel-wasm")> {
  const { getKernelWasm } = await import("./wasm-singleton.js");
  return getKernelWasm();
}

/**
 * Check if physics simulation is available.
 *
 * Returns true if the WASM module was compiled with the physics feature.
 */
export async function isPhysicsAvailable(): Promise<boolean> {
  try {
    const module = await ensureWasmLoaded();
    return module.isPhysicsAvailable();
  } catch {
    return false;
  }
}

/**
 * Physics simulation environment for robot assemblies.
 *
 * Wraps the WASM PhysicsSim class with a clean TypeScript API.
 */
export class PhysicsEnv {
  private sim: WasmPhysicsSim;
  private _numJoints: number;
  private _actionDim: number;
  private _observationDim: number;
  private _jointIds: string[] | null;
  private _actuatedJointIds: string[] | null;

  private constructor(sim: WasmPhysicsSim) {
    this.sim = sim;
    this._numJoints = sim.numJoints();
    this._actionDim = sim.actionDim();
    this._observationDim = sim.observationDim();
    // jointIds() postdates some shipped kernel builds; feature-detect so a
    // stale WASM degrades to jointIds === null instead of throwing. The
    // structural cast (not WasmPhysicsSim) keeps typecheck green against a
    // checked-in .d.ts that predates the binding.
    const maybeJointIds = (sim as unknown as { jointIds?: () => unknown })
      .jointIds;
    const rawJointIds =
      typeof maybeJointIds === "function" ? maybeJointIds.call(sim) : null;
    // Runtime-check the shape: if a future binding returns something other
    // than a string[] (e.g. a bare JsValue), degrade to null rather than
    // stashing a non-array that would misindex observations downstream.
    this._jointIds = Array.isArray(rawJointIds)
      ? (rawJointIds as string[])
      : null;
    // Same feature-detection story for actuatedJointIds (newer still).
    const maybeActuated = (
      sim as unknown as { actuatedJointIds?: () => unknown }
    ).actuatedJointIds;
    const rawActuated =
      typeof maybeActuated === "function" ? maybeActuated.call(sim) : null;
    this._actuatedJointIds = Array.isArray(rawActuated)
      ? (rawActuated as string[])
      : null;
  }

  /**
   * Create a new physics environment from a vcad document.
   *
   * @param document - The vcad IR document with assembly, joints, etc.
   * @param options - Configuration options
   */
  static async create(
    document: Document,
    options: PhysicsEnvOptions,
  ): Promise<PhysicsEnv> {
    const module = await ensureWasmLoaded();

    if (!module.isPhysicsAvailable()) {
      throw new Error(
        "Physics simulation not available. WASM must be compiled with --features physics",
      );
    }

    const docJson = JSON.stringify(document);
    // Both the config-JSON and ground arguments postdate some shipped kernel
    // builds; extra args are ignored by older wasm-bindgen glue. The
    // structural cast keeps typecheck green against a checked-in .d.ts that
    // may predate them. An older WASM silently runs contact-free (its
    // previous behavior) — but a dropped `config` would silently disable
    // randomization, so create() probes for a same-vintage binding
    // (resetSeeded) and fails closed instead.
    const configJson = options.config ? JSON.stringify(options.config) : null;
    const Sim = module.PhysicsSim as unknown as new (
      docJson: string,
      endEffectorIds: string[],
      dt: number | null,
      substeps: number | null,
      configJson?: string | null,
      groundEnabled?: boolean | null,
      groundHeight?: number | null,
      groundFriction?: number | null,
      groundRestitution?: number | null,
    ) => WasmPhysicsSim;
    const sim = new Sim(
      docJson,
      options.endEffectorIds,
      options.dt ?? null,
      options.substeps ?? null,
      configJson,
      options.ground?.enabled ?? null,
      options.ground?.height ?? null,
      options.ground?.friction ?? null,
      options.ground?.restitution ?? null,
    );

    if (
      configJson &&
      typeof (sim as unknown as { resetSeeded?: unknown }).resetSeeded !==
        "function"
    ) {
      throw new Error(
        "This kernel WASM build predates gym env config (domain randomization / " +
          "observation noise / termination). Rebuild the kernel WASM or drop `config`.",
      );
    }

    if (options.maxSteps) {
      sim.setMaxSteps(options.maxSteps);
    }

    return new PhysicsEnv(sim);
  }

  /** Number of joints in the simulation */
  get numJoints(): number {
    return this._numJoints;
  }

  /**
   * Joint ids in observation order (document `joints` order), or null when
   * the loaded kernel WASM predates `jointIds()`. Index `i` of
   * `joint_positions` / `joint_velocities` corresponds to `jointIds[i]`.
   */
  get jointIds(): string[] | null {
    return this._jointIds;
  }

  /**
   * Actuated joint ids in action order (document order, Fixed joints
   * excluded), or null when the loaded kernel WASM predates
   * `actuatedJointIds()`. Action vector entry `i` drives
   * `actuatedJointIds[i]`.
   */
  get actuatedJointIds(): string[] | null {
    return this._actuatedJointIds;
  }

  /** Dimension of the action space (one entry per actuated, non-Fixed joint) */
  get actionDim(): number {
    return this._actionDim;
  }

  /** Dimension of the observation space */
  get observationDim(): number {
    return this._observationDim;
  }

  /**
   * Reset the simulation to its initial state.
   *
   * @returns Initial observation
   */
  reset(seed?: number | bigint): PhysicsObservation {
    if (seed !== undefined) {
      // resetSeeded postdates some shipped kernel builds — fail closed
      // rather than silently ignoring an explicit seed.
      const resetSeeded = (
        this.sim as unknown as { resetSeeded?: (s: bigint) => unknown }
      ).resetSeeded;
      if (typeof resetSeeded !== "function") {
        throw new Error(
          "This kernel WASM build predates seeded resets. Rebuild the kernel " +
            "WASM or call reset() without a seed.",
        );
      }
      const rawObs = resetSeeded.call(this.sim, BigInt(seed));
      return mapToObject(rawObs) as PhysicsObservation;
    }
    const rawObs = this.sim.reset();
    // serde_wasm_bindgen returns a Map, convert to plain object
    return mapToObject(rawObs) as PhysicsObservation;
  }

  /**
   * Step the simulation with the given action.
   *
   * @param actionType - Type of action: "torque", "position", or "velocity"
   * @param values - Action values for each joint
   * @returns Step result with observation, reward, and done flag
   */
  step(actionType: ActionType, values: number[]): PhysicsStepResult {
    const valuesArray = new Float64Array(values);

    let rawResult: unknown;
    switch (actionType) {
      case "torque":
        rawResult = this.sim.stepTorque(valuesArray);
        break;
      case "position":
        rawResult = this.sim.stepPosition(valuesArray);
        break;
      case "velocity":
        rawResult = this.sim.stepVelocity(valuesArray);
        break;
    }

    // serde_wasm_bindgen returns a Map, convert to plain object
    const result = mapToObject(rawResult);
    return result as PhysicsStepResult;
  }

  /**
   * Get the current observation without stepping.
   */
  observe(): PhysicsObservation {
    const rawObs = this.sim.observe();
    // serde_wasm_bindgen returns a Map, convert to plain object
    return mapToObject(rawObs) as PhysicsObservation;
  }

  /**
   * Set the random seed for reproducibility.
   */
  setSeed(seed: bigint): void {
    this.sim.setSeed(seed);
  }

  /**
   * Set the maximum episode length.
   */
  setMaxSteps(maxSteps: number): void {
    this.sim.setMaxSteps(maxSteps);
  }

  /**
   * Clean up the simulation resources.
   *
   * Call this when done with the simulation to free WASM memory.
   */
  close(): void {
    this.sim.free();
  }
}
