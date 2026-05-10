// Run-blob construction. Wraps the grader's partial blob with model
// identity, the prompt, the trace, and timestamps to form the final
// JSON written to mecheval/runs/<task_id>/<model_id>/<run_id>.json.
//
// Schema lives in mecheval/runs/SCHEMA.md.

import { createHash } from "node:crypto";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname } from "node:path";
import type { Task, TaskInput } from "./task.js";
import type { Solver, SolverOutput } from "./solver.js";

export const BLOB_SCHEMA_VERSION = 0;

export interface PartialGraderBlob {
  schema_version: number;
  task_id: string;
  task_sha256: string;
  checks: unknown[];
  summary: {
    passed: boolean;
    checks_passed: number;
    checks_total: number;
    score: number;
    anti_cheese_violated: boolean;
    limits_exceeded: string[];
  };
}

export interface FullRunBlob extends PartialGraderBlob {
  run_id: string;
  model: {
    id: string;
    name: string;
    provider: string;
    params: Record<string, unknown>;
  };
  harness: {
    version: string;
    commit: string | null;
    vcad_version: string | null;
    phyz_version: string | null;
    host: { os: string; arch: string };
  };
  submission_kind: "self-run" | "hosted" | "black-box";
  prompt: {
    seed: string;
    rendered: string;
    // Mirrors `task.inputs` — bare paths or structured input objects.
    // Full set recorded for forensics; the harness will eventually filter
    // `agent_visible: false` entries before invoking the solver, but the
    // blob keeps everything so audits can verify what was held back.
    attachments: TaskInput[];
  };
  trace: SolverOutput["toolCalls"] extends infer _T
    ? {
        tool_calls: SolverOutput["toolCalls"];
        tokens: SolverOutput["tokens"];
        wallclock_sec: number;
      }
    : never;
  output: {
    vcad_path: string;
    vcad_sha256: string;
    control_policy: string | null;
    renders: string[];
  };
  sim: null;
  timestamps: {
    started_at: string;
    ended_at: string;
  };
  signature: null;
}

export function sha256Hex(buf: string | Buffer): string {
  return createHash("sha256")
    .update(typeof buf === "string" ? Buffer.from(buf, "utf8") : buf)
    .digest("hex");
}

export interface BuildBlobInput {
  partial: PartialGraderBlob;
  task: Task;
  solver: Solver;
  solverOutput: SolverOutput;
  prompt: { seed: string; rendered: string; attachments: TaskInput[] };
  vcadPath: string;
  vcadSha256: string;
  runId: string;
  startedAt: Date;
  endedAt: Date;
  harnessVersion: string;
  submissionKind: FullRunBlob["submission_kind"];
}

export function buildBlob(inp: BuildBlobInput): FullRunBlob {
  return {
    schema_version: BLOB_SCHEMA_VERSION,
    task_id: inp.task.id,
    task_sha256: inp.partial.task_sha256,
    checks: inp.partial.checks,
    summary: inp.partial.summary,
    run_id: inp.runId,
    model: {
      id: inp.solver.id,
      name: inp.solver.name,
      provider: inp.solver.provider,
      params: inp.solver.params,
    },
    harness: {
      version: inp.harnessVersion,
      commit: process.env.MECHEVAL_HARNESS_COMMIT ?? null,
      vcad_version: process.env.MECHEVAL_VCAD_VERSION ?? null,
      phyz_version: process.env.MECHEVAL_PHYZ_VERSION ?? null,
      host: { os: `${process.platform}-${process.version}`, arch: process.arch },
    },
    submission_kind: inp.submissionKind,
    prompt: inp.prompt,
    trace: {
      tool_calls: inp.solverOutput.toolCalls,
      tokens: inp.solverOutput.tokens,
      wallclock_sec: inp.solverOutput.wallclockSec,
    },
    output: {
      vcad_path: inp.vcadPath,
      vcad_sha256: inp.vcadSha256,
      control_policy: inp.solverOutput.controlPolicy,
      renders: [],
    },
    sim: null,
    timestamps: {
      started_at: inp.startedAt.toISOString(),
      ended_at: inp.endedAt.toISOString(),
    },
    signature: null,
  };
}

export function generateRunId(now: Date = new Date()): string {
  const ts = now.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}/, "");
  const rand = Math.floor(Math.random() * 0xffff)
    .toString(16)
    .padStart(4, "0");
  return `${ts}-${rand}`;
}

export async function writeBlob(
  blob: FullRunBlob,
  outPath: string,
): Promise<void> {
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, JSON.stringify(blob, null, 2) + "\n", "utf8");
}

export async function loadJson<T>(path: string): Promise<T> {
  const raw = await readFile(path, "utf8");
  return JSON.parse(raw) as T;
}
