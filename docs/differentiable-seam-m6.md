# Differentiable seam — M6 design note

M0–M5 (`differentiable-seam-m0-m2.md` … `-m5.md`) built the seam and both
its modes, but every milestone still **hand-writes its `ParamSeeding`**: the
author knows "this θ moves these twenty surfaces, each a radius rate 1 and a
center retreat `(±1, ±1, 0)`", transcribes it, and asserts the surface census
by hand (`seed_where` returns the count precisely so they can). That
transcription is the last piece of the derivative pipeline a human still does
by eye — and the one most likely to be silently wrong on a real part. M6
derives the seeding from the build function itself.

## The idea: finite-difference the *fields*, not the mesh

The tempting shortcut — finite-difference the mesh node positions and call
that dx/dθ — is wrong for the exact reason the seam exists. The tessellator's
discrete choices (fan centers, segment counts, cap dispatch) are functions of
θ and flip under perturbation, so node `i` at θ+h is not node `i` at θ−h.
Even *under a frozen plan* mesh-level FD only reproduces what the analytic
seam already computes, plus the plan's O(h) correspondence noise.

Surface **fields** have no such problem. A plane's offset, a cylinder's
radius and axis line, a sphere's center and radius are each analytic in θ
with **no combinatorial structure** — a central difference recovers them to
O(h²) with no branch to cross. So M6:

1. rebuilds at θ_k ± h,
2. geometrically matches each base surface to its counterpart in each
   perturbed build,
3. reads the **observable** per-surface seed components off the matched field
   deltas,
4. hands the resulting `ParamSeeding` to the existing seam.

The θ → field map is the *only* numerical part, and it is the one part with
no combinatorics to get wrong. Everything downstream — lift-bridge velocities,
implicit rows, tangency completion, the reverse-mode transpose — stays
analytic and exact in the fields.

`synthesize_seeding(build, theta, k, h)` returns the seeding for one
parameter; `synthesize_all(build, theta, h)` returns one per parameter from a
single base build.

## Why matching must be geometric, and by identity only

Two builds of the same model enumerate the geometry store in **different
order**: the boolean pipeline and the fillet kernel are both
order-nondeterministic. Empirically, two `fillet_all_edges` builds of the
same cube permute the store, flip blend-axis signs, rotate reference
directions, and slide cylinder centers along their axes by O(1) mm. So a base
surface cannot be found in a perturbed build by store index — it is matched by
**frame-invariant geometric identity** (`same_surface`):

- **plane** → normal up to sign (`|n·n'| > 1 − 1e-4`) and coincident plane
  (perpendicular distance `< MATCH_TOL`);
- **cylinder** → axis line up to sign (parallel axes, point-to-line distance
  `< MATCH_TOL`) and equal radius — the center is compared only in its
  perpendicular part, so a center slid along the axis does not matter;
- **sphere** → center and radius.

`MATCH_TOL = 1e-3` mm is the house convention: far above the O(h·velocity)
≈ 1e-6 mm motion of a derivative step, far below feature separation. Two
genuinely distinct surfaces therefore never both fall inside a base surface's
ball; a rebuilt copy of the *same* surface always does. If two distinct
candidates *do* both match (a violated feature-separation assumption), that is
a hard `AmbiguousMatch` error, never a guess. Multiple candidates that are
mutually identical to `SAME_EPS = 1e-6` are accepted as **duplicate store
copies** — the boolean carries several copies of a moving surface, and each
base copy is matched and seeded independently, which is exactly the invariant
the vertex solve needs (`seed_where`'s reason for existing).

## Observability: seed only what a field motion can be seen through

The extraction reads each delta **in the base surface's own frame** and keeps
only the components that are physically observable — the same gauge argument
that governs the frozen plan's frame transport:

- **Plane** — in-plane origin drift is unobservable (the fillet kernel moves
  the anchor around freely between builds). Only the normal component survives:
  `Translate { velocity = n · (ẋ_offset) · n }`, computed as
  `n · (o₊ − o₋) / 2h · n`, so any in-plane part of `o₊ − o₋` vanishes under
  the dot.
- **Cylinder** — the along-axis position of `center` is gauge. Project the
  central-difference center velocity onto the plane perpendicular to the
  **base** axis (`reject`), giving the radial `Translate`; add
  `CylinderRadius { rate }` from the radius delta. Because the projection
  uses the base axis and the form `(v·a)a`, an axis-sign flip in the
  perturbed build cannot corrupt it.
- **Sphere** — center and radius are fully observable: full `Translate`
  velocity plus a `SphereRadius { rate }`.

**Composite seeds fall out for free**: a fillet blend at varying radius yields
both a radius rate and a radial center velocity, and `ParamSeeding` composes,
so both are pushed onto the same surface. This is the M4 composite seeding —
now derived rather than transcribed.

Components below `ZERO_TOL = 1e-7` in every element are dropped, keeping the
seeding sparse and honest. The central-difference roundoff floor at `h = 1e-6`
on mm-scale geometry is ≈ ε·|field|/(2h) ≈ 1e-9; the threshold sits an order
or two above that noise and seven orders below the genuine O(1) seed
magnitudes, so it never drops a real seed nor keeps a spurious one.

## Topology changes are errors

A step that crosses a topology change has no meaningful derivative. M6 guards
it twice: a cheap `TopologySignature` comparison of each ± build against the
base (the same signature the frozen plan uses), and — as a backstop against a
signature collision — a hard error if any base surface has *no* perturbed
match. Both surface as `FrozenError::TopologyChanged`, mirroring the frozen
seam's own contract. Surface kinds outside the seed vocabulary
(NURBS/bilinear) are rejected with `UnsupportedSynthesis` rather than
silently skipped — the synthesis vocabulary is exactly the seed vocabulary,
so a kind with no seeds has no synthesizable motion regardless. (M7 landed
alongside this note: cone and torus joined both vocabularies, and the
`cone_and_torus_parameters_synthesized` gate covers the composition.)

## The base-instance contract

A synthesized seeding is keyed by store index into `build(theta)`, and the
stores are order-nondeterministic, so a seeding is only meaningful against a
base built **identically**. In the seam's optimizer loop this is automatic:
`build(theta)` is called once per iterate and both the frozen plan and the
seeding come from that one instance. The gate tests reproduce that discipline
by handing the synthesizer a closure that returns a clone of a canonical base
at the base θ (and rebuilds fresh for the perturbations, which are matched
geometrically). The hard part — matching nondeterministic *perturbed* builds
through axis flips, rotated frames, and slid centers — is exercised in full;
only the base-index plumbing is pinned.

## Gates (`m6_synthesized_seeding.rs`)

Three prior milestones reproduced with **zero hand seeding** — synthesized
seedings only — plus a negative test and a reverse-mode composition, measured:

| Gate | synthesized dV/dθ | vs closed form / hand | vs FD oracle |
|---|---|---|---|
| M2 boolean hole, dV/dr | −78.036 | 1.4e-10 (`−N·sin(2π/N)·r·t`) | 4.8e-10 |
| M3 cylinder height, dV/dh | 77.646 | 3.0e-10 (N-gon area) | 4.1e-9 |
| Rounded cube, dV/da | 293.665 | 7.5e-10 (hand `seeding_a`) | 7.5e-10 |
| Rounded cube, dV/dr | −72.807 | 1.8e-9 (hand `seeding_r`) | 2.0e-8 |

All inside the 1e-6 gates. The rounded cube is the hard case — 20
simultaneously moving surfaces, composite seeds, frame nondeterminism, and
tangency-completion rows — and the synthesized seedings match the M4/M5 hand
seedings to ≤1.8e-9 relative (the residual is the synthesis's O(h²) field
error, not a modelling gap). The **reverse-mode composition** gate contracts
one `evaluate_with_pullback` against the two synthesized seedings and
reproduces forward mode to 3.9e-16 (dV/da) and 2.0e-15 (dV/dr) — the
synthesized seedings are ordinary `ParamSeeding`s, so both modes consume them
identically. The **negative** gates confirm a probe that steps across a
topology change (a through-hole appearing at θ > θ₀) errors as
`TopologyChanged`, and an out-of-range parameter index errors as
`ParameterOutOfRange`, never a partial seeding.

## Notes and boundaries

- **Seed vocabulary = synthesis vocabulary.** Plane translate, cylinder
  translate + radius, sphere translate + radius, cone translate (full apex —
  no along-axis gauge) + half-angle, torus translate + both radii — the
  cotangent space exactly. A future kind extends `SurfaceSeed`,
  `SurfaceCotangent`, *and* `SurfInfo`/`extract_seeds` together; nothing else
  changes.
- **The base-index contract is real.** `synthesize_seeding` builds its own
  base to key against, so the returned seeding must be applied to a base built
  identically (a clone, or the same optimizer-iterate instance). A future API
  that hands the caller its base — or a base wrapper carrying a stable
  geometric key — would remove the footgun; it was out of the minimal-diff
  budget here and the optimizer loop already satisfies the contract naturally.
- **Stretch (document parameter gradient) — rejected as unreachable.** A
  Rust-side .vcad document → BRep evaluator is not reachable from the
  non-excluded crates: `vcad-kernel` does not depend on `vcad-ir`, and the
  IR's own path to geometry routes through `vcad-loon` (converting to loon
  source), which needs the excluded `loon` sibling. Document evaluation in the
  product is the TypeScript `packages/engine/src/evaluate.ts`. So a
  `document_parameter_gradient` helper would have to live above the excluded
  boundary; deferred to a session with `loon` in scope. The synthesis itself
  is already document-agnostic — it takes any `Fn(&[f64]) -> BRepSolid`, which
  a document evaluator would satisfy directly.
