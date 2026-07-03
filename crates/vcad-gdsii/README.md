# vcad-gdsii

GDSII (Calma stream format) reader/writer for vcad — the foundation for
chip-layout support.

- **Record-level codec** for the binary stream format, including GDSII's
  8-byte excess-64 float (IBM System/360 hex float) with exact `f64`
  round-tripping. Big-endian throughout.
- **Plain-data model**: `Library` → `Cell` → `Element::{Boundary, Path,
  Text, Sref, Aref}` with `i32` database-unit coordinates.
- **Flattening**: recursive SREF/AREF resolution (translation, rotation,
  mirror, magnification) to flat per-layer polygons in f64 database units.
  PATH elements are expanded to boundary polygons (pathtype 0 / flush ends).
  Reference cycles are detected and rejected.
- **vcad-ir bridge** (`vcad-ir` feature, on by default): converts flattened
  layers into a `vcad_ir::Document` — one sketch-extrude part per entry of a
  caller-supplied layer stack, ready for the app, CLI, and renderers.

## Usage

```rust
use vcad_gdsii::{read_library, flatten, to_vcad_document, DEFAULT_VIEW_SCALE};

let bytes = std::fs::read("chip.gds")?;
let lib = read_library(&bytes)?;

// Flat per-layer polygons in f64 database units.
let flat = flatten(&lib, "TOP")?;

// Or go straight to a vcad document. The layer stack maps GDS layer
// numbers to physical films: (layer, z_bottom_um, thickness_um, name).
let stack = [
    (1, 0.0, 0.2, "diffusion"),
    (2, 0.4, 0.18, "poly"),
    (10, 1.0, 0.5, "metal1"),
];
// DEFAULT_VIEW_SCALE renders 1 µm of layout as 1 mm of model — dies are
// tiny; pass a different scale for true (µm → mm / 1000) dimensions.
let doc = to_vcad_document(&lib, "TOP", &stack, DEFAULT_VIEW_SCALE)?;
std::fs::write("chip.vcad", doc.to_json()?)?;
```

## Scope notes

- Flattened polygon coordinates are **f64 database units**; multiply by
  `Library::db_unit_in_meters` for physical units.
- Path end styles other than pathtype 0 (round / extended ends) are parsed
  but rejected by the flattener.
- TEXT elements round-trip through read/write but are ignored when
  flattening (they are annotations, not mask geometry).
- Unmodeled records (ELFLAGS, PLEX, properties, …) are skipped on read.
