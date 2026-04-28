#!/usr/bin/env node
// `mecheval-leaderboard` — read every blob under mecheval/runs/, aggregate
// pass^k by (model, task), and print a leaderboard table.
//
// Usage:
//   mecheval-leaderboard [--runs <path>] [-k <int>]

import { resolve } from "node:path";
import { loadAllRuns, modelSummary, passKBy } from "./pass_k.js";

function parseArgs(argv: string[]): { runs: string; k: number; help: boolean } {
  const out = { runs: "mecheval/runs", k: 5, help: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--runs") out.runs = argv[++i];
    else if (a === "-k" || a === "--k") out.k = parseInt(argv[++i], 10);
    else if (a === "-h" || a === "--help") out.help = true;
  }
  return out;
}

function pad(s: string, n: number, right = false): string {
  if (s.length >= n) return s.slice(0, n);
  return right ? " ".repeat(n - s.length) + s : s + " ".repeat(n - s.length);
}

function fmtScore(s: number): string {
  return s.toFixed(2);
}

function fmtCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n.toFixed(0)}`;
}

async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.error("usage: mecheval-leaderboard [--runs <path>] [-k <int>]");
    return 0;
  }
  const runs = await loadAllRuns(resolve(args.runs));
  if (runs.length === 0) {
    console.error(`no runs found under ${args.runs}`);
    return 2;
  }
  const entries = passKBy(runs, args.k);
  const models = modelSummary(entries);

  console.log(`MechEval leaderboard — pass^${args.k}, ${runs.length} total run blobs\n`);

  // Per-model summary.
  console.log(
    pad("model", 38) +
      pad("tasks", 7, true) +
      pad("runs", 6, true) +
      pad(`pass^${args.k}`, 10, true) +
      pad("score", 8, true) +
      pad("tokens", 9, true) +
      pad("wall(s)", 9, true),
  );
  console.log("-".repeat(38 + 7 + 6 + 10 + 8 + 9 + 9));
  for (const m of models) {
    const passK =
      m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—";
    console.log(
      pad(m.model_id, 38) +
        pad(`${m.tasks_attempted}`, 7, true) +
        pad(`${m.total_attempts}`, 6, true) +
        pad(passK, 10, true) +
        pad(fmtScore(m.mean_score), 8, true) +
        pad(fmtCompact(m.mean_tokens), 9, true) +
        pad(m.mean_wallclock_sec.toFixed(1), 9, true),
    );
  }

  // Per-task breakdown grouped by model.
  console.log();
  console.log(
    pad("model · task", 60) +
      pad("attempts", 10, true) +
      pad(`pass^${args.k}`, 10, true) +
      pad("score", 8, true) +
      pad("tokens", 9, true),
  );
  console.log("-".repeat(60 + 10 + 10 + 8 + 9));
  for (const e of entries) {
    const id = `${e.model_id} · ${e.task_id}`;
    const passK =
      e.pass_k === null
        ? `${e.pass_count_in_recent_k}/${e.recent_k}*`
        : e.pass_k
          ? "PASS"
          : `${e.pass_count_in_recent_k}/${e.recent_k}`;
    console.log(
      pad(id, 60) +
        pad(`${e.attempts}`, 10, true) +
        pad(passK, 10, true) +
        pad(fmtScore(e.mean_score_recent_k), 8, true) +
        pad(fmtCompact(e.mean_tokens_recent_k), 9, true),
    );
  }
  console.log(`\n* fewer than k=${args.k} attempts; pass^k pending`);

  return 0;
}

main().then((c) => process.exit(c));
