# @mecheval/leaderboard

Static site that renders the MechEval leaderboard from `mecheval/runs/`,
`mecheval/tasks/`, and `mecheval/corpus/operator.vcad`. Lives at
[mecheval.com](https://mecheval.com).

## Local dev

From the monorepo root:

```bash
# 1. Build the Rust render binary (once, or after touching it).
cargo build -p vcad-render

# 2. Build the harness (provides the pass_k aggregation library).
npm run build -w @mecheval/harness

# 3. Build the leaderboard (HTML + cache).
npm run build -w @mecheval/leaderboard

# 4. Serve.
npm run dev -w @mecheval/leaderboard
# → http://localhost:5174
```

`build:mecheval` at the root chains steps 2 and 3.

## Render cache

The leaderboard inlines isometric SVGs for OPERATOR (the hero mech) and
every committed run artifact. Renders are produced by [`vcad-render`](../../crates/vcad-render)
(Rust) and cached at `mecheval/leaderboard/cache/`.

- **Cache hit** → SVG read straight from disk, no Rust required.
- **Cache miss + binary present** → render, write to cache, use it.
- **Cache miss + binary missing** → placeholder (the build still
  succeeds, just less pretty).

This is what lets Vercel build without Rust: the cache files are
committed. Rebuild the cache locally whenever a `.vcad` changes
(operator + new run blobs) and commit the deltas.

## Vercel deployment

Configure a Vercel project pointing at this directory:

| Setting | Value |
|---|---|
| Root Directory | `mecheval/leaderboard` |
| Framework Preset | Other |
| Build Command | (defer to `vercel.json`) |
| Output Directory | (defer to `vercel.json`) |
| Install Command | (default — Vercel auto-detects npm workspaces) |

`mecheval/leaderboard/vercel.json` does the rest: it climbs back to the
monorepo root and runs `npm run build:mecheval`, then points Vercel at
`mecheval/leaderboard/dist/`.

The repo's *other* `vercel.json` at the root controls the vcad.io
deploy and is independent.
