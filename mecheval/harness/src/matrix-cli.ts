#!/usr/bin/env node
// `mecheval-matrix` — drive a solver across many tasks with a worker pool.
//
// Usage:
//   mecheval-matrix --solver <id> [--suite A] [--tasks id1,id2] \
//     [--attempts N] [--concurrency N] [--grader-bin <path>] [--dry-run]
//
// Each (task, attempt) pair runs as an independent `mecheval-run`
// subprocess, so WASM instances, MCP servers, and graders are fully
// isolated per attempt. Attempts are always FRESH — pass^k counts the
// most recent k run blobs, so running `--attempts k` makes this batch
// the scoring window for the leaderboard (deliberate: a rerun after a
// harness fix must not let stale runs leak into the new number).

import { spawn } from "node:child_process";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

interface CliArgs {
  solver: string | null;
  suite: string | null;
  tasks: string[] | null;
  attempts: number | null;
  concurrency: number;
  graderBin: string | null;
  since: string | null;
  dryRun: boolean;
  help: boolean;
}

function parseArgs(argv: string[]): CliArgs {
  const out: CliArgs = {
    solver: null,
    suite: null,
    tasks: null,
    attempts: null,
    concurrency: 4,
    graderBin: null,
    since: null,
    dryRun: false,
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    switch (a) {
      case "--solver": out.solver = next(); break;
      case "--suite": out.suite = next().toUpperCase(); break;
      case "--tasks": out.tasks = next().split(",").map((s) => s.trim()).filter(Boolean); break;
      case "--attempts": out.attempts = Number(next()); break;
      case "--concurrency": out.concurrency = Number(next()); break;
      case "--grader-bin": out.graderBin = next(); break;
      case "--since": out.since = next(); break;
      case "--dry-run": out.dryRun = true; break;
      case "-h":
      case "--help": out.help = true; break;
      default:
        console.error(`unknown flag: ${a}`);
        process.exit(2);
    }
  }
  return out;
}

function usage(): void {
  console.error(`usage: mecheval-matrix --solver <id> [--suite A] [--tasks id1,id2] [--attempts N] [--concurrency N] [--grader-bin <path>] [--dry-run]

Args:
  --solver       solver id (e.g. claude-mcp-claude-opus-4-7)
  --suite        only tasks in this suite (A, B, C, D, F)
  --tasks        comma-separated explicit task ids (overrides --suite)
  --attempts     fresh attempts per task (default: each task's pass_k, usually 5)
  --concurrency  parallel attempts (default 4)
  --grader-bin   path to mecheval-grade (default: target/release then target/debug)
  --since        top-up mode: count existing run blobs with run_id >= this
                 stamp (e.g. 20260611T170000Z) toward each task's attempts
                 and only schedule the shortfall — resume an interrupted batch
  --dry-run      print the run plan and cost surface, run nothing
`);
}

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const tasksDir = join(repoRoot, "mecheval", "tasks");
const runCli = join(here, "cli.js");

interface TaskMeta {
  id: string;
  suite: string;
  passK: number;
  maxWallclockSec: number;
}

function loadTaskMetas(args: CliArgs): TaskMeta[] {
  const metas: TaskMeta[] = [];
  for (const file of readdirSync(tasksDir).sort()) {
    if (!file.endsWith(".json")) continue;
    const raw = JSON.parse(readFileSync(join(tasksDir, file), "utf8"));
    metas.push({
      id: raw.id,
      suite: String(raw.suite ?? "?").toUpperCase(),
      passK: typeof raw.pass_k === "number" ? raw.pass_k : 5,
      maxWallclockSec: raw.limits?.max_wallclock_sec ?? 300,
    });
  }
  if (args.tasks) {
    const wanted = new Set(args.tasks);
    const found = metas.filter((m) => wanted.has(m.id));
    const missing = [...wanted].filter((id) => !found.some((m) => m.id === id));
    if (missing.length) {
      console.error(`unknown task ids: ${missing.join(", ")}`);
      process.exit(2);
    }
    return found;
  }
  if (args.suite) return metas.filter((m) => m.suite === args.suite);
  return metas;
}

function defaultGraderBin(): string | null {
  for (const profile of ["release", "debug"]) {
    const p = join(repoRoot, "target", profile, "mecheval-grade");
    if (existsSync(p)) return p;
  }
  return null;
}

interface Job {
  task: TaskMeta;
  attempt: number;
}

interface JobResult {
  task: string;
  attempt: number;
  status: "pass" | "fail" | "error";
  wallclockMs: number;
  detail?: string;
}

/** Patterns in stderr that warrant a backoff-and-retry rather than a
 *  recorded error (API rate limits, transient overload, network flaps —
 *  the Anthropic SDK reports slow/refused connects as "Connection
 *  error", which burned a whole queue once when it wasn't listed). */
const RETRYABLE =
  /429|rate.?limit|overloaded|529|ECONNRESET|ETIMEDOUT|connection error|fetch failed|socket hang up/i;
const MAX_RETRIES = 2;

/** Circuit breaker: after this many consecutive errored attempts the
 *  whole matrix pauses to let the API recover, instead of converting
 *  the remaining queue into instant failures. */
const BREAKER_THRESHOLD = 6;
const BREAKER_COOLDOWN_MS = 5 * 60_000;
const BREAKER_MAX_TRIPS = 3;

function runJob(
  job: Job,
  solver: string,
  graderBin: string,
): Promise<JobResult> {
  return new Promise((resolveJob) => {
    const t0 = Date.now();
    const child = spawn(
      process.execPath,
      [runCli, "--task", job.task.id, "--solver", solver, "--grader-bin", graderBin],
      { cwd: repoRoot, stdio: ["ignore", "pipe", "pipe"] },
    );
    let stderr = "";
    child.stderr.on("data", (d) => (stderr += d));
    child.stdout.on("data", () => {});
    // Hard kill at task wallclock limit + grader headroom so one hung
    // attempt can't stall the whole matrix.
    const killTimer = setTimeout(
      () => child.kill("SIGKILL"),
      (job.task.maxWallclockSec + 300) * 1000,
    );
    child.on("close", (code) => {
      clearTimeout(killTimer);
      const wallclockMs = Date.now() - t0;
      if (code === 0) {
        resolveJob({ task: job.task.id, attempt: job.attempt, status: "pass", wallclockMs });
      } else if (code === 1) {
        resolveJob({ task: job.task.id, attempt: job.attempt, status: "fail", wallclockMs });
      } else {
        resolveJob({
          task: job.task.id,
          attempt: job.attempt,
          status: "error",
          wallclockMs,
          detail: stderr.trim().slice(-400),
        });
      }
    });
  });
}

async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help || !args.solver) {
    usage();
    return args.help ? 0 : 2;
  }

  if (
    !args.dryRun &&
    /^(claude|openai)/.test(args.solver) &&
    !process.env.ANTHROPIC_API_KEY &&
    !process.env.OPENAI_API_KEY
  ) {
    console.error(
      "refusing to start: no ANTHROPIC_API_KEY / OPENAI_API_KEY in the environment.",
    );
    return 2;
  }

  const graderBin = args.graderBin ?? defaultGraderBin();
  if (!graderBin) {
    console.error(
      "mecheval-grade not found — cargo build --release -p mecheval-grader --bin mecheval-grade",
    );
    return 2;
  }

  const metas = loadTaskMetas(args);
  if (metas.length === 0) {
    console.error("no tasks matched the filter.");
    return 2;
  }

  const jobs: Job[] = [];
  for (const task of metas) {
    const attempts = args.attempts ?? task.passK;
    let have = 0;
    if (args.since) {
      // Top-up mode: blobs from this batch (run_id is a sortable
      // timestamp prefix) already count toward the attempt target.
      const dir = join(repoRoot, "mecheval", "runs", task.id, args.solver!);
      if (existsSync(dir)) {
        have = readdirSync(dir).filter(
          (f) => f.endsWith(".json") && f >= args.since!,
        ).length;
      }
    }
    for (let attempt = have + 1; attempt <= attempts; attempt++) {
      jobs.push({ task, attempt });
    }
  }
  if (args.since) {
    console.log(
      `top-up since ${args.since}: ${jobs.length} attempts still needed`,
    );
  }

  const serialBudgetMin = Math.round(
    jobs.reduce((s, j) => s + j.task.maxWallclockSec, 0) / 60,
  );
  console.log(
    `matrix: ${metas.length} tasks × attempts → ${jobs.length} runs, ` +
      `solver=${args.solver}, concurrency=${args.concurrency}, grader=${graderBin}`,
  );
  console.log(
    `worst-case wallclock budget ≈ ${serialBudgetMin} min serial, ` +
      `≈ ${Math.round(serialBudgetMin / args.concurrency)} min at this concurrency`,
  );
  if (args.dryRun) {
    for (const m of metas) {
      console.log(`  ${m.id} (suite ${m.suite}, pass_k ${m.passK}, ≤${m.maxWallclockSec}s)`);
    }
    return 0;
  }

  const results: JobResult[] = [];
  const queue = [...jobs];
  let done = 0;

  // Shared circuit-breaker state across workers.
  let consecutiveErrors = 0;
  let breakerTrips = 0;
  let pausedUntil = 0;
  let aborted = false;

  async function worker(): Promise<void> {
    for (;;) {
      if (aborted) return;
      const now = Date.now();
      if (now < pausedUntil) {
        await new Promise((r) => setTimeout(r, pausedUntil - now));
        continue;
      }
      const job = queue.shift();
      if (!job) return;
      let result = await runJob(job, args.solver!, graderBin!);
      for (
        let retry = 1;
        result.status === "error" &&
        result.detail &&
        RETRYABLE.test(result.detail) &&
        retry <= MAX_RETRIES;
        retry++
      ) {
        const backoffMs = 30_000 * retry;
        console.log(
          `  retry ${retry}/${MAX_RETRIES} for ${job.task.id}#${job.attempt} in ${backoffMs / 1000}s (${result.detail.slice(0, 80)})`,
        );
        await new Promise((r) => setTimeout(r, backoffMs));
        result = await runJob(job, args.solver!, graderBin!);
      }
      results.push(result);
      done++;
      const mark =
        result.status === "pass" ? "✓" : result.status === "fail" ? "✗" : "‼";
      console.log(
        `[${done}/${jobs.length}] ${mark} ${result.task}#${result.attempt} ` +
          `${result.status} (${Math.round(result.wallclockMs / 1000)}s)`,
      );
      if (result.status === "error") {
        console.log(`    ${result.detail?.split("\n").slice(-2).join(" | ")}`);
        consecutiveErrors++;
        if (consecutiveErrors >= BREAKER_THRESHOLD) {
          consecutiveErrors = 0;
          breakerTrips++;
          if (breakerTrips >= BREAKER_MAX_TRIPS) {
            aborted = true;
            console.log(
              `breaker tripped ${breakerTrips}× — aborting. Rerun with --since to top up the remaining attempts.`,
            );
            return;
          }
          pausedUntil = Date.now() + BREAKER_COOLDOWN_MS;
          console.log(
            `breaker tripped (${BREAKER_THRESHOLD} consecutive errors) — pausing all workers ${BREAKER_COOLDOWN_MS / 60_000} min`,
          );
        }
      } else {
        consecutiveErrors = 0;
      }
    }
  }

  await Promise.all(
    Array.from({ length: Math.max(1, args.concurrency) }, () => worker()),
  );

  // ── summary ──────────────────────────────────────────────────────────
  const byTask = new Map<string, JobResult[]>();
  for (const r of results) {
    const list = byTask.get(r.task) ?? [];
    list.push(r);
    byTask.set(r.task, list);
  }
  let fullPass = 0;
  let anyError = false;
  console.log("\n── matrix summary ──");
  for (const meta of metas) {
    const rs = byTask.get(meta.id) ?? [];
    if (rs.length === 0) {
      console.log(`  · ${meta.id}: not attempted`);
      continue;
    }
    const passes = rs.filter((r) => r.status === "pass").length;
    const errors = rs.filter((r) => r.status === "error").length;
    if (errors > 0) anyError = true;
    const allPassed = passes === rs.length;
    if (allPassed) fullPass++;
    console.log(
      `  ${allPassed ? "✓" : "✗"} ${meta.id}: ${passes}/${rs.length} passed${errors ? ` (${errors} errors)` : ""}`,
    );
  }
  console.log(
    `\npass^k (all attempts green): ${fullPass}/${metas.length} tasks ` +
      `(${((100 * fullPass) / metas.length).toFixed(1)}%)`,
  );
  const attemptPass = results.filter((r) => r.status === "pass").length;
  console.log(
    `per-attempt pass rate: ${attemptPass}/${results.length} ` +
      `(${((100 * attemptPass) / results.length).toFixed(1)}%)`,
  );
  if (anyError) {
    console.log("some attempts errored — rerun with the same flags to top up.");
  }
  return anyError ? 2 : 0;
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(e);
    process.exit(2);
  },
);
