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
vcad-render part.vcad -o out.jpg                 # format from extension (.svg/.jpg/.png)
vcad-render part.vcad -o out.png                 # transparent RGBA, 4096px
vcad-render parts/ --out-dir renders/ --format png    # batch a directory

# Ray-traced output (direct BRep ray tracing, no tessellation):
vcad-render part.vcad --raytrace -o out.png
vcad-render part.vcad --raytrace -o out.jpg --view hero --size 1440 --quality 95
```

| Flag | Default | Meaning |
|---|---|---|
| `--view <V>` | `iso` | Camera: `iso`/`front`/`side`/`top`/`hero`. |
| `--scale <N>` | `2.0` | Pixels per millimetre (SVG). Bigger = larger SVG. |
| `--transparent` | off | Transparent SVG background. |
| `-o, --output <PATH>` | stdout | Output path; format inferred from `.svg`/`.jpg`/`.jpeg`/`.png`. `-o -` = SVG on stdout. Single input only. |
| `--jpeg <PATH>` | — | Legacy alias for `-o <path.jpg>`. |
| `--raytrace` | off | Render the raster output via direct BRep ray tracing (needs `.png`/`.jpg`). |
| `--out-dir <DIR>` | sibling | Directory for batch outputs. |
| `--format <F>` | `svg` | Batch output format: `svg`, `jpeg`, or `png`. |
| `--size <N>` | `1024` (JPEG), `4096` (PNG) | Raster canvas size in pixels. Edge stroke weight and curve tessellation scale with it. |
| `--fill <F>` | `0.6` | Fraction of canvas the part's long axis fills (raster). |
| `--quality <Q>` | `92` | JPEG quality, 1–100 (ignored for PNG). |

PNG output is RGBA with a fully transparent background (alpha 0 wherever no
geometry or edge stroke was drawn) — the raster analogue of `--transparent`.
The `--raytrace` PNG path is transparent too, with fractional alpha
antialiasing its exact curved silhouettes.

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

## Raytrace mode

`--raytrace` swaps out the tessellation pipeline for direct BRep ray
tracing via [`vcad-kernel-raytrace`](../vcad-kernel-raytrace): every pixel
is an analytic ray–surface intersection (plane, cylinder, sphere, cone,
torus, NURBS) through a SAH BVH with trimmed-face tests, so curved
silhouettes are exact at any resolution — no facet banding, no segment
count to tune. The camera is the same orthographic `View` basis and
framing math as the tessellated raster path, and shading samples the same
vcad-Blue tonal ramp (tinted by document material colours), so the two
paths are drop-in alternatives. Assemblies render the same way as the
tessellation path (instances are world-placed before tracing); mesh-only
parts (e.g. frozen topology-optimization results) have no analytic
surfaces and are skipped.

It runs on the CPU (no GPU required) and lives behind the crate's
`raytrace` cargo feature — default-on for the binary, off for the WASM
build so it doesn't grow. Use it for marketing screenshots and
high-fidelity previews; SVG remains the right pick for the leaderboard's
drafting aesthetic.
