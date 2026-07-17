# An open, receipt-native Monte Carlo neutron transport code for benign shielding design

*Draft skeleton (M5). Numbers regenerate from `cargo test -p
vcad-kernel-neutronics` and the `fusor_shield` / `convergence` examples;
every figure cites its seed.*

## Abstract

Shielding design for small D-D neutron sources (fusors, sealed-tube
generators, activation-analysis benches) is textbook health physics, yet
the incumbent transport code (MCNP lineage) is export-controlled and
procedurally inaccessible to the hobbyists and small labs doing that
work. We present `vcad-kernel-neutronics`, an open multigroup Monte
Carlo transport code embedded in a parametric CAD kernel, scoped
deliberately to fixed-source moderation/shielding/dosimetry — no
fission, no multiplication, no criticality, refused permanently. Three
design commitments distinguish it: (1) **statistical honesty is
type-enforced** — every tally is a mean ± relative standard error from
batch statistics, a bare number is unrepresentable, and a zero-scored
tally reports infinite RSE rather than zero; (2) **provenance rides the
result** — seed, histories, batches, group structure, collision-physics
model and library version travel inside the same JSON object as every
dose claim, with Holds/Violated/Unmeasured verdicts when bench
measurements bind to predictions; (3) **the design loop is
differentiable** — a deterministic multigroup adjoint diffusion
companion prices d(dose)/d(layer thickness) in one extra solve,
FD-validated to 0.1%, with its bias against the Monte Carlo oracle
measured (×1.54 absolute at 1 m through 12 cm HDPE) rather than
assumed away.

## 1. Scope and refusals

Moderation, shielding, ambient dose. The material library is a
**design-estimate library** (±20–30% group constants read from
evaluated-data plots with per-value citations), not an evaluated
nuclear data file, and the code says so on every claim. Fission physics
is refused permanently; users needing k_eff need a regulated code and
an institutional review chain, not a CAD kernel.

## 2. Method

- 5 groups, 2.45 MeV → thermal (boundaries 3 MeV/1 MeV/100 keV/1 keV/
  0.5 eV/0.1 meV); source group anchored on the D-D line.
- Analog fixed-source MC in 1D spherical/slab layered geometries;
  exact analog balance (absorbed + leaked = 1 per batch, asserted at
  1e-12); collision-cap truncations counted, and claims **refuse**
  unbalanced runs.
- Collision physics (M1): exact two-body elastic kinematics — collision
  nuclide sampled from its Σ_s share, outgoing energy and lab angle
  correlated through the same isotropic-CM cosine. (The originally
  planned P1 bias is not even a valid pdf for hydrogen, μ̄ = 2/3.)
  Multigroup-isotropic mode retained as the diffusion mirror.
- Tallies: track-length flux per region/group; ICRP-74-style H*(10)
  factors (1/E-weighted per group, weighting stated); 20-batch
  statistics; xoshiro256++ with per-batch streams, bit-identical
  reruns.
- Adjoint companion (M2): cell-centered FV multigroup diffusion,
  forward/adjoint duality at machine precision; interface
  shape-derivative gradients (the flux form −[[1/D]]·J·J† — the naive
  δD·∇φ·∇φ† misprices shield/air interfaces by ×43, a bug the FD
  ladder caught).

## 3. Validation ladder (all in CI)

| rung | truth | result |
|---|---|---|
| uncollided point flux | e^{−Σr}/4πr² exact | 0.1–0.5% at 1–5 mfp (seed 20260717) |
| slab transmission | e^{−Σx} exact | within 4σ at 3 mfp |
| 1/√N | batch statistics | RSE·√N flat over 64× histories |
| buildup | physics inequality | water at 3.3 mfp: total ≫ uncollided |
| one-group triangle | MC ↔ diffusion ↔ e^{−r/L}/4πDr | all three agree ≤10%/3% |
| adjoint | forward/adjoint duality | gap ≤ 1e-15 |
| gradient | central FD | 0.1% (diffusion), 25% band vs MC FD |

## 4. Benchmarks against published values

Seeds in `tests/benchmarks.rs`; bands are stated and wide because a
5-group design-estimate library is *supposed* to sit near these values,
not on them.

| quantity | this code | published | dev |
|---|---:|---:|---:|
| water thermal diffusion length | 2.14 cm | 2.85 cm (Lamarsh T5-2) | −25% |
| water Fermi age to thermal | 33.1 cm² | ≈27 cm² (Lamarsh T5-3) | +23% |
| collisions to thermalize (water) | 18.3 | 19.5 (ln(E₀/E_th)/ξ̄ from the same library) | −6% |

The thermal-L deficit is the known signature of the thermal-group
simplifications (single thermal group, free-gas motion neglected, bound
σ_s as one calibrated constant); it errs toward *shorter* thermal
migration, i.e. conservative for shield-exit thermal flux.

## 5. Headline result

Isotropic 2.45 MeV point source, dose at 1 m (µSv/h per 10⁶ n/s, 10⁶
histories, seed 20260717): bare 12.11 (analytic anchor 12.03); 5 cm
HDPE 7.03; 10 cm 3.10; 20 cm 0.47. Exact kinematics vs
isotropic-multigroup: ×1.4/×1.9/×3.1 at 5/10/20 cm — the angle–energy
correlation of hydrogen scattering is a 3× effect at 20 cm and the
reason collision physics is provenance, not preference. Borated (5%)
vs plain poly at equal thickness: identical fast dose, >2× lower
thermal column — boron is a capture-gamma/activation tool, not a fast-
dose tool, and the gamma budget is explicitly out of scope (stated
caveat on every claim).

## 6. Limitations (each stated on results)

No photon transport (capture gammas flagged, never silently dropped);
free-atom scattering with one calibrated bound-H thermal constant;
isotropic-in-CM angles (MeV p-wave forward hardening absent); free
field (no room return); 1D geometries; design-estimate constants.
Cross-validation against an independent open transport code (OpenMC)
requires evaluated data files and is external — flagged, not run.

## 7. Application

The shielded-grid IEC experiment's Phase B dose plan
(`docs/shielded-grid-experiment.md`): operator at 2 m from a 5×10⁶ n/s
D-D source, dose budget stated in the experiment doc, shield sweep with
error bars and a receipt (`vcad.neutronics-claims/1`) whose verdicts
bind to survey-meter measurements. See the experiment pack section
written by M6.
