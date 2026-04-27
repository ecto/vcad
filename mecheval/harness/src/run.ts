// Orchestrate one attempt: load task, render prompt, run solver, persist
// .vcad output, shell out to the Rust grader, build the run blob, write it.

import { readFile, writeFile, mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { loadTask, type Task } from "./task.js";
import { getSolver, type Solver } from "./solver.js";
import {
  buildBlob,
  generateRunId,
  loadJson,
  sha256Hex,
  writeBlob,
  type FullRunBlob,
  type PartialGraderBlob,
} from "./blob.js";

export const HARNESS_VERSION = "0.0.1";

export interface RunOptions {
  taskPath: string;
  solverId: string;
  /** Where to write the run blob. Defaults to mecheval/runs/<task>/<model>/<run>.json. */
  outDir?: string;
  /**
   * Path to the `mecheval-grade` binary. Defaults to the workspace target
   * dir; CI / production deploys override this to point at an installed
   * binary.
   */
  graderBin?: string;
  /** Repo root, for default paths. */
  repoRoot?: string;
  submissionKind?: FullRunBlob["submission_kind"];
}

export async function runOne(opts: RunOptions): Promise<{
  blob: FullRunBlob;
  blobPath: string;
}> {
  const repoRoot = opts.repoRoot ?? process.cwd();
  const submissionKind = opts.submissionKind ?? "self-run";
  const graderBin =
    opts.graderBin ??
    resolve(repoRoot, "target", "debug", "mecheval-grade");

  const startedAt = new Date();

  const task = await loadTask(opts.taskPath);
  const solver = getSolver(opts.solverId);

  // Render the prompt. v0.0: trivial pass-through; later the harness can
  // template in attachments / reference images / starter .vcads. Seed is
  // currently the task id — deterministic by construction.
  const prompt = {
    seed: task.id,
    rendered: task.prompt,
    attachments: [...(task.inputs ?? [])],
  };

  // Run the solver.
  const solverOutput = await solver.solve(task, prompt.rendered);

  // Persist the .vcad output to a per-run scratch dir so the grader can
  // read it from disk. We then keep a copy alongside the blob.
  const scratch = await mkdtemp(join(tmpdir(), "mecheval-run-"));
  const vcadPath = join(scratch, "output.vcad");
  await writeFile(vcadPath, solverOutput.vcadJson, "utf8");
  const vcadSha = sha256Hex(solverOutput.vcadJson);

  // Shell out to the Rust grader.
  const partial = await invokeGrader(graderBin, opts.taskPath, vcadPath);

  const endedAt = new Date();
  const runId = generateRunId(startedAt);

  // Final blob, with the .vcad path made relative to the blob output dir.
  const outDir =
    opts.outDir ??
    resolve(repoRoot, "mecheval", "runs", task.id, solver.id);
  const blobPath = join(outDir, `${runId}.json`);
  const persistedVcadPath = join(outDir, `${runId}.vcad`);
  await mkdir(outDir, { recursive: true });
  await writeFile(persistedVcadPath, solverOutput.vcadJson, "utf8");

  const blob = buildBlob({
    partial,
    task,
    solver,
    solverOutput,
    prompt,
    vcadPath: `${runId}.vcad`,
    vcadSha256: vcadSha,
    runId,
    startedAt,
    endedAt,
    harnessVersion: HARNESS_VERSION,
    submissionKind,
  });

  await writeBlob(blob, blobPath);

  return { blob, blobPath };
}

async function invokeGrader(
  bin: string,
  taskPath: string,
  vcadPath: string,
): Promise<PartialGraderBlob> {
  return new Promise((res, rej) => {
    const proc = spawn(bin, [taskPath, vcadPath], { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    proc.stdout.on("data", (b) => { stdout += b.toString(); });
    proc.stderr.on("data", (b) => { stderr += b.toString(); });
    proc.on("error", rej);
    proc.on("close", (code) => {
      // grader exit codes: 0 pass, 1 fail, 2 error. 0 and 1 both produce a
      // valid blob on stdout; only 2 is fatal.
      if (code === 2) {
        rej(new Error(`grader errored (exit 2): ${stderr.trim() || "no stderr"}`));
        return;
      }
      try {
        res(JSON.parse(stdout) as PartialGraderBlob);
      } catch (e) {
        rej(
          new Error(
            `grader returned non-JSON on stdout (exit ${code}): ${(e as Error).message}\nstderr: ${stderr.trim()}`,
          ),
        );
      }
    });
  });
}

// Re-export for the CLI.
export type { Task };
export { loadJson };
