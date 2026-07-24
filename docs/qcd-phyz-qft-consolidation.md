# vcad-kernel-qcd × phyz-qft: consolidation assessment

**Question.** `phyz-qft` (in the phyz workspace) also implements lattice gauge
theory — U(1)/SU(2)/SU(3), Wilson action, HMC, plaquette/Wilson/Polyakov
observables. Should `vcad-kernel-qcd` wrap it the way `vcad-kernel-physics`
wraps `phyz`, keeping only the receipt family, analysis, and MCP seam on the
vcad side?

**Answer: not as it stands — and the write-up below is the evidence, per the
"don't force it" instruction.** The premise that phyz-qft is a strict superset
of vcad's sampler does not survive contact: it is a demo-grade scaffold whose
sampler is currently *non-functional*, and whose trait design structurally
cannot express the object (a staple **sum**) that every efficient pure-gauge
algorithm is built on. Wrapping it would replace a validated engine with one
that reproduces no known number. The honest consolidation path is the reverse
direction — port vcad's engine *into* phyz-qft — and that has real
infrastructure costs listed at the end.

Assessed 2026-07-24 against phyz @ `crates/phyz-qft` (1,411 LOC) and the
`claude/lattice-qcd-vcad-70be78` branch (PR #660, unmerged at time of
writing).

## Empirical disqualifier

A 5-minute oracle check (SU(2), 6⁴, β = 8, HMC with 12 leapfrog steps at
dt = 0.08, 200 trajectories — vanilla settings):

```
beta=8 plaquette=2.0000  expected ~0.9062 (weak coupling: 1 − 3/4β)
acceptance: 0/200
```

Zero trajectories accepted; the lattice never left the cold start, and the
"plaquette" it reports for the identity configuration is 2.0 — the
*unnormalized* Re Tr, not (1/N)Re Tr. phyz-qft's own 13 unit tests pass
because none of them compares an observable to a known value. (vcad-kernel-qcd
carries strong/weak-coupling expansions for both groups, hot/cold agreement,
the exact strong-coupling string tension, and both deconfinement β_c's as CI
oracles — that's the difference between a sampler and a demo of one.)

## Root causes (structural, not tuning)

1. **The staple "sum" is a product.** `Lattice::staple_sum` accumulates with
   `staple = staple.mul(&upper)` — and its `first` flag is `let first = true;`
   never flipped, so each upper staple *replaces* the accumulator and each
   lower one multiplies it. The Wilson force derived from this is not the
   Wilson force, hence ΔH blows up and Metropolis rejects everything.
2. **The `Group` trait cannot represent a staple sum at all.** A sum of group
   elements is not a group element (∉ SU(N)); it lives in the enveloping
   linear space. phyz-qft's trait has `mul/inv/re_tr` but no `zero/add/scale`,
   so the correct object is unrepresentable — the product-instead-of-sum bug
   is forced by the type design, not a typo. (vcad's `GaugeGroup` trait has
   `zero/add/scale` precisely for this; the same struct doubles as
   accumulator, which is what Kennedy–Pendleton and Cabibbo–Marinari consume.)
   U(1)'s staple-as-angle has the same disease.
3. **Global `rand::thread_rng()` everywhere** — `random()`,
   `sample_momentum()`, the Metropolis draw. No seed anywhere in the API.
   vcad's QCD contract is bit-reproducibility per seed: 56 deterministic
   tests, `SimSpec.seed` on the MCP surface, provenance embedded in
   `vcad.qcd-claims/1`. Also wasm-hostile (`rand`/`getrandom` js plumbing),
   where vcad-kernel-qcd is dependency-free by design and ships in the
   *default* kernel-wasm build.
4. **Observable gaps and bugs**: Wilson loops measured in a single (x,t)
   plane with a hardcoded `/3.0` normalization for every group (wrong for
   U(1) and SU(2)); no plane averaging, no spatial×temporal split, no
   smearing, no overrelaxation, no cooling, no topological charge, no field
   exports, no error bars of any kind. Everything the vcad surface exposes —
   Creutz ratios, static potential, Cornell fit, flux tube, jackknife —
   would remain vcad-side regardless.

## What consolidation would actually take (if still wanted)

Direction must be **vcad → phyz**, not the reverse: port
`su2/su3/lattice/update/smear/topology` (≈2,000 LOC, all deterministic and
oracle-tested) into phyz-qft, replacing its core:

- Extend/replace the `Group` trait with linear-space ops (`zero/add/scale`,
  `dagger`, `reunitarize`, `norm_trace`) — this is vcad's `GaugeGroup`.
- Thread a seeded RNG through `randomize`/HMC/heatbath (breaking API change;
  audit phyz consumers).
- Keep HMC (rebuilt on the corrected staple) *alongside* heatbath+OR — HMC is
  the only path to dynamical fermions later, which is a legitimate reason for
  phyz-qft to exist; heatbath+OR stays the pure-gauge workhorse.
- Add U(1) to vcad's trait (cheap; the compact-QED plaquette expansion is a
  free extra oracle).
- Parity: port verbatim (same RNG stream, same sweep order) so vcad's 56
  tests pass unchanged — the estimator-equivalence risk in the task prompt is
  avoidable by not changing the estimator.

Then `vcad-kernel-qcd` thins to spec/stats/analysis/receipt/fields-seam over
`phyz-qft` — but note the infrastructure cost: phyz becomes a **hard**
dependency of the default kernel-wasm build and the published MCP bundle
(today only `vcad-kernel-physics` depends on phyz, and it is feature-gated
`physics = [...]`, off the default path). CI, the npm publish pipeline, and
every fresh-checkout instruction inherit the `../phyz` sibling requirement.

## Recommendation

- **Now:** land PR #660 as-is. It is self-contained, validated, and carries
  the receipt/MCP surface either way. Nothing in it blocks a later engine
  swap behind the `spec::run()` seam.
- **phyz-qft, regardless of consolidation:** fix the staple accumulation
  (requires the trait change), normalize observables per group, add a seeded
  RNG, and add at least one coupling-expansion oracle test so the sampler
  can't silently regress to non-sampling again.
- **If one engine is wanted long-term:** do the vcad→phyz port above as its
  own project (phyz-side PR first, vcad wrapper PR second), and decide
  explicitly whether making phyz a hard dependency of the default vcad wasm
  build is acceptable — that's the real cost, and it's an infrastructure
  decision, not a physics one.
