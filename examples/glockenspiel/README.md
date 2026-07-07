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

The closed form assumes a uniform bar — but the cord holes remove bending
stiffness where mode-1 curvature is nonzero, and the **hole-aware 1-D FEM**
behind the `simulate_strike` MCP tool showed that flattens every bar by
**≈ −5 cents**. So the cut lengths are hole-compensated (≈ 0.2 mm shorter),
and every bar is then struck *in simulation* and verified through the audio
path: modal synthesis → WAV → FFT peak → cents error.

| note | target Hz | closed-form L | cut L (mm) | holes @ (mm) | strike FFT Hz | err (¢) |
|------|-----------|---------------|------------|----------------|---------------|---------|
| C6 | 1046.50 | 125.6 | 125.4 | 28.11 / 97.29 | 1046.23 | −0.44 |
| D6 | 1174.66 | 118.5 | 118.3 | 26.52 / 91.78 | 1175.42 | 1.12 |
| E6 | 1318.51 | 111.9 | 111.7 | 25.04 / 86.66 | 1318.29 | −0.29 |
| F6 | 1396.91 | 108.7 | 108.5 | 24.33 / 84.17 | 1396.97 | 0.08 |
| G6 | 1567.98 | 102.6 | 102.4 | 22.96 / 79.44 | 1568.03 | 0.06 |
| A6 | 1760.00 | 96.8 | 96.6 | 21.66 / 74.94 | 1761.71 | 1.68 |
| B6 | 1975.53 | 91.4 | 91.2 | 20.45 / 70.75 | 1976.03 | 0.43 |
| C7 | 2093.00 | 88.8 | 88.6 | 19.86 / 68.74 | 2093.45 | 0.37 |

(err is dominated by rounding the compensated lengths to the 0.1 mm cut
grid. The real-world delta will be dominated by stock-thickness tolerance:
±0.1 mm on 3.175 mm stock is ±3 % on pitch. That error is the demo — see
the calibration act in the plan.)

Reproduce the table without building anything (closed form; a built
workspace upgrades it to the hole-compensated lengths):

```bash
node examples/glockenspiel/frequencies.mjs
```

## The audio, before the metal

`node build.mjs` strikes every bar in simulation via the same code path as
the **`simulate_strike` MCP tool**: hole-aware FEM modal frequencies,
strike-position gains (center strike = antinode of mode 1, node of mode 2 —
like a real player), a half-sine hard-mallet contact filter, and Q-based
decay where the cord at the nodal holes selectively preserves the
fundamental (that's *why* the holes are there — now audible). The result is
a 16-bit/44.1 kHz WAV per bar in `out/`, and the **order gate**: the
dominant FFT peak of each synthesized strike must land within ±5 cents of
its target note, or the build fails.

The strike's upper partials sit at the non-harmonic free-free ratios
(≈ 2.76, 5.40, 8.93 × f₁) and die in tenths of a second while the
fundamental rings for seconds — that inharmonic ping into a pure tone *is*
the glockenspiel timbre. Play `out/bar-C6.wav` and judge.

Model limits, stated: 1-D transverse bending only (no torsional modes);
decay is a documented heuristic — the frequencies are not.

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
| `bar-C6.wav` … `bar-C7.wav` | the simulated strike of each bar — what the aluminum should sound like |
| `stand.dxf` | stand flat pattern: CUT layer + DASHED bend centerlines, relief notches included — **5052-H32, 0.125″** |
| `stand.step` | the folded stand as B-rep STEP — SCS auto-detects bends from it (zero data entry) |
| `stand.glb` | folded stand mesh for a 3D viewer |
| `*.vcad` | editable parametric sources |
| `frequency-table.md` | the tuning claims above, regenerated |
| `receipt.json` | every claim with its oracle: pitches, hole positions, masses, bend angles, DFM verdicts, audio verdicts, cost |

## The receipt

| claim | oracle | instrument |
|-------|--------|-----------|
| f₁ per bar | hole-aware FEM + strike sim FFT (`simulate_strike`) | phone FFT, error in cents |
| hole positions on nodal lines | `receipt.json` / drawing dims | caliper |
| total mass ≈ 715 g | sheet cost model (ρ · A · t) | kitchen scale |
| bend angles 90° | bend model + springback compensation | protractor |
| DFM legal for SCS | `sheet_metal_check` vs shop catalog | the order is accepted |
| cost estimate | `sheet_metal_cost` (generic rates) | the actual invoice |
| the sound itself | `out/bar-*.wav` (synthesized strike) | your ears vs the real strike |

Model caveats, stated up front: alloy tolerance on E moves all bars
together; thickness tolerance is the big lever and is exactly what the
calibration act measures; audio decay is a heuristic, frequencies are not.

## Assembly

Thread paracord through each bar's nodal holes and the matching deck holes;
knots ride inside the channel. Bars rest on the cord, touching nothing rigid.
Strike with a phenolic or hard-plastic mallet, watch the spectrogram.
