# Photonics M6: the tape-out pack

What has to be true — and what has to be *said* — before the inverse-
designed splitter's GDS goes on a shuttle run, and how the loop closes
when the chip comes back.

## What the GDS actually is

`gds::design_to_gds` ships the **exact pixel geometry the solver
simulated**: the binarized density (ρ̂ ≥ ½) as maximal-rectangle
decomposition of Δ-squares, plus the access waveguides, one cell, one
layer, conventional µm user / nm database units. Deliberately **no
smoothing**: a smoothed contour would be a different device than the one
the claims were computed on. Pixel-edge placement rounds to the 1 nm
grid (≤ 0.5 nm per edge at Δ = 38.75 nm).

Consequences a fab reviewer should expect:

- **Staircase edges** at the pixel pitch (Δ ≈ 39 nm at λ/40). E-beam
  writes this fine; DUV steppers will corner-round at ~½ their k₁λ/NA —
  which un-validates the prediction. This design targets e-beam shuttle
  services (SiEPIC-style ebeam runs) as-is.
- **Abutting rectangles** on one layer: mask prep unions them; no
  self-intersection, no holes-by-winding (rect decomposition cannot
  express a hole as a single polygon — donuts appear as C-shaped unions).
- **Minimum feature**: the cone-filter diameter (2·r_f·Δ) is a
  *regularization scale*, not a geometric guarantee — projection at
  finite β can still produce marginal necks. Before submission, run the
  fab's own DRC for min-width/min-gap at their stated limit; the claim
  set records the filter diameter under `min_feature_nm` with exactly
  this caveat in its note.

## Design-rule notes (e-beam shuttle checklist)

1. **Layer map**: the export uses layer 1/datatype 0; remap to the
   shuttle's silicon-etch layer (e.g. SiEPIC ebeam Si 1/0) at submission.
2. **Min width / min gap**: assert fab limit ≤ cone-filter diameter, and
   DRC anyway (see above). 155–232 nm designs clear typical 60–100 nm
   e-beam rules with margin.
3. **Snap check**: all coordinates are integer nm by construction
   (`(v·1000).round()`); no off-grid vertices exist.
4. **Port geometry**: access waveguides exit the design box on-axis with
   the simulated width; the shuttle's grating couplers / edge couplers
   connect outside this cell. Keep the simulated guide width up to the
   coupler taper, or re-simulate the taper.
5. **Density / fill**: a 2×2 µm design box needs no dummy fill at
   shuttle scales; check the run's global density rules if arraying.

## The 2D → 3D honesty clause

Every claim in `vcad.photonics-claims/1` is a **2D TM prediction**. A
fabricated chip is a 3D slab (typically 220 nm SOI) whose guided physics
differs: effective indices shift, radiation into the substrate exists,
and TE/TM in 3D do not map onto our 2D polarizations one-to-one. The 2D
numbers are exact for the 2D problem and *qualitative* for the chip:
splitting ratio and topology transfer well; absolute insertion loss does
not. The measurement plan below is how the gap gets priced instead of
argued about. (The standard bridge — effective-index reduction of the
SOI stack to a 2D ε map — is a planned upgrade and changes none of the
seams.)

## When the chip comes back: `receipt::compare`

The measurement schema binds lab numbers to predicted claims by name:

```json
[
  {"name": "transmission_arm_a", "value": 0.44, "uncertainty": 0.02},
  {"name": "splitting_ratio",    "value": 0.51, "uncertainty": 0.01}
]
```

`compare(&claims, &measured, tolerance_rel)` returns one row per claim
with a mechanical verdict:

- **Holds** — |measured − predicted| ≤ max(tol·|predicted|, 2σ),
- **Violated** — outside that band (NaN measurements are violations),
- **Unmeasured** — no measurement bound; **never assumed to hold**.

Recommended practice for the splitter: measure per-arm transmission at
3+ wavelengths against a reference straight guide on the same chip
(cut-back normalization), report σ from repeated couplings, and bind
`transmission_arm_a/b`, `splitting_ratio`, and (via linewidth SEM)
`min_feature_nm`. Expect `insertion_loss_db` to *violate* on a 3D chip
— that violation is the 2D→3D gap being measured, which is the point.
