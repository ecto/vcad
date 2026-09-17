# CAM claims M0: `vcad.cam-claims/1`, and the two rungs a CAM claim stands on

Wave 1 of the [CAM roadmap](cam-roadmap.md) gave CAM real oracles: a 2D
replay of a job against the part it is meant to make
(`vcad_kernel_cam::verify2d`), a cutter-fit report (`fit`), contours from a
solid (`outline`), exact tooth spaces and over-pins measurements (`gear`),
and a feeds-and-speeds rule pack (`materials`). None of them produced a
receipt, and the 3D oracle in `vcad-kernel-stocksim` had no consumer at
all. M0 closes both gaps.

The rule the roadmap serves is that **the app has to be able to say no by
itself**, and what cannot be verified is refused rather than assumed. A
claim family is how that refusal survives being written down: the receipt
is what a job carries to the machine, and every number on it says where it
came from, what it rests on, and what would make it false.

## The two rungs

CAM is unusual among the repo's claim families. `vcad.particle-claims/1`,
`vcad.thermal-claims/1` and `vcad.tolerance-claims/1` are entirely
predictions: a solver ran, a device has not been built, so every claim is
`basis: predicted` and the receipt rolls up **Provisional, never Pass**
until a bench measurement closes it.

Half of CAM is not like that. *"This program never brings the swept cutter
inside the part"* is arithmetic on the program, the contour and the tool.
It is true or false, and no measurement can make it truer — a caliper on
the finished part measures the machine, the fixturing and the cutter
deflection, not the G-code. But *"the cut gear measures 21.5691 mm over
Ø1.5 pins"* is a statement about metal nobody has touched.

So the ladder has two rungs, and which one a claim sits on is a property of
the claim, not of how hard the oracle worked:

| Rung | `Basis` | Best status on its own | Closed by |
|---|---|---|---|
| Computed | `Computed` | `Holds` | nothing — it is already decided |
| Predicted | `Predicted` | `Provisional` | a measurement of the real part |
| Measured | `Measured` | `Holds` | — |

and two statuses belong to neither:

| Status | Means | Never |
|---|---|---|
| `Unverified` | the oracle could not run: no work offset, no travel limits, no contact limit, no tabs to audit | a pass |
| `Stale` | an input the claim rests on moved since it was made | a pass |

A predicted claim that fails its *own* arithmetic is `Violated`
immediately: the fast path saying "no" is actionable.

Mapping onto the unified `vcad.receipt/1` schema loses nothing:

| CAM status | receipt verdict | receipt basis | receipt rolls up |
|---|---|---|---|
| `Holds`, computed | `Pass` | `Verified` | Pass |
| `Holds`, measured | `Pass` | `Measured` | Pass |
| `Provisional` | `Pass` | `Predicted` | **Provisional** |
| `Violated` | `Fail` | as recorded | Fail |
| `Unverified` | `Unverifiable` | — | Unverifiable |
| `Stale` | `Unverifiable` | — | Unverifiable |

The whole typed claim rides in the receipt claim's `details` as JSON, the
same trick `mech.clearance.*` uses, so a stored receipt can be re-stated
against a changed job without any external context.

## Where the family lives, and why

`crates/vcad-kernel-cam/src/receipt.rs`, next to the reports it consumes,
with a `vcad-receipt` dependency — the layout every other solver family
uses (`vcad-kernel-particle/src/receipt.rs`,
`vcad-kernel-tolerance/src/receipt.rs`, `vcad-kernel-em/src/receipt.rs`,
…). `vcad-kernel-stocksim` depends on `vcad-kernel-cam`, not the other way
round, and `vcad-receipt` depends on nothing, so there is no cycle.

**No TypeScript codegen is needed.** `npm run ir:gen` bundles ts-rs
bindings from exactly two crates, `vcad-ir` and `vcad-receipt`
(`scripts/gen-ir-types.mjs`, `SOURCE_CRATES`). This family rides the
unified schema's *open domain vocabulary* — `domain: "cam"`, claim ids
`cam.<name>` — over the generic `ReceiptClaim` type that is already
exported. Nothing new crosses the wire, exactly as for particle and
tolerance.

## The claims

Every claim carries a value, the limit it is held to, a unit, the metrics
behind it, the basis keys it rests on, and a machinist-readable note. The
`depends_on` keys index a `Fingerprint`.

### `job.*` — from a 2D replay (`verify2d::verify_gcode` / `verify_toolpath`)

Basis: `program`, `outline`, `tool`, `stock`.

| Claim | Value / limit | Rung | Violated when |
|---|---|---|---|
| `job.no_gouge` | worst intrusion / `VerifyOptions::tolerance` | Computed | any cutting move brings the swept cutter into the part. *Friction log item 32.* Metrics: `violations` |
| `job.material_left` | max stand-off / tolerance | Computed | a wall the full-depth passes reach was not cut to size. Metrics: `unswept_area_mm2`, `reachable_band_area_mm2`, `walls_machined`, `walls` |
| `job.depth` | material left under the part / declared `bottom_allowance` | Computed | the depth check fails, **or** the job cuts past the stock underside with no spoilboard declared. Metrics: `deepest_z`, `floor_z`, `stock_thickness`, `features`, `spoilboard_declared` |
| `job.rapids_safe` | worst / `safe_rapid_z` | Computed | a rapid travels in XY below the safe height, or dives into material |
| `job.tabs` | thinnest metal width / `min_tab_metal` | Computed | a tab leaves too little metal, or a pass that cuts below a tab does not step over it. Metrics: `tab_count`, `declared_tabs`, `observations`, `passes_below_tabs` |
| `job.tabs_hold` | same numbers | **Predicted** | the audit already fails. Otherwise `Provisional`: that a tab *holds* is a statement about metal |
| `job.loose_pieces` | pieces freed / 0 | Computed | the job frees stock that can move under the cutter. Metrics: `largest_area_mm2`, `skin_holds` |
| `job.envelope_in_travel` | worst overrun / 0 | Computed | the sweep reaches past a travel limit |
| `job.plunges` | worst plunge feed / `max_plunge_feed` | Computed | a straight vertical entry into uncut material is too fast. Metrics: `centre_cutting` |

Two of these exist because **the 2D oracle passes vacuously and a receipt
must not**:

- `job.envelope_in_travel` is `Unverified` without a work offset *and*
  travel limits. `verify2d`'s envelope check passes when there is nothing
  to compare the sweep against — correct for a check, a lie for a receipt.
  The swept extents are still recorded as metrics; they are just not a
  verdict.
- `job.tabs` / `job.tabs_hold` are `Unverified` when the job declares no
  tabs and the moves show none. Nothing to audit is not the same as tabs
  that hold.

And one is stricter than the oracle beneath it: `job.depth` refuses to
certify a job that cuts into a bed nobody declared, even though
`verify2d`'s depth check allows a negative bottom allowance.

### `fit.*` — from a cutter-fit report (`fit::fit_contour`)

Basis: `outline`, `tool`. Subject: the contour.

| Claim | Value / limit | Rung | Violated when |
|---|---|---|---|
| `fit.cutter_passes` | centre-region pieces / 1 | Computed | the tool centre region falls apart, so the cutter cannot follow the contour in one loop. The detail names the largest cutter that would pass |
| `fit.reachable` | max corner stand-off / a stated allowance | Computed | the cutter leaves more metal in a corner than the part allows. Metrics: `corners`, `unreachable_area_mm2`, `largest_corner_area_mm2`, `min_neck_width`, `slot_clearance_per_side`, `largest_tool_diameter` |

These are two different facts and only one of them reached the machine on
2026-09-17. The stator's bore-and-slots loop with the Ø3.175 that actually
ran: `fit.cutter_passes` **Holds** — 3.175 fits through the 3.87 mm slot
mouths — while `fit.reachable` is **Violated** with 24 corners, 10.57 mm²
of metal and 0.292 mm of worst stand-off. That is friction-log item 38,
stated as a number the app can refuse on.

### `outline.*` — from `outline::compare_outlines` / `is_prismatic`

| Claim | Value / limit | Rung | Violated when |
|---|---|---|---|
| `outline.matches_solid` | max boundary distance / tolerance | Computed | the contour and the solid differ past tolerance, **or** any hole has no partner. Metrics: `symmetric_difference_area_mm2`, `intersection_area_mm2`, `holes_matched`, `holes_solid`, `holes_contour`, `holes_unmatched` |
| `outline.prismatic` | worst section disagreement / tolerance | Computed | the section changes with depth, so a 2.5D contour job does not describe the part. Metrics: `symmetric_difference_area_mm2`, `worst_z`, `reference_z`, `levels` |

### `gear.*` — from `gear::GearReport`

Basis: `gear`, `tool`. Subject: the gear.

| Claim | Value / limit | Rung | Notes |
|---|---|---|---|
| `gear.contact_ratio` | transverse contact ratio / 1.0 | Computed | one per mesh the gear takes part in. Metrics: `path_of_contact`, `centre_distance`, `normal_backlash` |
| `gear.reachable` | flank deviation at the contact limit / the stated tolerance | Computed | graded by the **metal the fillet leaves on the active flank**, not by the radius it crosses. `Unverified` without a contact limit. Metrics: `radial_overlap`, `margin`, `contact_limit`, `form_radius` |
| `gear.over_pins` | the predicted measurement M | **Predicted** | `Provisional` until pins go in it. Metrics: `pin_diameter`, `sensitivity`, `contact_radius`, `pitch_tooth_thickness`, `even_teeth` |

`gear.reachable` is the wave-1 lesson in one claim: on the reference ring
the fillet overlaps the contact limit by 0.0336 mm of *radius* and leaves
0.0014 mm of *metal*. Those are the same fact, and only the second is
something a machine can be held to. The strict verdict (allow no deviation
at all) `Violates`; graded at 5 µm the same geometry `Holds`, and both
numbers are on the claim so a reader never has to recompute to see why.

### `feeds.within_recommendation` — from `materials::check`

Basis: `material`, `tool`, `stock`. Value is the worst note level
(0 info … 3 danger), limit is the level the caller blocks at. Metrics:
`notes`, `caution`, `warning`, `danger`. Computed: a rule pack saying
"copper without lubricant will weld to the flutes" is a rule, not a
prediction about this particular cut.

## Basis, and why a claim goes stale

A receipt that cannot say *which* job it certifies certifies nothing. Every
claim names the inputs it rests on by key into a `Fingerprint`:

| Key | Covers |
|---|---|
| `program` | the G-code text or the toolpath |
| `outline` | the part contour in the stock frame |
| `tool` | the cutter |
| `stock` | thickness, bottom allowance, spoilboard, work offset, travel limits, declared tabs |
| `gear` | the gear definition |
| `material` | material, machine and spindle |
| `solid` | the target solid an outline was compared against |

Digests are FNV-1a over the serialized value — a **change detector, not a
cryptographic hash**, chosen so the module stays dependency-free like the
other families; the same choice `vcad-ecad-verify`'s `board_hash` makes.
Loops hash by geometry rounded to the nanometre, so re-reading the same DXF
gives the same digest.

`restate(&set, &current_fingerprint)` re-hashes and brings back `Stale`
anything whose basis moved, naming the keys that changed. Claims that do
not depend on the changed key are untouched: edit one feed rate and the
nine `job.*` claims go stale while `fit.*` does not. A key the current
fingerprint does not carry *at all* counts as changed, not as unchanged.

This is the model `check_clearance`'s labeled assertions already follow
(`Holds` / `Stale` / `Violated`), at the granularity of one input rather
than one document.

## Measurement binding

```rust
Measurement {
    claim: "gear.over_pins",
    subject: Some("planet-20T"),
    kind: MeasurementKind::OverPins { pin_diameter: 1.5 },
    value, uncertainty, tolerance, instrument,
}
```

`MeasurementKind` also carries `Caliper { feature }`, `Span { teeth }` and
`HoleDiameter`. `bind(&set, &measurements)` is fail-closed three ways:

1. a measurement naming a claim that is not in the set is an error —
   a measurement of nothing is a bookkeeping bug;
2. a measurement of a claim on the **computed** rung is an error, with a
   message saying so: you cannot close arithmetic with a caliper;
3. unusable numbers (non-finite value, negative tolerance) are an error,
   never a quiet pass.

A bound claim comes back on `Basis::Measured`, `Holds` or `Violated` by
whether the reading is inside `tolerance + uncertainty` of the prediction,
with `predicted` and `delta` added to its metrics and the measurement
itself recorded. Claims nobody measured keep `Provisional`: a set with one
unclosed prediction is not clean.

### The compensation loop

`close_over_pins(&gear, &set, subject, &measurement)` is the loop the first
milestone rests on — measure a planet, get back *the number to change*
rather than a verdict:

```
GearClosure {
    compensation,        // SpurGear::compensation: exact involute algebra
    closed,              // gear.over_pins, now Measured, Holds or Violated
    corrected,           // gear.over_pins.compensated, Predicted/Provisional,
                         // supersedes: Some("gear.over_pins")
}
```

It refuses a measurement taken over a different pin than the prediction was
made with, rather than silently converting.

The corrected claim is `Provisional` again, on purpose. A compensation is a
plan; the second part has to be measured too.

**Worked numbers** (module 1.0, 20 T planet of
`docs/cam-fixtures/gears-60-cnc.json`, Ø1.5 pins, measured 0.02 mm over
nominal — all from `SpurGear`'s own API, asserted in the tests):

| Quantity | Value |
|---|---|
| nominal M over Ø1.5 pins | 21.569089087 mm |
| sensitivity dM/dt at the pitch circle | 2.678861 |
| tooth-thickness error | **+0.00749228 mm** (too thick) |
| the same as a profile-shift error | +0.01029 modules |
| cutter normal offset to apply | **−0.00352022 mm** (cut deeper) |
| cutter radial offset to apply | −0.01029244 mm |

The linearised reading — ΔM divided by the claim's own sensitivity — is
0.0074659 mm; `compensation` inverts the involute algebra exactly instead,
and the two agree to 0.4 %. (The roadmap ticket quoted ≈0.00727 mm and
≈−0.00342 mm; those are internally consistent with a sensitivity of 2.751,
which this gear and pin do not have. The API's numbers are the ones above,
and the test cross-checks them against both the linearised reading and the
`cos α / 2` identity that relates the two.)

## 3D: `vcad-kernel-stocksim::verify_toolpath_against_mesh`

```rust
verify_toolpath_against_mesh(&[SimOp], target_vertices, target_indices, &JobOptions)
    -> Result<StockJobVerification, JobError>
```

One call: build the stock, subtract every op's toolpath with its own cutter
envelope, grade against the target mesh, check shank and holder, check
rapids. The point of M0 here is as much *stating what the answer is worth*
as producing one.

### The resolution is part of the answer

A voxel oracle cannot see a defect thinner than its own cells, and the
failure mode is not a wrong number but a reassuring one. So the gouge
verdict has three values:

```rust
enum Verdict { Pass, Fail, Unresolved }
```

A gouge tolerance finer than the simulation's own discretization margin
comes back `Unresolved` with the margin attached, **never "no gouge"**.
`ResolutionReport` carries:

| Field | Meaning |
|---|---|
| `leaf_cell` | the octree's leaf size per axis. The octree splits the *box*, so a thin stock gets thin cells in Z and coarse ones in XY |
| `sample_pitch` | what the grader actually visited. It strides its grid to stay inside a sample budget, so on a deep octree this is coarser than the cell — and it, not the cell, bounds how *narrow* a defect can hide |
| `sim_margin` | added to both thresholds to absorb discretization |
| `min_detectable_depth` | `tolerance + sim_margin` |
| `min_detectable_extent` | the coarsest sample pitch |
| `octree_depth`, `octree_nodes`, `octree_bytes`, `grid_samples` | what it cost |

### Leftover material needs a band

Grading "excess" against a target part only means something where the job
was *meant* to clear material. A contour job leaves the waste frame
standing and drops a slug out of every pocket it cuts around; both read as
tonnes of excess against a target that is just the part, and neither is a
defect.

`MaterialLeft` therefore **brackets the leftover by stand-off**: the oracle
is run at two allowances and the difference is what is still sitting within
`excess_band` of the part — the direct analogue of the 2D oracle's wall
bands. Everything past the band is reported with its volume as stock, not
dropped and not counted as a violation. One tool diameter is the usual
band; the bore test uses 1 mm because its slug sits 2 mm off the wall.

### Certain versus possible

Material only ever decreases, so anything the **finished** stock still
collides with was certainly there at the time; anything only the
**starting** blank collides with may have been cut away first. Holder and
rapid hits carry `certain: bool` on that basis, and the two counts are
reported separately. An undeclared holder is `Unresolved`, not clear. The
first motion segment of a program only establishes position — grading a
move whose start we invented would be grading our own guess.

### Measured behaviour on the real job

`docs/cam-fixtures/stator-copper-d2.nc` — four inside contours and an
outside one, Ø2 flat end mill, 0.8 mm copper, 7 580 moves — against the
extruded `stator-outline.dxf` plus the waste frame the job is not meant to
remove (the part grown by one tool diameter, which is where the outside
contour puts the kerf).

| | |
|---|---|
| stock | 68 × 68 × 0.8 mm, resolution 1.0 mm |
| octree | depth 7, 860 841 nodes, **13.8 MB** |
| leaf cell | 0.531 × 0.531 × 0.00625 mm |
| sample pitch | 1.063 × 1.063 × 0.0125 mm (262 144 samples) |
| `sim_margin` | 0.1428 mm |
| runtime | **13.6 s** (2.4 s subtraction, ~5.6 s per grading pass, two passes) |
| gouge | `Pass`, worst 0.0000 mm — the 2D oracle agrees exactly |
| material left in band (2 mm) | 3 740 samples, **52.8 mm³** |
| beyond the band | 41 664 samples, 588 mm³ |
| rapids | 0 through material, 0 below the safe height — 2D agrees |
| holder | `Unresolved` (the fixture declares none), so the job does not pass |

The leftover agreement is stated as a **volume standing in the stock**,
not as a verdict. Contour-cutting the bore-and-slots loop drops a slug out
of the bore (721.8 mm²) and a wedge out of each of the twelve slots
(5.67 mm² each). Through 0.8 mm of copper:

| | 2D (exact polygons) | 3D (1.06 mm sample pitch) |
|---|---|---|
| within 2 mm of a wall | twelve wedges, 54.4 mm³ | 52.8 mm³ (−3 %) |
| beyond it | the bore slug, 577.4 mm³ | 588 mm³ (+1.8 %) |

That is a far sharper cross-check than comparing verdicts, and it is
stable in a way verdicts are not. `verify2d`'s `material_left` booked the
wedges as 26.29 mm² of unswept wall band before the w1-verify integration
fix and books them as 0 after it — the same metal, counted once as wall
band and once as freed pieces. The volume did not move, because the metal
did not, so `unswept_area` is checked as a *bound* on the same leftover
rather than summed with it.

**The deliberately wrong job** (friction-log item 32): an inside contour
pushed one tool diameter outward. A Ø2 cutter on a Ø10 bore in a 30 × 30 ×
6 plate at 0.5 mm resolution gouges **1.9978 mm**, inside one 0.469 mm
voxel of the 2.0 mm tool diameter. The plate is 6 mm thick on purpose:
gouge depth is distance to the nearest target surface, so on the 0.8 mm
stator the identical mistake can only ever read 0.4 mm.

### What resolution is practical

| Resolution | Depth | Nodes | Memory | Subtract | Grade (per pass) | `sim_margin` |
|---|---|---|---|---|---|---|
| 1.0 mm | 7 | 861 k | 13.8 MB | 2.4 s | 5.6 s | 0.143 mm |
| 0.5 mm | 8 | 3.87 M | 62 MB | 11.8 s | 13.5 s | 0.076 mm |
| 0.25 mm | 9 | 15.7 M | 251 MB | 64 s | 15.2 s | 0.043 mm |

Cost in the subtraction is roughly 8× per halving (it is a volume), while
the grading saturates because the sampler strides to a fixed budget. **1 mm
is what a CI test can afford on this part; 0.5 mm is the practical ceiling
for an interactive check.**

What that means for the claim: on a 63 mm part with a Ø2 cutter, a 3D
`job.no_gouge` can be stated **to 0.2 mm at 1 mm resolution** and to about
0.1 mm at 0.5 mm resolution — not to the 0.02 mm the 2D oracle reaches on
the polygons. So for a prismatic part the 2D oracle is the one to believe,
and the 3D one is for what 2D cannot see: non-prismatic parts, holders,
and the third dimension of a rapid. That is the seam, and the `Unresolved`
verdict is what keeps a caller from crossing it by accident.

## Validation ladder

All in `cargo test -p vcad-kernel-cam` and `cargo test -p vcad-kernel-stocksim`.

- A clean job's geometric claims `Hold`; the two with nothing to check come
  back `Unverified`, and that poisons the rollup.
- A Ø4 cutter run on the wall line gouges **2.0 mm** and the claim is
  `Violated` with the depth on it; the receipt rolls up `Fail`.
- A job that breaks through with no spoilboard declared is `Violated` —
  and `Holds` the moment one is declared.
- Envelope: vacuous 2D pass → `Unverified`; with a machine it becomes a
  real verdict, both ways.
- Edit one feed rate: every `program`-dependent claim goes `Stale`, the
  contour-only claims do not, and the receipt reads `Unverifiable`.
- A predicted dimension is `Provisional` with every other claim passing,
  and the receipt reads **`Provisional`, never `Pass`**.
- A measurement inside its band closes it (receipt → `Pass`, basis
  `Measured`); outside it, `Violated` (receipt → `Fail`).
- Binding refuses a measurement of nothing, of arithmetic, of a different
  pin, and with unusable numbers.
- The stator's bore-and-slots loop: Ø2 reaches everything; Ø3.175 passes
  the 3.87 mm mouths but leaves 24 corners / 10.57 mm² / 0.292 mm; Ø4 does
  not pass at all and the claim names Ø3.874.
- 3D vs 2D on the real job — no gouge either way, and the standing volume
  agrees to 3 % inside the band and 2 % beyond it — and the wrong job's
  1.9978 mm gouge.
- `Unresolved` below the margin; the margin falls as the cells shrink.
- Serde round-trips for the claim set and the 3D report.

**Mutation checks** (break it on purpose, confirm the test fails, restore):

| Mutation | Test that caught it |
|---|---|
| `Claim::settled` lets a predicted claim reach `Holds` | `a_predicted_dimension_is_never_a_pass_without_a_measurement` (+2 others) |
| `design_claims` maps `Predicted` → `ClaimBasis::Verified` | the same test, by a different route |
| the gouge verdict ignores the resolution floor | `a_tolerance_below_the_voxel_margin_is_unresolved_not_clean` |
| `worst_gouge` read off the excess check instead of the gouge check | `an_inside_contour_offset_outward_…`, `the_stator_job_agrees_with_the_2d_oracle` |
| the job is never subtracted from the stock | both of the above |

## Milestone ladder

- **M0 — this.** The family, built from the wave-1 reports; fingerprint
  basis and `restate`; measurement binding with the gear compensation loop;
  3D job verification in stocksim with honest resolution reporting.
- **M1 — a measured part closes it in the app.** The receipt reaches the
  app's run blocker and the measurement goes back the other way: cut the
  part, measure it, and the `Provisional` claims settle to `Holds` or
  `Violated` on the document. Needs the MCP surface
  (`build_receipt`/`verify_receipt` learning the `cam` domain, alongside
  the `pcb`, `mechanical` and `constraint` families it already knows) and
  a `record_measurement` that binds to a receipt claim rather than only to
  a print prediction.
- **M2 — the compensation loop closes.** `close_over_pins` drives a re-cut:
  the corrected offset is applied to the contour op, the new job is
  verified, and the superseding claim is measured in its turn. The first
  brass planet, cut, measured over pins, within tolerance — the roadmap's
  first milestone — is this loop running once.
- **M3 — 3D for the parts 2D cannot describe.** The stocksim oracle wired
  behind a claim for non-prismatic jobs, where `outline.prismatic` fails
  and the 2D replay has no standing. Needs the resolution ladder above to
  stop being the binding constraint: a sparse or narrow-band stock instead
  of a full octree over the blank.
- **M4 — fixturing and the machine.** Clamps as keep-outs in the 3D sweep,
  the machine profile behind `job.envelope_in_travel` so it stops being
  `Unverified` by default, and probing results as measurements that close
  work-offset claims.

## Non-goals at M0

This family does not predict surface finish, tool life, cutting forces or
deflection — the things that decide whether a part is *good* rather than
whether it is the *right shape*. It does not model fixturing or workholding
(so `job.loose_pieces` says what comes free, not whether it matters), and
it does not verify the post-processor's dialect against a particular
controller. It says what the job will cut, what it cannot certify, and what
would have to be measured to close the difference.
