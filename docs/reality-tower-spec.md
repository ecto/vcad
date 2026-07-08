# The Reality Tower — cross-scale representation spec

Status: strawman spec (v0). Owner: cam. 2026-07-08.

## Thesis

vcad's document format becomes an executable model of the effective-field-theory
tower — from fundamental constants up through electronic structure, atomistics,
continuum materials, parts, **manufacturing processes**, machines, and finally a
calibrated macroscopic world model. Every number carries provenance and
uncertainty; every seam between scales is differentiable where physics permits,
stochastic-with-error-bars where it is an ensemble average, and fail-closed
always.

The one-sentence claim the tower makes possible:

> *the derivative of a macroscopic performance metric with respect to a
> chemical or process decision, with an error bar and a receipt.*

## Structure: rungs and arrows

Rungs are **states**; arrows are **transformations**. There are two arrow kinds:

- **Scale seams** (vertical): ensemble averages / matching interfaces that
  compress a lower rung into a small typed API for the rung above
  (Hellmann-Feynman forces, reweighting, homogenize).
- **Processes** (horizontal, at the part rung): manufacturing operations that
  map *(as-designed geometry, stock material state, machine profile)* →
  *(as-built geometry, as-built material state, cost, time)*. Processes have
  cross-scale side effects — bending work-hardens, lasers leave a HAZ — so an
  as-built part's material state can differ **per region** from its stock card.

```
Planck constants (axiomatic root: G, ħ, c, α — data, never simulated)
  ↓ matching (data tables: CODATA, ENDF)
nuclear data → electronic structure        [vcad-kernel-electrons — NEW]
  ↓ Hellmann-Feynman / MLIP validation
interatomic potentials                     [vcad-kernel-atoms — exists]
  ↓ reweighting gradients + IFT            [RT2, RT3]
ensembles → MaterialCard v2                [homogenize — exists, upgraded]
  ↓ (exists)
as-designed parts / assemblies             [BRep kernel — exists]
  ══ process arrows ══►  as-built parts    [vcad-kernel-process — NEW;
      press brake · laser · mill · FDM      stocksim + predict_print retrofit]
  ↓ phyz / gym                             [exists]
machines → environments → world model      [RT8]
  ◄─ evidence flows DOWN (record_measurement → Bayesian update)  [RT7]
```

Key invariants:

1. **Lazy resolution.** Every material node has a `scale_floor` — the deepest
   rung currently grounding it (`assumed` … `standard-model`). Rungs resolve
   on demand: a query's precision requirement determines resolution depth.
   Nothing below the floor is ever computed speculatively.
2. **Decoupling is the API design.** Interfaces between rungs are the small
   typed structs physics mandates (nucleus → chemistry is exactly
   {mass, charge, spin, moment}). We transcribe, we don't invent.
3. **DFM is derived, not declared.** Handbook rule packs remain as the
   fallback tier; when a process simulator + MaterialCard can derive a rule
   (min bend radius from fracture strain), the derived value supersedes the
   handbook value and the receipt records both and their disagreement.
4. **The zoom renders the provenance chain.** At every LOD, the viewer draws
   the model that produced the numbers at that rung (continuum shading →
   instanced homogenization supercell → density isosurfaces → ray-marched ρ(r)).
   No art-direction crossfades.

## Core abstractions

### 1. `Quantity` + trust lattice (in `vcad-receipt`)

```rust
/// Ordered trust lattice; propagation takes the minimum over inputs.
enum Trust {
    Assumed,
    Handbook,
    Fitted { training_domain: DescriptorRange },
    DerivedStochastic { variance: f64, n_samples: u64, estimator: String },
    DerivedExact,
    Measured { measurement_id: MeasurementId },
}

struct Quantity {
    value: f64,
    unit: Unit,
    trust: Trust,
    /// Parents in the provenance DAG (other Quantities or receipt claims).
    parents: Vec<ProvenanceRef>,
}
```

- Propagation rule: a computed `Quantity`'s trust = min over parents (with
  variance propagation through the seam Jacobian for the stochastic tier).
- New receipt query: `weakest_link(claim) -> Vec<Quantity>` — the minimal-trust
  cut under a claim. This is where the next simulation/measurement dollar goes.
- `DesignReceipt` gains a `provenance` section; existing adapters unaffected
  (all their inputs default to `Handbook`/`Assumed`, which is the honest truth
  today and immediately makes the gap visible).

### 2. `Field3` — sparse chunked scalar/vector fields (`vcad-kernel-field`, NEW)

Universal currency for volumetric state: charge density, ESP, spin density,
plastic strain, residual stress, HAZ temperature history, stock SDF.

- Block-sparse (8³ or 16³ chunks) + empty-space skipping; cell + affine frame;
  f32 storage with f64 accessors.
- Ops: sample, trilinear gradient, marching-cubes isosurface → mesh (feeds the
  existing tessellation → GLB → viewer path), boolean-with-BRep clip, reduce.
- Unify with `vcad-kernel-stocksim`'s octree SDF where practical (stocksim
  becomes the first consumer, not a parallel implementation).
- Viewer: isosurface mode ships first; WebGPU ray-marched volume mode second
  (sibling of the existing ray-trace pipeline).

### 3. Rung traits

```rust
/// Electronic rung. Built-in: SCC tight-binding on tang-expr, Fermi smearing
/// (Mermin free energy) so occupations are smooth ⇒ ladder stays differentiable.
/// Gradients: Hellmann-Feynman for dE/dθ; implicit-function theorem through
/// the SCC fixed point (same math as the constraint-solver symbolic Jacobian).
trait ElectronicSolver {
    fn solve(&self, sys: &ElectronicSystem) -> Result<ElectronicState>;
    fn gradient(&self, state: &ElectronicState, wrt: &[ParamId]) -> Result<Vec<Quantity>>;
}
// ElectronicState: eigenvalue table + occupations + Field3 densities + Quantities
// (gap, dipole, per-atom charges). Adapters (xTB, PySCF) out-of-process,
// fail-closed, receipt-tagged `Fitted`/`DerivedExact` per method.

/// Manufacturing process arrow. THE new abstraction.
trait Process {
    fn simulate(
        &self,
        part: &AsDesigned,        // BRep + intent (bend lines, cut paths, toolpaths)
        stock: &MaterialState,    // MaterialCard v2 + optional per-region Field3s
        machine: &MachineProfile, // capability envelope + calibration posterior
    ) -> Result<ProcessOutcome>;
}

struct ProcessOutcome {
    as_built: AsBuilt,            // geometry after springback/kerf/deviation
    material_state: MaterialState,// per-region: plastic strain, hardening, HAZ (Field3)
    dfm: Vec<DerivedDfmFinding>,  // derived rules, each vs. its handbook counterpart
    cost_time: CostEstimate,
    receipt: ProcessReceipt,      // model tier used, inputs' trust, residuals
}
```

Process implementations, each with a **tiered model ladder** (analytic →
reduced-order → adapter to external FE), receipt records the tier:

- **PressBrake** — elastic-plastic beam bending per bend line: springback angle
  from elastic-core ratio (needs yield σ_y, hardening modulus from MaterialCard
  v2), bend allowance derived (vs. current K-factor table), min radius from
  fracture strain, work-hardening band written as a Field3. Supersedes
  `sheet_metal_bend_table` values when trust ≥ handbook.
- **LaserCut** — moving-line-heat-source thermal model (Rosenthal solution as
  tier 1): kerf width, HAZ extent (Field3 of peak temperature), max thickness
  and dross prediction from melt ejection threshold. Temperature-dependent
  properties requested from homogenize (which forces deeper resolution — the
  first *natural* demand-driven descent through the tower).
- **Mill** — retrofit `vcad-kernel-stocksim` behind the trait (it already is a
  process simulator: outcome = stock SDF + gouge/excess oracle).
- **Fdm** — retrofit `predict_print` (as-built deviation prediction) behind the
  trait; `record_measurement` becomes the generic calibration channel (see RT7).

```rust
/// Machine rung: what a specific machine can do and how well we know it.
struct MachineProfile {
    capabilities: CapabilityEnvelope,       // travels, tonnage, wattage, tooling
    calibration: BTreeMap<ParamId, Posterior>, // updated by record_measurement
}
```

### 4. IR changes (`vcad-ir`, ts-rs regenerated as usual)

- `ElectronicSystem { molecule_ref, charge, multiplicity, model, kpoints, observables }`
- Process ops as DAG nodes: `AsBuilt` nodes reference their `AsDesigned` parent
  + `ProcessSpec` + `MachineProfileRef`. Assemblies/phyz consume as-built nodes
  when present (the robot grips the real part, not the CAD ideal).
- `scale_floor` + provenance refs on material assignments.

### 5. MCP tools

| tool | rung | notes |
|---|---|---|
| `solve_electronic` | electronic | gap/dipole/charges + receipt |
| `render_density` / `render_orbital` | electronic | isosurface/ESP PNG via Field3 path |
| `simulate_process` | process | press_brake \| laser \| mill \| fdm; returns as-built diff + derived DFM |
| `weakest_link` | receipt | minimal-trust cut under a claim |
| `parameter_gradient` (extend) | all | cross-scale: allow `wrt` at any rung |

## Milestones

Ordered so the *statistical* seams (no prior art, highest risk) land before the
quantum rung (commodity eigensolvers), and the process layer lands early
because it pays rent immediately (better DFM) without waiting for the bottom
of the tower.

- **RT0 — Quantity + trust lattice.** In `vcad-receipt`. `weakest_link` query.
  Existing adapters tag inputs `Handbook`/`Assumed`. *Accept:* a current
  sheet-metal receipt answers "weakest link" with the K-factor table entry.
- **RT1 — `vcad-kernel-field` + isosurface viewer path.** Sparse Field3,
  marching cubes, GLB export; stocksim consumes it (or documents why not).
  *Accept:* a Gaussian density renders as an isosurface in the app viewer.
- **RT2 — Reweighting gradients in `vcad-kernel-atoms`.**
  d⟨A⟩/dθ = ⟨dA/dθ⟩ − β·cov(A, dU/dθ). *Accept:* d(lattice constant)/d(LJ ε)
  matches finite difference within estimator error, with reported variance.
- **RT3 — MaterialCard v2.** homogenize returns `Quantity`s (fluctuation
  formulas + IFT through the relaxed state); adds plasticity fields needed by
  RT4 (σ_y, hardening modulus, fracture strain — `Fitted`/`Handbook` tier
  initially, that's fine: the *slot* is what matters). *Accept:*
  d(MaterialCard.E)/d(potential param) with error bar.
- **RT4 — Process layer v1: PressBrake + LaserCut.** `Process` trait; stocksim
  + predict_print retrofit behind it; `simulate_process` MCP tool; derived-DFM
  findings with handbook disagreement reporting; as-built nodes in IR.
  *Accept:* springback prediction for 5052-H32 within tolerance of published
  bend tables **derived, not looked up**; HAZ Field3 renders in viewer.
- **RT5 — Tight-binding on tang-expr.** SCC-TB, Fermi smearing,
  Hellmann-Feynman + IFT gradients; `solve_electronic` + density rendering;
  WASM-able for small systems. *Accept:* fd-oracle validation (reuse
  `atoms::fd` pattern); ethylene HOMO renders; gap gradient vs FD.
- **RT6 — The flagship gradient.** One integration test:
  d(as-built bend angle)/d(potential parameter) — electrons→card→process, or
  minimally d(part mass)/d(lattice param). *Accept:* the number, its σ, its
  receipt chain terminating at the axiomatic-constants node. This test IS the
  landing-page sentence.
- **RT7 — Evidence flows down.** `record_measurement` generalized: measured
  bend angles update MachineProfile calibration posteriors AND MaterialCard
  posteriors (linearized Bayesian update through the process Jacobian).
  *Accept:* three logged coupon bends measurably tighten the next springback
  prediction's variance.
- **RT8 — World rung + the zoom.** As-built parts feed phyz gym (friction/
  inertia as Quantities); LOD zoom continuum → supercell instancing → density
  isosurface in one camera path. *Accept:* the Powers-of-Ten demo recording,
  every frame backed by resolved state.

## Cornerstone projects

Glockenspiel template: a physical artifact with a **brutal external oracle**
that is unbuildable without at least two new tower arrows. Each proves a
milestone band; each ends in a measurement, not a screenshot.

1. **Tuning fork** (proves RT0–RT4) — 440 Hz fork; pitch predicted from the
   homogenize-derived elastic tensor *with error bars*, geometry cut and bent
   (springback + work-hardened bend zone shift the modal frequency, and the
   model must include that). Oracle: a tuner app — does the measured pitch
   fall inside the predicted σ? First cornerstone; PR-sequenced like the
   glockenspiel.
2. **Bimetallic thermostat** (RT3/RT6) — two-material strip, curvature vs
   temperature from two MaterialCards (CTE is a pure fluctuation quantity —
   the honest RT3 stretch), laser-cut (HAZ arrow), geometry **optimized by the
   cross-scale gradient** d(deflection@60°C)/d(thickness ratio, composition).
   Oracle: heat gun + dial indicator. RT6's flagship gradient, embodied.
3. **Flexure gripper** (RT4/RT8) — compliant mechanism whose *as-built*
   hinge stiffness (work hardening / HAZ embrittlement) feeds phyz; a policy
   trained in the gym transfers to the physical gripper. Oracle: it picks up
   the egg the sim said it could. The sim-to-real claim: transfer *because*
   the world model's numbers carry receipts.
4. **Ferrite motor, round 2** (RT5 stretch) — the already-proven PCB motor,
   but stator μ/saturation/loss derived from spin-polarized electronic
   structure instead of a datasheet; d(torque)/d(dopant fraction) through
   `calc_motor`. Oracle: dyno. The twelve-orders-one-gradient claim on
   de-risked hardware.
5. **Calibration coupon ladder** (RT7, runs continuously under all of the
   above) — standard coupon set (bend coupons, kerf combs, print towers);
   each measurement Bayesian-updates MachineProfile + MaterialCard
   posteriors; downstream prediction σ visibly tightens. Oracle: prediction-
   interval coverage tests. The commercially real one: "your shop's machines
   become priors in the physics."
6. **Powers-of-ten film** (RT8, after 1–2 exist) — one continuous zoom from
   assembled artifact to charge density, every frame backed by resolved
   state, receipt chain as overlay. Oracle: none — it is the marketing
   artifact the others earn.

Sequence: tuning fork → thermostat; gripper + motor second wave; coupon
ladder underneath everything.

## Research appendix: math the tower will force

Not decoration — RT0, RT6, and requirement-lowering each quietly need an
answer, and whatever ad-hoc answer gets coded becomes de-facto new applied
math. Notice it and write it down (one `docs/research/` note each; papers if
they hold up).

- **Algebra of trust propagation.** Min-lattice + variance propagation over a
  provenance DAG whose nodes are *correlated estimators* (quantities derived
  from the same MD trajectory share noise) is a graphical model over physical
  quantities. Open question with product consequence: when does the
  weakest-link (min-trust cut) coincide with the max-variance-reduction
  measurement target? Characterizing the divergence changes what
  `weakest_link` should return.
- **Composable stochastic adjoints.** Chaining seams needs reverse-mode AD
  where some nodes are expectations over Gibbs measures. No composable
  calculus exists: no chain rule for the variance of a product of stochastic
  Jacobians, no checkpointing analog for ensemble seams. RT6 computes this
  composition numerically; the composition law itself is new math.
- **Renormalization as a type system.** Rungs as categories, scale seams as
  functors, matching coefficients as natural transformations; decoupling =
  the functor factors through a finite-dimensional interface. Written down
  properly, this *derives* the minimal type that must cross each seam
  (e.g. what MaterialCard's fields are forced to be) instead of guessing.
  vcad would be the first such formalization with a compiler keeping it
  honest.
- **Bidirectional set propagation across seams.** Requirement lowering =
  inverse-image propagation of spec intervals through chained stochastic
  nonlinear maps. Interval arithmetic compounds too loosely over 6 seams;
  full set propagation is intractable. Middle path (probabilistic zonotopes /
  conformal feasible sets with coverage guarantees) has no worked theory for
  chained physical models. This is the math under the "watch the dopant
  window tighten live" UI.
- **Sheaf-shaped cross-scale consistency** (speculative). Overlapping models
  agreeing on overlaps is verbatim a sheaf condition; a nonzero cocycle is
  "these rungs disagree and no reconciliation exists." Possibly the right
  formalism for what `verify_receipt` means when a claim spans rungs — and
  the precise statement of "the zoom is honest."

## Non-goals

- **Not a DFT code, not a general FEM package.** Built-in solvers are the
  always-available differentiable floor (TB, analytic process models);
  accuracy comes through adapters, trust comes from receipts and fd oracles.
- **Nothing below nuclear data tables is simulated.** The Planck rung is an
  axiomatic data node — the root of provenance, not a compute target.
- **No neural world model in-tree.** The world rung exposes the calibrated
  analytic model + gym; learned proposers live outside and get verified here.

## Risks

- Ensemble-gradient variance may be impractically large for stiff observables
  → RT2 must report variance honestly; shadowing methods are the fallback and
  a receipt can say "gradient not meaningful" (fail-closed applies to
  derivatives too).
- Process model tier 1 (analytic) accuracy vs. shop reality → RT7 exists
  precisely to absorb this; calibration posture beats model heroics.
- Scope gravity: every rung is a career. The trait seams + adapter posture are
  the containment strategy — vcad owns interfaces and verification, not
  solvers.
