# Handoff: vcad boolean `Difference` mis-classifies faces against curved geometry

## Context

I was modelling a real part in vcad — a two-piece clamp with a hemispherical
socket (a Ø50 ball capture cuff for a Booster K1 humanoid hand). It is about the
simplest real mechanical feature there is: a hemispherical pocket in a block,
then ordinary box cuts around it. It cannot currently be built. Every failure was
**silent** — `Difference` returned a wrong solid with no error and no warning, so
each one looked like success until I measured the volume.

Please fix the boolean, and add a validity guard so this class of failure can
never again be silent.

## Reproducing

Two notes on the harness, because they cost me time:

1. `vcad info` / `vcad export` **cannot read v0.2 VCode `.vcad` files**, even
   though `vcad-ir` parses them fine. `load_vcad_document` in
   `crates/vcad-cli/src/main.rs:863` only accepts `FileFormat::V2Crdt` and
   `FileFormat::V1Json`, and falls through to `unrecognized .vcad file format`.
   Worth wiring `vcad_ir::vcode::from_vcode` into that match as a separate small
   fix. I worked around it with a 10-line shim calling `from_vcode` then
   `Document::to_json`.

2. Verify by **volume**, never by render. Several of these failures look
   completely plausible in a screenshot.

Each case below is VCode. Analytic expected volumes are given; the tessellated
sphere reads about 0.5–1.5% off, which is fine and not the issue.

### Case 1 — PRIMARY. Planar tool crossing an existing spherical face

```
C 80 60 29.5
S 25.35
T 1 40 30 0
D 0 2          # hemispherical pocket -- THIS WORKS, 107,481 expected, 108,026 actual
C 80 18 29.5
D 3 4          # box cut slicing through the pocket -- BROKEN
```

- expected `70,852`
- actual **`34,665`**
- result bbox is `[0, 4.7, -25.3] .. [80, 60, 29.5]`

That `z = -25.3` is the tell. The sphere's *lower* hemisphere was never part of
the solid, but its surface appears in the output mesh. The tool's far side is
being retained with the wrong orientation — this is face/shell classification
after intersection-curve computation, not a missing capability. Note that the
preceding pocket cut on the raw primitive is correct, so the intersector handles
plane/sphere fine; it is the classification of the *second* cut against the
resulting BRep that goes wrong.

**This one case unblocks the part.** The rest are likely the same root cause.

### Case 2 — Sphere protruding through two faces sharing an edge: silent no-op

```
C 80 45 29.5
S 25.35
T 1 40 12 0
D 0 2
```

- expected `77,932`
- actual **`106,200`** — exactly the bare cube. Total no-op.

Compare with the one-face protrusion in Case 1, which is correct, and with a
fully interior sphere (`C 80 60 60` − `S 25.35` at `(40,30,30)`, expected
`219,763`, actual `220,852`), also correct. So the trigger is specifically the
intersection curve running across a cube edge.

### Case 3 — Quadric/quadric: silent no-op, tool surface merged into result

```
S 30
Y 10 80
T 1 0 0 -40
D 0 2
```

- expected `94,782` (sphere `113,097` less an `18,316` cylindrical plug)
- actual **`111,292`** — the bare sphere, unchanged

```
Y 30 61
Y 18.5 40
R 1 90 0 0
D 0 2
```

- actual volume **grew** from `171,367` to `172,080`, and triangle count from
  `128` to `588`. The tool's surface was merged into the result instead of
  subtracting from it. Same signature as Case 1: classification, not
  intersection.

### What works (regression guard — keep these passing)

| operation | expected | actual |
|---|---|---|
| `cube − sphere`, fully interior | 219,763 | 220,852 |
| `cube − sphere`, one-face protrusion | 107,481 | 108,026 |
| `cube − cylinder`, axis-aligned | 197,271 | 197,271 |
| `cube − cylinder`, rotated 90° | 197,271 | 197,271 |
| `sphere − sphere`, concentric | 47,647 | 46,887 |
| `cylinder − coaxial cylinder` | 70,641 | 70,641 |
| `cylinder − parallel-axis cylinder` | — | 152,327 |
| `union(box, box) − cylinder` | 147,750 | 147,965 |

Note the last row: a cylinder cut against a *derived* body is fine. So "derived
body" is not the trigger by itself — the presence of a curved face that the tool
boundary must be classified against is.

## Also seen: unbounded triangle growth

Chained box cuts through a spherical face blow up instead of failing: a two-cut
chain produced **246,446 triangles in 21 s** for a part whose correct output is
under 1,000 triangles. Consistent with garbage intersection curves feeding
downstream booleans. Probably fixed by the same change, but worth a perf
assertion so it cannot regress quietly.

## Asks

1. **Fix `Difference` face/shell classification against curved faces.** Case 1
   is the priority; please check whether Cases 2 and 3 fall out of the same fix.
2. **Add a post-boolean validity check** — closed, orientable, consistently
   oriented, positive volume, Euler characteristic sane. Return an error instead
   of a plausible-looking wrong solid. Every bug above would have been caught
   instantly, and silence is what made this expensive.
3. **Add the cases above as tests**, asserting volume within a few percent of the
   analytic value, not just "does not panic". The no-op failures all produce
   perfectly valid meshes of the *wrong solid*, so mesh-validity assertions alone
   will not catch them; assert volume.
4. **Wire VCode into `load_vcad_document`** so `vcad info` / `vcad export` can
   read v0.2 files directly.

## Acceptance

```
C 80 60 29.5
S 25.35
T 1 40 30 0
D 0 2
C 80 18 29.5
D 3 4
```

returns ~70,900 mm³ with a bbox of `[0, ~4.7, 0] .. [80, 60, 29.5]` — no negative
Z, because no part of the lower hemisphere belongs in the result.

Likely area: `crates/vcad-kernel-booleans` (~5.4K LOC). The root-cause repro for
Case 1 is two operations long, so it should bisect quickly.

---

# POST-FIX EVALUATION — 2026-08-11, main @ 4517f564 (#789 + #792 landed)

Re-ran every case above, plus the real part, against the fixed kernel.

## Fixed and confirmed

| case | expected | before | after |
|---|---|---|---|
| Case 1 pocket + box cut | ~70,852 | 34,665, ghost lower hemisphere | **71,245, zmin = 0.0** |
| Case 2 two-face sphere | ~77,932 | 106,200 (no-op) | **78,325** |
| Case 3a sphere − cylinder | ~94,782 | 111,292 (no-op) | **93,163** |
| Case 3b cyl − perp cyl | 157,165 (MC) | 171,367→172,080 (merged) | **156,194** |

Note on Case 3b: the "~136,000" I originally wrote was a bad estimate on my
part; Monte-Carlo ground truth is 157,165 ± 50, and the kernel now lands
within tessellation error of it. The full regression table also still passes,
and `vcad export` reads both VCode and `.loon` directly now — asks 1, 3 (in
part), and 4 are done. Thank you; Case 1 unblocked the real part.

## Still broken

### A. Cylinder cut breaking through a side face of a union'd body: silent no-op

```
C 80 45 29.5
C 60 35 34.5
T 1 10 5 29.5
U 0 2
Y 12.8 68
R 4 0 90 0
T 5 6 15 47
D 3 6
```

- expected 149,555 ± 55 (MC); actual **178,471** = the bare union. Total no-op.
- Control: the identical construction with the bore CONTAINED in the boss
  cross-section gives 147,770 vs 147,965 expected — correct. The delta between
  the two cases is only that the bore's circle breaks through the boss's
  y = 5 face.

### B. Cylinder cut crossing a 45-degree derived face: cut is ~20% short

From the real part (block − 45°-rotated-box V-groove − cylinder bore):

```
[let bx [fn [sx sy sz px py pz] [translate px py pz [cube sx sy sz]]]]
[let vg [translate 0.0 0.0 -7.4955 [rotate 45.0 0.0 0.0 [bx 60.0 60.0 60.0 -30.0 -30.0 -30.0]]]]
[let nb [translate 0.0 -47.0 0.0 [rotate -90.0 0.0 0.0 [cylinder 17.2 28.0]]]]
[difference nb [difference vg [bx 92.0 70.0 24.0 -46.0 -44.0 0.0]]]
```

- exact (manifold3d): 83,317. kernel: **84,845** (+1,528).
- The bore should remove 7,595; the kernel removes ~6,070. Bare-cylinder
  tessellation deficit is 0.64%, so ~50 mm³ of that gap is legitimate —
  the other ~1,480 is the cut failing where the cylinder crosses the
  V-groove's 45-degree face. Milder than A (partial, not no-op), likely the
  same classification root.

### C. Tessellation emits T-junctions (the mesh side of ask 2)

Both halves of the real part export with correct volume but broken
connectivity: 333 + 16 undirected edges with use-count != 2. I classified
every one — all are T-junctions (a vertex lying exactly on a neighbor's edge
interior, unstitched), zero true cracks, concentrated where the cylinder bore
and bolt slots meet planar faces. Slicers and renderers show these as spurious
backfaces. #792's T-junction stitch evidently does not cover the STL export
path. The post-boolean validity guard (ask 2) is also still open — failures A
and B above were, again, silent.

## Workaround in use

Until A–C land, the printable STLs are produced by mirroring the same CSG in
manifold3d (96-segment circles) — watertight, winding-consistent, and matching
Monte-Carlo ground truth within noise on both halves. Script:
`build_manifold.py` alongside this file. The .loon source remains the design
of record.

---

# ROUND-2 EVALUATION — 2026-08-11, main @ 8e747cfe (#793 landed)

- **Defect A fixed**: union side-face bore now 149,602 vs 149,555 ± 55 MC.
- **Defect B fixed**: 45°-face bore now 83,311 vs 83,317 exact (was 84,845).
- **Defect C remains**: STL export still emits T-junctions. Real part, upper
  half: 61 undirected edges with use != 2 (down from 333 pre-#793), lower: 16.
  Volumes are now within tessellation error (121,591 vs 121,091 exact; 74,312
  vs 73,912), so this is purely the export stitching. The validity-guard ask
  also remains open.

Workaround retired 2026-08-11: the printable STLs are now exported by vcad
itself (volumes +0.5%/+0.6% vs ground truth). Defect C's T-junctions
(61 upper / 16 lower edges) ship in those STLs until the export stitch lands.

---

# NEW FINDING, 2026-08-12 — Defect D: tessellation puts vertices off the surface

Sphere and boolean logic are now correct (all volumes verify), but the exported
mesh's vertices do not lie on the analytic surfaces. Control case, current main:

    [root [difference [translate 0.0 0.0 0.2 [sphere 25.0]]
                      [translate -38.0 -40.0 0.2 [cube 76.0 62.0 19.8]]] "m"]

The spherical-pocket vertices should all sit at r = 25.000 from the sphere
center. Measured: **r = 24.770 .. 25.593** — up to 0.6 mm off the surface, with
a histogram smeared across the whole band (not one snapped shell). Chord sag at
the observed facet size would be 0.03 mm; this is 20x that, so it is vertex
placement, not facet density. Suspect the healed/stitch fallback quantizing or
re-welding vertices off-surface.

Impact: this is now the accuracy floor for every printed part. A Ø50 conforming
socket with a 0.05–0.4 mm designed fit is unmanufacturable from these meshes —
the surface error exceeds the entire fit budget. (Planar geometry is exact, and so are
cylinders — a bore control exports every wall vertex at r = 17.5000 flat. The
defect is specific to SPHERE tessellation, which narrows the search a lot.)

Ask: vertices of tessellated quadric faces on the analytic surface to ~1e-3 mm,
plus a fidelity assertion (max |r - R| over sampled surface vertices) in tests.
Volume assertions cannot catch this — the errors average out.
