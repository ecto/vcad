# The receipt you can hear — a laser-cut glockenspiel

The first public prompt-to-part loop, per
[the SCS closed-loop demo plan](../../docs/plans/2026-07-06-scs-closed-loop-demo.md):
eight aluminum bars tuned to a C6–C7 major scale plus a folded sheet-metal
stand, one SendCutSend order. **Every dimension is physics**, and every claim
has an oracle a normal person owns — the loudest one being a phone
spectrogram.

## The physics

A free-free rectangular bar's fundamental is closed-form (Euler–Bernoulli,
first mode):

```
f₁ = (4.730)² / (2π L²) · sqrt(E I / ρ A)   with I/A = t²/12
   = 3.5608 · (t / L²) · sqrt(E / 12ρ)
```

With E = 69 GPa and ρ = 2700 kg/m³ for 6061-T6 — the values in the kernel's
own registry (`crates/vcad-kernel-sheet/src/materials.rs`, asserted at build
time) — and t = 3.175 mm (0.125″), that's **f₁ ≈ 16.50 / L²** (L in meters).
Bar lengths are solved from the target pitches; the cord holes sit exactly on
the fundamental's nodal lines at **0.2242·L** from each end, so the
suspension neither damps nor detunes the note.

| note | target Hz | L (mm) | holes @ (mm) | predicted Hz | err (¢) |
|------|-----------|--------|----------------|--------------|---------|
| C6 | 1046.50 | 125.6 | 28.16 / 97.44 | 1045.84 | −1.10 |
| D6 | 1174.66 | 118.5 | 26.57 / 91.93 | 1174.92 | 0.38 |
| E6 | 1318.51 | 111.9 | 25.09 / 86.81 | 1317.60 | −1.20 |
| F6 | 1396.91 | 108.7 | 24.37 / 84.33 | 1396.32 | −0.74 |
| G6 | 1567.98 | 102.6 | 23.00 / 79.60 | 1567.29 | −0.77 |
| A6 | 1760.00 | 96.8 | 21.70 / 75.10 | 1760.73 | 0.72 |
| B6 | 1975.53 | 91.4 | 20.49 / 70.91 | 1974.93 | −0.53 |
| C7 | 2093.00 | 88.8 | 19.91 / 68.89 | 2092.27 | −0.61 |

(err is the cost of rounding lengths to the 0.1 mm cut grid — inaudible.
The real-world delta will be dominated by stock-thickness tolerance: ±0.1 mm
on 3.175 mm stock is ±3 % on pitch. That error is the demo — see the
calibration act in the plan.)

Reproduce the table without building anything:

```bash
node examples/glockenspiel/frequencies.mjs
```

## The stand

A folded **5052-H32** U-channel (SCS cuts 6061 but doesn't bend it — 5052 is
their bending stock at 0.125″): a 300 × 100 mm deck with chamfered corners,
two 30 mm walls folded down, and outward feet. The deck carries 16 cord
anchor holes in two converging rows — each pair sits directly under a bar's
nodal holes, so the tuning physics is visible in the frame itself.

The fold chain is also the DFM demo: as naively designed, the chamfered
corners leave material at the wall-bend ends and `sheet_metal_check` flags
four tear-out warnings against SendCutSend's published capabilities.
`sheet_metal_suggest_fix` answers `add_bend_relief`; the build applies it
**in the design** (parametric relief notches, visible in the DXF) and
re-checks to zero violations. Bend angles ship springback-compensated
(form to 90.90°, springs back to 90°) via `sheet_metal_sequence`.

## Run it

```bash
# From the repo root (fresh worktree? run `npm ci` first):
npm run build --workspaces        # build @vcad/engine, @vcad/mcp …
node examples/glockenspiel/build.mjs
```

Outputs land in `out/`:

| File | What it is |
|------|-----------|
| `bar-C6.dxf` … `bar-C7.dxf` | one flat DXF per bar — upload each to SCS as **6061-T6, 0.125″, raw** (coatings damp the ring) |
| `stand.dxf` | stand flat pattern: CUT layer + DASHED bend centerlines, relief notches included — **5052-H32, 0.125″** |
| `stand.step` | the folded stand as B-rep STEP — SCS auto-detects bends from it (zero data entry) |
| `stand.glb` | folded stand mesh for a 3D viewer |
| `*.vcad` | editable parametric sources |
| `frequency-table.md` | the tuning claims above, regenerated |
| `receipt.json` | every claim with its oracle: pitches, hole positions, masses, bend angles, DFM verdicts, cost |

## The receipt

| claim | oracle | instrument |
|-------|--------|-----------|
| f₁ per bar | closed-form beam model + kernel materials registry | phone FFT, error in cents |
| hole positions on nodal lines | `receipt.json` / drawing dims | caliper |
| total mass ≈ 715 g | sheet cost model (ρ · A · t) | kitchen scale |
| bend angles 90° | bend model + springback compensation | protractor |
| DFM legal for SCS | `sheet_metal_check` vs shop catalog | the order is accepted |
| cost estimate | `sheet_metal_cost` (generic rates) | the actual invoice |

Model caveats, stated up front: the Ø4.2 mm nodal holes aren't in the beam
model (second-order at the nodes); alloy tolerance on E moves all bars
together; thickness tolerance is the big lever and is exactly what the
calibration act measures.

## Assembly

Thread paracord through each bar's nodal holes and the matching deck holes;
knots ride inside the channel. Bars rest on the cord, touching nothing rigid.
Strike with a phenolic or hard-plastic mallet, watch the spectrogram.
