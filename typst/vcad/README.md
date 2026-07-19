# vcad for Typst

Parametric CAD in your document. This Typst package embeds vcad's BRep
kernel and drafting renderer as a WASM plugin, so a Typst document can
**render**, **measure**, and **verify** CAD models at compile time — every
figure, dimension, mass, and assertion recomputes from the geometry on
each build. The document cannot disagree with its model.

```typst
#import "@preview/vcad:0.1.0": vcad-loon, vcad-inspect, vcad-mass, vcad-assert

// Model, inline. loon booleans are subject-last: [difference tool body].
#let bracket = "[difference [translate 30 20 -1 [cylinder 4 12]] [cube 60 40 8]]"

#vcad-loon(bracket, dims: true)                       // dimensioned iso view
#let m = vcad-inspect(bracket, format: "loon")
The plate weighs #calc.round(vcad-mass(m, 1.04), digits: 1) g in ABS.
#vcad-assert(vcad-mass(m, 1.04) < 30, message: "over mass budget")
```

## Functions

- `vcad-view(source, view: "iso", section: "z=10", dims: true, focus: "lid", ...)`
  — one drafting-style SVG view of a `.vcad` document (str/bytes, e.g.
  `read("part.vcad")`) or loon source (`format: "loon"`). Views:
  `iso|front|side|top|hero|orbit:AZ,EL`; sections get cross-hatched cut faces.
- `vcad-loon(source, ...)` — sugar for loon source; accepts a raw block.
- `vcad-sheet(source, title: "BRACKET")` — third-angle multi-view drawing
  sheet (front/side/top/iso + title block).
- `vcad-inspect(source)` — dictionary of exact measurements: `volume` (mm³),
  `area` (mm²), `bbox.(min|max|size)`, `center-of-mass`, `parts` (per-part
  name/material/volume/area/bbox/com).
- `vcad-mass(inspection, density)` — grams, density in g/cm³.
- `vcad-assert(cond, message: ..)` — fail the compile when a spec is violated.
- `vcad-eval-loon(source)` — loon → `.vcad` JSON string.
- `vcad-version()` — plugin version.

Units are millimeters; the coordinate system is Z-up.

## Building the plugin

`vcad.wasm` is built from `crates/vcad-typst` (not committed):

```bash
./scripts/build-typst-plugin.sh   # → typst/vcad/vcad.wasm (~2.4 MB)
```

Compile the demo: `typst compile --root . examples/demo.typ` (from this
directory, wasm present).
