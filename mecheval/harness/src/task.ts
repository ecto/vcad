// Task JSON loader (TypeScript twin of the Rust grader's task module).
// Schema lives in `mecheval/tasks/SCHEMA.md`.

import { readFile } from "node:fs/promises";
import { basename, extname } from "node:path";

export type Suite = "A" | "B" | "C";

export interface Task {
  id: string;
  suite: Suite;
  tier: string;
  title: string;
  prompt: string;
  inputs?: string[];
  checks: CheckSpec[];
  anti_cheese?: AntiCheese;
  limits?: Limits;
  pass_k?: number;
  tags?: string[];
}

// We don't enumerate every check type in TS — the Rust grader is the
// authoritative implementation. The harness just shuttles checks through
// without inspecting them, so an opaque `unknown` payload is sufficient.
export interface CheckSpec {
  type: string;
  [k: string]: unknown;
}

export interface AntiCheese {
  min_rigid_bodies?: number;
  min_actuated_joints?: number;
  max_solid_count?: number;
  max_total_mass_kg?: number;
  joint_torque_ceiling_nm?: number;
  required_links?: string[];
}

export interface Limits {
  max_tokens?: number;
  max_wallclock_sec?: number;
  max_tool_calls?: number;
}

export async function loadTask(path: string): Promise<Task> {
  const raw = await readFile(path, "utf8");
  const task = JSON.parse(raw) as Task;
  const filename = basename(path, extname(path));
  if (filename !== task.id) {
    throw new Error(
      `task filename "${filename}" does not match task id "${task.id}"`,
    );
  }
  return task;
}
