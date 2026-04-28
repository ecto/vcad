// Pass^k aggregation per the contract in mecheval/runs/SCHEMA.md:
//
//   blobs = filter(runs, model_id=M, task_id=T)
//   sort(blobs, by=run_id)
//   take last k
//   pass_k = all(b.summary.passed for b in last_k)
//
// Re-graded blobs: when a check is wired or the kernel changes, the same
// .vcad output may now pass or fail differently. We never rewrite history;
// we just take the *most recent k* attempts for the given (model, task).
// If the run corpus has fewer than k attempts at that pair, pass_k is
// reported as `null` and the actual count is in `attempts`.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

export interface RunMeta {
  task_id: string;
  model_id: string;
  run_id: string;
  passed: boolean;
  score: number;
  tokens_total: number;
  wallclock_sec: number;
  blob_path: string;
  /** Short reason for the first failed/errored check, or null if all checks passed. */
  first_fail: { type: string; reason: string } | null;
}

interface CheckRecord {
  type: string;
  result: "pass" | "fail" | "not_implemented" | "error";
  details: Record<string, unknown>;
}

/** Produce a short, human-readable reason for the first non-passing check.
 *  Returns null when every check passed. The reason is intentionally terse —
 *  it has to fit under a small score in a leaderboard card. */
export function summarizeFirstFailure(
  checks: CheckRecord[],
): { type: string; reason: string } | null {
  for (const c of checks) {
    const r = summarizeCheckFailure(c);
    if (r) return r;
  }
  return null;
}

/** Like summarizeFirstFailure but for a single check. Returns null if the
 *  check passed. */
export function summarizeCheckFailure(
  c: CheckRecord,
): { type: string; reason: string } | null {
  if (c.result === "pass") return null;
  return { type: c.type, reason: failureReason(c) };
}

function failureReason(c: CheckRecord): string {
  if (c.result === "error") return "grader errored";
  if (c.result === "not_implemented") return "check not implemented";
  const d = c.details ?? {};
  switch (c.type) {
    case "bbox": {
      // Pick the dominant axis from deviation_min/deviation_max.
      const dmax = (d.deviation_max as number[]) ?? [0, 0, 0];
      const dmin = (d.deviation_min as number[]) ?? [0, 0, 0];
      const axes = ["X", "Y", "Z"] as const;
      let bestAxis = "?", bestVal = 0;
      for (let i = 0; i < 3; i++) {
        for (const v of [dmax[i] ?? 0, dmin[i] ?? 0]) {
          if (Math.abs(v) > Math.abs(bestVal)) {
            bestVal = v;
            bestAxis = axes[i];
          }
        }
      }
      const sign = bestVal > 0 ? "+" : "";
      return `${bestAxis} off by ${sign}${bestVal.toFixed(2)}mm`;
    }
    case "mass_props": {
      const vol = d.volume as { pass?: boolean; deviation_pct?: number } | undefined;
      if (vol && vol.pass === false && typeof vol.deviation_pct === "number") {
        return `volume off by ${vol.deviation_pct.toFixed(1)}%`;
      }
      const com = d.center_of_mass as { pass?: boolean; max_abs_deviation_mm?: number } | undefined;
      if (com && com.pass === false && typeof com.max_abs_deviation_mm === "number") {
        return `center of mass off by ${com.max_abs_deviation_mm.toFixed(2)}mm`;
      }
      return "mass props off";
    }
    case "valid_solid": {
      const produced = d.solids_produced as number | undefined;
      const root = d.root_count as number | undefined;
      if (produced === 0) return "no valid solid produced";
      if (typeof produced === "number" && produced > 1) return `produced ${produced} solids (expected 1)`;
      if (typeof root === "number") return `root_count=${root}`;
      return "solid invalid";
    }
    case "hole_count": {
      const actual = d.actual as number | undefined;
      const expected = d.expected as number | undefined;
      const diam = d.diameter_mm as number | undefined;
      if (typeof actual === "number" && typeof expected === "number") {
        return `found ${actual}/${expected} holes${diam ? ` of ⌀${diam}mm` : ""}`;
      }
      return "hole count off";
    }
    case "hole_positions": {
      const per = (d.per_expected as Array<{ pass: boolean }> | undefined) ?? [];
      const failed = per.filter((p) => !p.pass).length;
      const extras = (d.unmatched_extras as unknown[] | undefined)?.length ?? 0;
      const parts: string[] = [];
      if (failed) parts.push(`${failed}/${per.length} mispositioned`);
      if (extras) parts.push(`${extras} extra`);
      return parts.length ? parts.join(", ") : "hole positions off";
    }
    case "step_roundtrip": {
      const per = (d.per_solid as Array<{ pass: boolean }> | undefined) ?? [];
      const failed = per.filter((p) => !p.pass).length;
      return failed
        ? `STEP drift on ${failed}/${per.length} solid${per.length === 1 ? "" : "s"}`
        : "STEP roundtrip failed";
    }
    default:
      return "failed";
  }
}

export interface PassKEntry {
  task_id: string;
  model_id: string;
  attempts: number;
  recent_k: number;
  pass_k: boolean | null;
  pass_count_in_recent_k: number;
  mean_score_recent_k: number;
  mean_tokens_recent_k: number;
  mean_wallclock_recent_k: number;
}

export async function loadAllRuns(runsDir: string): Promise<RunMeta[]> {
  const runs: RunMeta[] = [];
  let taskDirs: string[];
  try {
    taskDirs = await readdir(runsDir);
  } catch {
    return runs;
  }
  for (const taskId of taskDirs) {
    const taskPath = join(runsDir, taskId);
    let modelDirs: string[];
    try {
      modelDirs = await readdir(taskPath);
    } catch {
      continue;
    }
    for (const modelId of modelDirs) {
      const modelPath = join(taskPath, modelId);
      let entries: string[];
      try {
        entries = await readdir(modelPath);
      } catch {
        continue;
      }
      for (const entry of entries) {
        if (!entry.endsWith(".json")) continue;
        const blobPath = join(modelPath, entry);
        const raw = await readFile(blobPath, "utf8");
        const d = JSON.parse(raw) as {
          task_id: string;
          run_id: string;
          summary: { passed: boolean; score: number };
          trace: { tokens: { total: number }; wallclock_sec: number };
          checks?: CheckRecord[];
        };
        runs.push({
          task_id: d.task_id,
          model_id: modelId,
          run_id: d.run_id,
          passed: d.summary.passed,
          score: d.summary.score,
          tokens_total: d.trace.tokens.total,
          wallclock_sec: d.trace.wallclock_sec,
          blob_path: blobPath,
          first_fail: d.summary.passed ? null : summarizeFirstFailure(d.checks ?? []),
        });
      }
    }
  }
  return runs;
}

export function passKBy(
  runs: RunMeta[],
  k: number,
): PassKEntry[] {
  const groups = new Map<string, RunMeta[]>();
  for (const r of runs) {
    const key = `${r.model_id}::${r.task_id}`;
    (groups.get(key) ?? groups.set(key, []).get(key)!).push(r);
  }
  const out: PassKEntry[] = [];
  for (const [key, blobs] of groups) {
    const [model_id, task_id] = key.split("::");
    blobs.sort((a, b) => a.run_id.localeCompare(b.run_id));
    const recent = blobs.slice(-k);
    const passInRecent = recent.filter((b) => b.passed).length;
    const meanScore = mean(recent.map((b) => b.score));
    const meanTokens = mean(recent.map((b) => b.tokens_total));
    const meanWall = mean(recent.map((b) => b.wallclock_sec));
    out.push({
      task_id,
      model_id,
      attempts: blobs.length,
      recent_k: recent.length,
      pass_k: recent.length < k ? null : passInRecent === recent.length,
      pass_count_in_recent_k: passInRecent,
      mean_score_recent_k: meanScore,
      mean_tokens_recent_k: meanTokens,
      mean_wallclock_recent_k: meanWall,
    });
  }
  out.sort((a, b) => a.model_id.localeCompare(b.model_id) || a.task_id.localeCompare(b.task_id));
  return out;
}

function mean(xs: number[]): number {
  return xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0;
}

/** Per-model summary across all tasks: mean score, attempts, pass^k tally. */
export interface ModelSummary {
  model_id: string;
  tasks_attempted: number;
  total_attempts: number;
  pass_k_full: number;
  pass_k_total: number;
  mean_score: number;
  mean_tokens: number;
  mean_wallclock_sec: number;
}

export function modelSummary(entries: PassKEntry[]): ModelSummary[] {
  const grouped = new Map<string, PassKEntry[]>();
  for (const e of entries) {
    (grouped.get(e.model_id) ?? grouped.set(e.model_id, []).get(e.model_id)!).push(e);
  }
  const out: ModelSummary[] = [];
  for (const [model_id, es] of grouped) {
    const passKReady = es.filter((e) => e.pass_k !== null);
    out.push({
      model_id,
      tasks_attempted: es.length,
      total_attempts: es.reduce((a, e) => a + e.attempts, 0),
      pass_k_full: passKReady.filter((e) => e.pass_k === true).length,
      pass_k_total: passKReady.length,
      mean_score: mean(es.map((e) => e.mean_score_recent_k)),
      mean_tokens: mean(es.map((e) => e.mean_tokens_recent_k)),
      mean_wallclock_sec: mean(es.map((e) => e.mean_wallclock_recent_k)),
    });
  }
  out.sort((a, b) => b.mean_score - a.mean_score);
  return out;
}
