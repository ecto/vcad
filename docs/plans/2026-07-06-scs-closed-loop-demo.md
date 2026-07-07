# The receipt you can hear — SCS closed-loop demo

*First public prompt-to-part loop, per
[the convergence strategy](2026-07-06-convergence-strategy.md) sequencing
step 2. Vendor: SendCutSend (JLCPCB denied the API application; SCS export,
shop profiles, and cost model are already in `vcad-kernel-sheet`). 2026-07-06.*

## The pick: a laser-cut glockenspiel

An eight-bar aluminum glockenspiel (C6–C7 major scale) on a folded
sheet-metal stand, one SCS order. Every dimension is physics:

- **Bar lengths** are solved from the target pitches — a free-free bar's
  fundamental is closed-form, and `materials.rs` already carries the E and ρ
  the formula needs.
- **Mounting holes** sit exactly on the nodal lines of the fundamental mode
  (0.2242 · L from each end) — the hole *positions* are a receipt claim.
- **The frame** is a folded U-channel: bends, reliefs, unfold, DFM — the parts
  of `vcad-kernel-sheet` a flat part wouldn't exercise.

Why this beats a bracket: the receipt is **audible**. A phone spectrogram is a
precision instrument anyone owns, pitch error in cents is objective and
brutal, and "we compiled a C major scale into metal" needs no engineering
literacy to land. Vibraphone bars are aluminum in real life — this is not a
toy approximation.

## The physics (all closed-form, all differentiable)

Free-free transverse vibration of a rectangular bar:

```
f₁ = (4.730)² / (2π L²) · sqrt(E I / ρ A)   with I/A = t²/12
   = 3.5608 · (t / L²) · sqrt(E / 12ρ)
```

For 6061 (E = 69 GPa, ρ = 2700 kg/m³ — values from
`vcad-kernel-sheet/src/materials.rs`) at t = 3.175 mm (0.125″):
f₁ ≈ 16.50 / L² (L in meters). Solving for the scale:

| note | target Hz | bar length | nodal holes at |
|------|-----------|-----------|----------------|
| C6   | 1046.5    | 125.6 mm  | 28.2 mm from ends |
| D6   | 1174.7    | 118.5 mm  | 26.6 mm |
| E6   | 1318.5    | 111.9 mm  | 25.1 mm |
| F6   | 1396.9    | 108.7 mm  | 24.4 mm |
| G6   | 1568.0    | 102.6 mm  | 23.0 mm |
| A6   | 1760.0    | 96.8 mm   | 21.7 mm |
| B6   | 1975.5    | 91.4 mm   | 20.5 mm |
| C7   | 2093.0    | 88.8 mm   | 19.9 mm |

Bar width 25 mm (width doesn't move f₁ to first order — it cancels in I/A).
Bars suspended on cord through the nodal holes; frame powder-coated, bars raw
(coatings damp the ring).

## The receipt

| claim | oracle | instrument |
|-------|--------|-----------|
| f₁ per bar (Hz) | analytic beam model + materials.rs | phone FFT, error in cents |
| total mass (g) | inspect_cad mass properties | kitchen scale |
| bar lengths, hole positions | drawing dims | caliper |
| frame bend angles 90° | bend model + springback | protractor |
| DFM legal for SCS | sheet_metal_check, shop profile | the order is accepted |
| cost estimate | sheet_metal_cost | the actual invoice |

## The calibration act (the part that makes it strategy, not content)

Predicted f₁ will be off by a few percent — E and ρ have alloy tolerance, and
±0.1 mm on a 3.175 mm sheet is ±3 % on pitch, directly. **That error is the
demo.** On camera:

1. Predict from nominal dims → measure → show the delta (likely 20–60 cents).
2. Caliper the *as-built* thickness, re-run the receipt with measured t.
3. Residual collapses. The gap between step 1 and 3 is sim2real calibration
   happening in public — the "order flow is the instrument" thesis, performed.

## Pipeline exercised

`create_cad_loon` (bars + frame) → `sheet_metal_check` (SCS profile) →
`sheet_metal_unfold` → DXF export → `sheet_metal_cost` →
`quote_manufacturing` → `place_order` (human-authorized per the wallet
model) → wait for the box → measure → publish receipt vs. reality.

## Stretch goal — where gradients earn their keep

Bar lengths are closed-form; no gradient needed (say so honestly). But a
free-free bar's second partial sits at a non-harmonic 2.76 · f₁. Real
vibraphone bars are *undercut* to retune it to 4 · f₁. We can't vary
thickness (2D cutting), but **in-plane width profiling** w(x) changes the
variable-coefficient beam equation — EI(x) and ρA(x) no longer cancel — so
the overtone ratio is tunable. No closed form exists; a 1-D FEM beam
eigenproblem (tang-la) priced through the differentiable seam solves it.
Gradient-tuned harmonic bars, cut flat: that's the flex for act two.

## Runners-up (kept for later waves)

- **Servo gripper** (cross-domain: sheet metal + kinematics + phyz + gym) —
  "the robot was trained before its body existed." Wave-2 material; needs
  assembly, a servo, and the training story told well.
- **Balance sculpture** — center-of-mass receipt, balances only if the kernel
  is right. Minimal fallback; single number, less wow.
- **F405 enclosure** — practical, ties to `check_enclosure_fit` and the
  existing example, but visually a box.
