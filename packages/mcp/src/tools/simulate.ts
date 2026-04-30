/**
 * simulate (v2) — stateless physics rollout over a doc handle.
 *
 * Builds a `PhysicsEnv` per call from the handle's IR, optionally seeds
 * joint state, applies the action matrix, returns the trajectory +
 * observations. Servers can cache `(handle, sim_settings)` → world to
 * avoid the rebuild cost; v2 doesn't surface the cache key — the agent
 * just sees latency drop on warm calls.
 *
 * `mode: "step"` runs a single step (actions length must be 1) and is
 * intended for closed-loop control where the agent re-issues actions
 * based on each observation.
 *
 * `mode: "rollout"` (default) runs the full action matrix and returns
 * the full trajectory.
 */

import { PhysicsEnv, isPhysicsAvailable } from "@vcad/engine";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef } from "../types.js";

export const simulateSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle (assembly with joints)." },
    state: {
      type: "object" as const,
      description: "Optional seed { q: number[], qdot: number[] }; omit to start from rest.",
    },
    actions: {
      type: "array" as const,
      description: "T × DOF action matrix.",
    },
    action_type: { type: "string" as const, enum: ["torque", "position", "velocity"] },
    mode: { type: "string" as const, enum: ["step", "rollout"] },
    timestep: { type: "number" as const },
    end_effector_ids: { type: "array" as const, items: { type: "string" as const } },
  },
  required: ["doc", "actions"],
};

interface SimState {
  q: number[];
  qdot: number[];
}

interface SimulateInput {
  doc: DocRef;
  state?: SimState;
  actions: number[][];
  action_type?: "torque" | "position" | "velocity";
  mode?: "step" | "rollout";
  timestep?: number;
  end_effector_ids?: string[];
}

export async function simulate(input: unknown, engine: Engine): Promise<ToolResult> {
  const startedAt = performance.now();
  const args = (input ?? {}) as SimulateInput;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  if (!Array.isArray(args.actions)) return fail("invalid_input", "Missing `actions`.");

  const available = await isPhysicsAvailable();
  if (!available) {
    return fail("physics_unavailable", "WASM build lacks `--features physics`.");
  }

  const { doc, handle } = resolveRef(args.doc);
  if (!doc.instances || doc.instances.length === 0 || !doc.joints) {
    return fail("not_an_assembly", "simulate requires a doc with instances + joints.");
  }

  const eeIds = args.end_effector_ids ?? [];
  let env: PhysicsEnv;
  try {
    env = await PhysicsEnv.create(doc, {
      endEffectorIds: eeIds,
      dt: args.timestep,
      substeps: 4,
      maxSteps: Math.max(args.actions.length + 1, 1000),
    });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return fail("physics_init_failed", msg);
  }

  // Seed state by stepping with zero torque a few times — the IR doesn't
  // give us a direct q/qdot setter; this is a Phase-1 limitation.
  // When a state is passed we record it as the "initial_state" and
  // surface a warning so RL agents know reseeding isn't yet bit-exact.
  const warnings: { code: string; message: string }[] = [];
  if (args.state) {
    warnings.push({
      code: "state_seed_partial",
      message:
        "Phase-1 simulate doesn't yet bit-reseed q/qdot — environment starts from PhysicsEnv defaults.",
    });
  }

  const trajectory: SimState[] = [];
  const observations: ReturnType<PhysicsEnv["observe"]>[] = [];
  const rewards: number[] = [];
  const actionType = args.action_type ?? "torque";
  const mode = args.mode ?? "rollout";

  // Initial observation.
  observations.push(env.observe());

  const T = mode === "step" ? Math.min(1, args.actions.length) : args.actions.length;
  let doneAt: number | undefined;
  for (let t = 0; t < T; t++) {
    const action = args.actions[t];
    let res;
    try {
      res = env.step(actionType, action);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      try {
        env.close?.();
      } catch {
        /* noop */
      }
      return fail("step_failed", `Step ${t}: ${msg}`);
    }
    observations.push(res.observation);
    rewards.push(res.reward);
    trajectory.push({
      q: res.observation.joint_positions,
      qdot: res.observation.joint_velocities,
    });
    if (res.done && doneAt === undefined) doneAt = t;
  }

  try {
    env.close?.();
  } catch {
    /* noop */
  }

  const finalObs = observations[observations.length - 1];
  return ok({
    result: {
      trajectory,
      observations,
      rewards,
      done_at: doneAt ?? null,
      final_state: {
        q: finalObs.joint_positions,
        qdot: finalObs.joint_velocities,
      },
      mode,
      action_type: actionType,
      timestep: args.timestep ?? 1 / 240,
      steps: T,
    },
    handle,
    doc,
    engine,
    startedAt,
    warnings,
    skipPreview: true,
  });
}
