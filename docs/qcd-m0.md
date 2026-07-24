# Lattice gauge theory M0: SU(2) pure-gauge Monte Carlo

`vcad-kernel-qcd` brings the vcad solver zoo to the strong interaction: a
laptop-scale lattice gauge theory kernel that computes **confinement from
first principles** — the plaquette, Wilson loops, the area law, and (via
Creutz ratios) the string tension that makes pulling two color charges
apart cost linear energy. It follows the same discipline as every other
solver crate in the repo: deterministic runs, an error bar on every
number, fail-closed claims, and honesty bounds stated on results rather
than in a README three repos away.

Why it belongs in vcad at all: unlike thermal/EM/tolerance, nothing here
feeds back into a manufacturable part. It is a **credibility and
visualization flagship** — the M2 flux-tube seam ("drag two quarks apart
in the viewport and watch the chromoelectric tube stretch") is a demo no
CAD tool and few physics tools have, and the claim machinery shows the
receipt system carrying genuinely hard statistics (Monte Carlo with
autocorrelations, not just FV residuals).

## What M0 ships

Crate `crates/vcad-kernel-qcd`, dependency-free except `serde`:

- **`su2`** — SU(2) in the quaternion parameterization `U = a₀ + i a·σ`.
  No complex matrices exist anywhere; unitarity is a normalization.
- **`lattice`** — link variables on a periodic 4D hypercubic lattice
  (flat `Vec`, `4·site + μ` indexing), staple sums, average plaquette,
  planar Wilson loops `W(r,t)` averaged over all sites and planes.
- **`update`** — Kennedy–Pendleton heatbath (exact local conditional,
  rejection-sampled) + microcanonical overrelaxation `U → Ā†U†Ā†`,
  interleaved. Deterministic per seed (bundled xoshiro256++, same recipe
  as the neutronics crate).
- **`stats`** — binned jackknife. `Estimate {mean, err, n_bins,
  bin_size}` is the only way an observable leaves the crate.
- **`spec`** — `SimSpec` → `run()` → `SimResult`, serde end to end (the
  future `simulate_lattice_gauge` MCP seam). Fail-closed validation: no
  thermalization, statistics too starved for ≥ 2 jackknife bins,
  degenerate extents (< 2), or Wilson loops big enough to wrap the
  lattice are all rejected before a single sweep runs.
- **`receipt`** — `vcad.qcd-claims/1`: plaquette, Wilson-loop, and
  Creutz-ratio claims. Fail-closed: < 5 jackknife bins mints nothing; a
  degenerate error bar mints nothing; Creutz ratios are only emitted
  when all four constituent loops are ≥ 3σ from zero (the log of a
  statistically-zero number is not a measurement). Every claim carries
  the caveat list in the same JSON object. Claims are `basis: predicted`
  and cap at **Provisional** — registration in `crates/vcad-receipt` +
  the MCP surface is the flagged follow-up, same staging as the particle
  and neutronics families.

## Validation oracles (in CI)

- Strong coupling: ⟨P⟩ = β/4 − β³/96 + O(β⁵) at β = 0.75 on 6⁴.
- Weak coupling: ⟨P⟩ = 1 − 3/(4β) + O(1/β²) at β = 8 on 6⁴.
- ⟨P⟩(β) strictly monotone across β ∈ {0.5, 1.5, 2.5, 4.0}.
- Hot start and cold start thermalize to the same ⟨P⟩ (β = 2).
- `W(1,1) ≡ ⟨P⟩` exactly; area-law ordering `W(1,1) > W(1,2) > W(2,2) > 0`
  in the confined phase.
- Overrelaxation preserves the local action to 1e−10; the
  Kennedy–Pendleton sampler matches direct quadrature of its target
  density; staple/plaquette consistency (`Σ Re Tr(U·A) = 4·Σ Re Tr U_p`);
  jackknife matches the naive error on iid data and scales as 1/√N.

All tests are seeded and deterministic; the full suite runs in ~4 s in
debug.

## Honesty bounds (M0, stated on every claim)

- Quenched SU(2) pure gauge — no dynamical fermions, not SU(3). Nothing
  here is a number about physical QCD; the claims are about the lattice
  model and say so.
- Lattice units at fixed coupling: no continuum extrapolation, no scale
  setting.
- Finite volume, no infinite-volume extrapolation.
- Jackknife errors correct autocorrelation only up to the bin size.

## The ladder

- **M0 (this)** — SU(2) heatbath+OR, plaquette + Wilson loops, jackknife,
  claims. ✅
- **M1** — string tension: Creutz-ratio scaling study vs β against the
  literature (Creutz 1980 SU(2) numbers), smearing for signal, static
  quark potential V(r) fit (Cornell form). Claim: σa²(β).
- **M2** — the visualization seam: action-density and Polyakov-loop
  fields exported per configuration (time-sliced 3D scalar fields), the
  flux-tube-between-static-charges demo, gradient flow / cooling for
  topological charge. This is the viewport flagship.
- **M3** — SU(3) links (the real gauge group; heatbath via Cabibbo–
  Marinari SU(2) subgroups), deconfinement transition via Polyakov-loop
  susceptibility at finite temperature.
- **M4+** (stretch, unpromised) — GPU sweeps via `vcad-kernel-gpu`
  (checkerboard update parallelism), glueball 0⁺⁺ correlators, Wilson
  fermions on tiny lattices. Dynamical QCD at physical parameters is
  permanently out of scope — that is supercomputer territory and the
  docs say so rather than pretend otherwise.

## References

- M. Creutz, *Monte Carlo study of quantized SU(2) gauge theory*,
  Phys. Rev. D 21, 2308 (1980).
- A. D. Kennedy, B. J. Pendleton, *Improved heatbath method for Monte
  Carlo calculations in lattice gauge theories*, Phys. Lett. B 156, 393
  (1985).
- K. G. Wilson, *Confinement of quarks*, Phys. Rev. D 10, 2445 (1974).
