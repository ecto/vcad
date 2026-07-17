# Phase B neutron dose plan (shielded-grid IEC experiment)

**The "dose budget and distance plan before Phase B" required by
`docs/shielded-grid-experiment.md` → Safety** (whose neutron bullet now
carries the summary and points here). Every number regenerates from
`cargo run --release -p vcad-kernel-neutronics --example phase_b_shield`
(seed 20260717, 10⁶ histories/config).

## Design point and budget

- Source: isotropic 2.45 MeV D-D point source at **5×10⁶ n/s** — the
  amateur-record scale the chain+volume channels aim at, an order above
  the predicted beam-on-background floor (1.9×10⁵–5×10⁵ n/s), so the
  plan is conservative against the machine *over*-performing its floor.
- Operator position: **2 m** from the chamber center. Bystander check
  at 1 m.
- Stated design budget: **2.5 µSv/h at the operator** — a design
  choice (≈1 mSv across 400 h of run time, the general-public annual
  limit), **not a regulatory determination**; verify local rules before
  Phase B. Pass criterion is fail-closed: mean + 2σ under budget.

## The table (µSv/h, mean ± MC relative standard error)

| shield | dose @ 1 m | dose @ 2 m | budget @ 2 m |
|---|---:|---:|---|
| bare | 60.57 ± 0.0% | 15.17 ± 0.0% | over budget |
| 5 cm HDPE | 35.61 ± 0.1% | 8.88 ± 0.1% | over budget |
| 10 cm HDPE | 16.01 ± 0.2% | 3.99 ± 0.2% | over budget |
| **15 cm HDPE** | **6.53 ± 0.3%** | **1.62 ± 0.3%** | **PASS (mean + 2σ under)** |
| 20 cm HDPE | 2.52 ± 0.5% | 0.62 ± 0.5% | PASS |
| 25 cm HDPE | 0.93 ± 0.6% | 0.23 ± 0.6% | PASS |
| 15 cm HDPE + 5 cm borated-5% | 2.43 ± 0.4% | 0.60 ± 0.4% | PASS |

**Chosen design: 15 cm HDPE** (castle around the chamber, ~15 cm air
gap chamber-wall to shield), margin 35% under budget at the mean.
Design compass at the chosen point: d(dose@2m)/d(thickness) =
−0.037 µSv/h per mm (M2 diffusion adjoint) — the next 2 cm of HDPE buys
roughly another ×2.6, matching the MC rows above.

The borated option (swap the outer 5 cm for 5% borated poly at 20 cm
total) does **not** buy fast-neutron dose over plain 20 cm HDPE — its
value is the >2× lower thermal column, i.e. a smaller H(n,γ)
capture-gamma source term and less activation. Which leads to:

## Caveats that are load-bearing for Phase B

1. **This is a neutron-only number.** Capture gammas (2.22 MeV from
   H(n,γ) in the shield itself) are NOT included and are the reason
   shield designs pair poly with a few mm of lead on the *outside*.
   Survey with a gamma instrument too; budget gammas separately.
2. Design-estimate library: ±20–30% on group constants bounds every row
   (the MC error bars are much smaller than the library uncertainty —
   the receipt carries both).
3. Free field: no room return. A concrete room adds back-scatter;
   measure, don't assume.
4. The model chamber is air — no steel wall (a real ~3–6 mm steel
   chamber perturbs the fast flux by a few %, well inside the library
   band).

## The receipt loop (binds to the experiment's Phase B)

`predicted_claims` on the chosen design emits
`vcad.neutronics-claims/1` with per-position dose-rate, attenuation and
thermal-flux claims, each carrying MC uncertainty and full method
provenance (seed 20260717, 10⁶ histories, 5 groups, exact-kinematics,
library vcad-neutronics-lib/0.1.0-design-estimate). Phase B binds
survey-meter readings at 1 m and 2 m via `receipt::compare` —
band_factor ≥ 1.5 on absolute doses (the library caveat), tighter on
the attenuation *ratio* (library-common-mode cancels). Holds /
Violated / Unmeasured, fail-closed; a Violated dose claim is a
publishable result about the library and a **stop-work** result for the
session. Survey-meter verification on every voltage/current increase
remains mandatory regardless of what this table says.

## Second customer: activation-analysis feasibility

Thermal flux at a sample tucked 2.5 cm into the shield's inner face:
**3.3×10³ n/cm²/s ± 0.2%** at 5×10⁶ n/s. Foil activation for detector
calibration: workable (indium/gold foils with long counts).
Trace-element NAA wants ≥10⁵–10⁷ n/cm²/s: this source is 1.5–3.5
orders below — calibration yes, assay no. (The honest number, so
nobody plans an assay program around a fusor.)
