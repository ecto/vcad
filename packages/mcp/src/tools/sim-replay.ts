/**
 * Physics replay tools for the MCP Apps viewer.
 *
 * The gym tools record every step's joint positions into a ring buffer on the
 * env record (tools/gym.ts). These two app-only tools serve that rollout to
 * the inline viewer's playback UI: `get_sim_replay` returns the trajectory
 * plus per-step FK-solved instance transforms so the viewer can re-pose the
 * assembly, and `get_sim_version` is the cheap change token its poll watches.
 *
 * Like the preview tools, envs are in-process — replay works within a warm
 * server instance only (the same caveat as the gym itself).
 */

import type { Document } from "@vcad/ir";
import { getKernelWasm, resetKernelWasm } from "@vcad/engine";
import { getEnvRecord } from "./gym.js";
import { resolveObservationJoints } from "./joint-order.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** MCP tool result for the replay tools. Error paths set `isError: true` so
 *  hosts treat them as failures rather than reading a `{"error": ...}` body
 *  as a successful result. */
type ReplayResult = {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
};

/**
 * FNV-1a change token over `(env_id, reset_epoch, step_count)` — same hash
 * the preview poll uses (`previewVersion` in tools/preview.ts), but keyed on
 * the env's counters instead of document bytes: no FK, no serialization, so
 * the viewer can poll it every tick. `resetEpoch` participates so an
 * equal-length rollout AFTER a gym_reset still changes the token (stepCount
 * alone rewinds to 0 and would collide with the prior episode).
 */
function replayVersion(envId: string, resetEpoch: number, stepCount: number): string {
  const s = `${envId}:${resetEpoch}:${stepCount}`;
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

/** One replay pose on the wire: plain `[x, y, z]` ARRAYS for every component
 *  — the viewer contract (`SimTrsLike` in viewer-app/main.ts indexes
 *  `translation[0]` etc.), NOT the IR's `{x, y, z}` Vec3 objects. */
interface SimTrs {
  translation: [number, number, number];
  rotation: [number, number, number];
  scale: [number, number, number];
}

/** A Map (serde_wasm_bindgen HashMap) or plain object → plain record. */
function toRecord(value: unknown): Record<string, unknown> {
  if (value instanceof Map) {
    return Object.fromEntries(value as Map<string, unknown>);
  }
  if (value && typeof value === "object") {
    return value as Record<string, unknown>;
  }
  return {};
}

/** Normalize one Vec3-ish value ({x,y,z} object, Map, or an already-array
 *  triple) to a plain `[x, y, z]` array; anything unusable → `fallback`. */
function toVec3Array(
  value: unknown,
  fallback: [number, number, number],
): [number, number, number] {
  if (Array.isArray(value) && value.length >= 3) {
    const [x, y, z] = value as unknown[];
    if (typeof x === "number" && typeof y === "number" && typeof z === "number") {
      return [x, y, z];
    }
    return fallback;
  }
  const rec = toRecord(value);
  const { x, y, z } = rec as { x?: unknown; y?: unknown; z?: unknown };
  if (typeof x === "number" && typeof y === "number" && typeof z === "number") {
    return [x, y, z];
  }
  return fallback;
}

/**
 * Normalize the kernel's FK return into the viewer wire shape.
 * `serde_wasm_bindgen` emits a JS `Map` for the Rust HashMap and Vec3 fields
 * as `{x, y, z}` structs — but the viewer's `SimTrsLike` contract is plain
 * `[x, y, z]` ARRAYS (it reads `translation[0]`, `rotation[0]`…; object
 * fields would index to `undefined` and NaN every pose). Every Transform3D
 * is therefore normalized server-side to array triples here.
 *
 * The kernel is called directly (via `getKernelWasm`, the record_simulation
 * pattern) rather than through `@vcad/engine`'s `solveForwardKinematics`
 * wrapper, which mis-handles the Map return and comes back empty.
 */
function toTransformRecord(value: unknown): Record<string, SimTrs> {
  const raw = toRecord(value);
  const out: Record<string, SimTrs> = {};
  for (const [id, t] of Object.entries(raw)) {
    const rec = toRecord(t);
    out[id] = {
      translation: toVec3Array(rec.translation, [0, 0, 0]),
      rotation: toVec3Array(rec.rotation, [0, 0, 0]),
      scale: toVec3Array(rec.scale, [1, 1, 1]),
    };
  }
  return out;
}

/** JSON Schema for get_sim_replay input */
export const getSimReplaySchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID returned by create_robot_env",
    },
  },
  required: ["env_id"],
};

/** JSON Schema for get_sim_version input */
export const getSimVersionSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment ID returned by create_robot_env",
    },
  },
  required: ["env_id"],
};

/**
 * Return the recorded rollout for an env: the joint trajectory, rewards,
 * done flags, and per-step FK-solved instance transforms.
 *
 * FK runs on a single clone of the stored assembly — each trajectory row is
 * written into the clone's `joints[j].state` (the record_simulation pattern,
 * tools/record.ts) and the kernel solver reconstructs world poses. Documents
 * without joints or instances still return the trajectory with
 * `instance_transforms: []`; the viewer falls back to static geometry.
 */
export async function getSimReplay(input: unknown): Promise<ReplayResult> {
  const args = input as { env_id: string };

  const record = getEnvRecord(args.env_id);
  if (!record) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
      isError: true,
    };
  }

  // Per-step FK: clone the stored assembly ONCE, write each trajectory row
  // into the FK clone's joints, and let the kernel solver reconstruct world
  // poses (the record_simulation pattern, tools/record.ts). The per-row
  // serialization happens at the WASM boundary, so the single clone never
  // aliases across rows.
  //
  // Observation slots map onto doc joints BY ID via resolveObservationJoints
  // (shared with record_simulation): trajectory row[i] is the kernel's
  // joint_positions[i], which refers to jointIds[i], so a permuted kernel
  // joint order can't land a pose on the wrong joint. Older WASM builds
  // without jointIds() leave record.jointIds null and fall back to positional
  // order. Resolved once, before the row loop.
  const instanceTransforms: Array<Record<string, SimTrs>> = [];
  const jointCount = record.document.joints?.length ?? 0;
  const instanceCount = record.document.instances?.length ?? 0;
  if (jointCount > 0 && instanceCount > 0 && record.trajectory.length > 0) {
    try {
      const wasm = (await getKernelWasm()) as unknown as {
        solveForwardKinematics?: (docJson: string) => unknown;
      };
      if (typeof wasm.solveForwardKinematics === "function") {
        const docClone: Document = JSON.parse(JSON.stringify(record.document));
        const obsJoints = resolveObservationJoints(
          docClone.joints!,
          record.jointIds,
        );
        // A mismatch means the record's jointIds don't match its own document
        // — impossible for a live env, so degrade to a transform-less replay
        // (the viewer falls back to static geometry) rather than mis-posing.
        if (!("error" in obsJoints)) {
          for (const row of record.trajectory) {
            for (let j = 0; j < obsJoints.joints.length; j++) {
              const pos = row[j];
              if (typeof pos === "number") obsJoints.joints[j]!.state = pos;
            }
            instanceTransforms.push(
              toTransformRecord(
                wasm.solveForwardKinematics(JSON.stringify(docClone)),
              ),
            );
          }
        }
      }
    } catch (err) {
      // A kernel trap poisons the shared WASM instance — recover it, then
      // degrade to a transform-less replay (the viewer falls back to static
      // geometry) rather than erroring the playback poll.
      if (err instanceof WebAssembly.RuntimeError) {
        resetKernelWasm(`get_sim_replay FK trap: ${err.message}`);
      }
      instanceTransforms.length = 0;
    }
  }

  const body = {
    env_id: args.env_id,
    document_id: record.documentId,
    dt: record.dt,
    substeps: record.substeps,
    steps: record.trajectory.length,
    total_steps: record.stepCount,
    joint_trajectory: record.trajectory,
    rewards: record.rewards,
    dones: record.dones,
    instance_transforms: instanceTransforms,
    reset_epoch: record.resetEpoch,
    version: replayVersion(args.env_id, record.resetEpoch, record.stepCount),
  };

  return {
    content: [{ type: "text", text: JSON.stringify(body) }],
  };
}

/**
 * Return a cheap `{env_id, document_id, step_count, version}` change token
 * for an env — no FK, no geometry. The viewer polls this to learn "did the
 * rollout advance?" and only re-fetches the heavy replay when it did.
 */
export function getSimVersion(input: unknown): ReplayResult {
  const args = input as { env_id: string };

  const record = getEnvRecord(args.env_id);
  if (!record) {
    return {
      content: [
        { type: "text", text: JSON.stringify({ error: `Unknown env_id: ${args.env_id}` }) },
      ],
      isError: true,
    };
  }

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          env_id: args.env_id,
          document_id: record.documentId,
          step_count: record.stepCount,
          reset_epoch: record.resetEpoch,
          version: replayVersion(args.env_id, record.resetEpoch, record.stepCount),
        }),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "get_sim_replay",
    pack: null,
    description:
      "Return the recorded joint trajectory and per-step instance transforms for a physics env. Internal to the inline viewer's replay UI — agents should use gym_observe or record_simulation instead.",
    inputSchema: getSimReplaySchema,
    handler: (a) => getSimReplay(a),
    behavior: behavior({ appOnly: true }),
  },
  {
    name: "get_sim_version",
    pack: null,
    description:
      "Return a cheap {env_id, step_count, version} change token for a physics env (no FK eval). Internal to the inline viewer's replay poll — agents should ignore it.",
    inputSchema: getSimVersionSchema,
    handler: (a) => getSimVersion(a),
    behavior: behavior({ appOnly: true }),
  },
];
