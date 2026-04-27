#!/usr/bin/env node
// `mecheval-run` — drive a solver against a task, emit a run blob.
//
// Usage:
//   mecheval-run --task <path-or-id> --solver <id> [--out-dir <path>] [--grader-bin <path>]

import { resolve } from "node:path";
import { runOne } from "./run.js";

interface CliArgs {
  task: string | null;
  solver: string | null;
  outDir: string | null;
  graderBin: string | null;
  help: boolean;
}

function parseArgs(argv: string[]): CliArgs {
  const out: CliArgs = {
    task: null,
    solver: null,
    outDir: null,
    graderBin: null,
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    switch (a) {
      case "--task": out.task = next(); break;
      case "--solver": out.solver = next(); break;
      case "--out-dir": out.outDir = next(); break;
      case "--grader-bin": out.graderBin = next(); break;
      case "-h":
      case "--help": out.help = true; break;
      default:
        if (a.startsWith("-")) {
          console.error(`unknown flag: ${a}`);
          process.exit(2);
        }
    }
  }
  return out;
}

function usage(): void {
  console.error(`usage: mecheval-run --task <path-or-id> --solver <id> [--out-dir <path>] [--grader-bin <path>]

Args:
  --task         path to a task JSON, or a bare task id (resolved against mecheval/tasks/)
  --solver       solver id (skeleton ships only "default-cube")
  --out-dir      override output directory (default: mecheval/runs/<task>/<solver>/)
  --grader-bin   path to mecheval-grade (default: target/debug/mecheval-grade)

Exit codes:
  0  attempt completed AND every check passed
  1  attempt completed but some check failed
  2  harness or grader error (no run blob produced)
`);
}

function resolveTaskPath(input: string): string {
  if (input.endsWith(".json")) return resolve(input);
  // Bare id form: look up in the in-monorepo public corpus.
  return resolve("mecheval", "tasks", `${input}.json`);
}

async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help || !args.task || !args.solver) {
    usage();
    return args.help ? 0 : 2;
  }
  try {
    const { blob, blobPath } = await runOne({
      taskPath: resolveTaskPath(args.task),
      solverId: args.solver,
      outDir: args.outDir ?? undefined,
      graderBin: args.graderBin ?? undefined,
    });
    console.log(`wrote ${blobPath}`);
    console.log(
      `summary: passed=${blob.summary.passed} score=${blob.summary.score.toFixed(3)} ` +
      `(${blob.summary.checks_passed}/${blob.summary.checks_total} checks)`,
    );
    return blob.summary.passed ? 0 : 1;
  } catch (e) {
    console.error(`harness error: ${(e as Error).message}`);
    return 2;
  }
}

main().then((code) => process.exit(code));
