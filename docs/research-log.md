# Research log

Append-only journal of the physics-design research program (fusion/IEC
first; other domains welcome). Rules:

- **Dated entries, newest last.** Never rewrite history — corrections are
  new entries that cite the entry they correct.
- **Every quantitative claim carries its provenance**: a commit, an
  example invocation, or a literature citation. Numbers without receipts
  don't go in the log.
- **Decisions are logged with their reasons**, so future-us can tell a
  changed mind from a forgotten one.

---

## 2026-07-16 — vcad-kernel-particle M0: the shielded-grid effect, from first principles

New crate (commit `18064428`): axisymmetric Poisson (SOR), exact ring-coil
B (AGM elliptic integrals), Boris tracer, electrode figures of merit.
Findings from `fusor_baseline`:

- **Magnetically-shielded-grid effect reproduced** ([Hedditch/Bowden-Reid/
  Khachan 2015, arXiv:1510.01788](https://arxiv.org/abs/1510.01788)):
  cathode ring current cuts wire interception 1.00 → 0.07 by 160 kA·turns
  (−3 kV). Runs in CI as an integration test.
- **r_L ∝ √V law falls out**: the −30 kV interception curve is the −3 kV
  curve shifted ~3–4× right in current. Commission cold, then climb.
- **Recirculation is non-monotonic in shield current**: past the optimum
  the cusp reflects ions off the core (magnetic aperture). There is an
  optimal shield — a real objective for gradient design.

Context that started this: the amateur DIY record is ~5×10⁶ n/s
([fusor.net fusioneer list](https://fusor.net/board/viewtopic.php?t=13));
the only Guinness record in the space is *youngest* (Jackson Oswalt, 12,
[verified via fusor.net](https://www.guinnessworldrecords.com/world-records/596440-youngest-person-to-achieve-nuclear-fusion));
nobody tracks integrated energy — no amateur has banked 1 J of fusion
("Joule One" ≈ 8.5×10¹¹ D-D reactions).

## 2026-07-17 — M1–M6 in one arc; two bugs the physics caught

Commits `7aea1cd7`…`de6c9278` (PR #553), details in
[particle-optics-m0.md](particle-optics-m0.md):

- Bosch–Hale D-D yield along trajectories → predicted neutron rates.
  Classic fusor floor: **1.9×10⁵ n/s at 30 kV / 10 mA / 2 mTorr** — ~25×
  under the record with the conservative channel only (correct: real
  fusors add fast-neutral chains).
- Bug 1: E-sampling wasn't the gradient of the interpolated potential —
  energy drift was structural. Fix: conservative bilinear-patch gradient.
- Bug 2: optimizer's absolute gradient epsilon read 1e-32-scale objectives
  as converged. Fix: scale-invariant stopping. Both regression-tested.
- **Yield landscape is multimodal** (recirculation hill ~26 kA·t vs
  energy-quality hill ~165 kA·t): single-start ascent stalls at 3× lower
  yield; multi-start required. Review (PR #553) found a latent OOB in the
  bilinear clamp at grids ≳4100 — fixed index-space in 4 sites
  (`ef879d31`).
- Discrete adjoint (FD-validated 0.1–0.8%) + DeviceSpec seam + receipt
  claims (`vcad.particle-claims/1`, Q, distance-to-Lawson) + analytic
  benchmarks (mirror loss cone, well-curvature period) + experiment pack.

## 2026-07-17 — the five-domain receipt and its three vetoes (PR #561, #564)

`fusor_codesign`: one machine priced by five crates (particle, em,
thermal, neutronics, tolerance) into one Provisional DesignReceipt, with
real cross-domain wires (interception × beam power = thermal load;
predicted n/s = neutronics source; coil force = mount load). First run
vetoed the design three ways — cathode at ~23× copper melt, 14.9 kN
coil repulsion, worst-case ring gap −0.30 mm at RSS yield 1.0. Rev-b
turned each veto into a priced resolution (2% duty → 527 °C at 2.1×
margin; explicit mount-load claim; physics-justified ±1.5 mm gap → WC
+0.20 mm). Verdict stays Provisional by design.

## 2026-07-17 — record hunt and ceiling hunt: 46× headline, 12× honest

`record_hunt` + `ceiling_hunt` (+ new `space_charge` module), PR #564:

- At 30 mA / 4 mTorr: **75 kV + 380 kA·t → 9.6×10⁷ n/s (19× record)**;
  escalation finds the 75 kV turnover at ~760 kA·t; **100 kV → 2.28×10⁸
  = 46×** at 1.17 MA·t; 45 mm rings near-optimal; thermal veto dissolves
  at max shield (interception 7.8% → 234 W of a 3 kW beam).
- **Space-charge gauge** (dwell-deposited beam density → grounded-
  conductor Poisson): ratio φ_beam/well = **0.363 at 30 mA** → linearity
  valid only to ~8 mA → **gauge-valid ceiling 6.1×10⁷ = 12×**; the 46×
  requires self-consistent treatment. Emergent: the gauge binds hardest
  at the best-confined configs — the classic IEC confinement/space-charge
  dilemma, rediscovered by the model auditing itself.
- Sanity anchor: an unshielded fusor at 60 kV / 30 mA prices at 1.2× the
  record — the regime real record-holders occupy.

## 2026-07-17 — step-back synthesis: three lanes (decision)

Literature refresh against our own findings:

- **Neutralization is the published frontier**: the [2026 Orbitron
  power-balance paper](https://iopscience.iop.org/article/10.1088/1361-6587/ae3ad8)
  reports combined e⁻/ion injection *temporarily exceeding the
  space-charge limit*; power balance Q≈0.6 Coulomb-limited, Q≈0.13 when
  cyclotron radiation dominates. Electron-injection IEC neutralization is
  also [patented (US 11901086)](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/11901086).
- **POPS**: [10⁴ core density multiplication in 1D PIC](https://www.osti.gov/biblio/20736607)
  ([approach paper, arXiv:1307.0151](https://arxiv.org/pdf/1307.0151)) —
  requires a *harmonic* well (electrode inverse design: our optimizer's
  home turf), electron injection, and RF phase-locking.
- **Beam-target is the record shortcut**: [Ti drive-in D-D generator:
  1.9×10⁸ n/s from 7.6 mA at 94 keV](https://www.sciencedirect.com/science/article/abs/pii/S0168583X08000621)
  — solid-density TiD beats our optimized gas machine on bench hardware.
  Our "interception loss" onto TiD-coated rings becomes a *yield channel*,
  and the shield current becomes a dial between beam-target and
  recirculating-gas regimes. Q-capped ~10⁻⁴ by stopping power (records
  lane only).
- **Direct energy recovery**: [venetian-blind converters demonstrated
  59–65%, projected ~75%](http://www.ralphmoir.com/wp-content/uploads/2012/10/venBlnd.pdf)
  ([IOP](https://iopscience.iop.org/article/10.1088/0029-5515/13/1/005)),
  works down to ~10 keV — an amateur-buildable Q multiplier (×2.5–4).

**Decisions:**
1. Split the program into three lanes: **records** (TiD cathode +
   interception dial; months), **physics** (shielded grid → e-injection →
   POPS; the papers), **Q** (neutralize + recover + compress; target an
   *amateur Q record* ~10⁻³, not Q>1).
2. **Amateur Q>1: no credible path** — our gauge + Rider + the 2026 power
   balance agree. Say so everywhere; the honesty is the brand.
3. Next kernel spine: time-dependent drives, electron species, PIC-lite
   self-consistency — the modules lanes 2 and 3 stand on.

## 2026-07-17 — the spine: RF drive, electrons, PIC-lite self-consistency

Kernel modules the three-lane plan stands on (PR #564):

- **`Drive`**: sinusoidal modulation of all electrode potentials —
  exact by superposition (wall at 0 V), no re-solve. Zero-depth drive is
  bit-identical to static (tested); a 25% drive at the measured bounce
  frequency measurably moves yield (tested on the two-ring axial
  oscillator — single fusor trajectories defocus into wires and make bad
  probes; lesson kept as a comment).
- **`ELECTRON`** species: sign physics verified (expelled from the
  negative well, zero core passes, zero D-D yield).
- **`space_charge::self_consistent`**: deposit → grounded source solve →
  re-trace with under-relaxed density updates. Benign at 0.1 mA
  (near-vacuum stats), bites at 500 mA (ratio > 0.2, confinement moves) —
  both regression-tested. Censoring under-weights long-lived charge:
  converged densities are floors.

Verdict probe on the ceiling-hunt winner (100 kV + 584 kA·t, 8 vs 30 mA)
running; result to be logged when in.
