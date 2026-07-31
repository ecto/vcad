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
}

/** Result from stepping the simulation */
export interface PhysicsStepResult {
  observation: PhysicsObservation;
  reward: number;
  done: boolean;
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
   * Ground-plane contact. Defaults to enabled at z = 0 with friction 0.8.
   * A kernel WASM predating ground contact ignores these extra constructor
   * arguments and runs contact-free, as before.
   */
  ground?: PhysicsGroundOptions;
  /**
   * Explicit per-joint PD gains keyed by joint id, overriding the
   * inertia-scaled defaults for position/velocity servos on those joints.
   * Gains are in physics units (N·m/rad and N·m·s/rad for revolute; N/m and
   * N·s/m for prismatic). A kernel WASM predating setJointGains ignores them.
   */
  jointGains?: Record<string, { kp: number; kd: number }>;
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
    // The ground-config arguments postdate some shipped kernel builds; the
    // structural cast keeps typecheck green against a checked-in .d.ts that
    // predates them, and an older WASM simply ignores the extras (running
    // contact-free, its previous behavior).
    const Sim = module.PhysicsSim as unknown as new (
      docJson: string,
      endEffectorIds: string[],
      dt: number | null,
      substeps: number | null,
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
      options.ground?.enabled ?? null,
      options.ground?.height ?? null,
      options.ground?.friction ?? null,
      options.ground?.restitution ?? null,
    );

    if (options.maxSteps) {
      sim.setMaxSteps(options.maxSteps);
    }

    if (options.jointGains && Object.keys(options.jointGains).length > 0) {
      // setJointGains postdates some shipped kernel builds; feature-detect so
      // a stale WASM silently keeps its inertia-scaled defaults.
      const maybeSetGains = (
        sim as unknown as { setJointGains?: (json: string) => void }
      ).setJointGains;
      if (typeof maybeSetGains === "function") {
        maybeSetGains.call(sim, JSON.stringify(options.jointGains));
      }
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
  reset(): PhysicsObservation {
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
