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
