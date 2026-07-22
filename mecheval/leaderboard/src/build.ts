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

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  loadAllRuns,
  modelSummary,
  passKBy,
  summarizeCheckFailure,
  type ModelSummary,
  type PassKEntry,
  type RunMeta,
} from "@mecheval/harness/pass_k";
import { copy, domains, family, fonts, fontsHref, theme, type TitleBlock } from "./tokens.js";

const PASS_K = 5;
// Path prefix for the MechEval chapter inside dist/. All of the chapter's
// internal links are relative, so the whole site relocates by prefixing
// its emitted paths here. mecheval.com 301s to /mech (vercel.json).
const MECH = domains.find((d) => d.slug === "mech")!.pathPrefix;
// Resolve REPO_ROOT relative to this script (mecheval/leaderboard/dist/build.js)
// so we work the same whether invoked via `npm run build -w …` (cwd = leaderboard
// package), `node mecheval/leaderboard/dist/build.js` (cwd = repo root), or
// Vercel (cwd = the configured Root Directory).
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "../../..");
const RUNS_DIR = resolve(REPO_ROOT, "mecheval/runs");
const TASKS_DIR = resolve(REPO_ROOT, "mecheval/tasks");
const OUT_DIR = resolve(REPO_ROOT, "mecheval/leaderboard/dist");
const CACHE_DIR = resolve(REPO_ROOT, "mecheval/leaderboard/cache");
// Resolution order:
//   1. VCAD_RENDER_BIN  — current name
//   2. MECHEVAL_RENDER_BIN  — legacy name from when this lived in
//      mecheval/graders/. Kept as a one-release bridge so anyone with
//      the old env var or cached scripts doesn't suddenly lose renders.
//   3. target/debug/vcad-render  — default after `cargo build -p vcad-render`.
const RENDER_BIN =
  process.env.VCAD_RENDER_BIN ??
  process.env.MECHEVAL_RENDER_BIN ??
  resolve(REPO_ROOT, "target/debug/vcad-render");

/** Spawn vcad-render against a `.vcad` and capture the SVG. Returns
 *  null if the binary is missing or rendering errors out. */
function renderVcad(vcadPath: string): string | null {
  if (!existsSync(RENDER_BIN)) return null;
  try {
    return execFileSync(RENDER_BIN, [vcadPath, "--transparent"], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (e) {
    console.warn(`renderVcad failed for ${vcadPath}: ${(e as Error).message}`);
    return null;
  }
}

/** Cache-first render. Looks for a pre-rendered SVG at
 *  `mecheval/leaderboard/cache/<cacheKey>.svg`; if absent, falls back to
 *  the live binary and writes the result to cache. Lets Vercel builds
 *  succeed without Rust as long as someone has run `npm run build`
 *  locally and committed the cache.
 *
 *  Returns:
 *  - cached svg string if cache hit,
 *  - newly rendered svg if cache miss + binary available (and writes
 *    it to cache),
 *  - null if both fail. */
async function getOrRenderSvg(
  vcadPath: string,
  cacheKey: string,
): Promise<string | null> {
  const cachePath = resolve(CACHE_DIR, `${cacheKey}.svg`);
  if (existsSync(cachePath)) {
    return readFile(cachePath, "utf8");
  }
  const svg = renderVcad(vcadPath);
  if (svg) {
    await mkdir(cachePath.replace(/\/[^/]+$/, ""), { recursive: true });
    await writeFile(cachePath, svg, "utf8");
  }
  return svg;
}

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
/** Friendly display name for a model id.
 *  - "claude-direct-claude-opus-4-7" → "Opus 4.7 (direct)"
 *  - "claude-mcp-claude-opus-4-7"   → "Opus 4.7 (mcp)"
 *  - "default-cube"                  → "DEFAULT_CUBE"
 */
function modelDisplayName(modelId: string): string {
  if (modelId === "default-cube" || modelId === "DEFAULT_CUBE") {
    return "DEFAULT_CUBE";
  }
  const tokens = modelId.split("-");
  for (let i = 0; i < tokens.length; i++) {
    if (/^(opus|sonnet|haiku)$/i.test(tokens[i])) {
      const family = tokens[i][0].toUpperCase() + tokens[i].slice(1).toLowerCase();
      const ver: string[] = [];
      let j = i + 1;
      while (j < tokens.length && /^\d+$/.test(tokens[j])) {
        ver.push(tokens[j]);
        j++;
      }
      const v = ver.length ? " " + ver.join(".") : "";
      const mode =
        tokens.includes("direct") ? "direct"
        : tokens.includes("mcp") ? "mcp"
        : null;
      return `${family}${v}${mode ? ` (${mode})` : ""}`;
    }
  }
  return modelId;
}

/** Brand identity for a model id: a human label, the harness mode (mcp/
 *  direct), and a brand colour for charts. Drives the leaderboard.
 *  - "openai-direct-gpt-5"            → { label: "GPT-5", color: green }
 *  - "claude-mcp-claude-opus-4-7"     → { label: "Claude Opus 4.7", mode: "mcp" }
 *  - "wafer-direct-GLM-5.2"           → { label: "GLM-5.2", color: violet }
 */
/** Collapse solver-harness variants of the same model into one identity.
 *  "claude-direct-claude-opus-4-7" and "claude-mcp-claude-opus-4-7" are the
 *  same model reached through different harness plumbing; the harness must
 *  not affect the result, so the leaderboard aggregates them together.
 *  Strips `direct`/`mcp` tokens and collapses the duplicated family token
 *  that leaves behind. */
function canonicalModelId(modelId: string): string {
  const tokens = modelId.split("-");
  const drop = new Set(["direct", "mcp"]);
  let parts = tokens.filter((t) => !drop.has(t.toLowerCase()));
  parts = parts.filter((t, i) => i === 0 || t.toLowerCase() !== parts[i - 1].toLowerCase());
  return parts.join("-");
}

function modelIdentity(modelId: string): {
  label: string;
  color: string;
  provider: "openai" | "anthropic" | "zhipu" | "google" | "meta" | null;
} {
  const lower = modelId.toLowerCase();
  const provider =
    lower.includes("gpt") || lower.startsWith("openai") ? "openai" as const
    : lower.includes("claude") || lower.startsWith("anthropic") ? "anthropic" as const
    : lower.includes("glm") || lower.startsWith("zhipu") ? "zhipu" as const
    : lower.includes("gemini") || lower.startsWith("google") ? "google" as const
    : lower.includes("llama") || lower.startsWith("meta") ? "meta" as const
    : null;
  // Brand-accurate accents.
  const color =
    provider === "openai" ? "#10a37f"
    : provider === "anthropic" ? "#cc785c"
    : provider === "zhipu" ? "#3859ff"
    : provider === "google" ? "#1a73e8"
    : provider === "meta" ? "#0866ff"
    : "var(--accent)";

  const tokens = modelId.split("-");

  if (modelId === "default-cube" || modelId === "DEFAULT_CUBE") {
    return { label: "Default cube", color: "var(--ink-faint)", provider: null };
  }

  // Drop provider + mode tokens, collapse a duplicated family token
  // (e.g. "claude-direct-claude-opus" → "claude-opus"), and strip a
  // trailing release-date stamp.
  const drop = new Set(["openai", "anthropic", "google", "xai", "meta", "wafer", "zhipu", "direct", "mcp"]);
  let parts = tokens.filter((t) => !drop.has(t.toLowerCase()));
  parts = parts.filter((t, i) => i === 0 || t.toLowerCase() !== parts[i - 1].toLowerCase());
  parts = parts.filter((t) => !/^\d{8}$/.test(t));

  const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();
  const head = (parts[0] ?? "").toLowerCase();
  let label: string;
  if (head === "gpt") {
    const rest = parts.slice(1);
    const hasMini = rest[rest.length - 1]?.toLowerCase() === "mini";
    const ver = (hasMini ? rest.slice(0, -1) : rest).join("-");
    label = `GPT-${ver}${hasMini ? " mini" : ""}`.replace(/-$/, "");
  } else if (head === "claude") {
    // Drop the "Claude" prefix — the brand colour already signals the
    // family, and the shorter "Opus 4.7" keeps labels on one line.
    const tier = parts[1] ? cap(parts[1]) : "";
    const ver = parts.slice(2).filter((x) => /^\d+$/.test(x)).join(".");
    label = `${tier}${ver ? ` ${ver}` : ""}`.trim();
  } else if (head === "glm") {
    label = `GLM-${parts.slice(1).join(".")}`.replace(/-$/, "");
  } else {
    label = parts.map(cap).join(" ");
  }

  return { label: label || modelId, color, provider };
}

/** An inline SVG mark for a model's provider, tinted to the brand colour
 *  (via `fill:currentColor`, so callers set `color`). Returns a small
 *  geometric glyph for known providers, else an empty string. */
function providerMark(
  provider: "openai" | "anthropic" | "zhipu" | "google" | "meta" | null,
): string {
  const open = `<svg class="brand-mark" viewBox="0 0 24 24" role="img" aria-hidden="true">`;
  switch (provider) {
    case "openai":
      // OpenAI blossom mark.
      return `${open}<path d="M22.28 9.82a5.98 5.98 0 0 0-.52-4.91 6.05 6.05 0 0 0-6.51-2.9A6.07 6.07 0 0 0 4.98 4.18a5.98 5.98 0 0 0-4 2.9 6.05 6.05 0 0 0 .75 7.1 5.98 5.98 0 0 0 .51 4.91 6.05 6.05 0 0 0 6.51 2.9A5.98 5.98 0 0 0 13.26 24a6.06 6.06 0 0 0 5.77-4.21 5.99 5.99 0 0 0 4-2.9 6.06 6.06 0 0 0-.75-7.07Zm-9.02 12.6a4.48 4.48 0 0 1-2.88-1.04l.14-.08 4.78-2.76a.79.79 0 0 0 .39-.68v-6.74l2.02 1.17a.07.07 0 0 1 .04.05v5.58a4.5 4.5 0 0 1-4.49 4.49ZM3.6 18.3a4.47 4.47 0 0 1-.53-3.01l.14.08 4.78 2.76a.77.77 0 0 0 .78 0l5.84-3.37v2.33a.08.08 0 0 1-.03.06L9.74 19.95a4.5 4.5 0 0 1-6.14-1.65ZM2.34 7.9a4.49 4.49 0 0 1 2.37-1.98v5.7a.77.77 0 0 0 .39.68l5.81 3.35-2.02 1.17a.08.08 0 0 1-.07 0l-4.83-2.79A4.5 4.5 0 0 1 2.34 7.9Zm16.6 3.85-5.84-3.39 2.02-1.16a.08.08 0 0 1 .07 0l4.83 2.79a4.49 4.49 0 0 1-.68 8.1v-5.67a.79.79 0 0 0-.4-.67Zm2.01-3.02-.14-.09-4.77-2.78a.78.78 0 0 0-.79 0L9.41 9.23V6.9a.07.07 0 0 1 .03-.06l4.83-2.79a4.5 4.5 0 0 1 6.68 4.66ZM8.31 12.86l-2.02-1.16a.08.08 0 0 1-.04-.06V6.07a4.5 4.5 0 0 1 7.38-3.45l-.14.08L8.7 5.46a.79.79 0 0 0-.39.68Zm1.1-2.37 2.6-1.5 2.61 1.5v3l-2.6 1.5-2.6-1.5Z"/></svg>`;
    case "anthropic":
      // Anthropic / Claude burst mark.
      return `${open}<path d="M17.3 3.54h-3.67l6.7 16.92H24Zm-10.61 0L0 20.46h3.74l1.37-3.55h7l1.37 3.55h3.75L10.54 3.54Zm-.37 10.22 2.29-5.94 2.29 5.94Z"/></svg>`;
    case "zhipu":
      // GLM / Zhipu — angular Z lettermark.
      return `${open}<path d="M5 3h14v3.4l-9.1 11.2H19V21H5v-3.4l9.1-11.2H5Z"/></svg>`;
    case "google":
      // Gemini — four-point spark.
      return `${open}<path d="M12 2c.5 4.9 5.1 9.5 10 10-4.9.5-9.5 5.1-10 10-.5-4.9-5.1-9.5-10-10C6.9 11.5 11.5 6.9 12 2Z"/></svg>`;
    case "meta":
      // Llama / Meta — infinity-ish double loop, simplified to a spark.
      return `${open}<circle cx="8" cy="12" r="5.2"/><circle cx="16" cy="12" r="5.2" fill="none" stroke="currentColor" stroke-width="2.6"/></svg>`;
    default:
      return "";
  }
}

const fmtCompact = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M`
  : n >= 1_000 ? `${(n / 1_000).toFixed(1)}k`
  : `${Math.round(n)}`;

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function passKBadge(e: PassKEntry, k: number): string {
  if (e.pass_k === null) {
    return `<span class="pill pill-pending">${e.pass_count_in_recent_k}/${e.recent_k}*</span>`;
  }
  if (e.pass_k) return `<span class="pill pill-pass">PASS</span>`;
  return `<span class="pill pill-fail">${e.pass_count_in_recent_k}/${e.recent_k}</span>`;
}

function outcomeBadge(o: string): string {
  if (o === "pass") return `<span class="pill pill-pass">pass</span>`;
  if (o === "fail") return `<span class="pill pill-fail">fail</span>`;
  if (o === "error") return `<span class="pill pill-fail">error</span>`;
  return `<span class="pill pill-pending">not impl</span>`;
}

// ─── shared chrome ────────────────────────────────────────────────────────

/** Emit semantic colour + system tokens as CSS custom properties for one
 *  theme. Spacing, radius, shadow, and type scale are theme-independent and
 *  declared once in :root below. */
function cssVars(t: typeof theme.light | typeof theme.dark): string {
  return [
    `--ground:${t.ground}`,
    `--surface:${t.surface}`,
    `--sunken:${t.sunken}`,
    `--ink:${t.ink}`,
    `--ink-soft:${t.inkSoft}`,
    `--ink-faint:${t.inkFaint}`,
    `--rule:${t.rule}`,
    `--rule-strong:${t.ruleStrong}`,
    `--soft:${t.soft}`,
    `--hover:${t.hover}`,
    `--accent:${t.accent}`,
    `--accent-deep:${t.accentDeep}`,
    `--accent-soft:${t.accentSoft}`,
    `--pass:${t.pass}`,
    `--fail:${t.fail}`,
    `--pending:${t.pending}`,
    `--mat:${t.mat}`,
    `--mat-rule:${t.matRule}`,
    `--shadow:${t.shadow}`,
    `--shadow-lift:${t.shadowLift}`,
  ].join(";");
}

const STYLES = `
  :root {
    ${cssVars(theme.light)};

    --sans: ${fonts.body};
    --mono: ${fonts.mono};

    /* Spacing scale (4pt grid). */
    --s1: 4px; --s2: 8px; --s3: 12px; --s4: 16px; --s5: 24px;
    --s6: 32px; --s7: 48px; --s8: 64px; --s9: 96px;

    --radius: 10px;
    --radius-sm: 7px;
    --radius-lg: 14px;
    --maxw: 940px;
    --ease: cubic-bezier(0.22, 0.61, 0.36, 1);
  }

  @media (prefers-color-scheme: dark) {
    :root { ${cssVars(theme.dark)}; }
  }

  /* ─── reset & base ──────────────────────────────────────────────── */
  * { box-sizing: border-box; }
  html { scroll-behavior: smooth; }
  html, body {
    background: var(--ground);
    color: var(--ink);
    font-family: var(--sans);
    font-size: 16px;
    line-height: 1.6;
    margin: 0;
    padding: 0;
    font-feature-settings: "cv05" 1, "ss01" 1;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    text-rendering: optimizeLegibility;
  }
  ::selection { background: color-mix(in srgb, var(--accent) 22%, transparent); }

  .sheet {
    max-width: var(--maxw);
    margin: 0 auto;
    padding: var(--s9) var(--s6) var(--s8);
    position: relative;
  }
  .sheet::before, .sheet::after,
  .sheet > .corner-bl, .sheet > .corner-br { display: none; }
  .title-block { display: none; }

  /* ─── links ─────────────────────────────────────────────────────── */
  a {
    color: var(--accent);
    text-decoration: none;
    transition: color 0.15s var(--ease);
  }
  a:hover { color: var(--accent-deep); }
  a:visited { color: var(--accent); }
  :focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
    border-radius: 4px;
  }

  .crumb {
    font-size: 13px; margin: 0 0 var(--s6);
    color: var(--ink-faint);
    letter-spacing: 0.01em;
  }
  .crumb a { color: var(--ink-soft); font-weight: 500; }
  .crumb a:hover { color: var(--accent); }

  /* ─── headings & editorial type ─────────────────────────────────── */
  h1 {
    font-family: var(--sans);
    font-size: clamp(38px, 5vw, 52px);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.05;
    margin: 0 0 var(--s3);
    color: var(--ink);
  }
  h1 .tier {
    font-family: var(--mono);
    font-size: 12px; color: var(--ink-faint);
    margin-left: var(--s3); vertical-align: middle;
    font-weight: 500; letter-spacing: 0;
  }

  h2 {
    font-family: var(--sans);
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    margin: var(--s8) 0 var(--s4);
    color: var(--ink-faint);
    display: flex; align-items: center; gap: var(--s3);
  }
  h2::after {
    content: ""; flex: 1; height: 1px;
    background: var(--rule);
  }

  .tagline-main {
    font-family: var(--sans);
    font-size: clamp(17px, 2vw, 20px);
    font-weight: 400;
    color: var(--ink-soft);
    margin: 0 0 var(--s5);
    max-width: 36ch;
    line-height: 1.5;
  }
  .tagline { color: var(--ink-soft); margin-bottom: var(--s5); font-size: 14px; }
  .meta {
    color: var(--ink-faint); font-size: 12px;
    margin: var(--s5) 0 0; font-family: var(--mono);
  }

  /* ─── data tables ───────────────────────────────────────────────── */
  table.board {
    width: 100%; border-collapse: separate; border-spacing: 0;
    font-size: 14.5px;
    margin: var(--s2) 0;
  }
  table.board th {
    text-align: right; padding: 10px 14px;
    font-weight: 600; font-size: 11px;
    text-transform: uppercase; letter-spacing: 0.06em;
    border-bottom: 1px solid var(--rule);
    color: var(--ink-faint);
    white-space: nowrap;
  }
  table.board th:first-child, table.board th.left { text-align: left; padding-left: 0; }
  table.board td {
    padding: 11px 14px;
    border-bottom: 1px solid var(--soft);
    vertical-align: middle;
  }
  table.board td:first-child { padding-left: 0; }
  table.board tbody tr:last-child td { border-bottom: none; }
  table.board tbody tr { transition: background 0.12s var(--ease); }
  table.board tbody tr:hover { background: var(--hover); }
  td.id { white-space: nowrap; font-weight: 500; }
  td.id a { font-family: var(--mono); font-size: 13px; font-weight: 500; }
  td.num {
    text-align: right;
    font-variant-numeric: tabular-nums;
    font-feature-settings: "tnum" 1;
    color: var(--ink);
  }

  .pass { color: var(--pass); font-weight: 600; }
  .fail { color: var(--fail); font-weight: 500; }
  .pending { color: var(--pending); }

  /* Status pills for the matrix + run rows. */
  .pill {
    display: inline-block; padding: 1px 8px;
    border-radius: 999px; font-size: 11px; font-weight: 600;
    letter-spacing: 0.04em; line-height: 1.6;
  }
  .pill-pass { color: var(--pass); background: color-mix(in srgb, var(--pass) 13%, transparent); }
  .pill-fail { color: var(--fail); background: color-mix(in srgb, var(--fail) 12%, transparent); }
  .pill-pending { color: var(--pending); background: color-mix(in srgb, var(--pending) 14%, transparent); }

  .footnote {
    color: var(--ink-soft); margin-top: var(--s4);
    font-size: 13.5px; line-height: 1.6; max-width: 72ch;
  }
  .nodata { color: var(--ink-soft); padding: var(--s4) 0; }

  code {
    font-family: var(--mono); font-size: 0.84em;
    background: var(--sunken);
    border: 1px solid var(--soft);
    border-radius: 6px; padding: 1px 6px;
  }
  pre {
    font-family: var(--mono);
    background: var(--surface);
    padding: var(--s4) var(--s5); overflow-x: auto;
    font-size: 12.5px; line-height: 1.6;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    white-space: pre-wrap; word-break: break-word;
    color: var(--ink);
  }
  pre code { background: none; border: none; padding: 0; font-size: 1em; }

  details { margin: var(--s2) 0; }
  details summary {
    cursor: pointer; padding: var(--s1) 0; color: var(--accent);
    font-weight: 500; font-size: 14px;
  }

  /* ─── the matrix: a gallery of reference renders ────────────────── */
  .matrix table {
    border-collapse: separate; border-spacing: 0; margin: var(--s2) 0;
  }
  .matrix th, .matrix td {
    border-bottom: 1px solid var(--soft);
    padding: 10px 12px; min-width: 104px; text-align: center;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .matrix tbody tr:last-child td { border-bottom: none; }
  .matrix th {
    font-weight: 600; font-size: 11px;
    text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--ink-faint);
    border-bottom: 1px solid var(--rule);
  }
  .matrix th a { font-family: var(--mono); font-size: 11px; color: var(--ink-soft); }
  .matrix td.row-h { text-align: left; font-weight: 500; }
  .matrix td.row-h a { font-family: var(--mono); font-size: 12px; }
  .matrix td.row-ref {
    width: 116px; min-width: 116px; padding: 8px 12px 8px 0;
    vertical-align: middle;
    position: sticky; left: 0;
    background: var(--ground);
    z-index: 1;
  }
  .matrix th:first-child {
    position: sticky; left: 0;
    background: var(--ground);
    z-index: 2;
  }
  .matrix tbody tr:hover td { background: var(--hover); }
  .matrix tbody tr:hover td.row-ref { background: var(--ground); }
  .matrix .matrix-ref {
    display: flex; align-items: center; justify-content: center;
    height: 84px;
    transition: opacity 0.16s var(--ease);
  }
  .matrix .matrix-ref { opacity: 0.92; }
  .matrix tbody tr:hover .matrix-ref { opacity: 1; }
  .matrix .matrix-ref svg { width: 100px; height: 78px; }
  .matrix .matrix-ref-empty { color: var(--ink-faint); font-size: 12px; text-align: center; }
  .matrix td a { display: block; }
  .matrix td .pill { margin-bottom: 2px; }
  .matrix td small { color: var(--ink-faint); font-size: 11px; }

  /* ─── leaderboard column chart (vertical, brand-themed) ─────────── */
  .vbars {
    display: flex; align-items: flex-end;
    gap: var(--s4);
    padding: 30px 0 var(--s2);
    margin: var(--s2) 0;
    overflow-x: auto;
  }
  .vbar {
    flex: 1 1 0; min-width: 76px;
    display: flex; flex-direction: column; align-items: center;
    text-decoration: none; color: inherit;
  }
  /* plot area — fixed-height reference for 0..1; bars share a bottom line */
  .vbar-col {
    width: 100%; height: 196px;
    display: flex; flex-direction: column; justify-content: flex-end;
    align-items: center;
    overflow: visible;
  }
  .vbar-val {
    font-variant-numeric: tabular-nums; font-weight: 600;
    font-size: 14px; letter-spacing: -0.01em; color: var(--ink);
    margin-bottom: 5px;
  }
  .vbar-fill {
    width: 64%; max-width: 54px; min-height: 3px;
    border-radius: 5px 5px 0 0;
    background: var(--c, var(--accent));
    opacity: 0.82;
    transition: opacity 0.14s var(--ease), transform 0.14s var(--ease);
    transform-origin: bottom;
  }
  .vbar:hover .vbar-fill { opacity: 1; transform: scaleY(1.015); }
  .vbar.lead .vbar-fill {
    opacity: 1;
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--c, var(--accent)) 55%, transparent),
                0 6px 16px color-mix(in srgb, var(--c, var(--accent)) 30%, transparent);
  }
  .vbar-label {
    margin-top: 11px; text-align: center; line-height: 1.25;
    font-family: var(--mono); font-size: 11px; font-weight: 500;
    color: var(--ink-soft);
    max-width: 100%; word-break: break-word;
    min-height: 30px;
  }
  .vbar:hover .vbar-label { color: var(--accent); }
  .vbar.lead .vbar-label { color: var(--ink); font-weight: 700; }
  .vbar-name {
    display: inline-flex; align-items: center; gap: 5px;
    justify-content: center; flex-wrap: wrap;
  }
  .brand-mark {
    width: 13px; height: 13px; flex: none;
    color: var(--c, var(--accent)); fill: currentColor;
  }
  .vbar-label .dot {
    display: inline-block; width: 7px; height: 7px; border-radius: 50%;
    background: var(--c, var(--accent)); vertical-align: baseline;
  }
  .vbar-label small {
    display: block; margin-top: 3px;
    font-size: 10px; color: var(--ink-faint); font-weight: 400;
    font-variant-numeric: tabular-nums;
  }
  .vbar-label .mode {
    display: inline-block; margin-top: 3px; font-size: 9px;
    text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--ink-faint);
  }

  /* ─── hero ──────────────────────────────────────────────────────── */
  .hero {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: var(--s7);
    margin: 0 0 var(--s6);
  }
  .wordmark { display: flex; align-items: center; gap: 14px; }
  .wordmark .dot {
    width: 11px; height: 11px; border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 4px var(--accent-soft);
    flex: none; margin-bottom: 2px;
  }
  .hero .mascot { align-self: center; }
  .hero .mascot svg { height: 280px; width: auto; display: block; }

  /* hero metric — a quiet figure, no frame */
  .hero-metric {
    display: flex; align-items: center; gap: var(--s6);
    margin: var(--s6) 0 var(--s7);
    padding: 0;
  }
  .hero-metric-num {
    font-family: var(--sans);
    font-size: clamp(52px, 7vw, 68px);
    font-weight: 600; letter-spacing: -0.035em;
    line-height: 0.94;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }
  .hero-metric-meta { flex: 1; min-width: 0; }
  .hero-metric-label {
    font-family: var(--sans); font-size: 14px;
    color: var(--ink-soft); margin-bottom: var(--s2); line-height: 1.5;
  }
  .hero-spark { display: block; color: var(--accent); }
  .hero-spark-single { font-size: 13px; color: var(--ink-faint); }
  .hero-metric-range {
    font-size: 11px; color: var(--ink-faint);
    margin-top: var(--s1); font-family: var(--mono); letter-spacing: 0.02em;
  }

  /* ─── stat strip — summary figures, no rules ────────────────────── */
  .stat-row {
    display: grid; grid-template-columns: repeat(3, 1fr);
    gap: var(--s7);
    margin: 0 0 var(--s8);
  }
  .stat-cell { padding: 0; }
  .stat-num {
    font-family: var(--sans);
    font-size: 40px; font-weight: 600; letter-spacing: -0.025em;
    line-height: 1; color: var(--ink);
    font-variant-numeric: tabular-nums;
  }
  .stat-num .stat-denom { color: var(--ink-faint); font-weight: 400; }
  .stat-num.stat-num-text { font-size: 22px; }
  .stat-label {
    font-size: 11px; font-weight: 600;
    text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--ink-faint); margin-top: var(--s3);
  }
  .stat-label sup { letter-spacing: 0; }

  /* leading row in the models table — text emphasis, no fill */
  table.board tbody tr.rank-1 td.id a { font-weight: 700; color: var(--accent-deep); }
  table.board tbody tr.rank-1 td.id::before { content: "▸ "; color: var(--accent); }

  /* check / tool rows on detail pages */
  .checkrow { display: grid; grid-template-columns: 28px 200px 80px 1fr; gap: var(--s3); padding: var(--s2) 0; border-bottom: 1px solid var(--soft); font-size: 14px; }
  .checkrow .n { text-align: right; color: var(--ink-faint); font-family: var(--mono); font-size: 12px; }
  .check-reason { color: var(--fail); font-size: 14px; margin-bottom: var(--s1); }
  .checkrow:last-child { border-bottom: none; }
  .toolrow { display: grid; grid-template-columns: 28px 200px 70px 80px 1fr; gap: var(--s3); padding: 6px 0; border-bottom: 1px solid var(--soft); font-size: 14px; }
  .toolrow .n { text-align: right; color: var(--ink-faint); font-family: var(--mono); font-size: 12px; }
  .kvtable td { padding: 4px 14px 4px 0; vertical-align: top; font-size: 14.5px; }
  .kvtable td.k { color: var(--ink-faint); white-space: nowrap; }

  /* ─── renders sit directly on the page — no frame, no fill ──────── */
  .run-render {
    padding: var(--s5) 0;
    margin: var(--s2) 0 var(--s1);
    display: flex; justify-content: center;
  }
  .run-render svg { max-height: 460px; max-width: 100%; display: block; }

  .run-gallery {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(184px, 1fr));
    gap: var(--s4); margin: var(--s2) 0 var(--s1);
  }
  .run-card {
    display: block;
    text-decoration: none; color: var(--ink);
    transition: opacity 0.16s var(--ease);
  }
  .run-card:hover { opacity: 0.78; }
  .run-card-svg {
    height: 168px; display: flex; align-items: center; justify-content: center;
    padding: var(--s3) 0;
    border-bottom: 1px solid var(--soft);
  }
  .run-card-svg svg { max-height: 140px; max-width: 100%; }
  .run-card-empty { color: var(--ink-faint); font-size: 13px; }
  .run-card-meta {
    padding: 9px 12px; display: flex; justify-content: space-between;
    align-items: center; gap: var(--s2); font-size: 13.5px;
  }
  .run-card-model { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--mono); font-size: 12px; }
  .run-card-fail {
    padding: 6px 12px 9px; color: var(--fail);
    font-size: 12.5px; line-height: 1.4; border-top: 1px solid var(--soft);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .fail-summary { color: var(--fail); font-size: 14px; }
  .fail-summary code { color: var(--fail); }
  .muted { color: var(--ink-faint); }

  /* ─── scatter chart ─────────────────────────────────────────────── */
  .chart-frame {
    padding: var(--s4) 0 var(--s2);
    margin: var(--s2) 0;
  }

  /* ─── task page ─────────────────────────────────────────────────── */
  .task-head { margin-bottom: var(--s5); }
  .task-tags { margin-top: var(--s3); display: flex; flex-wrap: wrap; gap: var(--s4); }
  .task-tags .tag {
    font-family: var(--mono); font-size: 11.5px; color: var(--ink-faint);
  }
  .task-tags .tag::before { content: "#"; opacity: 0.45; }

  .task-summary {
    display: flex; flex-wrap: wrap; gap: var(--s8);
    margin-top: var(--s6);
  }
  .task-summary .figure { display: flex; flex-direction: column; gap: var(--s1); }
  .task-summary .figure b {
    font-size: 26px; font-weight: 600; color: var(--ink);
    font-variant-numeric: tabular-nums; letter-spacing: -0.015em; line-height: 1;
  }
  .task-summary .figure span {
    font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--ink-faint);
  }

  .spec-grid {
    display: grid; grid-template-columns: 1fr 0.82fr; gap: var(--s7);
    align-items: start; margin-top: var(--s2);
  }
  .spec-label {
    font-size: 11px; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.07em; color: var(--ink-faint); margin-bottom: var(--s4);
  }
  .prompt-prose {
    font-size: 16px; line-height: 1.7; color: var(--ink);
    margin: 0; max-width: 62ch;
  }
  .spec-expected .run-render { padding: 0; margin: 0; justify-content: center; }
  .spec-expected .run-render svg { max-height: 360px; }

  .check-list { margin: var(--s2) 0; }
  .check-item {
    display: grid; grid-template-columns: 24px 1fr; gap: var(--s3);
    padding: 13px 0; border-bottom: 1px solid var(--soft); align-items: baseline;
  }
  .check-item:last-child { border-bottom: none; }
  .check-num {
    font-family: var(--mono); font-size: 12px; color: var(--ink-faint);
    font-variant-numeric: tabular-nums; text-align: right;
  }
  .check-body { display: flex; flex-direction: column; gap: 4px; }
  .check-title { font-weight: 600; font-size: 15px; color: var(--ink); }
  .check-detail { font-family: var(--mono); font-size: 12.5px; color: var(--ink-soft); }

  .mr-list { margin: var(--s2) 0; }
  .mr {
    display: grid; grid-template-columns: 16px 1fr auto auto;
    align-items: center; gap: var(--s4);
    padding: 12px 0; border-bottom: 1px solid var(--soft);
    text-decoration: none; color: var(--ink);
    transition: opacity 0.12s var(--ease);
  }
  .mr:last-child { border-bottom: none; }
  .mr:hover { opacity: 0.62; }
  .mr-mark { color: var(--c, var(--accent)); display: inline-flex; }
  .mr-mark .brand-mark { width: 15px; height: 15px; }
  .mr-mark .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--c, var(--accent)); display: inline-block; }
  .mr-name { font-family: var(--mono); font-size: 13px; }
  .mr-pass { font-variant-numeric: tabular-nums; font-size: 12px; color: var(--ink-faint); }
  .mr-score {
    font-variant-numeric: tabular-nums; font-weight: 600; font-size: 15px;
    min-width: 3ch; text-align: right; color: var(--ink);
  }
  .mr.unsolved .mr-name, .mr.unsolved .mr-score { color: var(--ink-faint); }

  /* ─── run detail page ───────────────────────────────────────────── */
  .run-head { margin-bottom: var(--s5); }
  .run-head h1 a { color: var(--ink); }
  .run-head h1 a:hover { color: var(--accent); }
  .run-verdict { margin-bottom: var(--s3); }
  .verdict {
    display: inline-flex; align-items: center; gap: 6px;
    font-size: 12px; font-weight: 700; letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 4px 11px 4px 9px; border-radius: 999px;
  }
  .verdict::before {
    content: ""; width: 7px; height: 7px; border-radius: 50%;
  }
  .verdict-pass { color: var(--pass); background: color-mix(in srgb, var(--pass) 12%, transparent); }
  .verdict-pass::before { background: var(--pass); }
  .verdict-fail { color: var(--fail); background: color-mix(in srgb, var(--fail) 12%, transparent); }
  .verdict-fail::before { background: var(--fail); }

  .run-by {
    display: flex; flex-wrap: wrap; align-items: center; gap: var(--s2);
    margin-top: var(--s3); font-size: 13px; color: var(--ink-soft);
  }
  .run-by-model { display: inline-flex; align-items: center; gap: 7px; }
  .run-mark { color: var(--c, var(--accent)); display: inline-flex; }
  .run-mark .brand-mark { width: 15px; height: 15px; }
  .run-mark .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--c, var(--accent)); display: inline-block; }
  .run-by-model a { font-weight: 600; color: var(--ink); }
  .run-by-model a:hover { color: var(--accent); }
  .run-id, .run-date { font-family: var(--mono); font-size: 12px; color: var(--ink-faint); }

  /* run grading rows add a status column + a raw disclosure */
  .check-item.run-check {
    grid-template-columns: 24px 1fr auto;
    align-items: start;
  }
  .check-status { align-self: center; }
  .check-bad { color: var(--fail); }
  .check-raw { grid-column: 2 / -1; margin-top: 2px; }
  .check-raw summary { font-size: 12px; color: var(--ink-faint); }
  .check-raw[open] summary { color: var(--accent); }

  /* tool-call trace */
  .tool-list { margin: var(--s2) 0; }
  .tool-item {
    display: grid; grid-template-columns: 28px 1fr auto auto;
    align-items: baseline; gap: var(--s4);
    padding: 10px 0; border-bottom: 1px solid var(--soft);
  }
  .tool-item:last-child { border-bottom: none; }
  .tool-num { font-family: var(--mono); font-size: 12px; color: var(--ink-faint); text-align: right; font-variant-numeric: tabular-nums; }
  .tool-name { font-family: var(--mono); font-size: 13px; color: var(--ink); }
  .tool-kind { font-family: var(--mono); font-size: 11px; }
  .tool-kind.ok { color: var(--pass); }
  .tool-kind.bad { color: var(--fail); }
  .tool-ms { font-variant-numeric: tabular-nums; font-size: 12px; color: var(--ink-faint); }
  .tool-args { grid-column: 2 / -1; }
  .tool-args summary { font-size: 12px; color: var(--ink-faint); }

  /* ─── footer ────────────────────────────────────────────────────── */
  .footer {
    border-top: 1px solid var(--rule);
    margin-top: var(--s8); padding: var(--s5) 0 var(--s1);
    font-size: 13px; color: var(--ink-faint);
    display: flex; justify-content: space-between; flex-wrap: wrap; gap: var(--s4);
  }
  .footer a { color: var(--ink-soft); font-weight: 500; }
  .footer a:hover { color: var(--accent); }
  .footer .stack b { color: var(--ink); font-weight: 700; }

  .scroll-x {
    overflow-x: auto; -webkit-overflow-scrolling: touch;
    margin: 0 -4px; padding: 0 4px;
  }
  .scroll-x table { min-width: 560px; }
  .matrix.scroll-x table { min-width: 0; }

  /* ─── motion: one quiet entrance, then stillness ────────────────── */
  @keyframes rise {
    from { opacity: 0; transform: translateY(10px); }
    to   { opacity: 1; transform: none; }
  }
  .sheet > * { animation: rise 0.5s var(--ease) both; }
  .sheet > *:nth-child(2) { animation-delay: 0.04s; }
  .sheet > *:nth-child(3) { animation-delay: 0.08s; }
  .sheet > *:nth-child(4) { animation-delay: 0.12s; }
  .sheet > *:nth-child(n+5) { animation-delay: 0.16s; }

  @media (prefers-reduced-motion: reduce) {
    html { scroll-behavior: auto; }
    .sheet > * { animation: none; }
    * { transition: none !important; }
  }

  /* ─── mobile ────────────────────────────────────────────────────── */
  @media (max-width: 760px) {
    body { font-size: 15px; }
    .sheet { padding: var(--s7) var(--s4) var(--s7); }
    .hero { grid-template-columns: 1fr; gap: var(--s4); }
    .hero .mascot { justify-self: center; }
    .hero .mascot svg { height: 200px; }
    .hero-metric { flex-direction: column; align-items: flex-start; gap: var(--s3); padding: var(--s5); }
    h2 { margin: var(--s7) 0 var(--s4); }

    .spec-grid { grid-template-columns: 1fr; gap: var(--s6); }
    .task-summary { gap: var(--s5) var(--s6); }
    .tool-item { grid-template-columns: 24px 1fr auto; }
    .tool-item .tool-ms { grid-column: 2 / -1; text-align: left; }

    table.board th, table.board td { padding: 10px 11px; font-size: 13px; }
    .matrix th, .matrix td { padding: 7px 8px; min-width: 88px; font-size: 12.5px; }

    .checkrow {
      grid-template-columns: 24px 1fr;
      grid-template-areas: "n head" ". status" ". detail"; gap: 4px 8px;
    }
    .checkrow > :nth-child(1) { grid-area: n; }
    .checkrow > :nth-child(2) { grid-area: head; }
    .checkrow > :nth-child(3) { grid-area: status; }
    .checkrow > :nth-child(4) { grid-area: detail; }
    .toolrow {
      grid-template-columns: 24px 1fr auto;
      grid-template-areas: "n tool kind" ". time time" ". detail detail"; gap: 4px 8px;
    }
    .toolrow > :nth-child(1) { grid-area: n; }
    .toolrow > :nth-child(2) { grid-area: tool; }
    .toolrow > :nth-child(3) { grid-area: kind; }
    .toolrow > :nth-child(4) { grid-area: time; text-align: left; }
    .toolrow > :nth-child(5) { grid-area: detail; }

    .footer { flex-direction: column; gap: var(--s2); }
    .stat-row { grid-template-columns: 1fr; gap: var(--s5); }
    .stat-cell { padding: 0; }
    .stat-num { font-size: 34px; }
    .stat-num.stat-num-text { font-size: 21px; }
    .run-render { padding: var(--s4); }
    .run-render svg { max-height: 320px; }
  }

  @media (max-width: 400px) {
    .sheet { padding: var(--s6) var(--s4) var(--s6); }
    .hero .mascot svg { height: 168px; }
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
      <td><span class="k">project</span><br><span class="v">${escape(tb.project ?? "—")}</span></td>
    </tr>
  </table></div>`;
}

function footerHtml(): string {
  return `<div class="footer">
    <div class="stack">
      <b>${escape(copy.brand)}</b> &middot; part of <a href="/">${escape(family.brand)}</a> &middot; an evaluation suite by <a href="${copy.footerOwnerUrl}">${escape(copy.footerOwner)}</a>
    </div>
    <div>
      sibling project: <a href="${copy.siblingProjectUrl}">${escape(copy.siblingProjectName)}</a>
      &middot; <a href="${copy.repoUrl}">github</a>
    </div>
  </div>`;
}

/** Footer for the umbrella index — family-first, no chapter branding. */
function familyFooterHtml(): string {
  return `<div class="footer">
    <div class="stack">
      <b>${escape(family.brand)}</b> &middot; an evaluation family by <a href="${copy.footerOwnerUrl}">${escape(copy.footerOwner)}</a>
    </div>
    <div>
      graded by <a href="${copy.siblingProjectUrl}">${escape(copy.siblingProjectName)}</a>
      &middot; <a href="${copy.repoUrl}">github</a>
    </div>
  </div>`;
}

/** The umbrella index at `/` — the scale ladder of eval chapters plus a
 *  roll-up of the live leaderboards. Chapter pages carry their own brand;
 *  this page carries the family's. */
function familyIndexPage(models: ModelSummary[], taskCount: number, runCount: number, k: number): string {
  const cards = domains
    .map((d) => {
      const stats = d.live
        ? `<div class="fam-stats">${models.length} models · ${taskCount} tasks · ${runCount} runs</div>`
        : `<div class="fam-stats fam-soon">coming soon · <span class="mono">${escape(d.comDomain)}</span> reserved</div>`;
      const inner = `
        <div class="fam-scale mono">${escape(d.scale)}</div>
        <h2>${escape(d.name)}</h2>
        <p>${escape(d.tagline)}</p>
        ${stats}`;
      return d.live
        ? `<a class="fam-card" href="/${d.pathPrefix}/">${inner}</a>`
        : `<div class="fam-card fam-card-soon">${inner}</div>`;
    })
    .join("\n");

  const roster =
    models.length > 0
      ? `<div class="card">
      <h2>the roster</h2>
      <p class="muted">Every model with official runs, across all live chapters. The cross-chapter index appears once a model has runs in two or more chapters.</p>
      <table class="table">
        <thead><tr><th>model</th><th>chapters</th><th>mean score</th><th>pass^${k}</th></tr></thead>
        <tbody>
        ${models
          .map((m) => {
            const passk = m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—";
            return `<tr>
              <td class="id"><a href="/mech/model/${encodeURIComponent(m.model_id)}.html">${escape(m.model_id)}</a></td>
              <td>mech</td>
              <td>${fmtNum(m.mean_score)}</td>
              <td>${passk}</td>
            </tr>`;
          })
          .join("\n")}
        </tbody>
      </table>
    </div>`
      : "";

  const body = `
    <div class="hero">
      <div class="hero-copy">
        <h1>${escape(family.brand)}</h1>
        <p class="tagline">${escape(family.tagline)}</p>
        <p class="muted">${escape(family.subtagline)}</p>
      </div>
    </div>
    <div class="fam-grid">
      ${cards}
    </div>
    ${roster}
    <div class="card">
      <h2>how grading works</h2>
      <p class="muted">One kernel grades all of it. Every check is a deterministic computation — mass properties, DRC, circuit simulation, physics rollouts — never an LLM judgment. Tasks run pass^${k}; villain baselines must fail; held-out splits rotate. Click any number to read the full forensic run blob.</p>
    </div>
    <style>
      .fam-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: var(--s4, 16px); margin: var(--s6, 32px) 0; }
      .fam-card { display: block; border: 1px solid var(--rule); border-radius: 8px; padding: var(--s5, 20px); background: var(--surface); color: inherit; text-decoration: none; }
      a.fam-card:hover { background: var(--hover); }
      .fam-card h2 { margin: 6px 0 8px; }
      .fam-card p { color: var(--ink-soft, inherit); font-size: 14px; margin: 0 0 12px; }
      .fam-scale { font-size: 12px; color: var(--inkFaint, #9aa1ad); letter-spacing: 0.04em; }
      .fam-stats { font-size: 13px; }
      .fam-soon { color: var(--inkFaint, #9aa1ad); }
      .fam-card-soon { opacity: 0.75; }
    </style>`;

  const generatedAt = new Date().toISOString();
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="description" content="${escape(family.subtagline)}">
<meta name="theme-color" content="${theme.light.ground}" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="${theme.dark.ground}" media="(prefers-color-scheme: dark)">
<title>${escape(family.brand)} — ${escape(family.tagline)}</title>
${fontsHref ? `<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="${fontsHref}">` : ""}
<style>${STYLES}</style>
</head>
<body><main class="sheet">
${titleBlockHtml({ drawing: "vcad-evals", sheet: "0 of n", project: "family index" }, generatedAt)}
${body}
${familyFooterHtml()}
<p class="meta">generated ${generatedAt} · static site, regenerate with <code>npm run build -w @mecheval/leaderboard</code></p>
</main></body></html>`;
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
<meta name="description" content="${escape(copy.tagline)}">
<meta name="theme-color" content="${theme.light.ground}" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="${theme.dark.ground}" media="(prefers-color-scheme: dark)">
<title>${escape(title)}</title>
${fontsHref ? `<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="${fontsHref}">` : ""}
<style>${STYLES}</style>
</head>
<body><main class="sheet">
${crumbHtml ? `<div class="crumb">${crumbHtml}</div>` : ""}
${bodyHtml}
${footerHtml()}
<p class="meta">generated ${generatedAt} · static site, regenerate with <code>npm run build -w @mecheval/leaderboard</code></p>
</main></body></html>`;
}

// ─── tables ───────────────────────────────────────────────────────────────

/** The headline leaderboard: a vertical column chart of mean score per
 *  model, ranked, each bar themed by its provider's brand colour. Bars
 *  scale to the full 0..1 score range so the field reads at a glance. */
function modelBarChart(models: ModelSummary[], k: number): string {
  const showLeader = models.length > 1;
  const cols = models
    .map((m, i) => {
      const pct = Math.max(1.5, Math.min(100, m.mean_score * 100));
      const passk =
        m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—";
      const id = modelIdentity(m.model_id);
      const meta = passk;
      const mark = providerMark(id.provider) || `<span class="dot"></span>`;
      return `<a class="vbar${showLeader && i === 0 ? " lead" : ""}" href="model/${encodeURIComponent(m.model_id)}.html" style="--c:${id.color}" title="${escape(m.model_id)} · score ${fmtNum(m.mean_score)} · pass^${k} ${passk}">
        <span class="vbar-col">
          <span class="vbar-val">${fmtNum(m.mean_score)}</span>
          <span class="vbar-fill" style="height:${pct.toFixed(1)}%"></span>
        </span>
        <span class="vbar-label"><span class="vbar-name">${mark}${escape(id.label)}</span><small>${meta}</small></span>
      </a>`;
    })
    .join("");
  return `<div class="vbars" role="list" aria-label="models ranked by mean score">${cols}</div>
    <p class="footnote">Bar height is mean check-pass rate across the most recent ${k} attempts (0&ndash;1), coloured by provider. The figure under each bar is pass<sup>${k}</sup> &mdash; clean passes / pairs ready to score.</p>`;
}

function modelTable(models: ModelSummary[], k: number): string {
  // Highlight the leading model. Only meaningful with >1 row.
  const showLeader = models.length > 1;
  const rows = models
    .map(
      (m, i) => {
      const id = modelIdentity(m.model_id);
      return `
      <tr class="${showLeader && i === 0 ? "rank-1" : ""}">
        <td class="id"><a href="model/${encodeURIComponent(m.model_id)}.html">${escape(id.label)}</a></td>
        <td class="num">${m.tasks_attempted}</td>
        <td class="num">${m.total_attempts}</td>
        <td class="num">${m.pass_k_total > 0 ? `${m.pass_k_full}/${m.pass_k_total}` : "—"}</td>
        <td class="num">${fmtNum(m.mean_score)}</td>
        <td class="num">${fmtCompact(m.mean_tokens)}</td>
        <td class="num">${fmtNum(m.mean_wallclock_sec, 1)}s</td>
      </tr>`;
    })
    .join("");
  return `<div class="scroll-x"><table class="board">
    <thead><tr>
      <th class="left">model</th><th>tasks</th><th>runs</th>
      <th>pass^${k}</th><th>score</th><th>tokens</th><th>wall</th>
    </tr></thead><tbody>${rows}</tbody></table></div>`;
}

function matrix(
  taskIds: string[],
  modelIds: string[],
  byPair: Map<string, PassKEntry>,
  k: number,
  taskRefSvgs: Map<string, string | null>,
): string {
  const head = modelIds
    .map((mid) => {
      const id = modelIdentity(mid);
      const label = id.label;
      return `<th><a href="model/${encodeURIComponent(mid)}.html" title="${escape(mid)}">${escape(label)}</a></th>`;
    })
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
      const refSvg = taskRefSvgs.get(tid) ?? null;
      const refHtml = refSvg
        ? `<div class="matrix-ref">${refSvg}</div>`
        : `<div class="matrix-ref-empty">—</div>`;
      return `<tr>
        <td class="row-ref">${refHtml}</td>
        <td class="row-h"><a href="task/${encodeURIComponent(tid)}.html">${escape(tid)}</a></td>
        ${cells}
      </tr>`;
    })
    .join("");
  return `<div class="matrix scroll-x"><table>
    <thead><tr><th class="left">expected</th><th class="left">task ↓ · model →</th>${head}</tr></thead>
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

  const palette = ["#2563eb", "#e0613a", "#1aa06d", "#8b5cf6", "#d4a017"];
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
        <line x1="${x}" y1="${PAD_T}" x2="${x}" y2="${H - PAD_B}" style="stroke:var(--soft)" stroke-dasharray="2,4"/>
        <text x="${x}" y="${H - PAD_B + 14}" text-anchor="middle" font-size="10" style="fill:var(--ink-faint)">${lbl}</text>`;
    })
    .join("");

  const yAxisLines = yTicks
    .map((v) => {
      const y = yPos(v);
      return `
        <line x1="${PAD_L}" y1="${y}" x2="${W - PAD_R}" y2="${y}" style="stroke:var(--soft)" stroke-dasharray="2,4"/>
        <text x="${PAD_L - 8}" y="${y + 3}" text-anchor="end" font-size="10" style="fill:var(--ink-faint)">${v.toFixed(2)}</text>`;
    })
    .join("");

  // Pareto frontier — points that dominate everything to their left
  // on the cost axis. Iterate sorted-by-x ascending, keep points whose
  // score exceeds the running max. Connect with a dashed accent line.
  const sortedByX = [...points].sort((a, b) => xVal(a) - xVal(b));
  const frontier: typeof points = [];
  let maxY = -Infinity;
  for (const p of sortedByX) {
    if (p.mean_score_recent_k > maxY) {
      frontier.push(p);
      maxY = p.mean_score_recent_k;
    }
  }
  const frontierPath =
    frontier.length >= 2
      ? `<polyline points="${frontier
          .map((p) => `${xPos(xVal(p)).toFixed(1)},${yPos(p.mean_score_recent_k).toFixed(1)}`)
          .join(" ")}" fill="none" style="stroke:var(--accent)" stroke-width="1.6" stroke-dasharray="5,3" stroke-opacity="0.85"/>`
      : "";
  const frontierIds = new Set(frontier.map((p) => `${p.model_id}::${p.task_id}`));

  const dots = points
    .map((e) => {
      const cx = xPos(xVal(e));
      const cy = yPos(e.mean_score_recent_k);
      const color = colorOf(e.model_id);
      const onFront = frontierIds.has(`${e.model_id}::${e.task_id}`);
      const tt = `${e.model_id} · ${e.task_id}\nscore=${e.mean_score_recent_k.toFixed(2)}\ntokens=${fmtCompact(e.mean_tokens_recent_k)} · wall=${e.mean_wallclock_recent_k.toFixed(1)}s\nattempts=${e.attempts}${onFront ? "\n(on pareto frontier)" : ""}`;
      const r = onFront ? 6.5 : 5;
      const strokeStyle = onFront ? "stroke:var(--accent)" : `stroke:${color}`;
      const strokeW = onFront ? 1.8 : 1.2;
      return `<a href="run-link-${e.task_id}-${e.model_id}.html"><circle cx="${cx}" cy="${cy}" r="${r}" fill="${color}" fill-opacity="${onFront ? 0.9 : 0.62}" style="${strokeStyle}" stroke-width="${strokeW}"><title>${escape(tt)}</title></circle></a>`;
    })
    .join("");

  // Replace placeholder href with a real one (task page is the safest target —
  // we don't always have a single-run url for an aggregated entry).
  const dotsLinked = dots.replace(
    /href="run-link-([^"]+?)-((?:claude|default|openai|wafer|zhipu|google|meta|gpt|glm)[^"]+)"/g,
    (_, t) => `href="task/${encodeURIComponent(t as string)}.html"`,
  );

  const legend = modelIds
    .map((m, i) => {
      const cx = PAD_L + i * 250;
      return `<g transform="translate(${cx}, ${PAD_T - 12})">
        <circle cx="0" cy="0" r="4" fill="${colorOf(m)}"/>
        <text x="9" y="3" font-size="10" style="fill:var(--ink-soft)" font-family="${fonts.mono}">${escape(m)}</text>
      </g>`;
    })
    .join("");

  const xLabel = xKey === "tokens" ? "tokens (log)" : "wall-clock seconds (log)";

  return `<div class="chart-frame"><svg viewBox="0 0 ${W} ${H}" width="100%" role="img" aria-label="${xLabel} vs score scatter">
    ${yAxisLines}
    ${xAxisLines}
    <rect x="${PAD_L}" y="${PAD_T}" width="${innerW}" height="${innerH}" fill="none" style="stroke:var(--rule)"/>
    <text x="${PAD_L + innerW / 2}" y="${H - 4}" text-anchor="middle" font-size="10" style="fill:var(--ink-soft)">${xLabel}</text>
    <text transform="translate(${PAD_L - 38}, ${PAD_T + innerH / 2}) rotate(-90)" text-anchor="middle" font-size="10" style="fill:var(--ink-soft)">score</text>
    ${legend}
    ${frontierPath}
    ${dotsLinked}
  </svg></div>`;
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
        ? `<span class="pill pill-pass">PASS</span>`
        : `<span class="pill pill-fail">fail</span>`;
      const failCell = r.first_fail
        ? `<span class="fail-summary"><code>${escape(r.first_fail.type)}</code> · ${escape(r.first_fail.reason)}</span>`
        : `<span class="muted">—</span>`;
      return `<tr>
        <td class="id">${colA}</td>
        <td class="id"><a href="${detailHref}">${escape(r.run_id)}</a></td>
        <td class="num">${status}</td>
        <td class="num">${fmtNum(r.score)}</td>
        <td class="left">${failCell}</td>
        <td class="num">${fmtCompact(r.tokens_total)}</td>
        <td class="num">${fmtNum(r.wallclock_sec, 1)}s</td>
      </tr>`;
    })
    .join("");
  const header = pageKind === "task" ? "model" : "task";
  return `<div class="scroll-x"><table class="board">
    <thead><tr>
      <th class="left">${header}</th><th class="left">run</th>
      <th>status</th><th>score</th><th class="left">first fail</th><th>tokens</th><th>wall</th>
    </tr></thead><tbody>${rows}</tbody></table></div>`;
}

// ─── page renderers ───────────────────────────────────────────────────────

/** Parse a run_id prefix like "20260429T120648Z-5e06" into a Date.
 *  Returns null if the prefix is missing/malformed. */
function runIdDate(runId: string): Date | null {
  const m = runId.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z/);
  if (!m) return null;
  return new Date(Date.UTC(
    Number(m[1]), Number(m[2]) - 1, Number(m[3]),
    Number(m[4]), Number(m[5]), Number(m[6]),
  ));
}

/** Hero metric: industry pass^k as a percentage, plus a sparkline of the
 *  same metric across daily snapshots of run history. */
function renderHeroMetric(
  runs: RunMeta[],
  k: number,
  passKAchieved: number,
  passKReady: number,
): string {
  const pct = passKReady > 0 ? (100 * passKAchieved) / passKReady : 0;
  const pctStr = passKReady > 0 ? `${pct.toFixed(0)}%` : "—";

  // Build daily snapshots: for each distinct calendar date in run history,
  // recompute pass^k over runs with started_at <= end-of-day.
  const dated: Array<{ run: RunMeta; ts: number; day: string }> = [];
  for (const r of runs) {
    const d = runIdDate(r.run_id);
    if (!d) continue;
    const day = d.toISOString().slice(0, 10);
    dated.push({ run: r, ts: d.getTime(), day });
  }
  dated.sort((a, b) => a.ts - b.ts);
  const days = [...new Set(dated.map((d) => d.day))].sort();
  const series: Array<{ day: string; pct: number }> = [];
  for (const day of days) {
    const cutoff = new Date(`${day}T23:59:59Z`).getTime();
    const subset = dated.filter((d) => d.ts <= cutoff).map((d) => d.run);
    const entries = passKBy(subset, k);
    let achieved = 0;
    let ready = 0;
    for (const e of entries) {
      if (e.pass_k !== null) {
        ready += 1;
        if (e.pass_k) achieved += 1;
      }
    }
    series.push({ day, pct: ready > 0 ? (100 * achieved) / ready : 0 });
  }

  // Sparkline: simple polyline scaled to the actual range.
  let sparkSvg = "";
  if (series.length >= 2) {
    const W = 240;
    const H = 48;
    const pad = 2;
    const xs = series.map((_, i) => pad + (i * (W - 2 * pad)) / (series.length - 1));
    const ys = series.map((s) => H - pad - (s.pct / 100) * (H - 2 * pad));
    const points = xs.map((x, i) => `${x.toFixed(1)},${ys[i].toFixed(1)}`).join(" ");
    const lastX = xs[xs.length - 1];
    const lastY = ys[ys.length - 1];
    sparkSvg = `<svg class="hero-spark" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img" aria-label="industry pass^${k} over time">
      <polyline points="${points}" fill="none" stroke="currentColor" stroke-width="1.5"/>
      <circle cx="${lastX.toFixed(1)}" cy="${lastY.toFixed(1)}" r="2.5" fill="currentColor"/>
    </svg>`;
  } else if (series.length === 1) {
    sparkSvg = `<span class="hero-spark-single">single snapshot — ${series[0].day}</span>`;
  }

  const range =
    series.length >= 2
      ? `${series[0].day} &rarr; ${series[series.length - 1].day}`
      : series.length === 1
        ? series[0].day
        : "no history";

  return `<div class="hero-metric">
    <div class="hero-metric-num">${pctStr}</div>
    <div class="hero-metric-meta">
      <div class="hero-metric-label">industry pass<sup>${k}</sup> &middot; fraction of (model, task) pairs that have earned a clean pass<sup>${k}</sup></div>
      ${sparkSvg}
      <div class="hero-metric-range">${range}</div>
    </div>
  </div>`;
}

function indexPage(
  runs: RunMeta[],
  entries: PassKEntry[],
  models: ModelSummary[],
  taskIds: string[],
  k: number,
  mascotSvg: string | null,
  taskRefSvgs: Map<string, string | null>,
): string {
  const byPair = new Map<string, PassKEntry>();
  for (const e of entries) byPair.set(`${e.model_id}::${e.task_id}`, e);
  // Rank models by mean score so the table's first row is the leader.
  const rankedModels = [...models].sort(
    (a, b) =>
      b.mean_score - a.mean_score ||
      b.pass_k_full - a.pass_k_full ||
      a.mean_tokens - b.mean_tokens,
  );
  const modelIds = rankedModels.map((m) => m.model_id);
  const passKAchieved = rankedModels.reduce((a, m) => a + m.pass_k_full, 0);
  const passKReady = rankedModels.reduce((a, m) => a + m.pass_k_total, 0);
  const leader = rankedModels[0];
  const leaderName = leader ? modelIdentity(leader.model_id).label : "—";

  // Industry pass^k metric, with a daily time series for the sparkline.
  // Each run's date is parsed from the run_id prefix (YYYYMMDDTHHmmssZ).
  // For each unique date in history, snapshot all runs with timestamp <=
  // that date and recompute pass^k. The latest snapshot is the headline.
  const heroMetricHtml = renderHeroMetric(runs, k, passKAchieved, passKReady);

  // Stat row: pass^k achieved / ready, total runs, current leader.
  const passKDisplay =
    passKReady > 0
      ? `${passKAchieved}<span class="stat-denom"> / ${passKReady}</span>`
      : `0<span class="stat-denom"> / 0</span>`;
  const statRowHtml = `
    <div class="stat-row">
      <div class="stat-cell">
        <div class="stat-num">${passKDisplay}</div>
        <div class="stat-label">pass<sup>${k}</sup> · achieved / ready</div>
      </div>
      <div class="stat-cell">
        <div class="stat-num">${leader ? fmtNum(leader.mean_score) : "—"}</div>
        <div class="stat-label">top score · ${escape(leaderName)}</div>
      </div>
      <div class="stat-cell">
        <div class="stat-num">${runs.length}<span class="stat-denom"> runs</span></div>
        <div class="stat-label">${rankedModels.length} models · ${taskIds.length} tasks</div>
      </div>
    </div>`;

  const body = `
    <div class="hero">
      <div>
        <div class="wordmark"><span class="dot"></span><h1>${escape(copy.brand)}</h1></div>
        <p class="tagline-main">${escape(copy.tagline)}</p>
        ${heroMetricHtml}
      </div>
      <div class="mascot">${mascotSvg ?? ""}</div>
    </div>

    ${rankedModels.length ? statRowHtml : ""}

    <h2>Models</h2>
    ${rankedModels.length ? modelBarChart(rankedModels, k) : `<div class="nodata">no models yet</div>`}
    ${rankedModels.length ? `<details style="margin-top: 18px;"><summary>Full table — tasks, runs, tokens, wall-clock</summary>
    <div style="margin-top: 12px;">${modelTable(rankedModels, k)}</div></details>` : ""}

    <h2>Task &times; model matrix</h2>
    ${taskIds.length && modelIds.length ? matrix(taskIds, modelIds, byPair, k, taskRefSvgs) : `<div class="nodata">no run blobs found under <code>mecheval/runs/</code></div>`}
    <p class="footnote">Each cell shows pass<sup>${k}</sup> for the most recent ${k} attempts at (model, task). The leftmost column is the latest passing reference render.</p>

    <h2>Cost &middot; score Pareto</h2>
    ${entries.length ? paretoScatter(entries, "tokens") : `<div class="nodata">no points</div>`}
    <p class="footnote">Tokens (log) vs mean score across the most recent ${k} attempts. Each point is one (model, task) pair. The dashed line is the Pareto frontier &mdash; points on it are not dominated by any cheaper, better alternative.</p>

    ${entries.length ? `<details style="margin-top: 22px;"><summary>Wall-clock seconds vs score</summary>
    <div style="margin-top: 12px;">${paretoScatter(entries, "wallclock")}</div>
    <p class="footnote">Wall-clock seconds (log) vs mean score. The left edge is fast; the right edge is patient.</p>
    </details>` : ""}

    <p class="footnote">
      <span class="pending">k/k*</span> denotes fewer than ${k} attempts at this (model, task) &mdash; pass<sup>${k}</sup> pending.
      Score is the mean check-pass rate across the most recent ${k} attempts.
      Corpus: ${runs.length} run blobs across ${rankedModels.length} models and ${taskIds.length} tasks.
    </p>`;
  return pageShell(
    "mecheval — eval suite for AI mechanical design",
    "",
    body,
    {
      drawing: copy.brand,
      sheet: "INDEX",
      scale: passKReady > 0 ? `pass^${k} · ${passKAchieved}/${passKReady}` : `pass^${k}`,
      project: "leaderboard",
    },
  );
}

/** Recent-attempts gallery for a task page. Renders the latest few
 *  run artifacts as small clickable cards. */
function recentAttemptsGallery(
  runsForTask: RunMeta[],
  runSvgs: Map<string, string | null>,
): string {
  const recent = runsForTask
    .slice()
    .sort((a, b) => b.run_id.localeCompare(a.run_id))
    .slice(0, 8);
  if (!recent.length) return "";

  const cards = recent
    .map((r) => {
      const svgKey = `${r.task_id}::${r.model_id}::${r.run_id}`;
      const svg = runSvgs.get(svgKey) ?? null;
      const status = r.passed
        ? `<span class="pass">PASS</span>`
        : `<span class="fail">${fmtNum(r.score)}</span>`;
      const href = `../run/${encodeURIComponent(r.task_id)}/${encodeURIComponent(r.model_id)}/${encodeURIComponent(r.run_id)}.html`;
      const svgHtml = svg
        ? svg
        : `<div class="run-card-empty">no render</div>`;
      const failHtml = r.first_fail
        ? `<div class="run-card-fail">${escape(r.first_fail.type)} · ${escape(r.first_fail.reason)}</div>`
        : "";
      return `<a class="run-card" href="${href}">
        <div class="run-card-svg">${svgHtml}</div>
        <div class="run-card-meta">
          <div class="run-card-model">${escape(modelDisplayName(r.model_id))}</div>
          <div class="run-card-status">${status}</div>
        </div>
        ${failHtml}
      </a>`;
    })
    .join("");

  return `<h2>Recent attempts</h2>
    <div class="run-gallery">${cards}</div>`;
}

/** Compact number: integers stay whole, otherwise 1–2 decimals. */
function fmtN(n: number): string {
  if (Number.isInteger(n)) return String(n);
  return Math.abs(n) >= 100 ? n.toFixed(1) : n.toFixed(2);
}
/** Format a numeric vector "x, y, z". */
function fmtVec(a: unknown): string {
  return Array.isArray(a)
    ? a.map((x) => (typeof x === "number" ? fmtN(x) : String(x))).join(", ")
    : String(a);
}
/** Turn a grader check object into a human-readable title + detail line,
 *  so the task page reads as a spec rather than a JSON dump. Unknown
 *  check types fall back to their name. */
function describeCheck(
  c: Record<string, unknown> & { type: string },
): { title: string; detail: string } {
  const n = (k: string) => (typeof c[k] === "number" ? fmtN(c[k] as number) : undefined);
  switch (c.type) {
    case "valid_solid":
      return { title: "Valid solid", detail: "watertight, manifold body" };
    case "bbox":
      return {
        title: "Bounding box",
        detail: `[${fmtVec(c.min)}] → [${fmtVec(c.max)}] mm · ±${n("tolerance_mm")} mm`,
      };
    case "mass_props": {
      const bits: string[] = [];
      if (c.volume_mm3 != null) bits.push(`volume ${fmtN(c.volume_mm3 as number)} mm³`);
      if (c.center_of_mass != null) bits.push(`COM [${fmtVec(c.center_of_mass)}]`);
      if (c.tolerance_pct != null) bits.push(`±${n("tolerance_pct")}%`);
      return { title: "Mass properties", detail: bits.join(" · ") };
    }
    case "hole_count":
      return {
        title: `Hole count · Ø${n("diameter_mm")} mm`,
        detail: `${c.expected} expected · Ø ±${n("diameter_tolerance_mm")} mm`,
      };
    case "hole_positions": {
      const cnt = Array.isArray(c.positions) ? c.positions.length : 0;
      return {
        title: `Hole positions · Ø${n("diameter_mm")} mm`,
        detail: `${cnt} location${cnt === 1 ? "" : "s"} · ±${n("tolerance_mm")} mm`,
      };
    }
    default:
      return { title: c.type.replace(/_/g, " "), detail: "" };
  }
}

/** Per-model results for a single task: pass count, best score, solved. */
function taskModelResults(runs: RunMeta[]): Array<{
  modelId: string;
  passes: number;
  total: number;
  best: number;
  solved: boolean;
}> {
  const byModel = new Map<string, RunMeta[]>();
  for (const r of runs) {
    const arr = byModel.get(r.model_id) ?? [];
    arr.push(r);
    byModel.set(r.model_id, arr);
  }
  return [...byModel.entries()]
    .map(([modelId, rs]) => {
      const passes = rs.filter((r) => r.passed).length;
      const best = rs.reduce((m, r) => Math.max(m, r.score), 0);
      return { modelId, passes, total: rs.length, best, solved: passes > 0 };
    })
    .sort((a, b) => Number(b.solved) - Number(a.solved) || b.best - a.best || a.modelId.localeCompare(b.modelId));
}

function taskPage(
  spec: TaskSpec | null,
  taskId: string,
  runsForTask: RunMeta[],
  runSvgs: Map<string, string | null>,
  refSvg: string | null,
): string {
  if (!spec) {
    return pageShell(
      `mecheval — ${taskId}`,
      `<a href="../index.html">← ${escape(copy.brand)}</a> / task / ${escape(taskId)}`,
      `<h1>${escape(taskId)}</h1>
       <div class="nodata">no task spec found at mecheval/tasks/${escape(taskId)}.json</div>`,
      { drawing: copy.brand, sheet: `TASK · ${taskId}`, scale: "—", project: taskId },
    );
  }
  // ── per-task model results (a mini leaderboard for this task) ──
  const results = taskModelResults(runsForTask);
  const solvedCount = results.filter((r) => r.solved).length;
  const meanBest =
    results.length ? results.reduce((a, r) => a + r.best, 0) / results.length : 0;

  const resultsHtml = results.length
    ? `<div class="mr-list">${results
        .map((r) => {
          const id = modelIdentity(r.modelId);
          const mark = providerMark(id.provider) || `<span class="dot"></span>`;
          return `<a class="mr${r.solved ? "" : " unsolved"}" href="../model/${encodeURIComponent(r.modelId)}.html" style="--c:${id.color}">
            <span class="mr-mark">${mark}</span>
            <span class="mr-name">${escape(id.label)}</span>
            <span class="mr-pass">${r.passes}/${r.total} pass</span>
            <span class="mr-score">${fmtNum(r.best)}</span>
          </a>`;
        })
        .join("")}</div>`
    : `<div class="nodata">no runs yet for this task</div>`;

  // ── grading: human-readable checks + a collapsed raw spec ──
  const checksHtml = spec.checks.length
    ? `<div class="check-list">${spec.checks
        .map((c, i) => {
          const d = describeCheck(c);
          return `<div class="check-item">
            <span class="check-num">${i + 1}</span>
            <span class="check-body">
              <span class="check-title">${escape(d.title)}</span>
              ${d.detail ? `<span class="check-detail">${escape(d.detail)}</span>` : ""}
            </span>
          </div>`;
        })
        .join("")}</div>`
    : `<div class="nodata">no checks</div>`;

  const rawSpec = {
    checks: spec.checks,
    ...(spec.anti_cheese && Object.keys(spec.anti_cheese).length ? { anti_cheese: spec.anti_cheese } : {}),
    ...(spec.limits && Object.keys(spec.limits).length ? { limits: spec.limits } : {}),
  };
  const constraints: string[] = [];
  if (spec.anti_cheese && Object.keys(spec.anti_cheese).length) {
    constraints.push(`${Object.keys(spec.anti_cheese).length} anti-cheese rule${Object.keys(spec.anti_cheese).length === 1 ? "" : "s"}`);
  }
  if (spec.limits && Object.keys(spec.limits).length) {
    constraints.push(`limits: ${Object.keys(spec.limits).join(", ")}`);
  }

  const tagsHtml = (spec.tags ?? []).length
    ? `<div class="task-tags">${(spec.tags ?? [])
        .map((t) => `<span class="tag">${escape(t)}</span>`)
        .join("")}</div>`
    : "";

  const expectedHtml = refSvg
    ? `<div class="run-render">${refSvg}</div>`
    : `<div class="nodata">no reference render</div>`;

  const body = `
    <div class="task-head">
      <h1>${escape(spec.title)} <span class="tier">${escape(spec.suite)} · ${escape(spec.tier)} · ${escape(taskId)}</span></h1>
      ${tagsHtml}
      <div class="task-summary">
        <div class="figure"><b>${solvedCount} / ${results.length || 0}</b><span>models solve it</span></div>
        <div class="figure"><b>${results.length ? fmtNum(meanBest) : "—"}</b><span>mean best score</span></div>
        <div class="figure"><b>${spec.checks.length}</b><span>graded checks</span></div>
      </div>
    </div>

    <h2>Specification</h2>
    <div class="spec-grid">
      <div class="spec-prompt">
        <div class="spec-label">Prompt</div>
        <p class="prompt-prose">${escape(spec.prompt)}</p>
      </div>
      <div class="spec-expected">
        <div class="spec-label">Expected result</div>
        ${expectedHtml}
      </div>
    </div>

    <h2>What's graded</h2>
    ${checksHtml}
    ${constraints.length ? `<p class="footnote">${constraints.join(" · ")}.</p>` : ""}
    <details style="margin-top: 12px;"><summary>Raw spec (checks${constraints.length ? ", anti-cheese, limits" : ""})</summary>
    <pre>${escape(JSON.stringify(rawSpec, null, 2))}</pre></details>

    <h2>Model results</h2>
    ${resultsHtml}

    ${recentAttemptsGallery(runsForTask, runSvgs)}

    <details style="margin-top: 22px;"><summary>All runs (${runsForTask.length})</summary>
    <div style="margin-top: 12px;">${runsForTask.length
      ? runsTable(runsForTask, "task")
      : `<div class="nodata">no runs yet for this task</div>`}</div></details>
  `;
  return pageShell(
    `mecheval — ${taskId}`,
    `<a href="../index.html">← ${escape(copy.brand)}</a> / task / ${escape(taskId)}`,
    body,
    {
      drawing: copy.brand,
      sheet: `TASK · ${taskId}`,
      scale: `${runsForTask.length} runs`,
      project: `${spec.suite} · ${spec.tier}`,
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
      project: modelDisplayName(modelId),
    },
  );
}

/** Pull a short, human-readable measured value out of a run check's
 *  details, to sit beside the spec target ("got: …"). Best-effort. */
function measuredValue(c: { type: string; details: Record<string, unknown> }): string | null {
  const d = c.details ?? {};
  const g = (k: string) => (d[k] != null ? d[k] : undefined);
  switch (c.type) {
    case "bbox": {
      const lo = g("actual_min") ?? g("min");
      const hi = g("actual_max") ?? g("max");
      if (lo && hi) return `[${fmtVec(lo)}] → [${fmtVec(hi)}] mm`;
      return null;
    }
    case "mass_props": {
      const v = g("actual_volume_mm3") ?? g("volume_mm3");
      return typeof v === "number" ? `volume ${fmtN(v)} mm³` : null;
    }
    case "hole_count": {
      const n = g("actual") ?? g("found") ?? g("count");
      return typeof n === "number" ? `${n} found` : null;
    }
    default:
      return null;
  }
}

function runPage(
  blob: FullBlob,
  vcad: string | null,
  vcadSvg: string | null,
  spec: TaskSpec | null,
  refSvg: string | null,
): string {
  const taskId = blob.task_id;
  const modelId = canonicalModelId(blob.model.id);
  const runId = blob.run_id;
  const id = modelIdentity(modelId);
  const mark = providerMark(id.provider) || `<span class="dot" style="--c:${id.color}"></span>`;
  const passed = blob.summary.passed;
  const dateStr = blob.timestamps.started_at.slice(0, 16).replace("T", " ");
  const title = spec?.title ?? taskId;

  // ── verdict + summary figures ──
  const verdictHtml = passed
    ? `<span class="verdict verdict-pass">Pass</span>`
    : `<span class="verdict verdict-fail">Fail</span>`;
  const summaryFigures = `
    <div class="task-summary">
      <div class="figure"><b>${blob.summary.checks_passed}<span class="muted"> / ${blob.summary.checks_total}</span></b><span>checks passed</span></div>
      <div class="figure"><b>${fmtNum(blob.summary.score)}</b><span>score</span></div>
      <div class="figure"><b>${fmtCompact(blob.trace.tokens.total)}</b><span>tokens</span></div>
      <div class="figure"><b>${fmtNum(blob.trace.wallclock_sec, 1)}s</b><span>wall-clock</span></div>
      <div class="figure"><b>${blob.trace.tool_calls.length}</b><span>tool calls</span></div>
    </div>`;

  // ── expected vs built renders, side by side ──
  const builtHtml = vcadSvg
    ? `<div class="run-render">${vcadSvg}</div>`
    : `<div class="nodata">no render — vcad-render produced no geometry for this output</div>`;
  const expectedCol = refSvg
    ? `<div class="run-render">${refSvg}</div>`
    : `<div class="nodata">no reference render</div>`;

  // ── grading: merge spec targets with this run's results ──
  const checksHtml = blob.checks.length
    ? `<div class="check-list">${blob.checks
        .map((c) => {
          const specCheck = spec?.checks?.[c.n];
          const desc = describeCheck(
            (specCheck as (Record<string, unknown> & { type: string })) ?? { type: c.type },
          );
          const ok = c.result === "pass";
          const statusPill = ok
            ? `<span class="pill pill-pass">pass</span>`
            : c.result === "fail"
              ? `<span class="pill pill-fail">fail</span>`
              : `<span class="pill pill-pending">${escape(c.result.replace("_", " "))}</span>`;
          const reason = summarizeCheckFailure({ type: c.type, result: c.result, details: c.details });
          const got = measuredValue(c);
          const note = !ok && reason
            ? `<span class="check-detail check-bad">${escape(reason.reason)}</span>`
            : got
              ? `<span class="check-detail">got: ${escape(got)}</span>`
              : "";
          return `<div class="check-item run-check">
            <span class="check-num">${c.n + 1}</span>
            <span class="check-body">
              <span class="check-title">${escape(desc.title)}</span>
              ${desc.detail ? `<span class="check-detail">${escape(desc.detail)}</span>` : ""}
              ${note}
            </span>
            <span class="check-status">${statusPill}</span>
            <details class="check-raw"><summary>raw</summary><pre>params: ${escape(JSON.stringify(c.params, null, 2))}\n\ndetails: ${escape(JSON.stringify(c.details, null, 2))}</pre></details>
          </div>`;
        })
        .join("")}</div>`
    : `<div class="nodata">no checks</div>`;

  // ── tool-call trace ──
  const traceHtml = blob.trace.tool_calls.length
    ? `<div class="tool-list">${blob.trace.tool_calls
        .map(
          (tc) => `<div class="tool-item">
        <span class="tool-num">${tc.n}</span>
        <span class="tool-name">${escape(tc.tool)}</span>
        <span class="tool-kind ${tc.result_kind === "ok" ? "ok" : "bad"}">${escape(tc.result_kind)}</span>
        <span class="tool-ms">${tc.wallclock_ms.toFixed(0)} ms</span>
        <details class="tool-args"><summary>args</summary><pre>${escape(JSON.stringify(tc.args, null, 2))}</pre></details>
      </div>`,
        )
        .join("")}</div>`
    : `<div class="nodata">single-shot solver — no tool calls</div>`;

  const body = `
    <div class="run-head">
      <div class="run-verdict">${verdictHtml}</div>
      <h1><a href="../../../task/${encodeURIComponent(taskId)}.html">${escape(title)}</a></h1>
      <div class="run-by" style="--c:${id.color}">
        <span class="run-by-model"><span class="run-mark">${mark}</span><a href="../../../model/${encodeURIComponent(modelId)}.html">${escape(id.label)}</a></span>
        <span class="muted">·</span>
        <span class="run-id">${escape(runId)}</span>
        <span class="muted">·</span>
        <span class="run-date">${escape(dateStr)}</span>
      </div>
      ${summaryFigures}
    </div>

    <h2>Expected vs. built</h2>
    <div class="spec-grid">
      <div><div class="spec-label">Expected</div>${expectedCol}</div>
      <div><div class="spec-label">${id.label} output</div>${builtHtml}</div>
    </div>
    ${vcadSvg ? `<p class="footnote">Isometric render of the model's actual <code>.vcad</code> output · <a href="${escape(runId)}.vcad" download>download .vcad</a></p>` : ""}

    <h2>Grading</h2>
    ${checksHtml}

    <h2>Process</h2>
    ${traceHtml}

    <details style="margin-top: 22px;"><summary>Prompt sent to the model</summary>
    <p class="prompt-prose" style="margin-top: 12px;">${escape(blob.prompt.rendered)}</p></details>
    <details style="margin-top: 12px;"><summary>.vcad source${vcad ? "" : " (unavailable)"}</summary>
    ${vcad ? `<pre>${escape(vcad)}</pre>` : `<div class="nodata">.vcad not present beside this blob</div>`}</details>
    <details style="margin-top: 12px;"><summary>Provenance</summary>
    <table class="kvtable" style="margin-top: 12px;">
      <tr><td class="k">submission</td><td>${escape(blob.submission_kind)}</td></tr>
      <tr><td class="k">model id</td><td><code>${escape(modelId)}</code></td></tr>
      <tr><td class="k">started</td><td>${escape(blob.timestamps.started_at)}</td></tr>
      <tr><td class="k">ended</td><td>${escape(blob.timestamps.ended_at)}</td></tr>
      <tr><td class="k">tokens</td><td>${blob.trace.tokens.input.toLocaleString()} in · ${blob.trace.tokens.output.toLocaleString()} out</td></tr>
      <tr><td class="k">task hash</td><td><code>${escape(blob.task_sha256.slice(0, 16))}…</code></td></tr>
      <tr><td class="k">vcad hash</td><td><code>${escape(blob.output.vcad_sha256.slice(0, 16))}…</code></td></tr>
    </table></details>
  `;
  return pageShell(
    `mecheval — ${title} · ${id.label}`,
    `<a href="../../../index.html">← ${escape(copy.brand)}</a> / <a href="../../../task/${encodeURIComponent(taskId)}.html">${escape(taskId)}</a> / ${escape(id.label)}`,
    body,
    {
      drawing: copy.brand,
      sheet: `RUN · ${runId}`,
      scale: passed ? "PASS" : `${blob.summary.checks_passed}/${blob.summary.checks_total}`,
      project: `${taskId} · ${modelDisplayName(modelId)}`,
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
  // Merge solver-harness variants (direct vs mcp) of the same model before
  // any aggregation — the harness must not affect the result. Blob paths
  // keep their original on-disk ids; only the display/aggregation id and
  // the emitted URLs change.
  for (const r of runs) r.model_id = canonicalModelId(r.model_id);
  const entries = passKBy(runs, PASS_K);
  const models = modelSummary(entries);
  const taskSpecs = await loadTaskSpecs();
  const taskIds = [...taskSpecs.keys()].sort();
  const seenTaskIds = new Set([...taskIds, ...runs.map((r) => r.task_id)]);

  // Render the mascot once for the hero (cache-first).
  const mascotVcad = resolve(REPO_ROOT, "mecheval/corpus/mascot.vcad");
  const mascotSvg = existsSync(mascotVcad)
    ? await getOrRenderSvg(mascotVcad, "mascot")
    : null;

  // Pre-render every run artifact once into a shared map. Used by both
  // run detail pages and task-page galleries. Keyed by
  // "task::model::run" so a single lookup surface is reused everywhere.
  const runSvgs = new Map<string, string | null>();
  for (const r of runs) {
    const vcadPath = r.blob_path.replace(/\.json$/, ".vcad");
    // Cache key follows the on-disk blob path (original solver-qualified
    // model dir), not the canonicalized model_id — keeps the committed
    // cache valid across the direct/mcp merge.
    const diskModelDir = dirname(r.blob_path).split("/").pop() ?? r.model_id;
    const cacheKey = `runs/${r.task_id}/${diskModelDir}/${r.run_id}`;
    const svg = existsSync(vcadPath)
      ? await getOrRenderSvg(vcadPath, cacheKey)
      : null;
    runSvgs.set(`${r.task_id}::${r.model_id}::${r.run_id}`, svg);
  }

  // Per-task reference SVG for the matrix's "expected" column.
  // Resolution order:
  //   1. `mecheval/tasks/<task_id>.vcad` if present (an authored or
  //      generated reference — see scripts/gen-task-refs.mjs).
  //   2. Most-recent passing run for that task across any model.
  //   3. Most-recent run with a usable render, regardless of pass/fail.
  //   4. null (matrix shows "—").
  const taskRefSvgs = new Map<string, string | null>();
  for (const tid of seenTaskIds) {
    const authoredPath = resolve(TASKS_DIR, `${tid}.vcad`);
    if (existsSync(authoredPath)) {
      const svg = await getOrRenderSvg(authoredPath, `taskref/${tid}`);
      if (svg) {
        taskRefSvgs.set(tid, svg);
        continue;
      }
    }
    const taskRuns = runs
      .filter((r) => r.task_id === tid)
      .slice()
      .sort((a, b) => b.run_id.localeCompare(a.run_id));
    const pick =
      taskRuns.find((r) => {
        if (!r.passed) return false;
        const svg = runSvgs.get(`${r.task_id}::${r.model_id}::${r.run_id}`);
        return svg != null;
      }) ??
      taskRuns.find((r) => {
        const svg = runSvgs.get(`${r.task_id}::${r.model_id}::${r.run_id}`);
        return svg != null;
      });
    const svg = pick
      ? runSvgs.get(`${pick.task_id}::${pick.model_id}::${pick.run_id}`) ?? null
      : null;
    taskRefSvgs.set(tid, svg);
  }

  // Family index at the site root. Chapter sites live under their
  // pathPrefix; the branded .com domains 301 into them (vercel.json).
  await writePage(
    "index.html",
    familyIndexPage(models, seenTaskIds.size, runs.length, PASS_K),
  );

  // MechEval chapter index.
  await writePage(
    `${MECH}/index.html`,
    indexPage(
      runs,
      entries,
      models,
      [...seenTaskIds].sort(),
      PASS_K,
      mascotSvg,
      taskRefSvgs,
    ),
  );

  // Task pages.
  for (const tid of seenTaskIds) {
    const runsForTask = runs.filter((r) => r.task_id === tid);
    await writePage(
      `${MECH}/task/${tid}.html`,
      taskPage(taskSpecs.get(tid) ?? null, tid, runsForTask, runSvgs, taskRefSvgs.get(tid) ?? null),
    );
  }

  // Model pages.
  const modelIds = new Set(runs.map((r) => r.model_id));
  for (const mid of modelIds) {
    const runsForModel = runs.filter((r) => r.model_id === mid);
    await writePage(`${MECH}/model/${mid}.html`, modelPage(mid, runsForModel));
  }

  // Run pages — reuse the SVG map populated above; copy each .vcad
  // into dist alongside the blob so the run page can offer it as a
  // download link.
  let renderedRuns = 0;
  for (const r of runs) {
    const blob = await loadFullBlob(r.blob_path);
    const vcad = await loadVcadIfPresent(r.blob_path);
    const svgKey = `${r.task_id}::${r.model_id}::${r.run_id}`;
    const vcadSvg = runSvgs.get(svgKey) ?? null;
    if (vcadSvg) renderedRuns++;
    await writePage(
      `${MECH}/run/${r.task_id}/${r.model_id}/${r.run_id}.html`,
      runPage(blob, vcad, vcadSvg, taskSpecs.get(r.task_id) ?? null, taskRefSvgs.get(r.task_id) ?? null),
    );
    // Drop the .vcad file alongside the HTML so it can be linked.
    if (vcad) {
      await writeFile(
        resolve(
          OUT_DIR,
          `${MECH}/run/${r.task_id}/${r.model_id}/${r.run_id}.vcad`,
        ),
        vcad,
        "utf8",
      );
    }
  }
  console.log(`rendered ${renderedRuns} of ${runs.length} run artifacts`);

  console.log(
    `wrote ${OUT_DIR}: index + ${seenTaskIds.size} tasks + ${modelIds.size} models + ${runs.length} runs`,
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
