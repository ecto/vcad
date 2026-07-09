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
import { registerSession } from "./session.js";
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
  /** Session id the assembly was registered under, so the viewer can fetch
   *  its geometry via get_preview_glb. */
  documentId: string;
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
}
const batchGroups = new Map<string, BatchGroup>();

/** JSON Schema for create_robot_env input */
export const createRobotEnvSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document describing the robot assembly",
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
  },
  required: ["document", "end_effector_ids"],
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
    document: Document;
    end_effector_ids: string[];
    dt?: number;
    substeps?: number;
    max_steps?: number;
  };

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

    const env = await PhysicsEnv.create(args.document, {
      endEffectorIds: args.end_effector_ids,
      dt: args.dt,
      substeps: args.substeps,
      maxSteps: args.max_steps,
    });

    simulations.set(envId, env);

    // Register the assembly as a session so the inline viewer can render it,
    // and start the replay record its playback UI reads via get_sim_replay.
    const documentId = registerSession(args.document);
    envRecords.set(envId, {
      document: args.document,
      documentId,
      trajectory: [],
      rewards: [],
      dones: [],
      stepCount: 0,
      resetEpoch: 0,
      dt: args.dt ?? 1 / 240,
      substeps: args.substeps ?? 4,
    });

    const info = {
      env_id: envId,
      document_id: documentId,
      num_joints: env.numJoints,
      // Observation/action ordering contract: joint_positions[i],
      // joint_velocities[i], and action values[i] all refer to joint_ids[i].
      // Null when the loaded kernel WASM predates jointIds().
      joint_ids: env.jointIds,
      action_dim: env.actionDim,
      observation_dim: env.observationDim,
      end_effector_ids: args.end_effector_ids,
      dt: args.dt ?? 1 / 240,
      substeps: args.substeps ?? 4,
      max_steps: args.max_steps ?? 1000,
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

    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
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
    const observation = env.reset();

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
      content: [{ type: "text", text: JSON.stringify(observation, null, 2) }],
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
    return {
      content: [{ type: "text", text: JSON.stringify(observation, null, 2) }],
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
    document: {
      type: "object" as const,
      description: "vcad IR Document describing the robot assembly",
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
  },
  required: ["document", "n_envs", "end_effector_ids"],
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
    document: Document;
    n_envs: number;
    end_effector_ids: string[];
    dt?: number;
    substeps?: number;
    max_steps?: number;
  };

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
    const envOptions = {
      endEffectorIds: args.end_effector_ids,
      dt: args.dt,
      substeps: args.substeps,
      maxSteps: args.max_steps,
    };

    // Create N environments in parallel
    const envPromises = Array.from({ length: args.n_envs }, () =>
      PhysicsEnv.create(args.document, envOptions),
    );
    const envs = await Promise.all(envPromises);

    const actionDim = envs[0].actionDim;

    batchGroups.set(batchId, {
      envs,
      actionDim,
      document: args.document,
      options: envOptions,
    });

    const info = {
      batch_id: batchId,
      n_envs: args.n_envs,
      action_dim: actionDim,
      observation_dim: envs[0].observationDim,
      num_joints: envs[0].numJoints,
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
      observations: results.map((r) => r.observation),
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
    const observations = group.envs.map((env) => env.reset());
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
      "Returns an environment ID that can be used with gym_step, gym_reset, and gym_observe. " +
      "The environment provides a gym-style interface for RL training. " +
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
      "Returns observation (joint positions/velocities, end effector poses), reward, and done flag.",
    inputSchema: gymStepSchema,
    handler: (a) => gymStep(a),
    behavior: behavior({}),
  },
  {
    name: "gym_reset",
    pack: "physics",
    description:
      "Reset the simulation environment to its initial state. Returns the initial observation.",
    inputSchema: gymResetSchema,
    handler: (a) => gymReset(a),
    behavior: behavior({}),
  },
  {
    name: "gym_observe",
    pack: "physics",
    description:
      "Get the current observation from the simulation without stepping. " +
      "Returns joint positions, velocities, and end effector poses.",
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
