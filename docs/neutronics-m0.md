# Neutronics M0: Monte Carlo neutron transport for shielding and dosimetry

`vcad-kernel-neutronics` makes vcad a design tool for the radiation-safety
question every neutron-producing bench experiment must answer before first
beam: **what is the dose rate at the operator's chair, and how much
moderator makes it acceptable?** The first customer is in this repo: the
shielded-grid IEC experiment (`docs/shielded-grid-experiment.md`) needs a
Phase B "dose budget and distance plan" for its 2.45 MeV D-D neutron
source. The second is neutron-activation-analysis feasibility (thermal
flux at a sample position).

The incumbent for this loop (MCNP lineage) is closed and export-controlled
— procedurally inaccessible to the hobbyist and small-lab users vcad
serves, even though *benign shielding design* is textbook health physics.
An open, receipt-native transport code for moderation/shielding/dose is a
real contribution, and it composes with the rest of the kernel: the shield
that this crate sizes is the same geometry the CAD side details and the
CAM side fabricates.

## Scope and refusals (load-bearing)

**In scope:** fixed-source neutron moderation, shielding, and dosimetry.

**Refused, permanently:** fission physics. No fission cross sections, no
neutron multiplication, no criticality search, no fissile-material data —
not at M0, not at any milestone. The legitimate uses of those capabilities
(reactor design) are served by regulated codes inside institutional review
chains that an open CAD kernel neither can nor should replace. If a
request needs k_eff, this is the wrong tool and the docs say so.

**Honesty bounds at M0** (each stated on results, never silent):

- **Design-estimate library, not an evaluated nuclear data file.** Group
  constants are single-point reads of evaluated-data plots (ENDF/B-VIII.0
  lineage) plus standard thermal references (Sears 1992, Mughabghab /
  Lamarsh 2200 m/s values with 1/v extrapolation), good to ±20–30%.
  Every value in `materials.rs` carries its citation as a comment.
- **Neutron dose only.** Capture gammas (H(n,γ) 2.22 MeV — the dominant
  secondary in any hydrogenous shield) are not transported. A lead liner
  is the gamma answer; lead is in the library for stack studies but its
  MeV *inelastic* scattering is unmodeled, so it reads more
  neutron-transparent than reality (conservative direction for neutron
  dose, useless for gamma dose — budget gammas separately).
- **Isotropic lab-frame scattering** (M1 adds the P1 correction;
  forward-peaked hydrogen scatter raises deep-penetration doses).
- **Free-atom scattering** except a documented bound-hydrogen thermal
  adjustment calibrated to water's published thermal diffusion
  coefficient (Lamarsh Table 5-2).
- **Free field**: no room return. Concrete walls add back-scatter that
  this geometry cannot see.

## What M0 is

- **Multigroup structure** (`groups.rs`): 5 groups, 2.45 MeV → thermal,
  boundaries 3 MeV / 1 MeV / 100 keV / 1 keV / 0.5 eV / 0.1 meV; source
  group anchored at the D-D line, others at log-midpoints.
- **Materials** (`materials.rs`): HDPE, paraffin, 5% borated poly, water,
  lead, concrete (NIST ordinary), air, plus validation fictions (pure
  absorber, one-group medium, void). Group-transfer matrices are derived
  *in the code* from the isotropic-CM elastic kernel (E′ uniform on
  [αE, E]) averaged over a flat-in-lethargy intra-group flux — the
  standard multigroup construction, numerically at build time, no opaque
  tables.
- **Geometry** (`geometry.rs`): 1D slab stacks and concentric spherical
  shells, thicknesses in millimeters (vcad convention). Every layer is a
  tally region; detectors are thin shells.
- **Transport** (`transport.rs`): analog fixed-source MC — exponential
  flights, absorption, group downscatter, isotropic scatter. Analog means
  the books must balance: `absorbed + leaked = 1` exactly per batch
  (asserted, not hoped). Collision-cap truncations are counted and
  reported; a claims-grade run requires zero.
- **Tallies** (`tally.rs`): track-length flux per region per group,
  surface-crossing currents, leakage spectrum, absorbed fraction,
  slowing-down observables (thermalization fraction, collisions-to-
  thermal, ⟨r²⟩ at thermalization — the Fermi-age observable). **Every
  quantity is an `Estimate` (mean ± relative standard error over ≥ 2
  batches); a result without an error bar is unrepresentable in the
  API.** A tally that scored nothing reports RSE = ∞ — zero events is a
  statistics floor, not a measured zero, and it fails closed.
- **Dose** (`dose.rs`): ambient dose equivalent H*(10) via an
  ICRP-74-style fluence-to-dose point curve (22 anchor energies,
  transcribed at design-estimate precision), log-log interpolated,
  1/E-weighted per group. The weighting is stated because the
  keV group's factor moves ~2× under different intra-group spectrum
  assumptions.
- **RNG** (`rng.rs`): hand-rolled xoshiro256++ + splitmix64 seeding.
  Zero dependencies, bit-identical reruns.

## Validation ladder (all in `cargo test -p vcad-kernel-neutronics`)

1. **Uncollided point-source flux, exact:** pure absorber,
   φ(r) = S·e^{−Σt·r}/4πr² volume-averaged over the tally shell; MC
   within 4σ and 5%.
2. **Slab transmission, exact:** T = e^{−Σt·x} at 3 mfp.
3. **1/√N honesty:** RSE falls ×2 when histories ×4.
4. **Buildup:** total flux in water at 3.3 mfp exceeds the uncollided
   line (1.5–100× band) while the source-group flux still contains it.
5. **Dose monotone in HDPE thickness**, with ≥ 7× reduction at 20 cm.
6. **Slowing-down physics:** ≳ 60% thermalization in 40 cm of water,
   collisions-to-thermal near the hydrogen ladder (~18 in continuous
   energy), ⟨r²⟩ in the Fermi-age ballpark (quantitative comparison is
   the M5 benchmark).
7. **Boron does its job:** borated poly absorbs more and cuts the
   detector thermal column > 2× vs plain poly.
8. Exact analog balance, bit-identical reproducibility, fail-closed
   config/geometry/material validation, fail-closed zero tallies.

## The M0 headline (examples/fusor_shield.rs)

Isotropic 2.45 MeV point source, 30 cm air chamber stand-in, HDPE shell,
air to detector shells at 1 m and 2 m. 10⁶ histories/config, seed
20260717:

| shield | dose @ 1 m (µSv/h per 10⁶ n/s) | @ 2 m |
|---|---:|---:|
| bare | 12.108 ± 0.0% | 3.275 ± 0.1% |
| 5 cm HDPE | 5.013 ± 0.1% | 1.345 ± 0.1% |
| 10 cm HDPE | 1.619 ± 0.3% | 0.432 ± 0.3% |
| 20 cm HDPE | 0.152 ± 1.0% | 0.0400 ± 1.1% |
| 15 cm HDPE + 5 cm borated-5% | 0.149 ± 0.9% | 0.0394 ± 1.0% |

Checks and findings:

1. **The bare row is the analytic anchor:** 12.03 µSv/h uncollided at
   1 m for 10⁶ n/s; MC reads 12.11 (air in-scatter sits on top). The
   1 m → 2 m ratio is 3.70 — 1/r² minus a little air scatter.
2. **HDPE tenth-value thickness for D-D dose ≈ 10 cm** (5 cm buys 2.4×,
   10 cm 7.5×, 20 cm 80×) — consistent with the fusor-community rule of
   thumb and with removal-theory expectations for 2.45 MeV.
3. **Borated ≈ plain for fast dose at equal thickness.** Boron kills the
   *thermal* column (rung 7: > 2× thermal-flux cut), which matters for
   the H(n,γ) capture-gamma source term and for activation — not for the
   fast-neutron dose that dominates these detectors. The honest sales
   pitch for the borated layer is gamma-budget and activation control,
   and the M0 gamma caveat stands next to it.

## Milestone ladder

- **M0 — analog multigroup MC + validation + dose. DONE** (this
  document).
- **M1 — energy/angle fidelity.** P1 (linearly anisotropic) lab-frame
  scattering from the stored per-group μ̄; thermal-group treatment
  honesty; effect quantified against M0 isotropic (deep-penetration dose
  shift).
- **M2 — gradients.** Deterministic multigroup adjoint **diffusion**
  companion: forward + adjoint solves, importance function, d(dose)/d
  (layer thickness) via interface perturbation theory, FD-validated. MC
  stays the truth oracle; the diffusion adjoint is the design compass
  (pairing documented, diffusion bias measured against MC).
- **M3 — parameter seam.** Serde `ShieldSpec` with named parameters,
  fail-closed resolution (unknown material / unbound name / bad
  thickness = error, never a default).
- **M4 — receipt claims.** `vcad.neutronics-claims/1`: dose-rate claims
  with MC uncertainty AND method provenance (histories, batches, groups,
  library version, seed), compare() with Holds / Violated / Unmeasured
  verdicts, fail-closed (an unmeasured receipt never passes).
- **M5 — benchmarks + convergence + paper draft.** Published-value
  benchmarks (water thermal diffusion length 2.85 cm, Fermi age ≈ 27 cm²,
  hydrogen collisions-to-thermal ≈ 18), one-group MC vs analytic
  diffusion cross-check, convergence study, paper skeleton.
- **M6 — the application.** The shielded-grid experiment's Phase B
  shield: operator at 2 m, 5×10⁶ n/s, stated dose budget — a
  section-ready table for `docs/shielded-grid-experiment.md` with error
  bars and library caveats on every number.

## Non-goals

No fission, no multiplication, no criticality — see Scope and refusals.
No claim of evaluated-data accuracy: this crate buys *honest error bars
on design estimates*, and every claim says which physics is missing.
