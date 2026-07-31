/**
 * Gym-style physics simulation tools for RL training.
 *
 * These tools provide a gym-like interface for simulating robot assemblies
 * with physics, enabling reinforcement learning training.
 *
 * Uses the phyz physics engine via WASM bindings.
 */

import { randomBytes } from "node:crypto";
import type { Document } from "@vcad/ir";
import {
  PhysicsEnv,
  isPhysicsAvailable,
  type PhysicsObservation,
  type PhysicsStepResult,
  type PhysicsActionType,
} from "@vcad/engine";
import { getSession, registerSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** Observation from the robot environment (re-export for API compatibility) */
export type Observation = PhysicsObservation;

/** Step result from the environment (re-export for API compatibility) */
export type StepResult = PhysicsStepResult;

/** MCP tool result for the gym tools. Error paths set `isError: true` so hosts
 *  (and the central next_actions enrichment) treat them as failures rather than
 *  reading a `{"error": ...}` body as a successful result. `structuredContent`
 *  carries the env/document handles for hosts (ChatGPT shim) that deliver ONLY
 *  structuredContent to the widget. */
type GymResult = {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
  structuredContent?: Record<string, unknown>;
};

/** In-memory storage for active simulations */
const simulations = new Map<string, PhysicsEnv>();

/** Look up an active env by id. Returns null if not found.
 *  Exposed for sibling tools (e.g. record_simulation) that need to drive
 *  an already-created env without re-creating it. */
export function getSimulation(envId: string): PhysicsEnv | null {
  return simulations.get(envId) ?? null;
}

/** Max trajectory entries retained per env — matches record_simulation's
 *  MAX_STEPS so a replay never exceeds what the GIF path would render. */
const MAX_TRAJECTORY = 600;

/** Replay record for a single env: the source assembly plus the rolling
 *  joint-trajectory ring buffer the inline viewer's playback UI reads via
 *  get_sim_replay / get_sim_version. */
export interface EnvRecord {
  /** The assembly the env was created from — replay FK re-poses a clone. */
  document: Document;
  /** Session id the env is bound to, so the viewer can fetch its geometry via
   *  get_preview_glb. The caller's own `document_id` when one was passed;
   *  otherwise a session freshly registered from the inline IR. */
  documentId: string;
  /** End-effector instance ids, in the order the caller passed them — the
   *  labeling for `end_effector_poses` rows. */
  endEffectorIds: string[];
  /** Per-step joint positions (degrees/mm), oldest first, capped at 600. */
  trajectory: number[][];
  /** Per-step rewards, index-aligned with `trajectory`. */
  rewards: number[];
  /** Per-step done flags, index-aligned with `trajectory`. */
  dones: boolean[];
  /** Total steps since the last reset. Keeps increasing after the ring buffer
   *  starts dropping old entries, so it doubles as a cheap change token. */
  stepCount: number;
  /** Monotonically increasing reset counter (gym_reset). Folded into the
   *  replay version token so an equal-length rollout AFTER a reset still
   *  changes the token — stepCount alone rewinds to 0 on reset and would
   *  collide with the previous episode at the same length. */
  resetEpoch: number;
  /** Simulation timestep in seconds. */
  dt: number;
  /** Physics substeps per step. */
  substeps: number;
  /** Observation-order joint id list captured from the env at creation, so
   *  replay FK maps `trajectory[row][i]` onto the doc joint with id
   *  `jointIds[i]` (via resolveObservationJoints) rather than by position.
   *  Null when the loaded kernel WASM predates `jointIds()` — replay then
   *  falls back to positional order, matching the shipped assumption. */
  jointIds: string[] | null;
}

/** Replay records keyed by env_id, populated by create_robot_env. */
const envRecords = new Map<string, EnvRecord>();

/** Look up an env's replay record. Returns null if not found.
 *  Exposed for the sim-replay tools (get_sim_replay / get_sim_version) that
 *  serve the inline viewer's playback UI. */
export function getEnvRecord(envId: string): EnvRecord | null {
  return envRecords.get(envId) ?? null;
}

/** In-memory storage for batch simulation groups */
interface BatchGroup {
  envs: PhysicsEnv[];
  actionDim: number;
  document: Document;
  options: {
    endEffectorIds: string[];
    dt?: number;
    substeps?: number;
    maxSteps?: number;
  };
  /** Observation-order joint ids captured at creation (null on kernel builds
   *  predating `jointIds()`), used to label batch observations. */
  jointIds: string[] | null;
  /** Observation slots per joint, in `jointIds` order (null on kernel builds
   *  predating `jointSlotCounts()`). Needed to split multi-DOF joints out of
   *  a batch observation. */
  jointSlotCounts: number[] | null;
}
const batchGroups = new Map<string, BatchGroup>();

/** Description shared by the two env-creating tools' `document` arg, so both
 *  advertise the same session-first contract as the rest of the surface. */
const INLINE_DOC_DESC =
  "Inline Document IR describing the robot assembly, used instead of a " +
  "session. Use this stateless path when no `document_id` is resident " +
  "(e.g. a cold serverless instance). Exactly one of `document_id` or " +
  "`document` must be given.";

/** Shared ground-plane schema fragment for the two env-creating tools. */
const GROUND_SCHEMA_PROPS = {
  ground_enabled: {
    type: "boolean" as const,
    description:
      "Ground-plane contact between robot collision shapes and a horizontal " +
      "plane at z = ground_height. Default: true. Set false for the old " +
      "contact-free dynamics (bodies fall forever).",
  },
  ground_height: {
    type: "number" as const,
    description: "Ground plane height in meters (default: 0)",
  },
  ground_friction: {
    type: "number" as const,
    description: "Ground Coulomb friction coefficient (default: 0.8)",
  },
  ground_restitution: {
    type: "number" as const,
    description:
      "Ground restitution: 0 = inelastic rest, 1 = elastic bounce (default: 0)",
  },
};

/** Ground-config args shared by create_robot_env and batch_create_envs. */
interface GroundArgs {
  ground_enabled?: boolean;
  ground_height?: number;
  ground_friction?: number;
  ground_restitution?: number;
}

/** Fold the flat ground_* args into the engine's ground options, or undefined
 *  when none were passed (the engine then applies its own defaults). */
function resolveGroundOptions(args: GroundArgs) {
  if (
    args.ground_enabled === undefined &&
    args.ground_height === undefined &&
    args.ground_friction === undefined &&
    args.ground_restitution === undefined
  ) {
    return undefined;
  }
  return {
    enabled: args.ground_enabled,
    height: args.ground_height,
    friction: args.ground_friction,
    restitution: args.ground_restitution,
  };
}

const SESSION_DOC_DESC =
  "Session id of the assembly to simulate (from open_document / " +
  "create_cad_loon). Preferred: the env binds to this same session, so no " +
  "IR round-trip is needed and the sim can never run against a stale copy.";

/** Resolve the assembly for an env-creating tool from either a resident
 *  session id or inline IR. Fails closed — naming the offending combination —
 *  when both or neither are supplied, so an agent can never quietly simulate
 *  a copy that has drifted from the session it also named.
 *
 *  Unlike `resolveDocInput` (which prefers `document_id` and ignores a
 *  redundant inline `document`), this is strict: passing both is ambiguous
 *  here because the returned `document_id` — what the replay viewer and
 *  get_preview_glb bind to — differs between the two paths. */
function resolveEnvDocument(args: {
  document_id?: unknown;
  document?: unknown;
}): {
  doc: Document;
  /** Non-null only on the session path — the inline path registers its
   *  session after the env is successfully created, so a failed create can't
   *  leave an orphan session behind. */
  documentId: string | null;
  source: "session" | "inline";
} {
  const id = typeof args.document_id === "string" ? args.document_id : "";
  const inline =
    args.document && typeof args.document === "object" ? args.document : null;

  if (id && inline) {
    throw new Error(
      "Pass either `document_id` or `document`, not both — they resolve to " +
        "different sessions and the inline copy may be stale. Drop `document` " +
        `to simulate session "${id}" in place.`,
    );
  }
  if (!id && !inline) {
    throw new Error(
      "Pass `document_id` (from open_document) to simulate a resident " +
        "session — or an inline `document` object for the stateless flow. " +
        "Exactly one is required.",
    );
  }
  if (id) {
    // Throws its own pinned "Unknown document_id" error when not resident.
    return { doc: getSession(id), documentId: id, source: "session" };
  }
  return { doc: inline as Document, documentId: null, source: "inline" };
}

/** A joint's observation entries, keyed by joint id so the caller never has to
 *  reconstruct the positional contract from `doc.joints`. */
interface LabeledJoint {
  id: string;
  /** First (and for single-DOF joints, only) position slot. */
  position: number;
  /** First (and for single-DOF joints, only) velocity slot. */
  velocity: number;
  /** Every position slot this joint owns — present only for multi-DOF joints
   *  (Ball 3, Free 6), where `position` alone would hide the rest. */
  positions?: number[];
  /** Every velocity slot this joint owns; see `positions`. */
  velocities?: number[];
}

/** An end effector's pose, keyed by the instance id it was requested under. */
interface LabeledEndEffector {
  id: string;
  pose: [number, number, number, number, number, number, number];
}

/** An observation with the bare arrays retained (unchanged wire contract) plus
 *  id-keyed views of the same numbers. */
type LabeledObservation = PhysicsObservation & {
  joint_ids?: string[] | null;
  joints?: LabeledJoint[];
  end_effectors?: LabeledEndEffector[];
};

/**
 * Attach id-keyed views to an observation.
 *
 * The bare `joint_positions` / `joint_velocities` / `end_effector_poses`
 * arrays are left exactly as-is — existing callers and the replay ring buffer
 * still read them positionally. What's added is the labeling those arrays
 * silently assumed: `joints[i].id` names the joint whose numbers sit at index
 * i, and `end_effectors[i].id` names the instance whose 7-float pose does.
 * A caller that reads only the labeled views cannot mis-attribute a value by
 * forgetting the order it passed `end_effector_ids` in.
 *
 * `jointIds` is null on kernel builds predating `jointIds()`; the labeled
 * `joints` view is then omitted rather than guessed, and `joint_ids: null`
 * marks the gap explicitly.
 *
 * A joint owns a *slice* of the observation, not a single entry:
 * `slotCounts[i]` consecutive values (Fixed 1, Revolute / Slider /
 * Cylindrical 1, Ball 3, Free 6). Walking that cursor is what keeps the
 * labeled view correct for multi-DOF joints — comparing total lengths
 * instead would silently drop the whole view for any env holding a Ball or
 * Free joint. When `slotCounts` is null (kernel predating
 * `jointSlotCounts()`), fall back to one slot per joint, which is exact
 * whenever the totals agree and is skipped otherwise.
 */
export function labelObservation(
  obs: PhysicsObservation,
  jointIds: string[] | null,
  endEffectorIds: string[],
  slotCounts?: number[] | null,
): LabeledObservation {
  const labeled: LabeledObservation = { ...obs, joint_ids: jointIds };
  if (jointIds) {
    const counts =
      slotCounts && slotCounts.length === jointIds.length
        ? slotCounts
        : jointIds.map(() => 1);
    const total = counts.reduce((a, b) => a + b, 0);
    // Only label when the cursor tiles the arrays exactly; a mismatch means
    // the slot metadata and the observation disagree, and a partial labeling
    // would mis-attribute values.
    if (
      total === obs.joint_positions.length &&
      total === obs.joint_velocities.length
    ) {
      let cursor = 0;
      labeled.joints = jointIds.map((id, i) => {
        const n = counts[i];
        const joint: LabeledJoint = {
          id,
          position: obs.joint_positions[cursor],
          velocity: obs.joint_velocities[cursor],
        };
        if (n > 1) {
          joint.positions = obs.joint_positions.slice(cursor, cursor + n);
          joint.velocities = obs.joint_velocities.slice(cursor, cursor + n);
        }
        cursor += n;
        return joint;
      });
    }
  }
  if (endEffectorIds.length === obs.end_effector_poses.length) {
    labeled.end_effectors = endEffectorIds.map((id, i) => ({
      id,
      pose: obs.end_effector_poses[i],
    }));
  }
  return labeled;
}

/** JSON Schema for create_robot_env input */
export const createRobotEnvSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: SESSION_DOC_DESC,
    },
    document: {
      type: "object" as const,
      description: INLINE_DOC_DESC,
    },
    end_effector_ids: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Instance IDs to track as end effectors",
    },
    dt: {
      type: "number" as const,
      description: "Simulation timestep in seconds (default: 1/240)",
    },
    substeps: {
      type: "number" as const,
      description: "Number of physics substeps per step (default: 4)",
    },
    max_steps: {
      type: "number" as const,
      description: "Maximum episode length (default: 1000)",
    },
    joint_gains: {
      type: "object" as const,
      description:
        'Per-joint PD gains keyed by joint id, e.g. { "knee": { "kp": 200, "kd": 8 } }. ' +
        "Overrides the inertia-scaled defaults for position/velocity servos on those joints. " +
        "Units are physics units: N\u00b7m/rad and N\u00b7m\u00b7s/rad for revolute, N/m and N\u00b7s/m for prismatic. " +
        "`config.randomization.pd_gain_scale` still multiplies these, and a joint's " +
        "effort_limit still caps the torque they produce.",
      additionalProperties: {
        type: "object" as const,
        properties: {
          kp: { type: "number" as const },
          kd: { type: "number" as const },
        },
        required: ["kp", "kd"],
      },
    },
    config: {
      type: "object" as const,
      description:
        "Optional env config for sim2real training. " +
        "`randomization`: seeded domain randomization applied on every reset — " +
        "{mass_scale: {min, max} (per-link multiplicative, e.g. 0.9–1.1), " +
        "friction_scale: {min, max} (per-joint friction/damping), " +
        "pd_gain_scale: {min, max} (global PD gain), " +
        "action_latency_steps: [min, max] (actuator delay in physics substeps, " +
        "e.g. [2, 8]), joint_pos_perturb / joint_vel_perturb (uniform ± initial " +
        "state, deg/mm)}. " +
        "`observation_noise`: gaussian std-devs {joint_pos_std, joint_vel_std, " +
        "base_pos_std, base_rot_std, base_vel_std}. " +
        "`termination`: {base_height_below (m), base_tilt_above_deg, " +
        "terminate_on_joint_limit}. " +
        "`base_instance_id`: instance used for base pose/velocity observations " +
        "(default: the ground instance).",
    },
    ...GROUND_SCHEMA_PROPS,
  },
  required: ["end_effector_ids"],
};

/** JSON Schema for gym_step input */
export const gymStepSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID returned by create_robot_env",
    },
    action_type: {
      type: "string" as const,
      enum: ["torque", "position", "velocity"],
      description: "Type of action to apply",
    },
    values: {
      type: "array" as const,
      items: { type: "number" as const },
      description:
        "Action values for each joint (Nm for torque, degrees/mm for position, deg/s or mm/s for velocity)",
    },
  },
  required: ["env_id", "action_type", "values"],
};

/** JSON Schema for gym_reset input */
export const gymResetSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID returned by create_robot_env",
    },
    seed: {
      type: "number" as const,
      description:
        "Optional new random seed. Re-seeds the domain-randomization stream " +
        "(episode counter rewinds), so identical seeds reproduce identical " +
        "randomized episodes.",
    },
  },
  required: ["env_id"],
};

/** JSON Schema for gym_observe input */
export const gymObserveSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID returned by create_robot_env",
    },
  },
  required: ["env_id"],
};

/** JSON Schema for gym_close input */
export const gymCloseSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID to close",
    },
  },
  required: ["env_id"],
};

/** Create a new robot simulation environment */
export async function createRobotEnv(input: unknown): Promise<GymResult> {
  const args = input as {
    document_id?: string;
    document?: Document;
    end_effector_ids: string[];
    dt?: number;
    substeps?: number;
    max_steps?: number;
    joint_gains?: Record<string, { kp: number; kd: number }>;
    config?: import("@vcad/engine").PhysicsEnvConfig;
  } & GroundArgs;

  // Check if physics is available
  const available = await isPhysicsAvailable();
  if (!available) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: "Physics simulation not available. WASM must be compiled with --features physics",
          }),
        },
      ],
      isError: true,
    };
  }

  try {
    const envId = `sim_${randomBytes(12).toString("base64url")}`;

    // Session-first: a `document_id` binds the env to that live session (no IR
    // round-trip, no stale copy). Inline IR registers a new session so the
    // inline viewer still has geometry to fetch.
    const { doc, documentId: sessionId, source } = resolveEnvDocument(args);

    const env = await PhysicsEnv.create(doc, {
      endEffectorIds: args.end_effector_ids,
      dt: args.dt,
      substeps: args.substeps,
      maxSteps: args.max_steps,
      config: args.config,
      ground: resolveGroundOptions(args),
      jointGains: args.joint_gains,
    });

    simulations.set(envId, env);

    // On the inline path, register the assembly as a session so the inline
    // viewer can fetch its geometry. On the session path we reuse the caller's
    // id — one session, so a later mutation of it and the env's replay
    // geometry can't diverge.
    const documentId = sessionId ?? registerSession(doc);

    // Start the replay record the viewer's playback UI reads via
    // get_sim_replay.
    envRecords.set(envId, {
      document: doc,
      documentId,
      endEffectorIds: args.end_effector_ids,
      trajectory: [],
      rewards: [],
      dones: [],
      stepCount: 0,
      resetEpoch: 0,
      dt: args.dt ?? 1 / 240,
      substeps: args.substeps ?? 4,
      jointIds: env.jointIds,
    });

    const info = {
      env_id: envId,
      // Binding contract, spelled out because two handles come back:
      //  · env_id      → gym_step / gym_reset / gym_observe / gym_close
      //  · document_id → the replay viewer, get_preview_glb, and every other
      //                  session tool (render_view, inspect_cad, update, …)
      // On the session path document_id IS the id you passed — the env and the
      // document you keep editing are the same session. On the inline path it
      // is a NEW session minted from the IR you sent.
      document_id: documentId,
      document_source: source,
      binds: {
        env_id: "gym_step, gym_reset, gym_observe, gym_close",
        document_id:
          "replay viewer, get_preview_glb, and session tools (render_view, inspect_cad, update)",
      },
      num_joints: env.numJoints,
      // Observation ordering contract: joint_positions[i] and
      // joint_velocities[i] refer to joint_ids[i]. Null when the loaded
      // kernel WASM predates jointIds().
      joint_ids: env.jointIds,
      // Action ordering contract: action values[i] drives
      // actuated_joint_ids[i] — Fixed (zero-dof) joints are excluded, so
      // action_dim can be smaller than num_joints.
      actuated_joint_ids: env.actuatedJointIds,
      action_dim: env.actionDim,
      observation_dim: env.observationDim,
      end_effector_ids: args.end_effector_ids,
      dt: args.dt ?? 1 / 240,
      substeps: args.substeps ?? 4,
      max_steps: args.max_steps ?? 1000,
      // Echo explicit gains so the caller can see which joints run custom
      // PD constants (the rest use inertia-scaled defaults).
      joint_gains: args.joint_gains ?? null,
      // Echo the env config so the caller can see what randomization /
      // noise / termination the env was armed with.
      config: args.config ?? null,
      // Ground-contact contract, echoed so the caller knows there is a
      // floor: robot collision shapes rest on the plane z = ground_height
      // instead of falling forever.
      ground: {
        enabled: args.ground_enabled ?? true,
        height: args.ground_height ?? 0,
        friction: args.ground_friction ?? 0.8,
        restitution: args.ground_restitution ?? 0,
      },
    };

    return {
      content: [{ type: "text", text: JSON.stringify(info, null, 2) }],
      // The ChatGPT shim delivers ONLY structuredContent to the widget — the
      // viewer's sim mode keys off env_id here (attachPreviewHandle merges
      // document_version on top).
      structuredContent: { env_id: envId, document_id: documentId },
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Step the simulation with an action */
export function gymStep(input: unknown): GymResult {
  const args = input as {
    env_id: string;
    action_type: PhysicsActionType;
    values: number[];
  };

  const env = simulations.get(args.env_id);
  if (!env) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
      isError: true,
    };
  }

  try {
    const result = env.step(args.action_type, args.values);

    // Append to the replay ring buffer. stepCount keeps the monotonic total
    // so the viewer's change token still advances once old entries drop.
    const record = envRecords.get(args.env_id);
    if (record) {
      record.trajectory.push([...result.observation.joint_positions]);
      record.rewards.push(result.reward);
      record.dones.push(result.done);
      record.stepCount += 1;
      if (record.trajectory.length > MAX_TRAJECTORY) {
        record.trajectory.shift();
        record.rewards.shift();
        record.dones.shift();
      }
    }

    const labeled = {
      ...result,
      observation: labelObservation(
        result.observation,
        env.jointIds,
        record?.endEffectorIds ?? [],
        env.jointSlotCounts,
      ),
    };

    return {
      content: [{ type: "text", text: JSON.stringify(labeled, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Reset the environment to initial state */
export function gymReset(input: unknown): GymResult {
  const args = input as { env_id: string; seed?: number };

  const env = simulations.get(args.env_id);
  if (!env) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
      isError: true,
    };
  }

  try {
    const observation = env.reset(args.seed);

    // A reset starts a fresh episode — drop the recorded rollout and bump
    // the epoch so the replay version token can't collide with an
    // equal-length rollout from the previous episode.
    const record = envRecords.get(args.env_id);
    if (record) {
      record.trajectory.length = 0;
      record.rewards.length = 0;
      record.dones.length = 0;
      record.stepCount = 0;
      record.resetEpoch += 1;
    }

    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            labelObservation(
              observation,
              env.jointIds,
              record?.endEffectorIds ?? [],
              env.jointSlotCounts,
            ),
            null,
            2,
          ),
        },
      ],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Get current observation without stepping */
export function gymObserve(input: unknown): GymResult {
  const args = input as { env_id: string };

  const env = simulations.get(args.env_id);
  if (!env) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
      isError: true,
    };
  }

  try {
    const observation = env.observe();
    const record = envRecords.get(args.env_id);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            labelObservation(
              observation,
              env.jointIds,
              record?.endEffectorIds ?? [],
              env.jointSlotCounts,
            ),
            null,
            2,
          ),
        },
      ],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Close and clean up a simulation environment */
export function gymClose(input: unknown): GymResult {
  const args = input as { env_id: string };

  const env = simulations.get(args.env_id);
  if (env) {
    env.close();
    simulations.delete(args.env_id);
    envRecords.delete(args.env_id);
    return {
      content: [{ type: "text", text: JSON.stringify({ success: true }) }],
    };
  }

  return {
    content: [
      { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
    ],
    isError: true,
  };
}

// ── Batch simulation tools ──────────────────────────────────────────────

/** JSON Schema for batch_create_envs input */
export const batchCreateEnvsSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: SESSION_DOC_DESC,
    },
    document: {
      type: "object" as const,
      description: INLINE_DOC_DESC,
    },
    n_envs: {
      type: "number" as const,
      description: "Number of parallel environments to create",
    },
    end_effector_ids: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Instance IDs to track as end effectors",
    },
    dt: {
      type: "number" as const,
      description: "Simulation timestep in seconds (default: 1/240)",
    },
    substeps: {
      type: "number" as const,
      description: "Number of physics substeps per step (default: 4)",
    },
    max_steps: {
      type: "number" as const,
      description: "Maximum episode length (default: 1000)",
    },
    ...GROUND_SCHEMA_PROPS,
  },
  required: ["n_envs", "end_effector_ids"],
};

/** JSON Schema for batch_step input */
export const batchStepSchema = {
  type: "object" as const,
  properties: {
    batch_id: {
      type: "string" as const,
      description: "Batch ID returned by batch_create_envs",
    },
    action_type: {
      type: "string" as const,
      enum: ["torque", "position", "velocity"],
      description: "Type of action to apply",
    },
    actions: {
      type: "array" as const,
      items: {
        type: "array" as const,
        items: { type: "number" as const },
      },
      description: "Per-environment actions. Each sub-array has one value per joint.",
    },
  },
  required: ["batch_id", "action_type", "actions"],
};

/** JSON Schema for batch_reset input */
export const batchResetSchema = {
  type: "object" as const,
  properties: {
    batch_id: {
      type: "string" as const,
      description: "Batch ID returned by batch_create_envs",
    },
  },
  required: ["batch_id"],
};

/** Create N parallel simulation environments from a single assembly */
export async function batchCreateEnvs(input: unknown): Promise<GymResult> {
  const args = input as {
    document_id?: string;
    document?: Document;
    n_envs: number;
    end_effector_ids: string[];
    dt?: number;
    substeps?: number;
    max_steps?: number;
  } & GroundArgs;

  const available = await isPhysicsAvailable();
  if (!available) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: "Physics simulation not available. WASM must be compiled with --features physics",
          }),
        },
      ],
      isError: true,
    };
  }

  try {
    const batchId = `batch_${randomBytes(12).toString("base64url")}`;
    // Same session-first contract as create_robot_env. Batches mount no
    // viewer, so a resolved session id is used only to read the assembly —
    // the inline path registers nothing.
    const { doc, documentId, source } = resolveEnvDocument(args);
    const envOptions = {
      endEffectorIds: args.end_effector_ids,
      dt: args.dt,
      substeps: args.substeps,
      maxSteps: args.max_steps,
      ground: resolveGroundOptions(args),
    };

    // Create N environments in parallel
    const envPromises = Array.from({ length: args.n_envs }, () =>
      PhysicsEnv.create(doc, envOptions),
    );
    const envs = await Promise.all(envPromises);

    const actionDim = envs[0].actionDim;

    batchGroups.set(batchId, {
      envs,
      actionDim,
      document: doc,
      options: envOptions,
      jointIds: envs[0].jointIds,
      jointSlotCounts: envs[0].jointSlotCounts,
    });

    const info = {
      batch_id: batchId,
      // batch_step / batch_reset bind to batch_id. No document session is
      // minted here — a batch has no viewer; `document_id` echoes the session
      // read from, and is null on the inline path.
      document_id: documentId,
      document_source: source,
      n_envs: args.n_envs,
      action_dim: actionDim,
      observation_dim: envs[0].observationDim,
      num_joints: envs[0].numJoints,
      // Observation ordering contract, echoed so batch callers get the same
      // labeling the single-env path returns per observation.
      joint_ids: envs[0].jointIds,
      actuated_joint_ids: envs[0].actuatedJointIds,
      end_effector_ids: args.end_effector_ids,
    };

    return {
      content: [{ type: "text", text: JSON.stringify(info, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Step all environments in a batch simultaneously */
export function batchStep(input: unknown): GymResult {
  const args = input as {
    batch_id: string;
    action_type: PhysicsActionType;
    actions: number[][];
  };

  const group = batchGroups.get(args.batch_id);
  if (!group) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown batch_id: ${args.batch_id}` }) },
      ],
      isError: true,
    };
  }

  if (args.actions.length !== group.envs.length) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `Expected ${group.envs.length} action arrays, got ${args.actions.length}`,
          }),
        },
      ],
      isError: true,
    };
  }

  try {
    const results = group.envs.map((env, i) =>
      env.step(args.action_type, args.actions[i]),
    );

    // Return compact summary: per-env observations and done flags
    const summary = {
      observations: results.map((r) =>
        labelObservation(
          r.observation,
          group.jointIds,
          group.options.endEffectorIds,
          group.jointSlotCounts,
        ),
      ),
      rewards: results.map((r) => r.reward),
      dones: results.map((r) => r.done),
    };

    return {
      content: [{ type: "text", text: JSON.stringify(summary, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

/** Reset all environments in a batch */
export function batchReset(input: unknown): GymResult {
  const args = input as { batch_id: string };

  const group = batchGroups.get(args.batch_id);
  if (!group) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown batch_id: ${args.batch_id}` }) },
      ],
      isError: true,
    };
  }

  try {
    const observations = group.envs.map((env) =>
      labelObservation(
        env.reset(),
        group.jointIds,
        group.options.endEffectorIds,
        group.jointSlotCounts,
      ),
    );
    return {
      content: [{ type: "text", text: JSON.stringify({ observations }, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
      isError: true,
    };
  }
}

export const toolDefs: ToolDef[] = [
  {
    name: "create_robot_env",
    pack: "physics",
    description:
      "Create a physics simulation environment from a vcad assembly. " +
      "Session-first: pass the `document_id` of the assembly already open — the env " +
      "binds to that same session, so there is no IR round-trip and the sim can't run " +
      "against a stale copy. Inline `document` IR is the stateless fallback; exactly " +
      "one of the two is required. " +
      "Returns env_id (for gym_step / gym_reset / gym_observe / gym_close) and " +
      "document_id (what the replay viewer and the other session tools bind to). " +
      "Optional `config` arms sim2real training: seeded domain randomization " +
      "(per-link mass, joint friction, PD gains, actuator latency, initial state), " +
      "gaussian observation noise, and termination conditions (base height/tilt, " +
      "joint limits). " +
      "A ground plane at z = 0 (friction 0.8) is on by default, so dropped bodies land " +
      "and legged assemblies can touch a floor — tune or disable it via the ground_* params. " +
      "Mounts the inline 3D viewer with a play button — gym_step rollouts replay right in the chat.",
    inputSchema: createRobotEnvSchema,
    handler: (a) => createRobotEnv(a),
    // writesDoc: the registered session must persist durably (signed-in hosted
    // users) so the mounted viewer can fetch geometry across instances — the
    // dispatch pipeline persists via effectiveDocId → structuredContent.document_id.
    behavior: behavior({ mount: true, geometry: true, writesDoc: true }),
  },
  {
    name: "gym_step",
    pack: "physics",
    description:
      "Step the physics simulation with an action. " +
      "action_type can be 'torque' (Nm), 'position' (degrees/mm), or 'velocity' (deg/s or mm/s). " +
      "Returns observation (joint positions/velocities, end effector poses, base " +
      "pose+velocity), reward, done flag, and an `info` map with per-step reward " +
      "inputs: base height/tilt, joint-limit violations, termination reason, and " +
      "the episode's sampled actuator latency. " +
      "The observation carries both the bare positional arrays and id-keyed views: " +
      "`joints[i] = {id, position, velocity}` and `end_effectors[i] = {id, pose}`, so no " +
      "ordering has to be remembered.",
    inputSchema: gymStepSchema,
    handler: (a) => gymStep(a),
    behavior: behavior({}),
  },
  {
    name: "gym_reset",
    pack: "physics",
    description:
      "Reset the simulation environment to its initial state, re-drawing any " +
      "configured domain randomization. Optional `seed` re-seeds the randomization " +
      "stream for reproducible episodes. Returns the initial observation, with " +
      "joint values keyed by joint id (`joints`) and end effector poses keyed by " +
      "instance id (`end_effectors`) alongside the bare arrays.",
    inputSchema: gymResetSchema,
    handler: (a) => gymReset(a),
    behavior: behavior({}),
  },
  {
    name: "gym_observe",
    pack: "physics",
    description:
      "Get the current observation from the simulation without stepping. " +
      "Returns joint positions, velocities, and end effector poses — both as bare " +
      "arrays and keyed by joint / instance id.",
    inputSchema: gymObserveSchema,
    handler: (a) => gymObserve(a),
    behavior: behavior({}),
  },
  {
    name: "gym_close",
    pack: "physics",
    description: "Close and clean up a simulation environment.",
    inputSchema: gymCloseSchema,
    handler: (a) => gymClose(a),
    behavior: behavior({}),
  },
  {
    name: "batch_create_envs",
    pack: "physics",
    description:
      "Create N parallel simulation environments from a single robot assembly. " +
      "Session-first: pass the `document_id` of the open assembly, or inline `document` " +
      "IR as the stateless fallback — exactly one is required. " +
      "Returns a batch_id for use with batch_step and batch_reset. " +
      "Enables parallel RL training across multiple environments.",
    inputSchema: batchCreateEnvsSchema,
    handler: (a) => batchCreateEnvs(a),
    behavior: behavior({}),
  },
  {
    name: "batch_step",
    pack: "physics",
    description:
      "Step all environments in a batch simultaneously with per-env actions. " +
      "Returns observations, rewards, and done flags for all environments. " +
      "action_type can be 'torque', 'position', or 'velocity'.",
    inputSchema: batchStepSchema,
    handler: (a) => batchStep(a),
    behavior: behavior({}),
  },
  {
    name: "batch_reset",
    pack: "physics",
    description:
      "Reset all environments in a batch to their initial state. " +
      "Returns initial observations for all environments.",
    inputSchema: batchResetSchema,
    handler: (a) => batchReset(a),
    behavior: behavior({}),
  },
];
