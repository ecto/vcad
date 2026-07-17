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

## The headline (examples/fusor_shield.rs)

Isotropic 2.45 MeV point source, 30 cm air chamber stand-in, HDPE shell,
air to detector shells at 1 m and 2 m. 10⁶ histories/config, seed
20260717, M1 exact-kinematics default:

| shield | dose @ 1 m (µSv/h per 10⁶ n/s) | @ 2 m | M0-isotropic @ 1 m |
|---|---:|---:|---:|
| bare | 12.110 ± 0.0% | 3.279 ± 0.0% | 12.108 |
| 5 cm HDPE | 7.025 ± 0.1% | 1.884 ± 0.1% | 5.013 |
| 10 cm HDPE | 3.098 ± 0.2% | 0.826 ± 0.2% | 1.619 |
| 20 cm HDPE | 0.474 ± 0.5% | 0.125 ± 0.5% | 0.152 |
| 15 cm HDPE + 5 cm borated-5% | 0.454 ± 0.5% | 0.120 ± 0.5% | 0.149 |

Checks and findings:

1. **The bare row is the analytic anchor:** 12.03 µSv/h uncollided at
   1 m for 10⁶ n/s; MC reads 12.11 (air in-scatter sits on top). The
   1 m → 2 m ratio is 3.69 — 1/r² minus a little air scatter.
2. **Angle–energy correlation is a 3× effect at 20 cm.** The last
   column is the M0 isotropic-multigroup model: it *understated* the
   1 m dose by ×1.4 at 5 cm, ×1.9 at 10 cm, ×3.1 at 20 cm — the
   forward-peaked, low-energy-loss tail of hydrogen scattering carries
   dose through thick shields, growing with depth exactly as it should.
   This is why exact kinematics is the default and the model tag rides
   in provenance on every result.
3. **HDPE tenth-value thickness for D-D dose ≈ 14 cm** with the honest
   physics (25× at 20 cm; the isotropic model flattered it at ~10 cm) —
   still inside the fusor-community rule-of-thumb range, now with the
   caveat budget visible.
4. **Borated ≈ plain for fast dose at equal thickness.** Boron kills
   the *thermal* column (rung 7: > 2× thermal-flux cut), which matters
   for the H(n,γ) capture-gamma source term and for activation — not
   for the fast-neutron dose that dominates these detectors. The honest
   sales pitch for the borated layer is gamma-budget and activation
   control, and the M0 gamma caveat stands next to it.

## Milestone ladder

- **M0 — analog multigroup MC + validation + dose. DONE** (this
  document).
- **M1 — energy/angle fidelity. DONE** (`scatter.rs`,
  `EnergyModel::ExactKinematics`, now the default). Instead of the
  planned P1 bias — which is not even a valid pdf for hydrogen (μ̄ = 2/3
  drives (1+3μ̄μ)/2 negative) — M1 samples the collision nuclide from
  its Σ_s share and takes outgoing energy AND lab angle from the same
  isotropic-CM cosine: exact two-body elastic kinematics, the
  distribution P1 approximates. The particle carries continuous energy
  (groups only index σ). Hydrogen's classic results fall out and are
  tested: μ_lab = √(E′/E) ≥ 0 (no lab backscatter), ⟨μ_lab⟩ = 2/(3A),
  collisions-to-thermal in water ≈ the ln(E₀/E_th) ladder. Quantified
  effect: deep-penetration dose up ×1.4/×1.9/×3.1 at 5/10/20 cm HDPE vs
  the M0 isotropic model (headline table). The multigroup model is
  retained as `EnergyModel::Multigroup` — it is the exact stochastic
  mirror of the M2 diffusion companion — and the model tag rides in
  provenance. Stated limits: isotropic-in-CM (MeV p-wave forward
  hardening not modeled), thermal group in-group isotropic with no
  free-gas motion.
- **M2 — gradients. DONE** (`diffusion.rs`). Deterministic multigroup
  diffusion companion: cell-centered FV (harmonic face D, Marshak
  vacuum boundary), downscatter-ordered tridiagonal solves; adjoint =
  same symmetric per-group operator with the group coupling transposed,
  solved in reverse — forward/adjoint duality holds to ~1e-15
  (asserted). `d(dose)/d(layer thickness)` via the interface
  shape-derivative with growth-into-neighbor semantics (shield grows
  into its air gap; detector and outer wall fixed — the design-real
  parameterization). Two bugs the FD validation caught and the tests
  now pin: (1) at a shield/air interface the naive δD·∇φ·∇φ† surface
  term is off ×43 — ∇φ is discontinuous there; the correct form is the
  flux term −[[1/D]]·J·J† with the continuous face current; (2) its
  sign (Hadamard interface derivative). Final agreement: adjoint vs
  diffusion-FD to 0.1%, adjoint log-gradient vs **Monte Carlo** FD
  through 12 cm HDPE within the 25% test band. The pairing, measured:
  diffusion absolute dose at 1 m through 12 cm HDPE reads ×1.54 the MC
  oracle (void-region flux flattening + diffusion-in-shield bias) —
  the compass steers, the oracle prices. One-group triangle test: MC ↔
  diffusion ↔ e^{−r/L}/4πDr all agree deep in a scattering sphere.
- **M3 — parameter seam. DONE** (`spec.rs`). Serde `ShieldSpec`
  (spherical layer stack, central point source, labeled dose detectors)
  where every numeric field is a literal **or a named document
  parameter**. Fail-closed resolution: unbound name, unknown material,
  source energy outside the group structure, non-positive rate, detector
  outside/overlapping — each an error, never a default; a spec with no
  detectors is rejected outright (a shield with nothing to protect is a
  bookkeeping bug). The resolver *builds* the tally regions by
  splitting layers around detector radii and returns the label→region
  map. `parameter_roles()` classifies gradient paths: thicknesses →
  M2 diffusion adjoint (`d_dose_d_param_via_diffusion`, µSv/h per mm),
  source rate → exactly linear, source energy → group-discrete.
  BRep extraction (shield solid → layer stack) lands on the vcad side
  of the seam, emitting this schema.
- **M4 — receipt claims. DONE** (`receipt.rs`).
  `vcad.neutronics-claims/1`: per-detector dose-rate, attenuation-factor
  and thermal-flux (NAA feasibility) claims, each carrying its MC
  relative standard error AND the method provenance (seed, histories,
  batches, groups, energy model, library version) plus the standard
  caveat list in the same JSON object — a dose number without its
  uncertainty and its recipe is a rumor, not a claim. Fail-closed at
  construction: truncated-history runs refuse to claim; a zero-scored
  tally refuses by name (statistics floor ≠ measured zero). `compare()`
  binds survey-meter/foil measurements with Holds / Violated /
  Unmeasured verdicts; bands widen by both the measurement uncertainty
  and the claim's own MC σ; an unmeasured receipt never passes and a
  measurement matching no claim is an error. Registration in
  `crates/vcad-receipt` + MCP tools = flagged follow-up PR (cross-crate
  schema + TS codegen), same staging as the particle family.
- **M5 — benchmarks + convergence + paper draft. DONE**
  (`tests/benchmarks.rs`, `examples/convergence.rs`,
  `docs/neutronics-paper-draft.md`). Published-value scorecard: water
  thermal diffusion length **2.14 cm vs 2.85 published** (−25%, band
  edge — the thermal-group simplifications err toward shorter thermal
  migration, stated); Fermi age **33.1 cm² vs ≈27** (+23%);
  collisions-to-thermal **18.3 vs 19.5** predicted by moderation theory
  *from the same library constants* (−6%). Convergence example:
  uncollided MC within 0.1–0.5% of exact at 1–5 mfp; RSE·√N flat over
  64× histories. External cross-validation (OpenMC + evaluated data)
  flagged as external, like the particle crate's SIMION runs.
- **M6 — the application. DONE** (`examples/phase_b_shield.rs`,
  `docs/neutronics-phase-b-dose-plan.md`). The shielded-grid
  experiment's Phase B dose plan: 5×10⁶ n/s (record-scale, an order
  above the predicted floor — conservative), operator at 2 m, stated
  design budget 2.5 µSv/h (≈1 mSv per 400 run-hours; a design choice,
  not a regulatory determination). **Answer: 15 cm HDPE** — 1.62 ±
  0.3% µSv/h at the operator, pass = mean + 2σ under budget; compass
  gradient −0.037 µSv/h/mm at the design point. Whole pipeline
  dogfooded: ShieldSpec (M3) → MC (M0/M1) → claims JSON (M4) →
  adjoint gradient (M2). Section-ready insert for
  `docs/shielded-grid-experiment.md` (that doc lives on the particle
  branch; fold in when both land). NAA feasibility answered honestly:
  3.3×10³ n/cm²/s thermal at an in-shield sample — foil calibration
  yes, trace-element assay no (1.5–3.5 orders short).

## Non-goals

No fission, no multiplication, no criticality — see Scope and refusals.
No claim of evaluated-data accuracy: this crate buys *honest error bars
on design estimates*, and every claim says which physics is missing.
