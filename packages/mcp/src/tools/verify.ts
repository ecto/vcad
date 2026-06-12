/**
 * verify_part + list_eval_tasks — expose the mecheval graders as a
 * self-grading oracle so agents can verify-and-iterate instead of
 * guessing.
 *
 * Grading shells out to the `mecheval-grade` Rust binary (the exact
 * grader the benchmark harness and leaderboard use — one source of
 * grading truth, zero drift). The candidate document is written to a
 * temp file; the grader prints a run blob on stdout with per-check
 * pass/fail details, which we relay verbatim minus harness metadata.
 *
 * Resolution order for the binary: $MECHEVAL_GRADE_BIN, then
 * target/release/mecheval-grade, then target/debug/mecheval-grade
 * relative to the repo root. Tasks live in mecheval/tasks ($MECHEVAL_DIR
 * overrides the repo-root search).
 */

import { execFile } from "child_process";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "fs";
import { tmpdir } from "os";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import { promisify } from "util";
import { getSession } from "./session.js";

const execFileAsync = promisify(execFile);

export const verifyPartSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document — the candidate document to grade.",
    },
    task_id: {
      type: "string" as const,
      description:
        "mecheval task id (e.g. 'a1-plate-01'). Use list_eval_tasks to browse available tasks.",
    },
  },
  required: ["document_id", "task_id"],
};

export const listEvalTasksSchema = {
  type: "object" as const,
  properties: {
    suite: {
      type: "string" as const,
      description:
        "Optional suite filter: A (authoring), B (kernel), C (mech/physics), D (visual), F (fit).",
    },
  },
};

interface TextResult {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
}

function textResult(payload: unknown, isError = false): TextResult {
  return {
    content: [{ type: "text", text: JSON.stringify(payload) }],
    ...(isError ? { isError: true } : {}),
  };
}

/** Walk up from `start` looking for a directory containing `probe`. */
function findUpward(start: string, probe: string): string | null {
  let dir = start;
  for (let i = 0; i < 12; i++) {
    if (existsSync(join(dir, probe))) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

/** Locate the mecheval directory (contains tasks/). $MECHEVAL_DIR wins;
 *  otherwise walk up from this module and from cwd looking for
 *  mecheval/tasks in a vcad checkout. */
function findMechevalRoot(): string | null {
  const override = process.env.MECHEVAL_DIR;
  if (override && existsSync(join(override, "tasks"))) {
    return override;
  }
  const here = dirname(fileURLToPath(import.meta.url));
  const repoRoot =
    findUpward(here, join("mecheval", "tasks")) ??
    findUpward(process.cwd(), join("mecheval", "tasks"));
  return repoRoot ? join(repoRoot, "mecheval") : null;
}

function findGraderBin(mechevalRoot: string): string | null {
  const override = process.env.MECHEVAL_GRADE_BIN;
  if (override && existsSync(override)) return override;
  for (const profile of ["release", "debug"]) {
    const candidate = join(
      dirname(mechevalRoot),
      "target",
      profile,
      "mecheval-grade",
    );
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

// ─── list_eval_tasks ──────────────────────────────────────────────────────

export function listEvalTasks(args: Record<string, unknown>): TextResult {
  const mechevalRoot = findMechevalRoot();
  if (!mechevalRoot) {
    return textResult(
      {
        error:
          "mecheval tasks directory not found — run from a vcad checkout or set MECHEVAL_DIR to the mecheval directory.",
      },
      true,
    );
  }
  const tasksDir = join(mechevalRoot, "tasks");
  const suiteFilter = args.suite ? String(args.suite).toUpperCase() : null;

  const tasks: Array<Record<string, unknown>> = [];
  for (const file of readdirSync(tasksDir).sort()) {
    if (!file.endsWith(".json")) continue;
    try {
      const raw = JSON.parse(readFileSync(join(tasksDir, file), "utf8"));
      if (suiteFilter && String(raw.suite).toUpperCase() !== suiteFilter) {
        continue;
      }
      tasks.push({
        id: raw.id,
        suite: raw.suite,
        tier: raw.tier,
        title: raw.title,
        prompt: raw.prompt,
        checks: Array.isArray(raw.checks) ? raw.checks.length : 0,
        tags: raw.tags ?? [],
      });
    } catch {
      // Skip unparseable entries rather than failing the whole listing.
    }
  }
  return textResult({ count: tasks.length, tasks });
}

// ─── verify_part ──────────────────────────────────────────────────────────

interface GraderBlob {
  task_id?: string;
  checks?: Array<{
    n?: number;
    type?: string;
    result?: string;
    details?: unknown;
  }>;
  summary?: Record<string, unknown>;
}

export async function verifyPart(
  args: Record<string, unknown>,
): Promise<TextResult> {
  const documentId = String(args.document_id ?? "");
  const taskId = String(args.task_id ?? "");
  const doc = getSession(documentId);

  const mechevalRoot = findMechevalRoot();
  if (!mechevalRoot) {
    return textResult(
      {
        error:
          "mecheval tasks directory not found — run from a vcad checkout or set MECHEVAL_DIR to the mecheval directory.",
      },
      true,
    );
  }
  // Task ids are filenames; reject path separators so a crafted id can't
  // escape the tasks directory.
  if (!/^[A-Za-z0-9._-]+$/.test(taskId)) {
    return textResult({ error: `invalid task_id: ${taskId}` }, true);
  }
  const taskPath = join(mechevalRoot, "tasks", `${taskId}.json`);
  if (!existsSync(taskPath)) {
    return textResult(
      {
        error: `unknown task_id "${taskId}"`,
        hint: "Use list_eval_tasks to see available task ids.",
      },
      true,
    );
  }

  const graderBin = findGraderBin(mechevalRoot);
  if (!graderBin) {
    return textResult(
      {
        error: "mecheval-grade binary not found.",
        hint: "Build it with `cargo build -p mecheval-grader --bin mecheval-grade` (or set MECHEVAL_GRADE_BIN).",
      },
      true,
    );
  }

  const scratch = mkdtempSync(join(tmpdir(), "vcad-verify-"));
  const vcadPath = join(scratch, "candidate.vcad");
  try {
    writeFileSync(vcadPath, JSON.stringify(doc));

    let stdout: string;
    try {
      // Async so a long grade doesn't freeze the MCP server's event loop
      // (other sessions, pings, and concurrent tool calls keep flowing).
      ({ stdout } = await execFileAsync(graderBin, [taskPath, vcadPath], {
        encoding: "utf8",
        timeout: 180_000,
        maxBuffer: 64 * 1024 * 1024,
      }));
    } catch (e) {
      const err = e as { code?: number | string; stdout?: string; stderr?: string };
      // Exit 1 = graded but failed; the blob is still on stdout.
      if (err.code === 1 && err.stdout) {
        stdout = err.stdout;
      } else {
        return textResult(
          {
            error: `grader error: ${err.stderr?.trim() || String(e)}`,
            task_id: taskId,
          },
          true,
        );
      }
    }

    let blob: GraderBlob;
    try {
      blob = JSON.parse(stdout);
    } catch {
      return textResult(
        { error: "grader produced unparseable output", task_id: taskId },
        true,
      );
    }

    // Relay the verdict + per-check feedback; drop forensic metadata the
    // agent doesn't need (hashes, schema versions).
    return textResult({
      task_id: taskId,
      document_id: documentId,
      summary: blob.summary ?? {},
      checks: (blob.checks ?? []).map((c) => ({
        n: c.n,
        type: c.type,
        result: c.result,
        details: c.details,
      })),
    });
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
