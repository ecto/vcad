# w1-union: the stator's remaining boolean defect

**This is a diagnosis, not a fix.** No kernel source changed on this branch.
What landed is the measurement harness, three `#[ignore]`d reproducers that
fail today, and this document.

Exit criterion for `w1-union` was: *the stator solves to 7848 mm³ ± 0.5 % as a
B-rep (not soup), with no regression in the boolean/torture suites.* Measured
on `claude/cam-roadmap` (8daf2aa6), **the volume half is already met and the
validity half is not.**

## What the work package was briefed on vs. what is actually open

The package was written against the pre-#901 state. Three of the four items
have since landed; only the fourth is open.

| Briefed defect | Status on `claude/cam-roadmap` |
|---|---|
| (a) `post ∪ ring` ≠ `ring ∪ post` (+5.4 mm³, 30 open edges) | **Fixed** by #901 (`FaceClassification::OnSameInner`, `classify.rs:1111`). Both orders now give 4726.99 mm³ with the same 8 open edges — the asymmetry is gone. Covered by `coplanar_cap_union.rs::a_contained_coplanar_patch_does_not_double_the_larger_face`. |
| (b) many-lump operand ∪ ring loses the caps over the overlaps (1309 open edges, −0.7 %) | **Fixed** by #901 (`pipeline.rs::flush_wall_phantom_cut`, union-only). Covered by `twelve_filleted_posts_stay_analytic` and `twelve_filleted_posts_into_the_ring`. |
| (c) mesh fallback not bit-reproducible across processes | **Fixed on `main` by #903, absent from this base branch.** See "Determinism" below: still reproducible here, and the integrator should merge `main` into `claude/cam-roadmap` rather than have it re-fixed. |
| the stator falls to triangle soup | **No longer true.** The stator is `Analytic`, 501 faces, 12 186 triangles. |
| — | **OPEN: the result is not watertight.** 642 unpaired and 95 over-used edges. |

`Analytic` is not a validity signal and neither is a right volume; this defect
is invisible to both, which is why it survived #901.

## Baseline

`claude/cam-roadmap` @ 8daf2aa6, debug profile (workspace `opt-level = 2`),
machine carrying seven other builds:

| | value |
|---|---|
| volume (`Solid::volume`) | 7852.96 / 7854.46 mm³ (bimodal, see Determinism) — +0.06 % / +0.08 % |
| volume (divergence integral over the 256-segment tessellation) | 7869.62 mm³, +0.28 % |
| fidelity | `Analytic`, `is_triangle_soup == false` |
| faces / triangles | 501 / 12 186 |
| **unpaired (open) edges** | **642** |
| **over-used edges (> 2 triangles)** | **95** |
| solve | 22.1–22.5 s |

Reproduce with `crates/vcad-eval/tests/stator_measure.rs`:

```bash
VCAD_STATOR_LOON=…/rana/cad/parts-60-cnc/stator.loon VCAD_CACHE_DIR=$(mktemp -d) \
  cargo test -p vcad-eval --test stator_measure -- --ignored --nocapture
```

Every one of the 642 unpaired edges lies **exactly in one of the two cap
planes** (z = 11.1 or z = 17.1), in two families:

* **F1 — post-root fillets, at r ≈ 24.** Four unpaired edges per cap plane per
  fillet block, at the point where the block's fillet arc touches the bore.
* **F2 — OD tab groups, at r ≈ 29.5…31.1, 40°…50°.** The larger family, and
  the source of most of the over-used edges: the tab's round-end cap disc ends
  up covered twice.

Both reduce to the same cause.

## The smallest reproducer

`crates/vcad-kernel-booleans/tests/tangent_fillet_rim_seam.rs` — two operands:

```
ring  = cyl(r 28.75, 256 seg) − cyl(r 24, 256 seg),   z 11.1 … 17.1
block = cube(1.35 × 1.5124) − cyl(r 1.05, 32 seg),    z 11.1 … 17.1
ring ∪ block → 4726.99 mm³ (closed form 4726.99), 8 unpaired, 9 over-used
```

Identical in both operand orders. The control in the same file passes: pull
the block 0.01 mm off the bore and the shell closes with zero unpaired edges.
So **tangency is the trigger**, and the window is about ±0.01 mm wide:

| block shifted radially | volume | open | over-used | fidelity |
|---|---|---|---|---|
| −0.20 mm | 4728.65 | 0 | 0 | Analytic |
| −0.05 | 4727.46 | 0 | 0 | Analytic |
| −0.01 | 4727.08 | 0 | 0 | Analytic |
| −0.002 | 4727.01 | 14 | 10 | Analytic |
| **0 (as designed)** | **4726.99** | **8** | **9** | Analytic |
| +0.002 | 4726.99 | 10 | 2 | Analytic |
| +0.01 | **4734.78** (+0.16 %, wrong) | 6 | 0 | Analytic |
| +0.05 | 4726.60 | 0 | 4 | mesh referee overruled |
| +0.20 | 4724.90 | 222 | 135 | Analytic |

(The +0.01 row is a second, separate bug: a *silently wrong volume* that no
gate catches. It is not on the stator's path — the stator sits at exactly
0 — but it is worth its own issue.)

The tangency is exact by construction, not by accident: the fillet cutter is
r 1.05 centred 22.95 mm from the axis, and 22.95 = 24 − 1.05, so the arc is
internally tangent to the bore. The same is true at the tab roots against the
OD at r 28.75. Every fillet in the part is built this way, which is why the
count is 642 and not 8.

## Root cause

### 1. Three splitters resolve the tangency corner to three different points

Near the touch point the fillet arc and the bore stay within 1.6 × 10⁻³ mm of
each other over 0.22 mm of boundary, so "where does the boundary cross r = 24"
has no well-conditioned answer, and each stage answers differently:

| who | what it computes | result |
|---|---|---|
| `split.rs:1962 split_planar_face_by_arc` on the block's cap | circle × arc-**polyline** crossing | (23.712183, 3.705720), r = 24.000000 |
| the block's own geometry | the cube corner the fillet arc ends on | (23.711165, 3.712400), r = 24.000026 |
| the bore wall's splitter | that corner projected onto the bore | (23.711138, 3.712400), r = 24.000000 |

Spread: 6.8 × 10⁻³ mm between the first and the last. So after the boolean the
result carries, around one corner:

* cap piece (Plane, z = 11.1): `… P4 P3 P2 P1 → P5 …` — arc up to the crossing,
  then the canonical bore grid;
* fillet wall (Cylinder r 1.05): `… P4 P3 P2 P1 → (23.711138, 3.7124) …`;
* bore wall (Cylinder r 24): starts at `(23.711138, 3.7124)` and runs the
  other way.

Nothing pairs across the corner, and a sliver of roughly 2 × 10⁻⁴ mm² per cap
is left uncovered.

The splitter itself is **not** at fault — driven in isolation it produces a
clean, simple 77-vertex loop, with the arc ending at the crossing and the bore
grid continuing from it. The reproducer file records that.

### 2. The repair pass hides the gap by pinching the loop

`crates/vcad-kernel-booleans/src/repair.rs:133
split_edges_at_interior_vertices_impl`, called at `pipeline.rs:1442` and
`:1447` with tolerance `1e-6`, subdivides any unpaired half-edge at any loop
vertex lying within tolerance of its interior. It does **not** check that the
vertex belongs to the same loop (`repair.rs:237–253` collects the hits,
`:260–272` rechains them).

Where the cap's seam chord P1 → P5 is collinear with the fillet arc it just
came from — and at a tangency it is, to about 1 × 10⁻⁶ — that chord is
subdivided by the cap's **own** vertices, and the loop comes back through
points it has already visited:

```
… v77 v78 v79 v175 v79 v78 v77 v189 …          (77 vertices become 80)
```

Triangulated, that covers the sliver twice with opposite winding. Signed
volume cancels. The *net directed* open-edge count cancels. The rims do not,
and neither does `overused_edges`. That is the whole symptom: right volume,
`Analytic`, not closed.

`repair_topology_impl` (`repair.rs:27`) runs `cleanup_loop_spikes` at line 29,
**before** the subdividing pass at line 31, so the spikes this pass creates are
never swept up.

### 3. The pinch is load-bearing

The obvious fix — skip a hit whose vertex is already in the loop being split —
was implemented and measured. It removes the pinch (the cap loop becomes
simple again) but it does **not** close the shell: the underlying corner gap is
still there, and the reproducer goes from 8/9 to 10/10. Worse, it regresses
`coplanar_cap_union.rs::twelve_filleted_posts_stay_analytic`: with the sliver
no longer double-covered, the analytic union's volume disagrees with the mesh
referee (`api.rs:648–680`), the referee overrules, and the twelve-post union
falls to triangle soup. The patch was reverted.

So the pinch is currently paying for the corner mismatch. It cannot be removed
before the corner is unified.

## Two candidate designs

### A. One tangency corner, computed once and shared

Detect the tangency analytically (two coplanar circles with
\|d − (R ± r)\| < ε — here d = 22.95002 against R − r = 22.95, i.e. 2 × 10⁻⁵),
compute the exact touch point `c + R·unit(centre − c)`, and make every
consumer use *that* vertex: the cap's arc split, the fillet wall's rim, and
the bore wall's split. The arc polyline's last vertices are then snapped onto
it rather than crossing it. This is the only fix that makes the corner
well-defined instead of well-hidden.

*Risks.* It touches `split_planar_face_by_arc`, the cylindrical splitters and
the circle-polygon crossing finder together — the torture corpus is the gate,
and the memory notes are explicit that the canonical rim grid and the sphere
splitter must move together with anything of this kind. It also needs a
decision about what "tangent" means at discretization scale: the existing
`arc guard` comment at `split.rs:2216` already warns that *"tangent-cyl
stadium cutters carry legitimately distinct arcs a few µm apart"*, so an ε
that is too generous will fuse real features.

### B. Collapse the sliver instead of covering it twice

Keep the pinch detection from the rejected patch, but rather than dropping the
subdivision, *collapse* the degenerate sliver: when an inserted run of vertices
reproduces, in reverse, a run the loop has just traversed, and the enclosed
area is below tessellation sag, weld the two rails onto one polyline and keep
the more accurate one (the analytic arc). Then re-run `pair_half_edges`.

*Risks.* Smaller blast radius — it lives entirely in `repair.rs` and only fires
on loops that are already non-simple. But it moves geometry (up to ~1.6 × 10⁻³
mm here), so it needs the same guard `weld_boundary_vertices` already carries,
and it will change the volume slightly, which means the union referee's slack
has to be checked against it. It also treats the symptom: the three splitters
still disagree, and the next tangency in a different arrangement will present
differently.

A ordered before B: B without A leaves the +0.01 mm row of the sweep table —
the silently wrong volume — untouched.

## Determinism (briefed defect (c))

Still reproducible on this base branch, which does **not** contain #903. Six
separate processes, varied environment padding, fresh `VCAD_CACHE_DIR` each:

* the tessellation is bit-identical every time — 12 186 triangles, 501 faces,
  7869.62 mm³, 642 open, 95 over-used;
* `Solid::volume()` (`vcad-kernel/src/lib.rs:1590`, which meshes at
  `self.segments` rather than at a fixed count) is **bimodal**: 7852.96 mm³ in
  2 of 6 runs, 7854.46 in 4 of 6 — a 1.50 mm³ (0.019 %) spread.

That is the class #903 fixed (HashMap/slotmap ordering leaking into
`tessellate::snap_boundary_rails` and `heal_t_junctions_pass`), so the action
is to merge `main` into `claude/cam-roadmap` and re-measure, not to re-fix it
here. Until then, any stator number quoted from this branch should be quoted
with the run count.

## What CI will show on the export shape guard

The torture baseline (`crates/vcad-torture/baseline.json`) is
platform-specific and deliberately untouched, so a PR carrying the export
repair's shape guard will show these against it:

| case | change | why |
|---|---|---|
| `chain-13` | bad-geometry → **pass** | its tessellation carries 0.0791 mm of sag, so the 0.032–0.067 mm repairs it needs are inside what the mesh can express and are no longer declined |
| `rand-094`, `rand-098` | bad-geometry → **pass** | |
| `chain-23` | pass → **bad-geometry**, 94 open boundary edges | **by design** |

`chain-23` is the honest regression and should not be tuned away. Its export
mesh is fine enough that its sag sits under the 0.02 mm floor, and the repair
that used to close those 94 edges wanted to move the surface **0.0498 mm** —
two and a half times what the mesh's own error can hide. The export now says
"94 open edges, repair declined: would move the surface 0.050 mm" instead of
quietly shipping a different part. The real fix for those edges is upstream in
the boolean, not in the repair. (For scale: the *permissive* mesh-boolean path
on the same case moves **1.97 mm**, over 6.12 % of the surface.)

Making the floor sag-relative, or nudging it to 0.05 mm to admit this one
move, would be tuning the rule to the corpus.

## Open item: the buried-face check, built and 80 % calibrated

A union of a fillet block drawn **0.01 mm off tangency** comes back
`Analytic` at 4734.78 mm³ against a closed form of 4726.91 — **+0.167 %** —
with 6 unpaired edges and nothing catching it. The volume bound
(`max(A,B) ≤ vol ≤ A+B`) is orders of magnitude too loose, and edge counts
cannot separate it from a healthy part: the stator carries 642 unpaired edges
and is right to 0.06 %, this carries 6 and is 7.9 mm³ wrong. Any part with a
fillet authored slightly off tangency — most hand-written CSG — is exposed.

The failure is a **missing trim**: a stretch of face that should have been cut
away survives inside the other operand, so the result encloses that material
twice. `validate::buried_retained_face` tests exactly that, orientation-free:
a face ON the result's boundary has material on one side and void on the
other; a buried one has material on both. Containment is judged against the
operand meshes (valid solids) by the same three-ray parity vote the mesh
boolean trusts.

### Located: the near miss is ONE missing wall face, and the shape is right

Rasterised the near-miss union's sections against the 2D CSG of its own
source (`scratchpad/nearmiss.loon`, truth from `stage_truth.py`). The answer
is not a missing trim, not a wrong shape, and not at the caps.

**The section is identical at every height** — 11.2, 12.5, 14.1, 15.5, 16.8,
17.0 — and at every one of them it refuses to close with the *same* gap:

    0.1640 mm between (23.7111, 3.7124) and (23.7338, 3.5500)

Identical at every z means a vertical wall, so the cap splitter is exonerated.
The two endpoints name the stretch exactly: (23.7111, 3.7124) is where the
r 24 bore meets the block's top edge y = 3.7124, and (23.7338, 3.5500) is the
fillet arc's apex, at r = 23.9978 — just *inside* the bore. Between them the
arc crosses r = 24 (at about y = 3.57), so the union's inner boundary has to
hand over from the arc to the bore mid-stretch. **Both sides of that handover
are missing from the result.**

Heal the gap and the rest of the section is right: area 787.8190 mm² against
a 2D-CSG truth of 787.7636, **max boundary distance 0.00499 mm**, 1 region,
1 hole, at every height.

So where is the 7.9 mm³? It is not in the cross-section at all. The sections
imply 787.8190 × 6 = **4726.914 mm³**; the divergence integral over the
tessellation reports **4734.783**. The gap between those two numbers is the
missing wall itself:

    missing wall area  0.16397 x 6      = 0.9838 mm²
    flux it would contribute  (1/3)·r·A = 7.8706 mm³   (r = 24)
    observed discrepancy               = 7.869 mm³

Four significant figures. **The +0.167 % is not a wrong solid — it is a right
solid with one face absent, and the divergence theorem billing for the hole.**

That explains everything that was confusing about this case: why the buried-
face check correctly does not fire (nothing is buried), why the volume bound
never noticed (the bound is on volume, and volume is exactly what the defect
corrupts), and why the six unpaired edges are the rim of the hole rather than
an incidental sliver.

### Fixed: parallel-axis cylinders had no SSI

The wall was never *classified* wrongly — it was never **split**. `ssi.rs`'s
`cylinder_cylinder` bailed out for parallel axes, by design and with a stated
reason:

> Coaxial / parallel axes → `Empty`. […] for parallel-but-distinct axes the
> SSI is either two parallel line generators or empty, but our downstream
> consumers only call this for face pairs whose AABBs already overlap, and
> parallel-axis cylinders that overlap will normally also share planar caps
> that drive the trimming.

The assumption fails when the crossing is in the **middle of a wall**. The
fillet's r 1.05 wall crosses the r 24 bore in two generators 0.294 mm apart,
with no cap anywhere near them; the face spanning the crossing got one
classification sample at r = 24.006 — correctly `Inside` for that sample,
wrong for half its area — and was dropped whole.

`parallel_cylinders` now returns the two generators. The near miss:

| | before | after |
|---|---|---|
| volume | 4734.783 | **4726.913** (2D-CSG truth 4726.58) |
| unpaired edges | 6 | **0** |
| section at 11.4 / 14.1 / 16.8 | refused, 0.164 mm gap | **closes with no healing**, max boundary 0.00499 mm |

**And a designed tangency must still return `Empty`.** Emitting a generator
for one cost the stator dearly — 7869.6 → 8222.5 mm³ (+4.5 %), 1452 unpaired
edges against 642, twice the solve time — because the part is *made* of them:
twelve post-root fillets and six tab-root fillets, each an r 1.05 wall meant
to touch the bore or the OD, each missing it by rounding and so crossing in
two generators 0.0154 mm apart. Splitting a wall on those mints a sliver the
classifier cannot judge. `MERGE_GENERATORS = 0.05 mm` separates the two
populations by a factor of twenty on one side and three on the other; below
it the old `Empty` stands. With that in, the stator is byte-identical to
before: 501 faces, 12 186 triangles, 7869.618 mm³, 642/95.

The tab corner did **not** close for free — it is a different mechanism.

### The buried-face check: calibrated, sound, and not the tool for this

The check is now clean: **zero false positives** across the whole boolean
suite with `VCAD_BURIED_FACE_CHECK=1`. Getting there took two structural
fixes, neither of them a tolerance:

1. **Sample in parameter space, not in 3D.** The obvious construction —
   centroids of a fan over the loop vertices — puts the sample on a *chord*,
   which on a cylinder of any size is deep inside the solid. Dumping the
   firing face on `stacked_rings_face_to_face` showed a sample at
   (30.697, 0.202, 0.417) with `MeshDistance` reading **0.417 mm to its own
   operand** — it was never on the face. Averaging the loop vertices' UVs and
   evaluating the surface there lands on the surface by construction, for
   planes and quadrics alike. This removed six of the eight false positives
   in one change.
2. **Never use the loop winding to pick an outward direction.** A face on the
   boundary has material on one side and void on the other; a buried one has
   it on both. That needs a normal *line*, not a normal *direction*. The
   first attempt used the winding and reported every second face.

(`CONTACT_TOL`, which excludes a sample resting *on* the other operand rather
than inside it, survives as a guard but was not what was firing.)

**And with all of that right, it does not fire on the near miss at all.**

That is the finding, and it retracts an earlier claim of mine: the "catch" I
reported first — a buried face at (23.73, 3.59, **12.1**) — was itself a
chord-interior artifact. z = 12.1 is not on any face of a part whose features
run z 11.1…17.1; it is the fan centroid of a wall. The check was firing on its
own sampling bug, which happened to land on that case.

So the near miss's +0.167 % is **not** a face with material on both sides, and
the missing-trim model of it is wrong. Whatever puts 7.9 mm³ into that union
does not leave a buried face behind. The next move is to locate the extra
volume directly — rasterise the result's caps against the 2D CSG at the
fillet, the way the stator's lost volume was located — rather than to guess
at another invariant.

The check stays opt-in until it earns its keep on a case it actually catches.
It is sound, and the invariant is real; it just has no demonstrated catch yet.

## Retracted: the tangency snap's cost is the snapping, not the lookup

An earlier note here blamed `split::snap_tangential_crossings` for
re-deriving the tangent carriers from the geometry store on every arc split,
and proposed handing it the pipeline's once-per-boolean lines instead. That
was wrong, and the fix made it worse.

Measured on the stator's 57 stages, debug profile, load ~8:

| | solve |
|---|---|
| snap off | **30.6 s** |
| snap on, re-deriving per split | 79.0 s |
| snap on, consuming the precomputed lines | **104.7 s** |

The re-plumb is a real improvement in structure — the splitter and the repair
pass now read one set of tangencies instead of deriving it twice, and it
covers cylinder–plane tangencies the old scan never saw — but it is *slower*,
because it fires more often. **The cost is the snapping itself**: pinning a
crossing moves where a face is cut, which makes more sub-faces and more work
for everything downstream. No amount of making the lookup cheaper touches it.

So the snap is now **opt-in** (`VCAD_TANGENCY_SNAP=1`), and the stator is back
to 29.8 s. What it buys, on the two-operand reproducer, is `block ∪ ring`
going from 10 unpaired edges to 6; `ring ∪ block` reaches 0 without it, on the
repair-side tangency zone alone. A 3.4x solve for four edges is not a trade
this part can make.

The idea is still right and the machinery is kept: a cheaper form would pin
only the crossings a tangency actually makes ill-conditioned, rather than
every crossing near one.

## Open item: the mesh fallback moves intermediate solids by millimetres

Separate from the seam, and not addressed here. The mesh-boolean path repairs
under `RepairPolicy::manifold_at_any_cost`, and measured on the real parts a
single step moves the surface **2.96 mm** (the eval shell ring), **2.65 mm**
(the rana-60c shell) and **1.97 mm** (torture `chain-23`). That is defensible
for a terminal result whose caller has already accepted a degraded solid — but
these are *intermediate* steps in a chain, and **their output is the next
boolean's input**. A 2.7 mm error does not stay where it was made.

The numbers are pinned in
`vcad-kernel-booleans/tests/rana_60c_shell.rs::the_permissive_repair_moves_this_shell_by_millimetres`
so a change shows up as a number. Closing it means either making the analytic
path handle these arrangements, or carrying the degradation forward as
provenance so a chain can refuse to build on a solid that moved this far.

## What landed on `cam/w1-union`

* `crates/vcad-kernel-booleans/tests/tangent_fillet_rim_seam.rs` — three
  `#[ignore]`d reproducers (shell closed, loop simple, tab cap not doubled)
  and one passing control (a fillet clear of the bore is closed). The ignored
  ones fail today with exactly the numbers above.
* `crates/vcad-eval/tests/stator_measure.rs` — `#[ignore]`d end-to-end
  measurement, driven by `VCAD_STATOR_LOON`.
* this document.

No change to `crates/vcad-kernel-booleans/src/**`, so the boolean suite (199
tests) and the torture tracks are unchanged by construction; the suite was run
green on the base to confirm the starting point.
