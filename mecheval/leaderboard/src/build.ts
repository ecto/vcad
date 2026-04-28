// Static site generator for the MechEval leaderboard.
//
// Walks mecheval/runs/ + mecheval/tasks/ and emits:
//   index.html                                  — leaderboard + matrix
//   task/<task_id>.html                          — one page per task
//   model/<model_id>.html                        — one page per model
//   run/<task_id>/<model_id>/<run_id>.html       — full forensic detail per attempt
//
// Drafting-blueprint aesthetic: black on bone-white, Berkeley Mono with
// fallback, dotted-rule dimension-callout dividers.

import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";
import {
  loadAllRuns,
  modelSummary,
  passKBy,
  type ModelSummary,
  type PassKEntry,
  type RunMeta,
} from "@mecheval/harness/pass_k";

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

function relLink(fromAbs: string, toAbs: string): string {
  const r = relative(fromAbs.replace(/\/[^/]+$/, ""), toAbs);
  return r.split("/").map(encodeURIComponent).join("/");
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
    --ink: #111;
    --ground: #fbf6ee;
    --rule: #111;
    --fail: #c0392b;
    --pass: #27ae60;
    --pending: #888;
    --soft: #c8bfb1;
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
  .frame { max-width: 1180px; margin: 0 auto; padding: 28px 28px 80px; }
  .crumb { color: #777; font-size: 11px; margin-bottom: 8px; letter-spacing: 0.04em; }
  .crumb a { color: var(--ink); }
  h1 { font-size: 22px; letter-spacing: 0.06em; margin: 0 0 4px; font-weight: 600; }
  h1 .tier { font-size: 11px; color: #777; margin-left: 10px; vertical-align: middle; letter-spacing: 0.08em; }
  h2 {
    font-size: 11px; letter-spacing: 0.08em; text-transform: uppercase;
    margin: 32px 0 12px; border-top: 1px solid var(--rule); padding-top: 12px; color: #333;
  }
  .tagline { color: #666; margin-bottom: 24px; font-size: 12px; }
  .meta { color: #666; font-size: 11px; margin: 12px 0 0; }
  a { color: var(--ink); text-decoration: none; border-bottom: 1px dotted var(--soft); }
  a:hover { border-bottom-style: solid; }
  table.board {
    width: 100%; border-collapse: collapse;
    border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule);
  }
  table.board th {
    text-align: right; padding: 8px 10px; font-weight: 500;
    border-bottom: 1px solid var(--rule);
    letter-spacing: 0.04em; text-transform: uppercase; font-size: 10px; color: #444;
  }
  table.board th:first-child,
  table.board th.left { text-align: left; }
  table.board td { padding: 6px 10px; border-bottom: 1px dotted var(--soft); vertical-align: middle; }
  table.board tr:last-child td { border-bottom: none; }
  td.id { white-space: nowrap; }
  td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .pass { color: var(--pass); font-weight: 600; letter-spacing: 0.04em; }
  .fail { color: var(--fail); }
  .pending { color: var(--pending); }
  .footnote { color: #888; margin-top: 14px; font-size: 11px; }
  .nodata { color: #888; padding: 20px 0; }
  code { background: rgba(0,0,0,0.05); padding: 1px 5px; }
  pre {
    background: rgba(0,0,0,0.04); padding: 12px 14px; overflow-x: auto;
    font-size: 12px; line-height: 1.5; border: 1px solid var(--soft);
    white-space: pre-wrap; word-break: break-word;
  }
  details { margin: 8px 0; }
  details summary { cursor: pointer; padding: 4px 0; }
  details summary:hover { color: #444; }
  .matrix table { border-collapse: collapse; }
  .matrix th, .matrix td {
    border: 1px solid var(--soft); padding: 6px 8px; min-width: 90px; text-align: center;
    font-size: 11px;
  }
  .matrix th { background: rgba(0,0,0,0.03); }
  .matrix td.row-h { text-align: left; font-weight: 500; background: rgba(0,0,0,0.02); }
  .matrix td a { display: block; }
  .checkrow { display: grid; grid-template-columns: 28px 180px 80px 1fr; gap: 8px; padding: 6px 0; border-bottom: 1px dotted var(--soft); }
  .checkrow .n { text-align: right; color: #888; }
  .checkrow:last-child { border-bottom: none; }
  .toolrow { display: grid; grid-template-columns: 28px 180px 70px 80px 1fr; gap: 8px; padding: 4px 0; border-bottom: 1px dotted var(--soft); font-size: 12px; }
  .toolrow .n { text-align: right; color: #888; }
  .kvtable td { padding: 3px 10px 3px 0; vertical-align: top; }
  .kvtable td.k { color: #666; white-space: nowrap; }
`;

function pageShell(title: string, crumbHtml: string, bodyHtml: string): string {
  const generatedAt = new Date().toISOString();
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${escape(title)}</title>
<style>${STYLES}</style>
</head>
<body><main class="frame">
<div class="crumb">${crumbHtml}</div>
${bodyHtml}
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
  const body = `
    <h1>MechEval</h1>
    <div class="tagline">mechanical, physical, and CAD evaluation suite for AI models · pass<sup>${k}</sup></div>

    <h2>Models</h2>
    ${models.length ? modelTable(models, k) : `<div class="nodata">no run blobs found under <code>mecheval/runs/</code></div>`}

    <h2>Task × model matrix</h2>
    ${taskIds.length && modelIds.length ? matrix(taskIds, modelIds, byPair, k) : `<div class="nodata">no entries</div>`}

    <h2>Per task · per model</h2>
    ${entries.length ? entryTable(entries, k) : `<div class="nodata">no entries</div>`}

    <p class="footnote">
      <span class="pending">k/k*</span> = fewer than ${k} attempts at this (model, task) — pass<sup>${k}</sup> pending.
      Score is the mean check-pass rate across the most recent ${k} attempts.
      Total run blobs in corpus: ${runs.length}. Click any task, model, or run for full forensic detail.
    </p>`;
  return pageShell("MechEval — leaderboard", "MechEval", body);
}

function taskPage(spec: TaskSpec | null, taskId: string, runsForTask: RunMeta[]): string {
  if (!spec) {
    return pageShell(
      `MechEval — ${taskId}`,
      `<a href="../index.html">← MechEval</a>`,
      `<h1>${escape(taskId)}</h1><div class="nodata">no task spec found at mecheval/tasks/${escape(taskId)}.json</div>`,
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
    `MechEval — ${taskId}`,
    `<a href="../index.html">← MechEval</a> / task / ${escape(taskId)}`,
    body,
  );
}

function modelPage(modelId: string, runsForModel: RunMeta[]): string {
  const body = `
    <h1>${escape(modelId)}</h1>
    <div class="tagline">${runsForModel.length} run blobs across ${new Set(runsForModel.map((r) => r.task_id)).size} tasks</div>

    <h2>All runs</h2>
    ${runsForModel.length ? runsTable(runsForModel, "model") : `<div class="nodata">no runs</div>`}
  `;
  return pageShell(
    `MechEval — ${modelId}`,
    `<a href="../index.html">← MechEval</a> / model / ${escape(modelId)}`,
    body,
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
    `MechEval — ${taskId} / ${modelId} / ${runId}`,
    `<a href="../../../index.html">← MechEval</a> / run / ${escape(taskId)} / ${escape(modelId)} / ${escape(runId)}`,
    body,
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
