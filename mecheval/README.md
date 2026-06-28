# MechEval

The mechanical, physical, and CAD evaluation suite for AI models.

> **The AI designs the robot AND drives it.**

Headline tier: AI models design mechs in vcad and the vcad gym simulates them. The winning designs get manufactured and shipped IRL.

## What it grades

vcad is the gym. phyz (articulated dynamics) and tang (math + symbolic gradients) are its internals. Every grader call goes through deterministic kernel + physics validation — no LLM-as-judge, no shape-similarity heuristics, no third-party physics. *"Pass in vcad → walks in real life"* is the bar.

Three coupled suites:

- **Suite C — Mech** (headline). Agent outputs a `.vcad` mech (body + joints) and either a control policy or just the body; the gym simulates and grades. Sub-tiers: Reacher, Picker, Carrier, Walker, Climber, Jumper, Dueler, Boss missions.
- **Suite A — Agent** (CAD authoring). Tiers A1–A6, primitives → manufacturing-aware parts.
- **Suite B — Kernel** (no AI). Boolean stress, STEP round-trip, fillet success rate, constraint solver convergence, tessellation quality, articulated dynamics.

## Status

v0.0 — design phase, in-monorepo. Spins out to its own repo at v1.0. See [the strategic plan](../) for the full design.

## Layout

```
mecheval/
├── tasks/           # public task definitions (one JSON per task) + SCHEMA.md
├── corpus/          # reference .vcad / .step files, golden mass-props
├── runs/            # every official run, immutable JSON; checked-in
├── private/         # held-out tasks (gitignored, synced from a private repo)
├── harness/         # TS runner that drives a submission's MCP solver against the vcad gym
├── graders/         # Rust crates that wrap the vcad-kernel-* crates
├── submissions/     # Docker spec for how a submission is packaged
└── leaderboard/     # static site reading from runs/
```

(Subdirectories appear as we ship them — the structure above is the target.)

## Adding a task

See [tasks/SCHEMA.md](tasks/SCHEMA.md). One JSON file per task, filename matches `id`. Tasks are deterministic: every check is something vcad can compute exactly.

## Adding a result

Every official run lands in `runs/<task_id>/<model_id>/<timestamp>.json` with the full forensic blob: prompt seed, agent output, sim traces, grader output, harness version. Anyone can audit any number on the leaderboard by reading the blob.

## Submission

(Not yet open.) When live: ship a Docker image exposing `solve(prompt) -> .vcad` over HTTP/MCP. We run it against the private split. SWE-Bench / Cybench pattern.

## Running locally

Build everything once:

```
cargo build -p mecheval-grader
(cd mecheval/harness && npx tsc)
```

Run the DEFAULT_CUBE villain against a task — no API key required:

```
node mecheval/harness/dist/cli.js --task a1-cube-01 --solver default-cube
```

Run real Claude — single-shot, prompt-only, no MCP loop yet:

```
ANTHROPIC_API_KEY=sk-... node mecheval/harness/dist/cli.js \
  --task a1-cube-01 --solver claude-direct
```

Override the model:

```
ANTHROPIC_API_KEY=sk-... node mecheval/harness/dist/cli.js \
  --task a1-cube-01 --solver claude-direct-claude-haiku-4-5-20251001
```

Run a wafer.ai model — single-shot, prompt-only (OpenAI-compatible endpoint,
defaults to GLM-5.2):

```
WAFER_API_KEY=wfr_... node mecheval/harness/dist/cli.js \
  --task a1-cube-01 --solver wafer-direct          # GLM-5.2
WAFER_API_KEY=wfr_... node mecheval/harness/dist/cli.js \
  --task a1-cube-01 --solver wafer-direct-GLM-5.1  # override the model
```

Each run writes a forensic blob to `mecheval/runs/<task_id>/<model_id>/<run_id>.json`.

## License

Apache-2.0 — same as vcad.
