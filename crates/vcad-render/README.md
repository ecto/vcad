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
```

| Flag | Default | Meaning |
|---|---|---|
| `--scale <N>` | `2.0` | Pixels per millimetre. Bigger = larger SVG. |
| `--exact-edges` | off | Emit BRep-exact linework where available (see below). |

### `--exact-edges`: BRep-exact curves

By default every curved edge is a tessellated polyline, which facets
visibly at high `--scale`. With `--exact-edges` the renderer walks the
evaluated BRep and replaces recognisable curved linework with
mathematically exact SVG elliptical-arc paths:

- circular model edges (cylinder/cone rims — a bore's mouth, a boss's cap
  edge — including ones produced by booleans), projected to exact ellipse
  arcs;
- sphere view outlines (the silhouette great circle for the current
  orthographic view).

Fills, shading, and hidden-line removal still run on the tessellation;
exact curves replace only the linework, and anything the extractor doesn't
recognise (tori, NURBS, boolean intersection seams) falls back to
polylines. Cylinder/cone silhouette rulings are straight lines and stay as
`<line>`s. Arc extents are matched against the mesh linework that would
otherwise be drawn, so trimmed rims keep exactly the coverage of the
polyline render.

Exit codes: `0` on success, `2` on parse/eval/render failure (with a
human-readable message on stderr).

## Tunable constants

Edit at the top of `src/lib.rs` if you need a different look:

| Constant | Default | Effect |
|---|---|---|
| `TESSELLATION_SEGMENTS` | `64` | Segments per cylinder/cone/sphere. Bumping this smooths curves at the cost of file size (`--exact-edges` sidesteps this for linework entirely). |
| `COPLANAR_DOT_TOL` | `0.997` (~4.5°) | Tighter values reveal more crease lines; looser values hide more. |
| `BACKFACE_DOT_MIN` | `-0.04` | How aggressively to cull back-facing triangles. Slightly negative so silhouette edges survive. |
| `LIGHT` | `[-0.6, -0.7, 0.8]` | Light direction in kernel space (Z-up). |
| `FILL_DARK` / `FILL_LIGHT` | `#0e3960` / `#c8dceb` | Lambertian shade endpoints. |

## History

Originally `mecheval-render` inside `mecheval/graders/`. Promoted to a
standalone crate so other consumers (docs, marketing, CAD previews) can
depend on it without pulling in the eval grader.

## Future: raytrace mode

For vector output, `--exact-edges` already delivers resolution-independent
linework straight from the BRep. What remains raster-bound is *shading*: a
future `--raytrace` flag could swap the tessellated fill pipeline for
direct BRep ray tracing via [`vcad-kernel-raytrace`](../vcad-kernel-raytrace),
producing pixel-perfect PNG output. That makes sense for marketing
screenshots and high-fidelity previews; SVG remains the right pick for the
leaderboard's drafting aesthetic.
