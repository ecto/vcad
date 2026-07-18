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

## 2026-07-17 — self-consistency verdict: 46× deflates to ~15×, record intact

`self_consistent_probe` at the ceiling winner (100 kV + 584 kA·t, 4 mTorr):

| current | vacuum (linear) | self-consistent | correction |
|---|---|---|---|
| 8 mA | 5.8×10⁷ (11.6×) | **5.2×10⁷ (10.5×)** | 0.90 |
| 30 mA | 2.2×10⁸ (43×) | **7.4×10⁷ (15×)** | **0.34** |

- The space-charge gauge was right: two-thirds of the linear 30 mA claim
  was fiction. The gauge-valid 8 mA regime survives nearly intact.
- Self-limiting observed: the self-consistent ratio settles ~0.23 (naive
  gauge said 0.36) — the beam partially disperses its own charge.
- **Both runs `converged: false`** (dRho 0.4–0.7 at 5 iterations,
  relax 0.5): ensemble noise dominates the update. Treat as unsettled;
  refinement path = more particles, relax ~0.25, trailing-average dwell.
  Receipts must not carry these as settled numbers yet.
- Program impact: records lane unaffected (10–15× margin even taxed);
  Q-lane multipliers must be priced self-consistently from now on.

## 2026-07-17 — settled verdict + two-species neutralization (updates prior entry)

Re-ran the self-consistency probe with the damped-update convergence
metric and settled parameters (128 particles, relax 0.25, 8 iterations) —
supersedes the noisy numbers in the previous verdict entry:

| current | vacuum | self-consistent | correction | state |
|---|---|---|---|---|
| 8 mA | 5.3×10⁷ | **4.5×10⁷ (9.0×)** | 0.85 | stationary (ratio flat 0.051→0.055) |
| 30 mA | 2.0×10⁸ | **8.4×10⁷ (16.7×)** | 0.42 | still creeping (ratio 0.192→0.217) |

- Metric lesson: max-norm node-density deltas floor at ensemble shot
  noise (~0.1 at N=128) even when every physical observable is flat —
  `converged` should track observables (ratio, passes), not raw ρ.
  Flagged for the next loop revision; until then the flag stays
  conservative (false) by design.
- Record margin after the audit: **9× at 8 mA, ~15–17× at 30 mA** —
  the record attempt survives self-consistency with room.
- **`space_charge::neutralized` landed** (`e0db4a69`): the
  perfect-injection electron-cloud bound — ion loop → electrons traced in
  applied+beam fields → net density → ions re-traced. Test-verified: the
  electron cloud reduces the net beam potential and moves confinement
  back toward vacuum. Explicitly an upper bound: injector efficiency,
  cusp electron losses, and e-thermalization (the polywell's demons) are
  unmodeled and every claim built on it must say so.

## 2026-07-17 — e-injection sweep: a receipted dead end, and the current sweet spot

`neutralization_sweep` at the ceiling config (100 kV + 584 kA·t, e-current
= ion current, perfect injection):

| mA | ion ratio | recovery | self-consistent yield |
|---|---|---|---|
| 30 | 0.215 | 0.06 | 8.6×10⁷ (**17.1× record**) |
| 60 | 0.429 | −0.00 | 4.1×10⁷ (8.3×) |
| 100 | 0.621 | 0.00 | 4.9×10⁶ (**1.0×**) |

- **Negative result, receipted**: naive electron injection neutralizes
  nothing at any current (neutralization fraction ≈ 0). The −100 kV well
  that confines ions is a potential *maximum* for electrons — they exit
  through the grid gaps with ~100 keV of gain in one transit. Electron
  confinement requires the magnetic architecture (Orbitron crossed-field /
  polywell cusp trapping); it is a machine design, not a retrofit. The
  model re-derived in an afternoon what the polywell program learned in
  hardware.
- **The current sweet spot**: past ~30 mA the space-charge tax outruns the
  linear gain — absolute yield *falls* with current (60 mA: −47%; 100 mA:
  −94%, landing at exactly 1.0× the record). This machine's operating
  point is ~30 mA.
- **The sturdy number**: this architecture's honest ceiling has survived
  four audits: 19× (linear) → 12× (gauge) → 16.7× (settled) → **17.1×**
  (with the useless cloud). Records lane unaffected; Q-lane requires the
  architecture pivot, which the kernel can now price (electrons ARE
  magnetized near the cusp wires — trapping studies are simulatable next).
- `observably_converged` flag worked as designed: true/true/false across
  the sweep, honestly marking the 100 mA row unsettled.

## 2026-07-17 — electron trapping is real: the "dead end" was a launch-location artifact

Corrects the e-injection entry above. `orbitron_probe` traces electrons in
the two-ring device's own cusp field vs the demagnetized copy (same
electrostatics, currents zeroed), launched at the **mid-shell** (r ≈ 0.5,
near the rings) rather than the 0.2 inner shell the neutralization sweep
used:

| shield A·t | enhancement | survivor frac | gyroradius (mm) |
|---|---|---|---|
| 0 | 1.0 | 0.00 | ∞ |
| 80 k | 121× | **0.75** | 0.38 |
| 200 k | 133× | 0.83 | 0.15 |
| 400 k–1.2 M | 140× (budget-capped) | **0.875** | 0.08→0.03 |

- **The cusp traps electrons hard**: at ≥80 kA·t, 75–87% never leave (vs
  100% expelled in 1.2 ns with B off). Enhancement saturates at the flight
  budget — 140× is a *floor*. Gyroradius collapses to 0.03 mm ≪ device:
  deeply magnetized.
- **Why the e-injection sweep read ~0**: it launched electrons at r=0.2,
  deep on-axis where the opposed coils cancel (B≈0) and E drives them
  straight out the axial point cusp before they can magnetize. Launch
  *near the rings* (strong B) and they trap. Injection location is
  everything — the exact polywell lesson, now measured in our geometry.
- This reopens the neutralization lane. Open question the
  `virtual_cathode` probe answers next: do the trapped electrons form a
  central *virtual cathode* (deepen the on-axis ion well = help fusion) or
  circulate peripherally at the rings (trapped but useless)?
- Caveat: non-relativistic Boris (ratio cancels most of the ~10–15% error
  at these energies); enhancement budget-capped; single particles.

## 2026-07-17 — virtual-cathode verdict: trapping is real, but charge-limited

`virtual_cathode` sweeps the electron launch radius at 30 mA ions + 30 mA
electrons, 100 kV, reporting the electron contribution to the **on-axis
core** potential (the well deepening that actually helps fusion, vs
`net_ratio` which a peripheral pile-up can move):

| e-shell | survivor | core ΔV | ion-yield gain |
|---|---|---|---|
| 0.20 | 0.71 | −0.3 kV | 0.93 |
| 0.40 | 0.94 | −0.3 kV | 0.92 |
| 0.55 | 0.88 | −0.2 kV | 0.93 |
| 0.70 | 0.80 | −0.2 kV | 1.00 |

- **Trapping ≠ neutralization.** Electrons trap 71–94% at every launch
  radius, but the core well deepens only 0.2–0.3 kV — **0.2–0.3% of the
  100 kV well** — and ion yield doesn't recover (gain ≈ 0.9–1.0, noise
  around unity). Launch radius is *not* the lever: the core effect is flat
  across it.
- **The lever is electron charge.** 30 mA of electrons, however well
  confined, is ~0.3 kV of central potential (order-of-magnitude check:
  Q_e = I·τ ≈ 0.03 A × 150 ns ≈ 4.5 nC → φ ~ Q/4πε₀R ≈ 0.8 kV at 5 cm,
  matching the measured few-tenths kV). A meaningful virtual cathode needs
  far more electron current — quantified by `virtual_cathode_current`
  (next entry).
- Honest program status: the trapped cloud is a *real but shallow* virtual
  cathode at matched current. The neutralization lane isn't dead, but its
  cost is electron current (with a real power price), not geometry — the
  opposite of what the "launch location" reopening suggested. The receipt
  keeps changing its own mind, on evidence.

## 2026-07-17 — the virtual cathode ignites (with two asterisks)

`virtual_cathode_current`: 30 mA ions at 100 kV, electrons at shell 0.55,
sweeping electron current:

| e-current | e/i | core ΔV | ion-yield gain | vs record |
|---|---|---|---|---|
| 30 mA | 1× | −0.2 kV | 0.99 | 16.3× |
| 300 mA | 10× | −2.5 kV | 1.06 | 17.4× |
| 1 A | 33× | −8.2 kV | 1.17 | 19.2× |
| 3 A | 100× | −24.7 kV | 1.98 | 32.5× |
| 10 A | 333× | **−82.3 kV** | **8.40** | **138×** |

- **Mechanism demonstrated end-to-end in our geometry**: cusp-trapped
  electrons form a central virtual cathode that deepens the fusing core's
  well (−82 kV at 10 A — nearly doubling the effective well) and
  multiplies ion yield 8.4×. The required e/i ratio (~100–300×) matches
  polywell practice (electron currents orders above ion currents).
- **Asterisk 1 — e-e self-repulsion unmodeled**: at 10 A the electron
  cloud's own charge is ~300× the ion cloud's; the perfect-injection
  bound ignores its self-force, which is dominant there. The 138× row is
  a mechanism demonstration, not a design claim. Next module: run the
  electron cloud through the same PIC self-consistency the ions got.
- **Asterisk 2 — sustaining power**: the circulating current is not the
  injector current; steady state replaces only the cusp losses
  (survivor fractions 80–94% are budget-capped floors). Power = loss
  rate × injection energy (~|φ(shell)| per electron) — needs the
  loss-rate measurement before any Q claim.
- Arc summary: dead end (launch artifact) → trapping real (140×) →
  trapping ≠ neutralization (charge-limited) → **neutralization real at
  polywell-scale electron currents**. Four self-corrections, all
  receipted. This is the Q-lane's road: e-cloud self-consistency, loss
  accounting, then the neutralized machine priced honestly.

## 2026-07-17 — optics recon: does lens design earn the ladder? Verdict: GO

Phase-0 scout for `vcad-kernel-optics` (sequential ray-tracing lens
design), testing the precondition triad that launched particle/em/
thermal/photonics:

1. **Crusty incumbent** — yes, the crustiest yet. Zemax/OpticStudio
   (acquired by Ansys, 2021) and Code V (Synopsys): closed, five-figure
   annual licenses, 1960s–80s cores, and the standard optimizer (damped
   least squares over FD derivatives) is non-differentiable end-to-end.
   The optics community's grumbling is public and perennial.
2. **Cheap analytic ground truth** — the *best* of any domain so far:
   sequential ray tracing IS the physics. No PDE, no grid, no
   discretization debate — each surface intersection is a closed-form
   quadratic (conics are quadrics) and vector-Snell refraction is exact
   in f64. The validation ladder writes itself, all with citations:
   thin-lens lensmaker's equation, thick-lens EFL/BFD closed forms
   (Hecht §6.1), the paraxial y-u trace + Lagrange invariant, the
   third-order Seidel spherical-aberration U-curve of a thin lens
   (Jenkins & White §9.5: best-form q = 2(n²−1)/(n+2) ≈ 0.714 at
   n = 1.5), the achromat condition φ₁/φ = V₁/(V₁−V₂) (Dollond, 1758),
   chromatic focal shift f/V from the Abbe number, and Schott Sellmeier
   dispersion data. A **published prescription** for the bench:
   Thorlabs AC254-075-A cemented doublet (R 46.5 / −33.9 / −95.5 mm,
   tc 7.0 / 2.5 mm, N-BK7/SF5, EFL 74.9 mm; [3DOptix catalog mirror of
   Thorlabs data](https://www.3doptix.com/catalog/optics/lens/thorlabs/AC254-075-A),
   fetched 2026-07-17) — the paraxial EFL of that prescription is a
   falsifiable claim against a $80 part anyone can buy.
3. **Differentiability + receipts story** — every operation in the
   Snell chain is smooth (intersection root, refraction, transfer):
   the adjoint is *easier* than particle's (no Dirichlet mask, no
   discrete grid). Prior art proves the gradient path works — dO
   (Wang et al., IEEE Trans. Comput. Imaging 2022), DeepLens curriculum
   design (Yang et al.), Mitsuba 3 — but none is receipts-native or
   lives next to the BRep that will mount the lens. RMS spot, EFL, BFD
   are bench-measurable (beam profiler, focimeter) → `basis: predicted`
   claims with a real measurement path.

Boundaries stated up front: **geometric optics only** — no diffraction,
no physical optics; RMS spot is a geometric claim and every receipt must
carry the Airy radius (1.22·λ·N) next to it so a sub-diffraction spot
number can't overreach. Distinct domain from `vcad-kernel-photonics`
(wave optics / FDTD — features ≈ λ); this crate is the features ≫ λ
regime. Tolerancing hooks belong to `vcad-kernel-tolerance`, later.

Transferable scar tissue encoded from day one: scale-invariant optimizer
stopping (the 1e-32 lesson), deterministic pupil ray sets frozen across
FD probes (the freeze-the-discretization lesson — here the image plane
follows the *paraxial* BFD, a smooth function of parameters, never a
re-gridded one), fail-closed ray fates (TIR and surface-miss are
reported outcomes, never dropped rays), and unfiltered exit-code gates.
Proceeding to M0.

## 2026-07-17 — vcad-kernel-optics M0: the optimizer rediscovers 1758

Same-day follow-through on the GO verdict above; details in
[optics-m0.md](optics-m0.md). New crate: exact sequential conic tracing
(closed-form quadric intersection, no iteration anywhere), vector Snell
with a per-ray |n·sinθ| invariant residual (~1e-16), fail-closed ray
fates, Sellmeier glasses gated by catalog n_d/V_d tests, independent
paraxial y-u + matrix traces, equal-area pupil lattice with
⟨ρ²⟩ = R²/2 exact, `vcad.optics-claims/1` receipts (Predicted →
Provisional). 40 tests, clippy/fmt clean.

Ladder results (all in `cargo test -p vcad-kernel-optics`):

- Thick-lens EFL/BFD closed forms (Hecht §6.1) to 1e-9; exact-trace
  h→0 limit = paraxial focus to 1e-6 mm with h² convergence.
- **Thorlabs AC254-075-A traces to its catalog EFL 74.9 mm** and shows
  <0.2 mm F→C shift — a falsifiable claim against an $80 part.
- **Seidel U-curve**: exact LSA vs Jenkins & White §9.5 within 8%
  across q ∈ [−2,2] (sub-1% over most of the range; e.g. q=−2:
  0.8284 vs 0.8203 mm); traced best-form minimum at the textbook
  q = 2(n²−1)/(n+2) ≈ 0.714.
- BK7 chromatic shift = f/V within 2%.

Flagship (`achromat_design`, multi-start FD at f/5, EFL pinned 100 mm,
poly F/d/C RMS objective): singlet optimizer independently finds the
best-form bending (R 60.2/−357.4, q ≈ 0.71); the BK7/F2 doublet lands
at R 48.2/−41.1/−347.8 with **8.14 µm poly RMS vs the singlet's
79.8 µm (9.8×)** and chromatic shift 1.534 → 0.050 mm (31×). The
optimized power split **φ₁/φ = 2.329 vs Dollond's V₁/(V₁−V₂) = 2.308
(0.9%)** — the 1758 achromat condition emerging from raw ray tracing
with no chromatic theory in the objective. Airy at f/5 is 3.58 µm:
the doublet is ~2.3× diffraction, and the receipt says "geometric"
next to every spot number.

Two build lessons, both caught by the ladder: (1) the first pupil
lattice (hexapolar, rim ring at R) had ⟨ρ²⟩ = 0.54R² — the
uniform-disk test caught the 8% rim bias, replaced by equal-area rings
exact by construction; (2) the defocus similar-triangles check
initially failed at 2.2% — the residual was *real physics* (the
third-order marginal-focus shift, 0.03·h² mm), reproduced by the
Seidel formula; the test now runs at f/100 where the closed form is
clean, and the effect is documented rather than tolerated away.

Next rungs queued: M1 aspheres + pupil imaging, M2 adjoint through the
Snell chain (every op smooth — easier than particle's), M3 wavefront/
Zernike, M4 tolerance seam, M5 MCP + lens-solid BRep.
## 2026-07-17 — differentiable circuit simulation: extend vcad-ecad-sim, not a new crate

Recon for a "differentiable SPICE" M0. Phase-0 verdict: **extend
`vcad-ecad-sim::circuit`**, no `vcad-kernel-spice` crate. Reasons, so a
changed mind stays distinguishable from a forgotten one:

- The circuit module is already a real MNA core, not a narrow SI tool:
  node+branch modified nodal analysis, backward-Euler companion models for
  C/L, Newton–Raphson with SPICE's `pnjlim` junction limiting for the
  Shockley diode, dense partial-pivot LU (`circuit/linalg.rs`), and even an
  electromechanical motor branch. ~1,100 lines of exactly the substrate M0
  needs.
- It has live consumers: `vcad-kernel-wasm::circuit_sim` exposes it to the
  app as a steppable JS class. A parallel crate would duplicate the stamps
  and orphan that wiring; extending means the app inherits DC operating
  point, trapezoidal accuracy, and adjoints for free.
- Prior art, honestly: ngspice exists and is good. The gap is NOT raw
  simulation quality — it is (a) exact adjoint sensitivities
  d(output)/d(every component), (b) fail-closed receipts, (c) agent-native
  workspace integration (netlist-from-ecad seam, MCP). All three land as
  additive modules inside `circuit/`; none require re-deriving what
  Nagel/Pederson wrote down in 1973 (SPICE2 memo, UCB ERL-M520, 1975).

M0 scope on top of the existing core: `dc.rs` (operating point, gmin
stepping), trapezoidal integration option (BE stays default for existing
consumers; motor stays BE), `ac.rs` (complex MNA, hand-rolled (re,im)),
`adjoint.rs` (transposed-system sensitivities, FD-validated per element
kind), Tellegen power-balance gates, `receipt.rs`
(`vcad.spice-claims/1`), `examples/filter_autotune.rs`. Transient adjoint
and MOSFET level-1 deferred to M1. `vcad-kernel-tolerance` flagged as the
natural partner (adjoint sensitivities × tolerance stackup = component
tolerance yield by gradient).

## 2026-07-17 — circuit M0 lands: MNA gets a DC solve, trapezoidal accuracy, and an adjoint

Extends `vcad_ecad_sim::circuit` per the decision above (branch
`claude/kernel-spice-m0`; details in `docs/spice-m0.md`). Results table,
all gates in CI:

| gate | oracle | result |
|---|---|---|
| divider | Ohm's law | exact (1e-13) |
| RC step, trapezoidal | V(1−e^{−t/RC}) | max err 2.5e-6 V of 5 V at dt = τ/1000; dt-halving error ratios 3.97 → 3.99 → 3.99 → 4.00 (2nd order clean) |
| RC step, backward Euler | same | ratios ≈ 2.0 — honestly 1st order, both integrators bracket the claim |
| RLC ringdown | ω_d, α closed forms | < 1e-3 / < 2e-2 rel |
| diode + R | Lambert-W (Corless 1996) | 1e-9 rel |
| Tellegen | Σv·i = 0 | < 1e-9 rel, every timestep, both integrators |
| adjoint vs central FD | frozen network | < 1e-5 every element kind (DC + AC); < 1e-4 through the diode Newton system |

- **The adjoint pays immediately**: `filter_autotune` drives a detuned RLC
  (15.9 kHz, Q = 0.5) to the 10 kHz / Q = 1/√2 Butterworth target —
  J: 1.16e-1 → 5.8e-17 in 104 gradient iterations, each costing one
  forward + one transposed complex solve per probe frequency. Final f0 =
  10000.0 Hz, Q = 0.7071 (< 0.1% error on both).
- **Bug the physics caught**: first trapezoidal run showed error 2.5e-3 =
  V·dt/2τ exactly — first-order-sized, not second. Cause: the t = 0 source
  discontinuity hands the trap companion a wrong initial history current
  (i₀ = 0, truth V/R). Fix is the standard SPICE startup rule — first step
  backward Euler — after which the 4.00 ratios appeared. The
  convergence-order gate, not eyeballing, caught it.
- **Test artifact worth remembering**: FD-validating d|H|/dL at exact
  resonance compares two zeros (it's a stationary point) and the FD term
  is pure truncation noise; probe off-resonance.
- `vcad.spice-claims/1` claims (predicted → Provisional) ride the unified
  receipt; closing instruments: $30 USB scope + signal generator.
## 2026-07-17 — Phase 0 recon: air-side acoustics is a GO (a new domain)

Scouting a new domain for the sim→measurement loop the workspace has run five
times (fusion/IEC, thermal, EM, photonics, neutronics). Triad confirmed for
**air-side acoustics** (cavities, ports, horns, rooms):

1. **Real domain, bad tooling**: loudspeaker enclosure / Helmholtz-resonator
   design. Incumbents are 1990s freeware ([Hornresp](https://www.hornresp.net/),
   BassBox) or five-figure FEA — a scatter of one-off calculators in between.
2. **Analytic ground truth**: rigid-cylinder axial modes `fₙ = n·c/2L`
   (exact), Helmholtz/bass-reflex tuning `f = (c/2π)√(S/(V·L_eff))` with
   end corrections (Beranek; Kinsler & Frey §10.5), baffled-piston on-axis
   Rayleigh closed form (Kinsler & Frey §7.4).
3. **$20 instrument**: a calibrated measurement microphone + swept sine — the
   exact loop the glockenspiel closed (`simulate_strike`, verified to −5 cents).

**Non-overlap with `simulate_strike` is clean.** `simulate_strike`
(`packages/mcp/src/tools/acoustics.ts`) is a *structural* 1-D Euler–Bernoulli /
Hermite beam FEM **in TypeScript** — how a solid bar bends. The new crate is the
*air-side* Helmholtz field in Rust — how the air resonates and radiates. They
meet at one BC (structural mode shape → surface velocity → Neumann datum for the
air solve); coupling is M2. Verdict: **GO**.

## 2026-07-17 — vcad-kernel-acoustics M0: the field solver reproduces the closed forms

New crate: axisymmetric Helmholtz field solve (vertex-centred finite volume,
direct block-Thomas — the operator is indefinite, so SOR would diverge), lumped
duct/cavity/Helmholtz oracles, baffled-piston radiation (Rayleigh + closed
form), port-sizing optimizer, `vcad.acoustics-claims/1`. Validation ladder
(`cargo test -p vcad-kernel-acoustics`, `docs/acoustics-m0.md`):

| check | result | oracle |
|---|---|---|
| closed cylinder axial mode 1 | **0.10%** err | `f₁ = c/2L` |
| closed cylinder axial mode 2 | **0.04%** err | `f₂ = 2c/2L` |
| grid convergence (dz 17→8.5→4.25 mm) | 0.104% → 0.025% → **0.005%** (2nd order, floor named) | — |
| reciprocity (source↔receiver) | **4.5×10⁻¹⁶** | symmetric FV |
| Rayleigh integrator vs on-axis | < 2% | piston closed form |
| Rayleigh directivity null | at `ka·sinθ = 3.8317` | first zero of `J₁` |

- **The finite-volume assembly pays off exactly as designed**: reciprocity to
  machine epsilon (4.5e-16) is the receipt that the operator is symmetric, and
  second-order convergence to a 0.005% floor confirms the discretisation is
  consistent. The floor was sweep-resolution, not grid — named and measured.
- **Flagship `examples/ported_box.rs`**: a 9.4 L bass-reflex box. Lumped `f_b`
  band 61.9/63.0/66.3 Hz at a 120 mm port; the field sweep reads **72.4 Hz**
  (port-velocity peak). The FD optimizer then sizes the port **120 → 339 mm**
  to hit a 45 Hz target, retuning the field solve **72.4 → 45.0 Hz** (residual
  0.14 Hz) — the loop closed against the sim, not the formula.
- **Negative result, logged proudly**: the pressure-release mouth reads tuning
  **~15% high** (resonator +18% on nominal, +11% over the interior-only bound;
  ported box +15%). It omits the exterior radiation mass and under-resolves the
  interior junction mass at M0 neck resolution. Not a bug — a known BC gap a
  radiation-impedance mouth closes (M1). The lumped band ships next to every
  field tuning so the gap is never hidden, and `distance`-style honesty is in
  the claim notes: lossless ⇒ Q is an upper bound, stated on every claim.
- Same pattern as the five prior domains: analytic ladder with citations,
  conservative/symmetric discretisation, fail-closed `predicted` claims that
  roll up Provisional (never Pass) until a mic measures them.

## 2026-07-17 — orbit recon: does astrodynamics earn the ladder? (verdict: yes)

Scouted the triad for `vcad-kernel-orbit`:

1. **Ground truth, free and always on**: analytic ladder (vis-viva,
   Kepler, J2 secular rates — Vallado 4th ed. Eqs. 9-38/9-39) *plus* the
   real sky. Fetched during recon and checked in as fixtures: a live ISS
   TLE (Celestrak, epoch 2026-198.573 — same day) and 72 h of JPL
   Horizons geocentric ICRF state vectors at 5-min steps
   (`crates/vcad-kernel-orbit/tests/fixtures/`). Tests never touch the
   network; the provenance headers are checked in raw.
2. **Prior art**: crates.io `sgp4` 2.4.0 (~272k downloads) — SGP4 is a
   solved problem and we will not reimplement it. Our lane: exact + J2
   propagation with receipts, headed differentiable (station-keeping ΔV
   adjoints), co-designed with the antenna/thermal/neutronics crates
   (receipted smallsat).
3. **Incumbents**: STK closed/expensive; GMAT non-differentiable, not
   agent-native. The crusty-incumbent + free-ground-truth + falsifiable-
   claims pattern holds — this is the only domain where the measured
   side of the receipt costs zero hardware.

Recon correction, logged with pride: the mission brief's "sun-synchronous
≈ 97.8° at 700 km" is folklore drift — Eq. 9-38 gives **98.19° at
700 km** (97.79° belongs to 600 km). The test asserts both values.

## 2026-07-17 — vcad-kernel-orbit M0: J2 tracks the real ISS to 10 km/day

New crate (branch `claude/kernel-orbit-m0`), details in
[orbit-m0.md](orbit-m0.md). 38 tests; clippy/fmt clean.

- **The sky graded us**: real Horizons ISS state propagated two-body+J2
  for 72 h vs the checked-in ephemeris — position error 0.44 km @ 1 h,
  **9.77 km @ 24 h**, 39.6 km @ 72 h. Two-body-only: 487 km @ 24 h; the
  J2 term buys **50×** against reality. The residual ~10 km/day is the
  honest M0 model gap (drag, above all) and is now a CI regression gate
  at measured × ~2.5 (2/8/25 km @ 1/6/24 h).
- **Headline validation**: least-squares nodal drift of the RK4+J2
  propagator over 10 orbits matches Vallado Eq. 9-38 to <1% at 51.63°,
  30°, 98.2°; conservation conscience: energy + h to 1e-9 over 10 orbits
  (two-body), J2-energy + h_z under J2.
- **Frame honesty made visible**: the fixture's ICRF inclination differs
  from the TLE's true-of-date 51.6316° by ~0.07° — 26 years of precession
  showing up in a unit test, tolerated and commented rather than hidden.
- **First measured-basis claim family**: `vcad.orbit-claims/1` ships with
  `orbit.position_error_km_at_24h` = 9.8 km vs a 25 km budget, `basis:
  Measured` (real sky data), Pass/Fail with no third outcome; predicted
  claims (period, dΩ/dt = −4.936 °/day for the ISS, passes) roll up
  Provisional as always.
- Flagship `examples/iss_pass.rs` also predicts 4 SF passes in the next
  24 h (max el 75.0° at 08:03 UTC), stated honest to ±minutes.
- M1 queued: drag + SGP4-compat (or a seam to the `sgp4` crate) +
  TEME↔ICRF, then the differentiable propagator (ΔV optimization).

## 2026-07-17 — circuit M1.1: transistors, and the Tellegen gate learns three terminals

Level-1 MOSFET (Shichman–Hodges, SPICE2 UCB ERL-M520 §2) and Ebers–Moll
BJT (transport form) land in `vcad-ecad-sim::circuit`, both polarities via
the sign transformation, wired through all four analyses (transient, DC,
AC, adjoint) plus the WASM `DeviceSpec`. Branch
`claude/circuit-m1-transistors`.

| rung | oracle | result |
|---|---|---|
| MOSFET saturation | (kp/2)·vov²·(1+λ·vds) square law | < 1e-9 rel |
| common-source gain | −gm·(Rd ∥ ro) at the op point | < 1e-9 rel, phase exactly real |
| CMOS inverter transfer | rail-to-rail, monotone, VDD/2 switch point | passes through the gmin ladder |
| BJT current mirror | I_out/I_ref = 1/(1 + 2/βF) | < 5e-3 abs |
| DC adjoint vs FD | central differences: kp, vt0, Is, βF + all linear slots | < 1e-4 rel |
| AC diode chain term vs FD | d\|H\|/dR and d\|H\|/dIs through the op-point shift | < 1e-4 rel |
| Tellegen with transistors | Σ (all-terminal) v·i, 2000 steps × 2 integrators | < 1e-9 rel |

- **The 3-terminal seam was the real work, not the I–V curves.** `Device`
  was structurally two-terminal: `terminals() → (p, n)` and power =
  `(v_p − v_n)·i` everywhere. A MOSFET survives that fiction (the gate
  draws nothing) but a BJT does not — its base current carries real power,
  and the Tellegen gate caught the miscount immediately. Fix:
  `Device::power()` sums over *all* terminals; `terminals()` documents its
  meaning as the current-carrying pair (drain/source, collector/emitter).
- **One eval, four consumers**: `MosfetModel::eval` returns
  (ids, gm, gds, ∂ids/∂kp, ∂ids/∂vt0) in external convention with polarity
  and the vds < 0 source-drain swap folded in; `BjtModel::eval` likewise
  returns the three branch currents + four conductances. Transient, DC,
  AC, and the adjoint all read the same numbers — no per-analysis model
  drift possible.
- **M0's flagged gap is closed**: the AC diode sensitivity slot was an
  honest placeholder; it now carries the full chain term
  dH/dp = ∂H/∂p + Σ (∂H/∂g_d)·(dg_d/dv_d)·(dv_d/dp), with ∂H/∂g_d from
  the AC adjoint and dv_d/dp from one DC adjoint solve per diode node.
  `deferred` now lists exactly the transistors (their version needs model
  second derivatives — deferred honestly, same pattern).
- Newton hygiene: BJT junctions get `pnjlim` on both vbe and vbc (in the
  internal N-frame so PNP limits correctly); FET voltages get a ±2 V step
  clamp (the square law can't explode, but undamped steps oscillate across
  the triode/saturation boundary).
- Next on the M1 ladder: transient adjoint, then the netlist-from-ecad
  seam so schematics simulate without re-entry.

## 2026-07-17 — the Q-lane triple: records 197×, POPS quantified, honest Q (PR #575)

Three lanes built on one branch, following the virtual-cathode arc.

**Lane C — TiD beam-target (records lane).** `beam_target` calibrates
thick-target D-D yield once to the published Ti drive-in anchor (1.9×10⁸
n/s at 7.6 mA / 94 keV) and predicts by cross-section ratio. `records_dial`
result at 100 kV / 30 mA onto TiD-coated rings:

| shield A·t | interception | beam-target n/s | gas n/s | total | vs record |
|---|---|---|---|---|---|
| 0 | 1.00 | 8.9×10⁸ | 2.2×10⁷ | 9.2×10⁸ | 183× |
| 160 k | 0.92 | 8.2×10⁸ | 1.6×10⁸ | **9.8×10⁸** | **197×** |
| 400 k | 0.70 | 6.2×10⁸ | 2.0×10⁸ | 8.3×10⁸ | 166× |
| 1.17 M | 0.08 | 7.5×10⁷ | 2.1×10⁸ | 2.8×10⁸ | 57× |

- **The dial has a combined-channel optimum**: total yield peaks at a
  *middle* shield current (160 kA·t → 197× the amateur record), where
  residual interception still feeds the solid-density TiD target while the
  shield boosts gas recirculation. No other machine has this knob.
- Beam-target Q ≈ 1.6×10⁻⁷ — correctly tiny (stopping-power-capped). This
  is a record/neutron-source lane, never a gain lane, and the module says
  so.

**Lane B — POPS harmonic well (physics lane).** `pops` measures the well's
harmonicity (min/max bounce frequency across launch amplitudes). Baseline
two-ring well: **harmonicity 0.545, 44% frequency droop** (big-amplitude
ions bounce 44% slower — strongly anharmonic, so a single drive can't
phase-lock the population → POPS-blocked). `pops_harmonic_well`: the
optimizer maximizes harmonicity over electrode geometry (+9% with two
knobs, 0.545→0.593); large gains need more electrode freedom. The finding:
harmonicity is measurable, optimizable, and is now the concrete FoM for
POPS electrode design — the thing only a differentiable field-solver can
target.

**Lane A — power ledger (honest Q).** `power` prices fusion vs ion beam +
electron sustain + magnet. Key physics: in steady state electron sustain
power = I_e · V (the injector replaces every cusp loss). `sustained_q`
verdict logged next.

## 2026-07-17 — the honest Q verdict: neutralization is a rate lever, not a Q lever

`sustained_q` at 100 kV / 30 mA ions; electron confinement τ = 0.88 µs
(enhancement **704×**, 83% survivors — strong cusp trapping confirmed at
full field). Ledger vs electron current:

| e-current | neutron n/s | vs record | P_fusion | P_e-sustain | Q |
|---|---|---|---|---|---|
| 30 mA | 8.4×10⁷ | 17× | 9.8×10⁻⁵ W | 3 kW | **1.6×10⁻⁸** |
| 1 A | 9.4×10⁷ | 19× | 1.1×10⁻⁴ W | 100 kW | 1.1×10⁻⁹ |
| 3 A | 1.7×10⁸ | 33× | 2.0×10⁻⁴ W | 300 kW | 6.4×10⁻¹⁰ |
| 10 A | 7.0×10⁸ | **140×** | 8.2×10⁻⁴ W | 1 MW | 8.2×10⁻¹⁰ |

- **The decisive result**: the virtual cathode raises neutron RATE up to
  140× the record, but **Q falls ~20× as the cloud grows** — Q is *best*
  at the smallest electron current. Electron sustain power (I_e · V)
  dominates the input the moment I_e exceeds the ion current, and it grows
  faster than fusion power. And the magnet term is still unpriced (MA·turn
  scale — needs the em+thermal crates), so these Q's are over-estimates.
- **Strategic redirect**: neutralization belongs to the records and
  physics lanes (rate, density, the polywell story), **not** the Q lane.
  The Q lane's real lever is **direct energy recovery on the losses**
  (venetian-blind, 59–75% demonstrated) — recovering the ion/electron
  power that currently exits to the walls. The ledger is now built to
  price exactly that: add a recovery-efficiency term on the loss channels.
- Cross-check: beam-target Q (~1.6×10⁻⁷) is the *highest* Q of any config
  we've priced — an order above the best neutralized-gas Q — consistent
  with "solid density beats everything on yield-per-input."

Arc close: the Q-lane triple priced all three roads honestly. Records:
197× (dial optimum), 140× (virtual cathode), 179× (bare TiD). Physics:
harmonicity 0.545, optimizable. Q: no lever here moves it — direct
recovery is the next module, and the ledger is ready for it.

## 2026-07-17 — circuit M1: netlist-from-ecad seam (Rust-side, decided)

Where should schematic→Circuit conversion live — Rust or TypeScript?
**Rust** (`vcad_ecad_sim::circuit::netlist`), decided after recon, for
three reasons that could each have gone the other way:

1. The schematic model's source of truth is Rust: `create_schematic`
   deserializes into `vcad_ir::ecad::SchematicSheet`, and net extraction
   (union-find over wires/labels/junctions + the explicit `nets` map)
   already exists as `vcad_ecad_schematic::generate_netlist`. A TS-side
   converter would have to re-derive connectivity the ERC path already
   owns.
2. The consumer is Rust: `Circuit`, `dc::operating_point`, `ac::ac_response`
   and the adjoint all live in `vcad-ecad-sim`. Converting where the
   consumer lives keeps the seam one hop wide and WASM inherits it for
   free (same argument that put the M0 module in `vcad-ecad-sim` rather
   than a new crate).
3. No `simulate_circuit` MCP tool has landed yet (checked: nothing in
   `packages/mcp` or the WASM bindings), so there was no TS chip to
   coordinate with — the scope's "ship standalone with a clean public
   API" branch applied. When that tool lands, `{document_id}` →
   `circuit_from_schematic` is a thin wire.

Dependency direction checked before committing: `vcad-ecad-schematic`
depends only on `vcad-ir`, so `vcad-ecad-sim → vcad-ecad-schematic` adds
no cycle.

What shipped: refdes-prefix mapping (R/C/L/V/I/D) + SI value parser
(suffix, infix `4R7`/`4k7`, case-sensitive `m`/`M`, typed rejections),
ground-family nets → node 0, **fail-closed** per-component blocker list
(ICs/connectors refuse simulation; explicit `stub_as_open` allowlist
with typo rejection), round-trip test: `create_schematic`-shaped sheet
(divider + RC) → DC exact to 1e-12 vs Ohm's law, AC corner |H| and phase
to 1e-12 vs the Thévenin closed form.

Scar earned: nodes must be allocated only for nets a mapped device
touches — a floating net (stubbed connector pins, netlist singletons)
that gets an MNA node makes the matrix singular. First cut allocated
every net and failed exactly that way in the stub test.

Honest gaps: no layout parasitics (that's M2 — trace R/L/C from the
routed board), distinct ground rails (AGND/DGND) collapse onto node 0 at
M1 (reported in `MappedCircuit::ground_nets`), diode model is chosen by
value-string sniffing ("LED" → LED model, else silicon).
