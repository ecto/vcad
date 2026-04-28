// Generate the static leaderboard HTML by walking mecheval/runs/ and
// computing pass^k aggregates. Output at mecheval/leaderboard/dist/index.html.
//
// Drafting-blueprint aesthetic: black on bone-white, Berkeley Mono with
// fallback, dimension-callout-style tables. Renders score as the
// dimension; cost-Pareto scatter overlay comes in a follow-up.

import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  loadAllRuns,
  modelSummary,
  passKBy,
  type ModelSummary,
  type PassKEntry,
  type RunMeta,
} from "@mecheval/harness/pass_k";

const PASS_K = 5;
const RUNS_DIR = resolve(process.cwd(), "mecheval/runs");
const OUT_DIR = resolve(process.cwd(), "mecheval/leaderboard/dist");
const OUT_HTML = resolve(OUT_DIR, "index.html");

function fmtNum(n: number, digits = 2): string {
  return n.toFixed(digits);
}

function fmtCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${Math.round(n)}`;
}

function escape(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function passKCell(e: PassKEntry, k: number): string {
  if (e.pass_k === null) {
    return `<span class="pending">${e.pass_count_in_recent_k}/${e.recent_k}*</span>`;
  }
  if (e.pass_k) return `<span class="pass">PASS</span>`;
  return `<span class="fail">${e.pass_count_in_recent_k}/${e.recent_k}</span>`;
}

function modelTable(models: ModelSummary[], k: number): string {
  const rows = models
    .map(
      (m) => `
      <tr>
        <td class="id">${escape(m.model_id)}</td>
        <td class="num">${m.tasks_attempted}</td>
        <td class="num">${m.total_attempts}</td>
        <td class="num">${m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—"}</td>
        <td class="num">${fmtNum(m.mean_score)}</td>
        <td class="num">${fmtCompact(m.mean_tokens)}</td>
        <td class="num">${fmtNum(m.mean_wallclock_sec, 1)}s</td>
      </tr>`,
    )
    .join("");
  return `
    <table class="board">
      <thead>
        <tr>
          <th>model</th>
          <th>tasks</th>
          <th>runs</th>
          <th>pass^${k}</th>
          <th>score</th>
          <th>tokens</th>
          <th>wall</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>`;
}

function entryTable(entries: PassKEntry[], k: number): string {
  const rows = entries
    .map(
      (e) => `
      <tr>
        <td class="id">${escape(e.model_id)}</td>
        <td class="id">${escape(e.task_id)}</td>
        <td class="num">${e.attempts}</td>
        <td class="num">${passKCell(e, k)}</td>
        <td class="num">${fmtNum(e.mean_score_recent_k)}</td>
        <td class="num">${fmtCompact(e.mean_tokens_recent_k)}</td>
        <td class="num">${fmtNum(e.mean_wallclock_recent_k, 1)}s</td>
      </tr>`,
    )
    .join("");
  return `
    <table class="board">
      <thead>
        <tr>
          <th>model</th>
          <th>task</th>
          <th>attempts</th>
          <th>pass^${k}</th>
          <th>score</th>
          <th>tokens</th>
          <th>wall</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>`;
}

function render(
  runs: RunMeta[],
  entries: PassKEntry[],
  models: ModelSummary[],
  k: number,
): string {
  const generatedAt = new Date().toISOString();
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>MechEval — leaderboard</title>
  <style>
    :root {
      --ink: #111;
      --ground: #fbf6ee;
      --rule: #111;
      --fail: #c0392b;
      --pass: #27ae60;
      --pending: #888;
    }
    html, body {
      background: var(--ground);
      color: var(--ink);
      font-family: "Berkeley Mono", "JetBrains Mono", ui-monospace, SFMono-Regular,
                   Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
      font-size: 13px;
      line-height: 1.4;
      margin: 0;
      padding: 0;
    }
    .frame {
      max-width: 1100px;
      margin: 0 auto;
      padding: 32px 28px 80px;
    }
    h1 {
      font-size: 26px;
      letter-spacing: 0.08em;
      margin: 0 0 6px;
      font-weight: 600;
    }
    h2 {
      font-size: 13px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      margin: 36px 0 12px;
      border-top: 1px solid var(--rule);
      padding-top: 12px;
    }
    .tagline {
      color: #666;
      margin-bottom: 28px;
    }
    .meta {
      color: #666;
      font-size: 11px;
      margin: 12px 0 0;
    }
    table.board {
      width: 100%;
      border-collapse: collapse;
      border-top: 1px solid var(--rule);
      border-bottom: 1px solid var(--rule);
    }
    table.board th {
      text-align: right;
      padding: 8px 10px;
      font-weight: 500;
      border-bottom: 1px solid var(--rule);
      letter-spacing: 0.04em;
      text-transform: uppercase;
      font-size: 10px;
      color: #444;
    }
    table.board th:first-child,
    table.board th:nth-child(2) {
      text-align: left;
    }
    table.board td {
      padding: 6px 10px;
      vertical-align: middle;
      border-bottom: 1px dotted #c8bfb1;
    }
    table.board tr:last-child td {
      border-bottom: none;
    }
    td.id { white-space: nowrap; }
    td.num {
      text-align: right;
      font-variant-numeric: tabular-nums;
    }
    .pass { color: var(--pass); font-weight: 600; letter-spacing: 0.04em; }
    .fail { color: var(--fail); }
    .pending { color: var(--pending); }
    .footnote { color: #888; margin-top: 14px; font-size: 11px; }
    .nodata { color: #888; padding: 20px 0; }
    code { background: rgba(0,0,0,0.05); padding: 1px 5px; }
  </style>
</head>
<body>
  <main class="frame">
    <h1>MechEval</h1>
    <div class="tagline">
      mechanical, physical, and CAD evaluation suite for AI models · pass<sup>${k}</sup>
    </div>

    <h2>Models</h2>
    ${models.length > 0 ? modelTable(models, k) : `<div class="nodata">no run blobs found under <code>mecheval/runs/</code></div>`}

    <h2>Per task · per model</h2>
    ${entries.length > 0 ? entryTable(entries, k) : `<div class="nodata">no entries</div>`}

    <p class="footnote">
      <span class="pending">k/k*</span> = fewer than ${k} attempts at this (model, task) — pass<sup>${k}</sup> pending.
      Score is the mean check-pass rate across the most recent ${k} attempts (or fewer, if fewer exist).
      Total run blobs in corpus: ${runs.length}.
    </p>
    <p class="meta">generated ${generatedAt} · static site, regenerate with <code>npm run build -w @mecheval/leaderboard</code></p>
  </main>
</body>
</html>
`;
}

async function main(): Promise<void> {
  const runs = await loadAllRuns(RUNS_DIR);
  const entries = passKBy(runs, PASS_K);
  const models = modelSummary(entries);
  const html = render(runs, entries, models, PASS_K);
  await mkdir(OUT_DIR, { recursive: true });
  await writeFile(OUT_HTML, html, "utf8");
  console.log(`wrote ${OUT_HTML} (${runs.length} run blobs, ${models.length} models)`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
