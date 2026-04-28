// Static site generator for the MechEval leaderboard.
//
// Walks mecheval/runs/ + mecheval/tasks/ and emits:
//   index.html                                  — leaderboard + matrix
//   task/<task_id>.html                          — one page per task
//   model/<model_id>.html                        — one page per model
//   run/<task_id>/<model_id>/<run_id>.html       — full forensic detail per attempt
//
// Identity lives in `tokens.ts` (colors, fonts, copy). This file is the
// renderer; touching the brand should mostly be a tokens-only change.

import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import {
  loadAllRuns,
  modelSummary,
  passKBy,
  type ModelSummary,
  type PassKEntry,
  type RunMeta,
} from "@mecheval/harness/pass_k";
import { colors, copy, fonts, fontsHref, type TitleBlock } from "./tokens.js";

const PASS_K = 5;
const REPO_ROOT = process.cwd();
const RUNS_DIR = resolve(REPO_ROOT, "mecheval/runs");
const TASKS_DIR = resolve(REPO_ROOT, "mecheval/tasks");
const OUT_DIR = resolve(REPO_ROOT, "mecheval/leaderboard/dist");

// ─── types ────────────────────────────────────────────────────────────────

interface TaskSpec {
  id: string;
  suite: string;
  tier: string;
  title: string;
  prompt: string;
  checks: Array<Record<string, unknown> & { type: string }>;
  anti_cheese?: Record<string, unknown>;
  limits?: Record<string, unknown>;
  pass_k?: number;
  tags?: string[];
}

interface FullBlob {
  schema_version: number;
  run_id: string;
  task_id: string;
  task_sha256: string;
  model: { id: string; name: string; provider: string; params: Record<string, unknown> };
  harness: Record<string, unknown>;
  submission_kind: string;
  prompt: { seed: string; rendered: string; attachments: string[] };
  trace: {
    tool_calls: Array<{
      n: number;
      tool: string;
      args: unknown;
      result_kind: string;
      wallclock_ms: number;
    }>;
    tokens: { input: number; output: number; total: number };
    wallclock_sec: number;
  };
  output: { vcad_path: string; vcad_sha256: string; control_policy: string | null };
  sim: unknown;
  checks: Array<{
    n: number;
    type: string;
    params: Record<string, unknown>;
    result: "pass" | "fail" | "not_implemented" | "error";
    details: Record<string, unknown>;
  }>;
  summary: {
    passed: boolean;
    checks_passed: number;
    checks_total: number;
    score: number;
    anti_cheese_violated: boolean;
    limits_exceeded: string[];
  };
  timestamps: { started_at: string; ended_at: string };
}

// ─── formatting ───────────────────────────────────────────────────────────

const fmtNum = (n: number, d = 2) => n.toFixed(d);
const fmtCompact = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M`
  : n >= 1_000 ? `${(n / 1_000).toFixed(1)}k`
  : `${Math.round(n)}`;

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function passKBadge(e: PassKEntry, k: number): string {
  if (e.pass_k === null) {
    return `<span class="pending">${e.pass_count_in_recent_k}/${e.recent_k}*</span>`;
  }
  if (e.pass_k) return `<span class="pass">PASS</span>`;
  return `<span class="fail">${e.pass_count_in_recent_k}/${e.recent_k}</span>`;
}

function outcomeBadge(o: string): string {
  if (o === "pass") return `<span class="pass">pass</span>`;
  if (o === "fail") return `<span class="fail">fail</span>`;
  if (o === "error") return `<span class="fail">error</span>`;
  return `<span class="pending">not impl</span>`;
}

// ─── shared chrome ────────────────────────────────────────────────────────

const STYLES = `
  :root {
    --ink: ${colors.ink};
    --ink-soft: ${colors.inkSoft};
    --ground: ${colors.ground};
    --rule: ${colors.rule};
    --fail: ${colors.fail};
    --pass: ${colors.pass};
    --pending: ${colors.pending};
    --soft: ${colors.soft};
    --accent: ${colors.accent};
    --display: ${fonts.display};
    --body: ${fonts.body};
  }
  * { box-sizing: border-box; }
  html, body {
    background-color: var(--ground);
    background-image:
      linear-gradient(to right, rgba(14,57,96,0.06) 1px, transparent 1px),
      linear-gradient(to bottom, rgba(14,57,96,0.06) 1px, transparent 1px);
    background-size: 24px 24px;
    color: var(--ink);
    font-family: var(--body);
    font-size: 13px;
    line-height: 1.45;
    margin: 0;
    padding: 0;
  }
  .sheet {
    max-width: 1180px;
    margin: 0 auto;
    padding: 28px 36px 28px;
    position: relative;
    background: var(--ground);
    border: 1px solid var(--ink);
    margin-top: 24px;
    margin-bottom: 24px;
  }
  /* Drafting corners — replace generic CSS borders with ASCII-feel marks. */
  .sheet::before, .sheet::after,
  .sheet > .corner-bl, .sheet > .corner-br {
    position: absolute;
    font-family: var(--body);
    font-size: 18px;
    color: var(--ink);
    line-height: 1;
    background: var(--ground);
    padding: 0 4px;
  }
  .sheet::before { content: "┌"; top: -10px; left: -1px; }
  .sheet::after  { content: "┐"; top: -10px; right: -1px; }
  .sheet .corner-bl { content: "└"; bottom: -10px; left: -1px; }
  .sheet .corner-br { content: "┘"; bottom: -10px; right: -1px; }

  .crumb { color: var(--ink-soft); font-size: 11px; margin: 0 0 18px 0; letter-spacing: 0.04em; }
  .crumb a { color: var(--ink); }

  /* Wordmark in the upper-left of the index hero. */
  .wordmark {
    font-family: var(--display);
    font-weight: 700;
    font-size: 56px;
    letter-spacing: -0.025em;
    color: var(--ink);
    margin: 0;
    line-height: 0.95;
  }
  .wordmark .dot { color: var(--accent); }
  .tagline-main {
    font-family: var(--display);
    font-weight: 500;
    font-size: 18px;
    color: var(--ink);
    margin: 14px 0 4px;
    max-width: 720px;
    letter-spacing: -0.005em;
  }
  .tagline-sub {
    font-size: 12px;
    color: var(--ink-soft);
    margin: 0 0 26px;
  }

  /* Title block — engineering-drawing convention, upper-right corner of the sheet. */
  .title-block {
    position: absolute;
    top: -1px;
    right: -1px;
    border-left: 1px solid var(--ink);
    border-bottom: 1px solid var(--ink);
    background: var(--ground);
    font-size: 9.5px;
    line-height: 1.3;
    letter-spacing: 0.04em;
    color: var(--ink);
    text-transform: uppercase;
  }
  .title-block table { border-collapse: collapse; }
  .title-block td {
    padding: 4px 8px;
    border-right: 1px solid var(--soft);
    border-bottom: 1px dotted var(--soft);
    vertical-align: middle;
    white-space: nowrap;
  }
  .title-block td:last-child { border-right: none; }
  .title-block tr:last-child td { border-bottom: none; }
  .title-block .k { color: var(--ink-soft); font-size: 8.5px; }
  .title-block .v { font-weight: 500; color: var(--ink); }

  h1 {
    font-family: var(--display);
    font-size: 28px;
    letter-spacing: -0.02em;
    margin: 0 0 4px;
    font-weight: 700;
    color: var(--ink);
  }
  h1 .tier { font-size: 11px; color: var(--ink-soft); margin-left: 10px; vertical-align: middle; letter-spacing: 0.08em; font-family: var(--body); font-weight: 500; }
  h2 {
    font-family: var(--display);
    font-size: 11px; font-weight: 600; letter-spacing: 0.16em; text-transform: uppercase;
    margin: 32px 0 12px; border-top: 1px solid var(--rule); padding-top: 14px; color: var(--ink);
  }

  .tagline { color: var(--ink-soft); margin-bottom: 24px; font-size: 12px; }
  .meta { color: var(--ink-soft); font-size: 10.5px; margin: 12px 0 0; }
  a { color: var(--ink); text-decoration: none; border-bottom: 1px dotted var(--soft); }
  a:hover { border-bottom-style: solid; border-bottom-color: var(--ink); }

  table.board {
    width: 100%; border-collapse: collapse;
    border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule);
  }
  table.board th {
    text-align: right; padding: 8px 10px; font-weight: 600;
    border-bottom: 1px solid var(--rule);
    letter-spacing: 0.1em; text-transform: uppercase; font-size: 10px; color: var(--ink);
    font-family: var(--display);
  }
  table.board th:first-child,
  table.board th.left { text-align: left; }
  table.board td { padding: 7px 10px; border-bottom: 1px dotted var(--soft); vertical-align: middle; }
  table.board tr:last-child td { border-bottom: none; }
  table.board tbody tr:hover { background: rgba(14,57,96,0.04); }
  td.id { white-space: nowrap; }
  td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .pass { color: var(--pass); font-weight: 700; letter-spacing: 0.06em; font-family: var(--display); }
  .fail { color: var(--fail); font-weight: 500; }
  .pending { color: var(--pending); }

  .footnote { color: var(--ink-soft); margin-top: 14px; font-size: 11px; }
  .nodata { color: var(--ink-soft); padding: 20px 0; font-style: italic; }

  code { background: rgba(14,57,96,0.06); padding: 1px 5px; font-family: var(--body); border-radius: 1px; }
  pre {
    background: rgba(14,57,96,0.04); padding: 12px 14px; overflow-x: auto;
    font-size: 12px; line-height: 1.55; border: 1px solid var(--soft);
    white-space: pre-wrap; word-break: break-word;
    color: var(--ink);
  }

  details { margin: 8px 0; }
  details summary { cursor: pointer; padding: 4px 0; color: var(--ink-soft); }
  details summary:hover { color: var(--ink); }
  details[open] summary { color: var(--ink); }

  .matrix table { border-collapse: collapse; }
  .matrix th, .matrix td {
    border: 1px solid var(--soft); padding: 7px 9px; min-width: 96px; text-align: center;
    font-size: 11px;
  }
  .matrix th {
    background: rgba(14,57,96,0.06); font-family: var(--display);
    font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; font-size: 10px;
  }
  .matrix td.row-h { text-align: left; font-weight: 500; background: rgba(14,57,96,0.04); }
  .matrix td a { display: block; }

  .checkrow { display: grid; grid-template-columns: 28px 180px 80px 1fr; gap: 10px; padding: 7px 0; border-bottom: 1px dotted var(--soft); }
  .checkrow .n { text-align: right; color: var(--ink-soft); }
  .checkrow:last-child { border-bottom: none; }
  .toolrow { display: grid; grid-template-columns: 28px 180px 70px 80px 1fr; gap: 10px; padding: 5px 0; border-bottom: 1px dotted var(--soft); font-size: 12px; }
  .toolrow .n { text-align: right; color: var(--ink-soft); }
  .kvtable td { padding: 3px 12px 3px 0; vertical-align: top; }
  .kvtable td.k { color: var(--ink-soft); white-space: nowrap; text-transform: uppercase; font-size: 10px; letter-spacing: 0.08em; }

  /* Footer — Municipal Robotics ↔ vcad ↔ mecheval. */
  .footer {
    border-top: 1px solid var(--ink);
    margin-top: 40px;
    padding: 18px 0 4px;
    font-size: 11px;
    color: var(--ink-soft);
    display: flex;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px;
  }
  .footer a { color: var(--ink); }
  .footer .stack { letter-spacing: 0.04em; }
  .footer .stack b {
    font-family: var(--display); font-weight: 700; letter-spacing: -0.01em; color: var(--ink);
  }
`;

function titleBlockHtml(tb: TitleBlock, generatedAt: string): string {
  const dateStr = generatedAt.slice(0, 10);
  return `<div class="title-block"><table>
    <tr>
      <td><span class="k">drawing</span><br><span class="v">${escape(tb.drawing)}</span></td>
      <td><span class="k">sheet</span><br><span class="v">${escape(tb.sheet)}</span></td>
      <td><span class="k">scale</span><br><span class="v">${escape(tb.scale ?? "1 : 1")}</span></td>
    </tr>
    <tr>
      <td><span class="k">date</span><br><span class="v">${escape(dateStr)}</span></td>
      <td><span class="k">drawn by</span><br><span class="v">muni</span></td>
      <td><span class="k">project</span><br><span class="v">mecheval</span></td>
    </tr>
  </table></div>`;
}

function footerHtml(): string {
  return `<div class="footer">
    <div class="stack">
      <b>${escape(copy.brand)}</b> &middot; an evaluation suite by <a href="${copy.footerOwnerUrl}">${escape(copy.footerOwner)}</a>
    </div>
    <div>
      sibling project: <a href="${copy.siblingProjectUrl}">${escape(copy.siblingProjectName)}</a>
      &middot; <a href="${copy.repoUrl}">github</a>
    </div>
  </div>`;
}

function pageShell(
  title: string,
  crumbHtml: string,
  bodyHtml: string,
  tb: TitleBlock,
): string {
  const generatedAt = new Date().toISOString();
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${escape(title)}</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="${fontsHref}">
<style>${STYLES}</style>
</head>
<body><main class="sheet">
<span class="corner-bl">└</span><span class="corner-br">┘</span>
${titleBlockHtml(tb, generatedAt)}
<div class="crumb">${crumbHtml}</div>
${bodyHtml}
${footerHtml()}
<p class="meta">generated ${generatedAt} · static site, regenerate with <code>npm run build -w @mecheval/leaderboard</code></p>
</main></body></html>`;
}

// ─── tables ───────────────────────────────────────────────────────────────

function modelTable(models: ModelSummary[], k: number): string {
  const rows = models
    .map(
      (m) => `
      <tr>
        <td class="id"><a href="model/${encodeURIComponent(m.model_id)}.html">${escape(m.model_id)}</a></td>
        <td class="num">${m.tasks_attempted}</td>
        <td class="num">${m.total_attempts}</td>
        <td class="num">${m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—"}</td>
        <td class="num">${fmtNum(m.mean_score)}</td>
        <td class="num">${fmtCompact(m.mean_tokens)}</td>
        <td class="num">${fmtNum(m.mean_wallclock_sec, 1)}s</td>
      </tr>`,
    )
    .join("");
  return `<table class="board">
    <thead><tr>
      <th class="left">model</th><th>tasks</th><th>runs</th>
      <th>pass^${k}</th><th>score</th><th>tokens</th><th>wall</th>
    </tr></thead><tbody>${rows}</tbody></table>`;
}

function matrix(
  taskIds: string[],
  modelIds: string[],
  byPair: Map<string, PassKEntry>,
  k: number,
): string {
  const head = modelIds
    .map(
      (mid) => `<th><a href="model/${encodeURIComponent(mid)}.html">${escape(mid)}</a></th>`,
    )
    .join("");
  const body = taskIds
    .map((tid) => {
      const cells = modelIds
        .map((mid) => {
          const entry = byPair.get(`${mid}::${tid}`);
          if (!entry) return `<td class="pending">—</td>`;
          return `<td>${passKBadge(entry, k)} <small>${fmtNum(entry.mean_score_recent_k)}</small></td>`;
        })
        .join("");
      return `<tr><td class="row-h"><a href="task/${encodeURIComponent(tid)}.html">${escape(tid)}</a></td>${cells}</tr>`;
    })
    .join("");
  return `<div class="matrix"><table>
    <thead><tr><th class="left">task ↓ · model →</th>${head}</tr></thead>
    <tbody>${body}</tbody>
  </table></div>`;
}

function paretoScatter(
  entries: PassKEntry[],
  xKey: "tokens" | "wallclock",
): string {
  const points = entries.filter((e) => {
    const x = xKey === "tokens" ? e.mean_tokens_recent_k : e.mean_wallclock_recent_k;
    return x > 0;
  });
  if (!points.length) return `<div class="nodata">no points</div>`;

  const W = 760;
  const H = 280;
  const PAD_L = 56;
  const PAD_R = 16;
  const PAD_T = 26;
  const PAD_B = 36;
  const innerW = W - PAD_L - PAD_R;
  const innerH = H - PAD_T - PAD_B;

  const xVal = (e: PassKEntry) =>
    xKey === "tokens" ? e.mean_tokens_recent_k : e.mean_wallclock_recent_k;
  const xs = points.map(xVal);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minLog = Math.log10(Math.max(1, minX));
  const maxLog = Math.log10(Math.max(2, maxX));
  const span = Math.max(0.5, maxLog - minLog);
  const xLo = minLog - span * 0.08;
  const xHi = maxLog + span * 0.08;

  const xPos = (v: number) =>
    PAD_L + ((Math.log10(Math.max(1, v)) - xLo) / (xHi - xLo)) * innerW;
  const yPos = (score: number) => PAD_T + (1 - score) * innerH;

  const palette = ["#2c3e50", "#c0392b", "#27ae60", "#8e44ad", "#d68910"];
  const modelIds = [...new Set(points.map((p) => p.model_id))];
  const colorOf = (m: string) =>
    palette[modelIds.indexOf(m) % palette.length];

  const xTicks: number[] = [];
  for (let p = Math.ceil(xLo); p <= Math.floor(xHi); p++) xTicks.push(p);
  const yTicks = [0, 0.25, 0.5, 0.75, 1.0];

  const xAxisLines = xTicks
    .map((p) => {
      const v = Math.pow(10, p);
      const x = xPos(v);
      const lbl =
        xKey === "wallclock"
          ? p === 0 ? "1s" : p === 1 ? "10s" : p === 2 ? "100s" : `1e${p}s`
          : p < 3 ? `10^${p}`
          : p === 3 ? "1k"
          : p === 4 ? "10k"
          : p === 5 ? "100k"
          : p === 6 ? "1M"
          : `1e${p}`;
      return `
        <line x1="${x}" y1="${PAD_T}" x2="${x}" y2="${H - PAD_B}" stroke="#dcd2c0" stroke-dasharray="2,3"/>
        <text x="${x}" y="${H - PAD_B + 14}" text-anchor="middle" font-size="10" fill="#666">${lbl}</text>`;
    })
    .join("");

  const yAxisLines = yTicks
    .map((v) => {
      const y = yPos(v);
      return `
        <line x1="${PAD_L}" y1="${y}" x2="${W - PAD_R}" y2="${y}" stroke="#dcd2c0" stroke-dasharray="2,3"/>
        <text x="${PAD_L - 8}" y="${y + 3}" text-anchor="end" font-size="10" fill="#666">${v.toFixed(2)}</text>`;
    })
    .join("");

  const dots = points
    .map((e) => {
      const cx = xPos(xVal(e));
      const cy = yPos(e.mean_score_recent_k);
      const color = colorOf(e.model_id);
      const tt = `${e.model_id} · ${e.task_id}\nscore=${e.mean_score_recent_k.toFixed(2)}\ntokens=${fmtCompact(e.mean_tokens_recent_k)} · wall=${e.mean_wallclock_recent_k.toFixed(1)}s\nattempts=${e.attempts}`;
      return `<a href="run-link-${e.task_id}-${e.model_id}.html"><circle cx="${cx}" cy="${cy}" r="5.5" fill="${color}" fill-opacity="0.7" stroke="${color}" stroke-width="1.4"><title>${escape(tt)}</title></circle></a>`;
    })
    .join("");

  // Replace placeholder href with a real one (task page is the safest target —
  // we don't always have a single-run url for an aggregated entry).
  const dotsLinked = dots.replace(
    /href="run-link-([^"]+?)-((?:claude|default)[^"]+)"/g,
    (_, t) => `href="task/${encodeURIComponent(t as string)}.html"`,
  );

  const legend = modelIds
    .map((m, i) => {
      const cx = PAD_L + i * 250;
      return `<g transform="translate(${cx}, ${PAD_T - 12})">
        <circle cx="0" cy="0" r="4" fill="${colorOf(m)}"/>
        <text x="9" y="3" font-size="10" fill="#333">${escape(m)}</text>
      </g>`;
    })
    .join("");

  const xLabel = xKey === "tokens" ? "tokens (log)" : "wall-clock seconds (log)";

  return `<svg viewBox="0 0 ${W} ${H}" width="100%" role="img" aria-label="${xLabel} vs score scatter" style="border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule); background: rgba(0,0,0,0.015);">
    ${yAxisLines}
    ${xAxisLines}
    <rect x="${PAD_L}" y="${PAD_T}" width="${innerW}" height="${innerH}" fill="none" stroke="#111"/>
    <text x="${PAD_L + innerW / 2}" y="${H - 4}" text-anchor="middle" font-size="10" fill="#333">${xLabel}</text>
    <text transform="translate(${PAD_L - 38}, ${PAD_T + innerH / 2}) rotate(-90)" text-anchor="middle" font-size="10" fill="#333">score</text>
    ${legend}
    ${dotsLinked}
  </svg>`;
}

function entryTable(entries: PassKEntry[], k: number): string {
  const rows = entries
    .map(
      (e) => `
      <tr>
        <td class="id"><a href="model/${encodeURIComponent(e.model_id)}.html">${escape(e.model_id)}</a></td>
        <td class="id"><a href="task/${encodeURIComponent(e.task_id)}.html">${escape(e.task_id)}</a></td>
        <td class="num">${e.attempts}</td>
        <td class="num">${passKBadge(e, k)}</td>
        <td class="num">${fmtNum(e.mean_score_recent_k)}</td>
        <td class="num">${fmtCompact(e.mean_tokens_recent_k)}</td>
        <td class="num">${fmtNum(e.mean_wallclock_recent_k, 1)}s</td>
      </tr>`,
    )
    .join("");
  return `<table class="board">
    <thead><tr>
      <th class="left">model</th><th class="left">task</th>
      <th>attempts</th><th>pass^${k}</th><th>score</th>
      <th>tokens</th><th>wall</th>
    </tr></thead><tbody>${rows}</tbody></table>`;
}

function runsTable(runs: RunMeta[], pageKind: "task" | "model"): string {
  const rows = runs
    .slice()
    .sort((a, b) => b.run_id.localeCompare(a.run_id))
    .map((r) => {
      const detailHref = `${pageKind === "task" ? ".." : ".."}/run/${encodeURIComponent(r.task_id)}/${encodeURIComponent(r.model_id)}/${encodeURIComponent(r.run_id)}.html`;
      const colA = pageKind === "task"
        ? `<a href="../model/${encodeURIComponent(r.model_id)}.html">${escape(r.model_id)}</a>`
        : `<a href="../task/${encodeURIComponent(r.task_id)}.html">${escape(r.task_id)}</a>`;
      const status = r.passed
        ? `<span class="pass">PASS</span>`
        : `<span class="fail">fail</span>`;
      return `<tr>
        <td class="id">${colA}</td>
        <td class="id"><a href="${detailHref}">${escape(r.run_id)}</a></td>
        <td class="num">${status}</td>
        <td class="num">${fmtNum(r.score)}</td>
        <td class="num">${fmtCompact(r.tokens_total)}</td>
        <td class="num">${fmtNum(r.wallclock_sec, 1)}s</td>
      </tr>`;
    })
    .join("");
  const header = pageKind === "task" ? "model" : "task";
  return `<table class="board">
    <thead><tr>
      <th class="left">${header}</th><th class="left">run</th>
      <th>status</th><th>score</th><th>tokens</th><th>wall</th>
    </tr></thead><tbody>${rows}</tbody></table>`;
}

// ─── page renderers ───────────────────────────────────────────────────────

function indexPage(
  runs: RunMeta[],
  entries: PassKEntry[],
  models: ModelSummary[],
  taskIds: string[],
  k: number,
): string {
  const byPair = new Map<string, PassKEntry>();
  for (const e of entries) byPair.set(`${e.model_id}::${e.task_id}`, e);
  const modelIds = models.map((m) => m.model_id);
  const passKAchieved = models.reduce((a, m) => a + m.pass_k_full, 0);
  const passKReady = models.reduce((a, m) => a + m.pass_k_total, 0);
  const body = `
    <h1 class="wordmark">${escape(copy.brand.slice(0, -1))}<span class="dot">.</span></h1>
    <div class="tagline-main">${escape(copy.tagline)}</div>
    <div class="tagline-sub">${escape(copy.subtagline)} · pass<sup>${k}</sup></div>

    <h2>Models</h2>
    ${models.length ? modelTable(models, k) : `<div class="nodata">no run blobs found under <code>mecheval/runs/</code> — looks lonely</div>`}

    <h2>Task × model matrix</h2>
    ${taskIds.length && modelIds.length ? matrix(taskIds, modelIds, byPair, k) : `<div class="nodata">no entries</div>`}

    <h2>Cost · score Pareto</h2>
    ${entries.length ? paretoScatter(entries, "tokens") : `<div class="nodata">no points</div>`}
    <p class="footnote">tokens (log) vs mean score across the most recent ${k} attempts. Each dot is one (model, task) pair; click to drill into the task page.</p>

    ${entries.length ? `<div style="margin-top: 18px;">${paretoScatter(entries, "wallclock")}</div>
    <p class="footnote">wall-clock seconds (log) vs mean score. The left edge is fast; the right edge is patient.</p>` : ""}

    <h2>Per task · per model</h2>
    ${entries.length ? entryTable(entries, k) : `<div class="nodata">no entries</div>`}

    <p class="footnote">
      <span class="pending">k/k*</span> = fewer than ${k} attempts at this (model, task) — pass<sup>${k}</sup> pending.
      Score is the mean check-pass rate across the most recent ${k} attempts.
      Corpus: ${runs.length} run blobs across ${models.length} models, ${taskIds.length} tasks. Click any task, model, or run for full forensic detail.
    </p>`;
  return pageShell(
    "mecheval — AI builds the mech",
    `${escape(copy.brand)}`,
    body,
    {
      drawing: copy.brand,
      sheet: "INDEX",
      scale: passKReady > 0 ? `pass^${k} · ${passKAchieved}/${passKReady}` : `pass^${k}`,
    },
  );
}

function taskPage(spec: TaskSpec | null, taskId: string, runsForTask: RunMeta[]): string {
  if (!spec) {
    return pageShell(
      `mecheval — ${taskId}`,
      `<a href="../index.html">← ${escape(copy.brand)}</a> / task / ${escape(taskId)}`,
      `<h1>${escape(taskId)}</h1><div class="nodata">no task spec found at mecheval/tasks/${escape(taskId)}.json — OPERATOR can't find this one</div>`,
      { drawing: copy.brand, sheet: `TASK · ${taskId}`, scale: "—" },
    );
  }
  const checksHtml = spec.checks
    .map(
      (c, i) => `
      <div class="checkrow">
        <div class="n">${i}</div>
        <div><code>${escape(c.type)}</code></div>
        <div></div>
        <div><pre style="margin: 0; padding: 6px 8px; font-size: 11px;">${escape(JSON.stringify(c, null, 2))}</pre></div>
      </div>`,
    )
    .join("");
  const acHtml = spec.anti_cheese && Object.keys(spec.anti_cheese).length
    ? `<pre>${escape(JSON.stringify(spec.anti_cheese, null, 2))}</pre>`
    : `<div class="nodata">none</div>`;
  const limitsHtml = spec.limits && Object.keys(spec.limits).length
    ? `<pre>${escape(JSON.stringify(spec.limits, null, 2))}</pre>`
    : `<div class="nodata">none</div>`;
  const body = `
    <h1>${escape(spec.title)} <span class="tier">${escape(spec.suite)} · ${escape(spec.tier)} · ${escape(taskId)}</span></h1>
    <div class="tagline">${escape((spec.tags ?? []).join(" · "))}</div>

    <h2>Prompt</h2>
    <pre>${escape(spec.prompt)}</pre>

    <h2>Checks</h2>
    ${checksHtml || `<div class="nodata">no checks</div>`}

    <h2>Anti-cheese</h2>
    ${acHtml}

    <h2>Limits</h2>
    ${limitsHtml}

    <h2>Runs (${runsForTask.length})</h2>
    ${runsForTask.length ? runsTable(runsForTask, "task") : `<div class="nodata">no runs yet for this task</div>`}
  `;
  return pageShell(
    `mecheval — ${taskId}`,
    `<a href="../index.html">← ${escape(copy.brand)}</a> / task / ${escape(taskId)}`,
    body,
    {
      drawing: copy.brand,
      sheet: `TASK · ${taskId}`,
      scale: `${runsForTask.length} runs`,
    },
  );
}

function modelPage(modelId: string, runsForModel: RunMeta[]): string {
  const body = `
    <h1>${escape(modelId)}</h1>
    <div class="tagline">${runsForModel.length} run blobs across ${new Set(runsForModel.map((r) => r.task_id)).size} tasks</div>

    <h2>All runs</h2>
    ${runsForModel.length ? runsTable(runsForModel, "model") : `<div class="nodata">no runs</div>`}
  `;
  const taskCount = new Set(runsForModel.map((r) => r.task_id)).size;
  return pageShell(
    `mecheval — ${modelId}`,
    `<a href="../index.html">← ${escape(copy.brand)}</a> / model / ${escape(modelId)}`,
    body,
    {
      drawing: copy.brand,
      sheet: `MODEL · ${modelId}`,
      scale: `${runsForModel.length} runs · ${taskCount} tasks`,
    },
  );
}

function runPage(blob: FullBlob, vcad: string | null): string {
  const taskId = blob.task_id;
  const modelId = blob.model.id;
  const runId = blob.run_id;

  const summaryHtml = `
    <table class="kvtable">
      <tr><td class="k">status</td><td>${blob.summary.passed ? `<span class="pass">PASS</span>` : `<span class="fail">fail</span>`}</td></tr>
      <tr><td class="k">score</td><td>${fmtNum(blob.summary.score)} (${blob.summary.checks_passed}/${blob.summary.checks_total})</td></tr>
      <tr><td class="k">submission</td><td>${escape(blob.submission_kind)}</td></tr>
      <tr><td class="k">model</td><td>${escape(modelId)} (${escape(blob.model.provider)})</td></tr>
      <tr><td class="k">started</td><td>${escape(blob.timestamps.started_at)}</td></tr>
      <tr><td class="k">ended</td><td>${escape(blob.timestamps.ended_at)}</td></tr>
      <tr><td class="k">tokens</td><td>${blob.trace.tokens.input.toLocaleString()} in · ${blob.trace.tokens.output.toLocaleString()} out · ${blob.trace.tokens.total.toLocaleString()} total</td></tr>
      <tr><td class="k">wallclock</td><td>${fmtNum(blob.trace.wallclock_sec, 1)}s</td></tr>
      <tr><td class="k">tool calls</td><td>${blob.trace.tool_calls.length}</td></tr>
      <tr><td class="k">task hash</td><td><code>${escape(blob.task_sha256.slice(0, 16))}…</code></td></tr>
      <tr><td class="k">vcad hash</td><td><code>${escape(blob.output.vcad_sha256.slice(0, 16))}…</code></td></tr>
    </table>`;

  const checksHtml = blob.checks
    .map((c) => {
      return `
      <div class="checkrow">
        <div class="n">${c.n}</div>
        <div><code>${escape(c.type)}</code></div>
        <div>${outcomeBadge(c.result)}</div>
        <div>
          <details>
            <summary>params + details</summary>
            <pre>params: ${escape(JSON.stringify(c.params, null, 2))}\n\ndetails: ${escape(JSON.stringify(c.details, null, 2))}</pre>
          </details>
        </div>
      </div>`;
    })
    .join("");

  const traceHtml = blob.trace.tool_calls.length
    ? blob.trace.tool_calls
        .map(
          (tc) => `
      <div class="toolrow">
        <div class="n">${tc.n}</div>
        <div><code>${escape(tc.tool)}</code></div>
        <div class="num"><span class="${tc.result_kind === "ok" ? "pass" : "fail"}">${escape(tc.result_kind)}</span></div>
        <div class="num">${tc.wallclock_ms.toFixed(0)}ms</div>
        <div><details><summary>args</summary><pre>${escape(JSON.stringify(tc.args, null, 2))}</pre></details></div>
      </div>`,
        )
        .join("")
    : `<div class="nodata">no tool calls (single-shot solver)</div>`;

  const vcadHtml = vcad
    ? `<pre>${escape(vcad)}</pre>`
    : `<div class="nodata">.vcad not present beside this blob</div>`;

  const body = `
    <h1>run ${escape(runId)}</h1>
    <div class="tagline">
      <a href="../../../task/${encodeURIComponent(taskId)}.html">${escape(taskId)}</a> ·
      <a href="../../../model/${encodeURIComponent(modelId)}.html">${escape(modelId)}</a>
    </div>

    <h2>Summary</h2>
    ${summaryHtml}

    <h2>Prompt</h2>
    <pre>${escape(blob.prompt.rendered)}</pre>

    <h2>Checks</h2>
    ${checksHtml}

    <h2>Tool calls</h2>
    ${traceHtml}

    <h2>.vcad output</h2>
    ${vcadHtml}
  `;
  return pageShell(
    `mecheval — ${taskId} / ${modelId} / ${runId}`,
    `<a href="../../../index.html">← ${escape(copy.brand)}</a> / run / ${escape(taskId)} / ${escape(modelId)} / ${escape(runId)}`,
    body,
    {
      drawing: copy.brand,
      sheet: `RUN · ${runId}`,
      scale: blob.summary.passed
        ? "PASS"
        : `${blob.summary.checks_passed}/${blob.summary.checks_total}`,
    },
  );
}

// ─── data loading ─────────────────────────────────────────────────────────

async function loadTaskSpecs(): Promise<Map<string, TaskSpec>> {
  const out = new Map<string, TaskSpec>();
  let entries: string[];
  try {
    entries = await readdir(TASKS_DIR);
  } catch {
    return out;
  }
  for (const e of entries) {
    if (!e.endsWith(".json")) continue;
    const raw = await readFile(join(TASKS_DIR, e), "utf8");
    const spec = JSON.parse(raw) as TaskSpec;
    out.set(spec.id, spec);
  }
  return out;
}

async function loadFullBlob(blobPath: string): Promise<FullBlob> {
  const raw = await readFile(blobPath, "utf8");
  return JSON.parse(raw) as FullBlob;
}

async function loadVcadIfPresent(blobPath: string): Promise<string | null> {
  const vcadPath = blobPath.replace(/\.json$/, ".vcad");
  try {
    return await readFile(vcadPath, "utf8");
  } catch {
    return null;
  }
}

// ─── main ─────────────────────────────────────────────────────────────────

async function writePage(relPath: string, html: string): Promise<void> {
  const abs = resolve(OUT_DIR, relPath);
  await mkdir(abs.replace(/\/[^/]+$/, ""), { recursive: true });
  await writeFile(abs, html, "utf8");
}

async function main(): Promise<void> {
  const runs = await loadAllRuns(RUNS_DIR);
  const entries = passKBy(runs, PASS_K);
  const models = modelSummary(entries);
  const taskSpecs = await loadTaskSpecs();
  const taskIds = [...taskSpecs.keys()].sort();
  const seenTaskIds = new Set([...taskIds, ...runs.map((r) => r.task_id)]);

  // Index.
  await writePage(
    "index.html",
    indexPage(runs, entries, models, [...seenTaskIds].sort(), PASS_K),
  );

  // Task pages.
  for (const tid of seenTaskIds) {
    const runsForTask = runs.filter((r) => r.task_id === tid);
    await writePage(
      `task/${tid}.html`,
      taskPage(taskSpecs.get(tid) ?? null, tid, runsForTask),
    );
  }

  // Model pages.
  const modelIds = new Set(runs.map((r) => r.model_id));
  for (const mid of modelIds) {
    const runsForModel = runs.filter((r) => r.model_id === mid);
    await writePage(`model/${mid}.html`, modelPage(mid, runsForModel));
  }

  // Run pages — load each blob in full, attach the .vcad inline.
  for (const r of runs) {
    const blob = await loadFullBlob(r.blob_path);
    const vcad = await loadVcadIfPresent(r.blob_path);
    await writePage(
      `run/${r.task_id}/${r.model_id}/${r.run_id}.html`,
      runPage(blob, vcad),
    );
  }

  console.log(
    `wrote ${OUT_DIR}: index + ${seenTaskIds.size} tasks + ${modelIds.size} models + ${runs.length} runs`,
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
