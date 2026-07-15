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
vcad-render path/to/part.vcad > out.svg
vcad-render part.vcad --scale 4.0 > big.svg
vcad-render part.vcad --sheet > sheet.svg                 # multi-view drawing sheet
vcad-render part.vcad --sheet --size 1600 --jpeg sheet.jpg
```

| Flag | Default | Meaning |
|---|---|---|
| `--scale <N>` | `2.0` | Pixels per millimetre. Bigger = larger SVG. (Single-view only; the sheet computes its own shared scale.) |
| `--view <v>` | `iso` | `iso`/`hero`, `front`, `side`, or `top`. |
| `--sheet` | off | Multi-view drawing sheet instead of a single view (see below). |
| `--jpeg <path>` | off | Raster output written to `path` instead of SVG on stdout. |
| `--size <N>` | `1024` | Raster canvas size; with `--sheet`, the overall sheet width. |

### Drawing-sheet mode (`--sheet`)

`--sheet` emits a single landscape sheet laying out **front, side, top, and
isometric** views in the classic third-angle arrangement (top view above
front, side view to the right, iso in the remaining corner). All four views
share one scale so they are dimensionally consistent, each carries a caption
(FRONT/TOP/SIDE/ISO), and a title block in the bottom-right corner shows the
document name, overall bounding-box dimensions in mm, the shared scale, and a
date placeholder. Works for both SVG (default, stdout) and `--jpeg` raster
output; `--size` sets the overall sheet width (height is derived, A-series
landscape).

Exit codes: `0` on success, `2` on parse/eval/render failure (with a
human-readable message on stderr).

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
