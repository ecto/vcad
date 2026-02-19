/**
 * Gym-style physics simulation tools for RL training.
 *
 * These tools provide a gym-like interface for simulating robot assemblies
 * with physics, enabling reinforcement learning training.
 *
 * Uses the Rapier3D physics engine via WASM bindings.
 */

import type { Document } from "@vcad/ir";
import {
  PhysicsEnv,
  isPhysicsAvailable,
  type PhysicsObservation,
  type PhysicsStepResult,
  type PhysicsActionType,
} from "@vcad/engine";

/** Observation from the robot environment (re-export for API compatibility) */
export type Observation = PhysicsObservation;

/** Step result from the environment (re-export for API compatibility) */
export type StepResult = PhysicsStepResult;

/** In-memory storage for active simulations */
const simulations = new Map<string, PhysicsEnv>();

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

let nextSimId = 1;
let nextBatchId = 1;

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
export async function createRobotEnv(input: unknown): Promise<{
  content: Array<{ type: "text"; text: string }>;
}> {
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
    };
  }

  try {
    const envId = `sim_${nextSimId++}`;

    const env = await PhysicsEnv.create(args.document, {
      endEffectorIds: args.end_effector_ids,
      dt: args.dt,
      substeps: args.substeps,
      maxSteps: args.max_steps,
    });

    simulations.set(envId, env);

    const info = {
      env_id: envId,
      num_joints: env.numJoints,
      action_dim: env.actionDim,
      observation_dim: env.observationDim,
      end_effector_ids: args.end_effector_ids,
      dt: args.dt ?? 1 / 240,
      substeps: args.substeps ?? 4,
      max_steps: args.max_steps ?? 1000,
    };

    return {
      content: [{ type: "text", text: JSON.stringify(info, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
    };
  }
}

/** Step the simulation with an action */
export function gymStep(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
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
    };
  }

  try {
    const result = env.step(args.action_type, args.values);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
    };
  }
}

/** Reset the environment to initial state */
export function gymReset(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = input as { env_id: string };

  const env = simulations.get(args.env_id);
  if (!env) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
    };
  }

  try {
    const observation = env.reset();
    return {
      content: [{ type: "text", text: JSON.stringify(observation, null, 2) }],
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      content: [{ type: "text", text: JSON.stringify({ error: message }) }],
    };
  }
}

/** Get current observation without stepping */
export function gymObserve(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = input as { env_id: string };

  const env = simulations.get(args.env_id);
  if (!env) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
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
    };
  }
}

/** Close and clean up a simulation environment */
export function gymClose(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = input as { env_id: string };

  const env = simulations.get(args.env_id);
  if (env) {
    env.close();
    simulations.delete(args.env_id);
    return {
      content: [{ type: "text", text: JSON.stringify({ success: true }) }],
    };
  }

  return {
    content: [
      { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
    ],
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
export async function batchCreateEnvs(input: unknown): Promise<{
  content: Array<{ type: "text"; text: string }>;
}> {
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
    };
  }

  try {
    const batchId = `batch_${nextBatchId++}`;
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
    };
  }
}

/** Step all environments in a batch simultaneously */
export function batchStep(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
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
    };
  }
}

/** Reset all environments in a batch */
export function batchReset(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = input as { batch_id: string };

  const group = batchGroups.get(args.batch_id);
  if (!group) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown batch_id: ${args.batch_id}` }) },
      ],
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
    };
  }
}
