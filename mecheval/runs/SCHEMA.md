# Run schema

One JSON file per attempt. Every attempt of every task by every model gets its own immutable blob, checked into git. The point is **forensic transparency** — anyone disputing a number on the leaderboard can read the blob.

## Path convention

```
mecheval/runs/<task_id>/<model_id>/<run_id>.json
```

- `task_id` — matches the task file (`a1-plate-01`, etc.).
- `model_id` — slug for the exact model + version + provider, e.g. `claude-opus-4-7-20260101`, `gpt-5-2026-q1`, `gemini-2-5-pro-20260214`. Use the date of the model release, not the run.
- `run_id` — sortable + unique. Convention: `<UTC-timestamp>-<short-uuid>`, e.g. `20260427T161200Z-a3f9`.

A blob is **one attempt** of one prompt. Pass^k is computed by aggregating k blobs at the leaderboard level — never baked into a single blob.

## Shape

```json
{
  "schema_version": 0,
  "run_id": "20260427T161200Z-a3f9",
  "task_id": "a1-plate-01",
  "task_sha256": "…",

  "model": {
    "id": "claude-opus-4-7-20260101",
    "name": "Claude Opus 4.7",
    "provider": "anthropic",
    "params": { "temperature": 1.0, "thinking_tokens": 8000 }
  },

  "harness": {
    "version": "0.1.0",
    "commit": "dc89e02",
    "vcad_version": "0.x.y",
    "phyz_version": "0.x.y",
    "host": { "os": "darwin-25.3.0", "arch": "arm64" }
  },

  "submission_kind": "self-run | hosted | black-box",

  "prompt": {
    "seed": "deterministic-seed-string",
    "rendered": "Make a 50mm × 30mm × 10mm rectangular plate…",
    "attachments": []
  },

  "trace": {
    "tool_calls": [
      { "n": 0, "tool": "create_cad_document", "args": {…}, "result_kind": "ok", "wallclock_ms": 142 },
      { "n": 1, "tool": "place_part", "args": {…}, "result_kind": "ok", "wallclock_ms": 87 }
    ],
    "tokens": { "input": 1234, "output": 5678, "total": 6912 },
    "wallclock_sec": 47.3
  },

  "output": {
    "vcad_path": "output.vcad",
    "vcad_sha256": "…",
    "control_policy": null,
    "renders": []
  },

  "sim": {
    "engine": "vcad-gym",
    "phyz_version": "0.x.y",
    "rollout_steps": 1000,
    "rollout_dt_ms": 16.667,
    "trace_path": null
  },

  "checks": [
    {
      "n": 0,
      "type": "valid_solid",
      "params": {},
      "result": "pass",
      "details": { "manifold": true, "closed": true, "shells": 1 }
    },
    {
      "n": 1,
      "type": "bbox",
      "params": { "min": [-25, -15, 0], "max": [25, 15, 10], "tolerance_mm": 0.1 },
      "result": "pass",
      "details": { "actual_min": [-25.001, -15.0, 0.0], "actual_max": [25.001, 15.0, 10.0] }
    },
    {
      "n": 2,
      "type": "mass_props",
      "params": { "volume_mm3": 14717.3, "tolerance_pct": 0.5 },
      "result": "fail",
      "details": { "actual_volume_mm3": 13520.1, "deviation_pct": 8.13 }
    }
  ],

  "summary": {
    "passed": false,
    "checks_passed": 4,
    "checks_total": 6,
    "score": 0.667,
    "anti_cheese_violated": false,
    "limits_exceeded": []
  },

  "timestamps": {
    "started_at": "2026-04-27T16:12:00Z",
    "ended_at":   "2026-04-27T16:12:47Z"
  },

  "signature": null
}
```

## Field rules

| Field | Required | Notes |
|---|---|---|
| `schema_version` | yes | Integer. Bumps on breaking changes. |
| `run_id` | yes | Globally unique within `runs/`. |
| `task_id` | yes | Must match an existing `mecheval/tasks/<id>.json`. |
| `task_sha256` | yes | Hash of the task JSON file at run time. Detects corpus drift. |
| `model.*` | yes | All four sub-fields required. `params` must include any non-default sampling settings. |
| `harness.*` | yes | Version + commit + vcad/phyz versions are non-negotiable; other fields best-effort. |
| `submission_kind` | yes | One of `self-run`, `hosted`, `black-box`. |
| `prompt.seed` | yes | Deterministic — re-running the harness with the same seed produces the same `prompt.rendered`. |
| `prompt.rendered` | yes | The exact text the model received. |
| `trace.tool_calls` | yes | Each call: index, tool name, args (or hash if huge), pass/fail kind, wallclock. |
| `trace.tokens` | yes | At minimum `input + output`. Total is sum. |
| `trace.wallclock_sec` | yes | End-to-end. |
| `output.vcad_sha256` | yes | Hash of the model's `.vcad` output. |
| `output.vcad_path` | yes | Path relative to the blob, OR (for tiny outputs) inline as `output.vcad_inline`. |
| `output.control_policy` | Suite C Track A only | Path to the agent's policy output. |
| `sim.*` | Suite C only | Suites A/B leave this `null`. |
| `checks[]` | yes | One entry per check in the task spec, in order. |
| `summary.*` | yes | Aggregate across checks. `score = checks_passed / checks_total`. Hard pass requires `summary.passed === true` (all checks + no anti-cheese violation + no limits exceeded). |
| `timestamps.*` | yes | UTC, ISO 8601. |
| `signature` | optional | Reserved for cryptographic signing of "official" maintainer-run blobs. |

## Forensic guarantees

1. **`task_sha256` matches the committed task.** If someone hand-edits a task to make a result look better, the hash mismatch is visible at audit time.
2. **`output.vcad_sha256` matches the file in the blob.** No swapping outputs after the fact.
3. **`harness.commit` resolves to a real commit in `vcad/` (or `mecheval/`, post-spinout).** Reproducibility check: clone that commit, run the harness, expect the same result up to non-determinism in the model.
4. **The blob is append-only.** Once committed, never mutated. Bug fixes to the harness produce *new* blobs at the corrected commit; we don't rewrite history.
5. **Public archive.** Every blob lives in git forever. Old benchmarks cite blob paths.

## Pass^k aggregation

Pass^k is a leaderboard-level computation:

```
blobs = filter(runs, model_id=M, task_id=T, harness.commit ∈ {v0.5})
sort(blobs, by=run_id)
take last k
pass_k = all(b.summary.passed for b in last_k)
```

This means: only the **k most recent** runs at a given harness version count. Resampling old failed runs does not improve pass^k.
