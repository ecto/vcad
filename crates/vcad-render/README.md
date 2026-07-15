# vcad-render

Project a `.vcad` document to a static isometric SVG — drafting-style line
art, suitable for documentation, marketing, README diagrams, and the
mecheval leaderboard.

## What it does

Given a `.vcad` file, `vcad-render`:

1. Parses + evaluates the document (via `vcad-eval`).
2. Tessellates each evaluated solid into triangles.
3. Canonicalizes vertices and classifies edges:
   - **boundary** (1 adjacent triangle) → drawn (silhouette)
   - **crease**   (2 adjacent triangles, non-coplanar normals) → drawn
   - **internal** (2 adjacent triangles, coplanar normals) → hidden
4. Projects to a 30°/30° isometric view, runs a painter's-algorithm sort,
   and emits a self-contained `<svg>` to stdout.

The result reads like drafting linework — flat faces don't show their
internal triangulation diagonals.

## Install

```bash
cargo build -p vcad-render --release
# binary lands at target/release/vcad-render
```

For day-to-day dev `cargo build -p vcad-render` (debug) is fine; the
mecheval leaderboard defaults to `target/debug/vcad-render`.

## Usage

```bash
vcad-render path/to/part.vcad > out.svg          # SVG on stdout
vcad-render part.vcad --scale 4.0 > big.svg
vcad-render part.vcad -o out.jpg                 # format from extension
vcad-render parts/ --out-dir renders/ --format jpeg   # batch a directory
```

| Flag | Default | Meaning |
|---|---|---|
| `--view <V>` | `iso` | Camera: `iso`/`front`/`side`/`top`/`hero`. |
| `--scale <N>` | `2.0` | Pixels per millimetre (SVG). Bigger = larger SVG. |
| `--transparent` | off | Transparent SVG background. |
| `-o, --output <PATH>` | stdout | Output path; format inferred from `.svg`/`.jpg`/`.jpeg`. `-o -` = SVG on stdout. Single input only. |
| `--jpeg <PATH>` | — | Legacy alias for `-o <path.jpg>`. |
| `--out-dir <DIR>` | sibling | Directory for batch outputs. |
| `--format <F>` | `svg` | Batch output format: `svg` or `jpeg`. |
| `--size <N>` | `1024` | Raster canvas size in pixels (JPEG). |
| `--fill <F>` | `0.6` | Fraction of canvas the part's long axis fills (JPEG). |
| `--quality <Q>` | `92` | JPEG quality, 1–100. |

Multiple inputs (or a directory, which expands to its `*.vcad` files)
render in batch, each to `<stem>.<ext>` next to the input or in
`--out-dir`. A per-file failure is reported on stderr but doesn't abort
the batch.

Exit codes: `0` on success, `2` on parse/eval/render failure (with a
human-readable message on stderr). A batch exits `2` if any file failed.

## Tunable constants

Edit at the top of `src/main.rs` if you need a different look:

| Constant | Default | Effect |
|---|---|---|
| `TESSELLATION_SEGMENTS` | `28` | Segments per cylinder/cone/sphere. Bumping this smooths curves at the cost of file size. |
| `COPLANAR_DOT_TOL` | `0.997` (~4.5°) | Tighter values reveal more crease lines; looser values hide more. |
| `BACKFACE_DOT_MIN` | `-0.04` | How aggressively to cull back-facing triangles. Slightly negative so silhouette edges survive. |
| `LIGHT` | `[-0.6, -0.7, 0.8]` | Light direction in kernel space (Z-up). |
| `FILL_DARK` / `FILL_LIGHT` | `#0e3960` / `#c8dceb` | Lambertian shade endpoints. |

## History

Originally `mecheval-render` inside `mecheval/graders/`. Promoted to a
standalone crate so other consumers (docs, marketing, CAD previews) can
depend on it without pulling in the eval grader.

## Future: raytrace mode

A future `--raytrace` flag could swap out the tessellation pipeline for
direct BRep ray tracing via [`vcad-kernel-raytrace`](../vcad-kernel-raytrace),
producing pixel-perfect PNG output instead of vector SVG. That makes
sense for marketing screenshots and high-fidelity previews; SVG remains
the right pick for the leaderboard's drafting aesthetic.
