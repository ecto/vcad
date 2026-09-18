# Indexed 4th axis (M0: design)

Wave 4 of the [CAM roadmap](cam-roadmap.md). The machine is the AnoleX
4030-Evo Ultra 2 on Grbl_ESP32 1.3a, whose `$$` **already carries an A axis**
(`$103`/`$113`/`$123`/`$133` = 444.444 steps/deg, 1000 deg/min, 200 deg/s²,
300 deg) and whose ncSender reports `axes: ["X","Y","Z","A"]`. Nothing in vcad
emits an A word, and `verify2d::parse_gcode` refuses one on purpose (test
`gcode_refuses_what_it_cannot_replay`: `A90` → `'A'`). What to build, in what
order, and what to keep refusing until then.

## 1. What an A axis buys this user, ranked

The gearbox is the reason. Ranked by (work it unlocks) ÷ (machinery to build):

| # | Job | Mode | Why it wins |
|---|---|---|---|
| 1 | **Sun-spider shaft**, four sides + ends | indexed | Today: a lathe for the journals, a manual flip-and-re-zero for the arms. One chucking, four A angles, one work offset — the flip stack-up (±0.1–0.2 mm over two zeroing operations) collapses into the rotary's own indexing error |
| 2 | **Radial features on the ring can** — mounting holes, side slots, keyway | indexed | A cylinder wall is a flat 2.5D job in a rotated frame; `Drill`/`Contour2D` unchanged |
| 3 | **Gear blanks from bar**, form-cutter indexing | indexed | `gear/cutter.rs` already gives the exact space a round cutter leaves and `SpurGear::tool_centre_path` the exact path. A = `k·360/N` per tooth, cut along X: full face width, no plate-and-flip |
| 4 | **Worm screw / helical flutes** | continuous | A helix cannot be indexed — the only item that *requires* wrapped mode |
| 5 | **Helical gear teeth** | continuous | Same wrap, but over a 10 mm face at 15° the helix is 2.7 mm of surface travel: least bought, most spent |

Not on the list, and not to be attempted: **bevel gears** (need tool-axis
tilt), **worm wheels** and **true hobbing** (need the cutter rotation
synchronised to A; the spindle is a relay-switched trim router with no
encoder — `MachineReq.spindle == "dial"`, `vcad-cam-api/src/types.rs`).

What it costs:

- **Rigidity.** A part chucked 60–100 mm from its support is a cantilever; the
  roadmap's ±0.05–0.1 mm assumes work clamped to the bed. Expect a tailstock
  to be mandatory past ~3 diameters of stickout.
- **Travel.** A rotary plus tailstock eats X ($130 = 400 mm) and raises the
  work centreline 40–65 mm into a 100 mm Z ($132); with a collet and 25 mm of
  stickout the usable Z band may be under 40 mm. **Measure before buying**
  (§7).
- **Speed.** $113 = 1000 deg/min caps surface feed at `17.45·R` mm/min:
  87 mm/min at R = 5, 175 at R = 10, 349 at R = 20. $123 = 200 deg/s² gives a
  tangential acceleration of `3.49·R` mm/s² — 35 mm/s² at R = 10 against the
  300 mm/s² of $120–122, **8.6× slower** — and acceleration, not feed, sets
  the time on a path that reverses often.
- **The Grbl model.** A is a linear axis whose unit happens to be a degree
  (§3): soft limits, feed and acceleration all treat it that way, and $133 =
  **300** is less than one revolution.

## 2. Two modes, and where each lands in the existing kernel

### (a) Indexed — A moves only between operations

Each setup is an ordinary 2.5D job in a work frame rotated about the rotary
centreline. Everything downstream of the rotation is already built:

| Reuse | File |
|---|---|
| Multi-op program assembly, ordering, `M0` pauses, one spindle start per tool | `crates/vcad-kernel-cam/src/job.rs` (`Job`, `JobOp`, `OpRole`, `ToolChangeStrategy::ManualPauseReprobe`) |
| Every operation, unchanged | `operation/contour.rs`, `operation/drill.rs`, `operation/pocket.rs`, `operation/face.rs` |
| The 2D oracle, unchanged, once per setup | `verify2d.rs` (`JobSpec`, `PartRegion`, `verify_toolpath`) |
| The per-setup section of the part | `outline.rs` (`section_at_z`, `silhouette_from_above`, `is_prismatic`, `compare_outlines`) |
| Whole-part 3D check across setups | `crates/vcad-kernel-stocksim/src/job.rs` (`SimOp`, `verify_toolpath_against_mesh`) |
| The 2D analogue of the new type | `crates/vcad-cam-api/src/placement.rs` (`Placement`: dx, dy, rotation about Z) |

The one new concept is a **`Setup`** — `Placement`'s sibling for the A axis:

```rust
pub struct Setup {
    pub name: String,
    pub a_deg: f64,               // commanded A for every op in this setup
    pub centreline: RotaryAxis,   // axis direction (+X) and a point in the work frame
    pub approach: IndexApproach,  // Direct | FromBelow { backoff_deg } — anti-backlash
    pub wcs: Wcs,                 // usually one G54 for the whole program
}

pub struct RotaryAxis { pub origin: [f64; 3], pub direction: [f64; 3], pub runout: Option<f64> }
```

Per setup: rotate the target solid by `−a_deg` about the centreline →
`section_at_z` / `silhouette_from_above` → the same `Contour`s as today →
`Job` → `verify2d` against **that setup's own section**. The rotation happens
in the CAD frame; the CAM side never learns a new geometry type. `Job` grows
`setups: Vec<Setup>` and `JobOp` a `setup: usize`; assembly emits the index
move (retract to `park_z`, `G0 A<deg>`, dwell) at each setup boundary, exactly
where it emits a tool change today.

Two things 2D cannot see, so the whole-part check is **not optional**: a setup
that cuts away material a later setup was relying on, and a rapid that clears
setup 2's own section but not the metal setup 1 left standing. Both are
`verify_toolpath_against_mesh` questions — one octree over the bar, each
setup's toolpath fed as a `SimOp` with its points rotated **into the stock
frame** (the inverse of the CAD rotation). Resolution binds, as
`docs/cam-claims-m0.md` measured: 1 mm cells state a gouge to 0.2 mm, 0.5 mm
to 0.1 mm, and `Verdict::Unresolved` is what stops that being read as "clean".

### (b) Continuous / wrapped — A moves during the cut

A 2.5D path on a developed (unrolled) surface at radius `R`: `A = Y·180/(π·R)`,
X and Z unchanged, Z measured from the cylinder surface. Consequences:

- **Feed.** Grbl computes feed over `√(dx²+dy²+dz²+dA²)` with degrees standing
  in for mm, so a mixed X–A move is fed along a meaningless hypotenuse. Two
  honest options: `G93` inverse-time on every wrapped move, or per-block `F`
  recomputed for the local X:A mix (`F = f_surface · |Δ| / |Δ_surface|`). §7
  says which measurement decides.
- **Arcs.** An arc in the wrapped frame is not an arc in A, so wrapped paths
  are **chorded, never arc-fitted**: `fit_arcs` (`arcfit.rs`) is skipped for
  wrapped ops. `ArcPlane::Xz`/`Yz` and the plane words the posts already emit
  (`post/grbl.rs:192–199`, `post/linuxcnc.rs:252–258`) serve a *different*
  case — an arc cut in XZ at fixed A — and the roadmap's prerequisite stands:
  `parse_gcode` refuses G18/G19 (`verify2d.rs:1279–1282`) and `job.rs:1093`
  pins G17 in the prologue. The reader learns them before any op emits one.
- **Verification.** Unwrap and replay: convert A back to Y at the design
  radius, hand it to `verify2d` unchanged. Exact for a true cylinder, silently
  wrong for anything else — so the unwrapped check is `Unverified` unless the
  surface is a cylinder within tolerance, and the real answer is stocksim with
  a rotating stock (rotate each toolpath point into the stock frame, sampling
  A finely enough that the chord sagitta stays under the cell size).

**Build indexed first.** It reuses every operation, both oracles and the job
assembler; the new code is a frame and a `G0 A` between blocks. It covers
items 1–3 — the entire gearbox except the worm screw. Wrapped mode needs a
feed model, an arc policy, a G18/G19-capable reader and a rotating-stock
oracle before it cuts anything, none of which the sun-spider needs.

## 3. Grbl_ESP32 specifics

- **A is degrees, linearly.** `$103 = 444.444` steps/deg → 160 000 steps/rev
  → 0.00225 deg/step, 0.4 µm of arc at R = 10 mm. Resolution is never the
  limit; backlash is. 160 000 steps/rev against a 1.8° motor at 16×
  microstepping implies a **50:1 worm** — to be confirmed (§7).
- **Calibration** is the `$130–132` procedure: command a known rotation,
  measure it, scale `$103`. A 1 % error is 3.6° per revolution — invisible on
  a single index, fatal on a 50-tooth gear.
- **Soft limits.** `$133 = 300` with `$20 = 1` rejects any A target past 300°
  (`error:15`, the refusal already seen on long jogs). A four-setup program at
  0/90/180/270 fits; a wrapped path that turns more than 300° does not. Either
  the post keeps A inside `[0, $133]`, or the machine profile declares A
  rotary-unlimited and the operator sets `$133 = 0`. Fail closed: no A word
  leaves the post without a declared A travel.
- **Homing.** There is almost certainly no A limit switch, so `$22` does not
  home it and A's zero is *set*, not *found*: the first A block is preceded by
  an operator pause saying "set A0 here" — the same shape as
  `ToolChangeStrategy::ManualPauseReprobe`, reusing `ToolpathSegment::Pause`
  (`toolpath.rs:108`).
- **Words the post needs:** `A` on `G0`/`G1`; `G93`/`G94` for wrapped mode
  only; nothing else. `G17` stays in the prologue.
- **What the oracle must learn:** parse `A` as a fourth coordinate instead of
  refusing it; carry a per-setup frame so a move's XYZ is rotated into the
  part frame before any check runs; and still refuse a program where A changes
  *inside* an operation until wrapped mode exists. Indexed mode is exactly "A
  is constant within a block range" — checkable by the reader, statable by the
  receipt.

## 4. Fixturing and probing

- **Rotary centreline.** Two unknowns matter — its Y and Z in the work frame
  (X is free, direction is +X by construction). Touch the top of the chucked
  cylinder at A = 0, rotate to A = 180, touch again: centreline Z is the mean
  of the two heights minus the radius, and Y comes from the two flanks or an
  X-parallel sweep for the highest point. Repeating the pair at a second X
  station gives the axis *direction* — a rotary bolted 0.2° off X puts 0.35 mm
  of error at the far end of a 100 mm part.
- **Runout** falls out of the same A = 0/180 pair, at no extra cost: half the
  difference of the two touch heights. Being measured, it binds a claim (§5)
  instead of being assumed.
- **Tailstock.** Not geometry at M1–M2; a keep-out box in the machine profile,
  so the envelope check refuses a job that reaches into it. Clamps-as-keep-outs
  is already a wave-3 item; this is one more box.
- **Where it plugs in.** The machine profile and two-point skew probe are
  wave-3 items still queued (roadmap "Wave 3 — running"), so this is a
  requirement on them, not on something that exists: `MachineReq`
  (`vcad-cam-api/src/types.rs:156`) gains an optional `rotary { axis,
  travel_deg, steps_per_deg, backlash_deg }`, and `TravelReq`'s `[f64; 3]`
  pairs gain a fourth component. Finding a rotary centreline is the skew
  probe's shape: two touches, one derived frame.

## 5. Claims (`vcad.cam-claims/1`)

The family already has the shape; multi-setup needs a basis key and a few
claims. Basis keys gain `setup` (A angle, centreline, approach), so moving one
setup restates only that setup's claims, the way editing a feed restates only
the `program`-dependent ones (`receipt.rs:1021`, `restate`).

| Claim | Rung | Says |
|---|---|---|
| `job.no_gouge` / `job.depth` / `job.material_left`, **per setup** | Computed | The existing claims, subjected once per setup against that setup's own section, with `subject: Some("setup-2")`. A per-setup pass is not a job pass |
| `job.setups_cover_part` | Computed | The setups' swept volume subtracted from the bar leaves the target within `allowance`, and every face the job means to machine is reached by at least one setup. The whole-part stocksim run; `Unresolved` (never `Pass`) when the tolerance is finer than the octree margin |
| `job.setup_sequence` | Computed | No setup cuts away material a later setup's fixturing or datum depends on, and no rapid in setup *k* crosses metal standing after setup *k−1* — the two failures §2 calls invisible to 2D |
| `setup.rotary_runout` | **Measured** | TIR of the chucked work at the cut station, from the A = 0/180 probe pair. `Unverified` until probed, and an indexed job without it should be refused the way one without travel limits is |
| `setup.index_repeatability` | **Measured** | Return-to-A0 error after a full sequence, indicator on a flat. Predicted-then-closed, like `gear.over_pins` |

The rule: **a whole-part claim cannot be synthesised from per-setup claims.**
Four setups that each `Hold` say nothing about the part; only
`job.setups_cover_part` does, and it is `Unverified` without a target mesh and
a declared bar — the same reasoning as `job.envelope_in_travel`'s vacuous 2D
pass.

## 6. Milestone ladder

**M0 — this document.** *Exit:* the mode decision (indexed first), the `Setup`
type, the Grbl constraints and the claim additions are written down and the
owner has answered §7's first three questions.

**M1 — indexed, two setups, verified headless.** `Setup`/`RotaryAxis` in
`vcad-kernel-cam`; `Job::setups`; index blocks in assembly; `parse_gcode`
accepts A and requires it constant within an operation; per-setup `verify2d`;
`job.setups_cover_part` via stocksim. *Exit:* a 25 × 25 × 60 mm bar with a
flat on two opposite faces and a cross hole, cut as one two-setup program;
per-setup claims `Hold`; the whole-part claim `Holds` at 0.5 mm resolution
with the margin on the receipt; a deliberately wrong job — setup 2 indexed 90°
instead of 180° — is `Violated`, not merely different. Mutation check: make
the index move a no-op, confirm `job.setup_sequence` fails. No machine.

**M2 — the sun-spider on the machine.** Rotary mounted and calibrated;
centreline probed; runout and repeatability measured and bound; the shaft cut
at four A positions in one program. *Exit:* spider arms within 0.1 mm of
position, journals concentric within the measured runout, the receipt `Pass`
with `setup.rotary_runout` on the `Measured` rung, and no lathe. A failure
here is a number (repeatability, runout, deflection), not a verdict.

**M3 — wrapped.** `G18`/`G19` in `parse_gcode`; the §7 feed model; `fit_arcs`
refused on wrapped ops; a rotating-stock oracle. *Exit:* a worm screw or
helical flute cut on the bar, verified by unwrapping *and* by the rotating
sim, the two agreeing on removed volume the way 2D and 3D agree to 3 % in
`docs/cam-claims-m0.md`.

**What not to build**, at any milestone: 5-axis simultaneous; tool-axis tilt
of any kind (so no bevel gears, no undercuts, no swarf milling); RTCP/TCPC
(nothing to compensate — the tool axis never moves); spindle-synchronised
hobbing; automatic setup *planning* (the operator declares setups, the oracle
checks them); a second rotary; and any inference of the rotary centreline from
anything but a probe.

## 7. Open questions, and what settles each

| Question | Measurement |
|---|---|
| Is A actually driven, or are the settings vestigial? | `$J=G91 A10 F500`, indicator on the A output. No motion ⇒ the axis is unpopulated in this build and the plan is hardware-blocked |
| Gear ratio, and is `$103` right? | `G0 A360`, measure the actual rotation. Confirms or corrects 50:1 / 444.444 steps/deg |
| Backlash, and does approach direction matter? | Indicator at a known radius; index to 90° from below and from above, five times each. Decides whether `IndexApproach::FromBelow` is default or optional |
| Does Grbl_ESP32 1.3a accept `G93`? | Send `G93 G1 X1 A10 F60`. `error:20` ⇒ per-block `F` recomputation is the only wrapped feed model |
| How does it feed a mixed X–A move under `G94`? | Time a 50 mm X + 90° A move at F1000 against the hypotenuse prediction — settles the degrees-as-mm assumption before any wrapped code is written |
| Does the rotary fit the Z envelope? | With it on the bed, measure centreline height and the Z reading with the shortest usable tool on top of a Ø25 bar. Under ~30 mm of band ⇒ a shorter holder or lower rotary is a prerequisite purchase |
| What does stocksim cost on bar-shaped stock? | `verify_toolpath_against_mesh` on a 34 × 34 × 84 mm box at 0.5 mm. Past the stator's 62 MB / 25 s, M1 needs the narrow-band stock `cam-claims-m0.md` M3 already calls for |
| Is a cantilevered cut inside ±0.1 mm? | Cut a 10 mm flat at 40 mm stickout with and without a tailstock, measure the taper. Sets the stickout rule the fit check refuses on |
