# vcad-process

Planar semiconductor process emulation — TCAD-lite. Takes **GDS masks +
a process recipe** and produces the resulting film-stack geometry, both
as a 3D stack and as the classic textbook process cross-section.

```text
GDS layout ──flatten──▶ per-layer masks (µm) ─┐
                                              ├─▶ planar film simulator ─▶ vcad_ir::Document
recipe (deposit/etch/grow/implant/CMP/litho) ─┘        (3D stack, cross-section)
```

## Recipe as plain data

A `Recipe` is a substrate plus an ordered `Vec<ProcessStep>`; everything
derives serde so recipes can live in JSON files next to the layouts:

- `Deposit { material, thickness_um }` — blanket film over the current
  top surface.
- `PatternEtch { mask_layer, polarity, depth_um }` — etch the current
  top film using a GDS layer as the mask. `Polarity::KeepMasked` keeps
  film under mask polygons (subtractive patterning of a deposited film);
  `Polarity::RemoveMasked` removes it there (opening windows). A depth
  shallower than the film leaves a recessed remnant.
- `GrowOxide { thickness_um }` — thermal oxidation: like a deposit, but
  it consumes silicon from the film below. The classic rule of thumb is
  **0.46 units of Si consumed per unit of oxide grown** (SiO₂ occupies
  ~2.2× the volume of the Si it came from), so 46% of the oxide ends up
  below the original surface and 54% above. Exposed as
  `SI_CONSUMED_PER_OXIDE`.
- `Implant { mask_layer, dopant, depth_um }` — doped region in the top
  of the substrate under mask openings (v0: a thin colored slab inset
  into the wafer).
- `Planarize { to_um }` — CMP: clip everything above a height.

### Photolithography (resist) steps

- `SpinResist { thickness_um, tone }` — spin-coat a blanket photoresist
  film on the current top surface (`tone: Positive | Negative`).
- `Expose { mask_layer, dose_mj_cm2 }` — expose the topmost resist film
  through a GDS layer (a maskless stepper writing the same polygons is
  equivalent). Pattern and dose are recorded on the film; exposure is
  binary for now and the dose is pure bookkeeping — dose-to-clear
  thresholding is future work. Multiple exposures union.
- `Develop` — resolve the topmost exposed resist film: positive tone
  removes the exposed regions, negative tone keeps only them. After
  develop the resist footprint reflects the pattern.
- `EtchThroughResist { depth_um }` — etch the topmost non-resist film
  wherever resist is absent: the physically honest replacement for
  `PatternEtch`'s idealized one-step mask, with the same
  through/partial-etch semantics. `PatternEtch` remains available as
  the shortcut, and the two land on identical geometry:

  ```text
  SpinResist(neg) → Expose(L) → Develop → EtchThroughResist(d) → Strip
      ≡ PatternEtch { mask_layer: L, KeepMasked,   depth_um: d }
  SpinResist(pos) → Expose(L) → Develop → EtchThroughResist(d) → Strip
      ≡ PatternEtch { mask_layer: L, RemoveMasked, depth_um: d }
  ```

- `Strip` — remove all resist films.

## Mask engine

Masks come from `vcad_gdsii::flatten` (f64 database units → µm),
clipped to the region of interest and unioned per layer with the pure-
Rust `geo` boolean ops (the same `geo = "0.28"` vcad-kernel-cam pins —
no geo-clipper, which doesn't build for WASM). Etch = film footprint ∖
mask or ∩ mask; all plan-view state is `geo::MultiPolygon<f64>` in µm.

## Outputs

Both emitters produce a `vcad_ir::Document` in the same sketch +
extrude (+ union/difference for holes) style as the vcad-gdsii bridge,
at the same **1 µm = 1 mm** view scale.

- `simulate_3d(lib, top_cell, recipe, window)` — one part per surviving
  film, bottom-up, with per-material colors (silicon gray-blue, SiO₂
  pale gray, poly red, aluminum metallic, dopants green/orange, resist
  the classic amber — resist films render like any other film while
  they're on the wafer, and vanish after `Strip`).
  `window: Option<[x0, y0, x1, y1]>` (µm) crops the die — pass one for
  real layouts.
- `cross_section(lib, top_cell, recipe, cut)` — the iconic deliverable:
  intersect each film's footprint with a `CutLine` (axis-aligned, at
  `position_um`, over `span`) to get exact 1D intervals, then emit one
  thin slab (0.05 µm) per interval × film thickness. Etched-away
  intervals are actually missing. Render with `--view front` for an
  `Axis::X` cut (`--view side` for `Axis::Y`).

## Planar v0 approximations (a.k.a. lies we tell on purpose)

- **The top surface is a single scalar height**, not a height field.
  Films deposit as flat slabs at the current top; a deposit over a
  patterned film **bridges the etched gaps** instead of conforming into
  them (no conformal/isotropic deposition, no step coverage).
- Etches are perfectly anisotropic and vertical; `GrowOxide` is blanket
  (no LOCOS bird's beak) and consumes from the current surface film
  only.
- Implants are uniform box profiles under mask polygons — no straggle,
  no diffusion, no channeling, and they don't move during later thermal
  steps (implant *before* oxidation will float inside the grown oxide;
  sequence recipes accordingly).
- Litho is binary: exposure dose is recorded but not modeled (no
  dose-to-clear threshold, no PEB, no proximity effects), develop is a
  perfect boolean, and resist doesn't reflow or erode during etch.
  `PatternEtch` remains as the even-more-idealized mask → result
  shortcut.

Cross-section slabs get a hair of extra thickness per film
(+0.002 µm/film) so coplanar front faces don't z-fight in orthographic
renders; implant slabs sit 0.002 µm proud of the wafer in 3D for the
same reason.

## Example

`examples/haiku_xsection.rs` runs a simplified sky130-ish flow (field
oxide + active openings, implants, poly gate, ILD/CMP, li1, met1, met2)
over a real GDS die and writes both a mid-die cross-section and a
windowed 3D stack:

```bash
cargo run -p vcad-process --example haiku_xsection -- chip.gds out_dir
```
