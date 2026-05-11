// Resolve a task's `inputs[]` into agent-visible attachments.
//
// The harness reads images from disk + base64-encodes them, pulls
// known-dimensions text inline, and strips anything `agent_visible: false`
// (e.g. Suite F host geometry, which goes only to the grader).

import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import type { AgentAttachment } from "./solver.js";
import { isStructured, type Task, type TaskInput } from "./task.js";

const IMAGE_MIME: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".gif": "image/gif",
};

/**
 * Resolve every agent-visible structured input in `task.inputs` into an
 * [AgentAttachment]. Bare-string inputs (legacy starter-`.vcad` cases)
 * are returned as `kind: "other"` with the path passed through.
 *
 * `taskDir` is the directory the task JSON was loaded from; used to
 * resolve relative `path` fields.
 */
export async function resolveAgentInputs(
  task: Task,
  taskDir: string,
): Promise<AgentAttachment[]> {
  const out: AgentAttachment[] = [];
  for (const input of task.inputs ?? []) {
    if (!isStructured(input)) {
      // Legacy: bare path. Pass it through as opaque metadata.
      out.push({
        kind: "other",
        meta: { kind: "legacy_path", agent_visible: true, path: input },
        path: input,
      });
      continue;
    }
    if (!input.agent_visible) continue;

    if (input.kind === "reference_image") {
      if (!input.path) continue;
      const abs = resolve(taskDir, input.path);
      const bytes = await readFile(abs);
      const ext = extname(input.path).toLowerCase();
      const mime = IMAGE_MIME[ext] ?? "application/octet-stream";
      out.push({
        kind: "reference_image",
        meta: input,
        mime,
        base64: bytes.toString("base64"),
      });
    } else if (input.kind === "known_dimensions") {
      const text = typeof input.text === "string" ? input.text : "";
      out.push({ kind: "known_dimensions", meta: input, text });
    } else {
      out.push({ kind: "other", meta: input, path: input.path });
    }
  }
  return out;
}

/**
 * Pure splitter — no I/O. Returns `(visible, private)` partitions. Useful
 * for tests and for the grader-side check that nothing leaked.
 */
export function partitionInputs(inputs: TaskInput[] | undefined): {
  visible: TaskInput[];
  private: TaskInput[];
} {
  const visible: TaskInput[] = [];
  const priv: TaskInput[] = [];
  for (const i of inputs ?? []) {
    if (isStructured(i) && i.agent_visible === false) priv.push(i);
    else visible.push(i);
  }
  return { visible, private: priv };
}
