# One brass planet

The CAM roadmap's first milestone — *20T module-1.0 planet from C360 plate, Ø1
cutter, verified in-app, measured over pins within tolerance* — rehearsed end to
end, headless, before any metal is cut.

Reproduce the whole thing with:

```bash
cargo build -p vcad-cli
node docs/cam-fixtures/planet-rehearsal.mjs        # writes target/planet-rehearsal/
```

Every number below comes out of `vcad cam`. The script asserts each one and
exits non-zero if any step does not come out as expected, so this document and
the code cannot drift apart quietly.

**The headline: the part is geometrically fine and the program posts, but three
of the five things the milestone asked for were refused, two of them correctly
and one by a defect.** Those refusals are the findings, and they are below.

---

## `vcad cam` — the command

```
vcad cam <job|verify|fit|outline|compare|materials|recommend|check-feeds|gear>
         [--request FILE | stdin] [--out FILE] [--json | --summary]
```

Every subcommand is a thin wrapper over `vcad-cam-api`, so this, the desktop
app's C ABI, the browser's WASM and an agent's MCP tools can never disagree
about whether a job is safe to run. A human summary is printed by default;
`--json` prints the whole answer document, and `--out` writes it whatever the
verdict — a refusal is the answer worth keeping.

`job` takes `--gcode FILE` and **writes it only when the job is not blocked**.
`verify` takes `--gcode FILE` to replay a program off disk. `outline` takes a
`.vcad`, `.loon` or `.stl` positionally, plus `--part` and `--z`. `compare`
takes `--dxf` and `--outline` (a `cam outline` answer, or a model that is
sectioned on the way in) and `--tolerance`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | the answer is yes |
| 1 | the request could not be **read** — no such file, not JSON, not a sectionable document |
| 2 | it was read, and the answer is **no** |

The line is whether anything was understood. A 1 means "fix your request"; a 2
means the CAM surface read it and said no — a failed check, a cut past the
stock, a cutter that does not fit, a torn section, outlines that disagree, a
flank the cutter cannot reach. **Nothing that exits 2 leaves a program on
disk.**

### `outline` sections the raw tessellation

A `.vcad`/`.loon` is evaluated exactly as `export` and `info` evaluate it, and
then the **raw tessellation of the B-rep** is sectioned — never the export
mesh, whose repair pass moves vertices onto their analytic carriers and (on the
stator, 2026-09-17) sections up to 0.4 mm from the wall the cutter follows. The
answer says which mesh was used in `mesh_source`; a part with no B-rep behind it
(an imported mesh, a root-mesh cache hit) reads `export_mesh` or
`cached_root_mesh` rather than pretending. Run `vcad --no-cache cam outline …`
when you need the B-rep guaranteed.

A section that does not close is **refused with its gaps** — the solid is torn
there, and healing the outline would hide it.

---

## 1 · The gear

`vcad cam gear --request docs/cam-fixtures/planet-gear.json`

| | sun 10T | planet 20T | ring 50T (internal) |
|---|---|---|---|
| profile shift | +0.47 | 0.00 | +0.47 |
| pitch radius | 5.0000 | 10.0000 | 25.0000 |
| base radius | 4.6985 | 9.3969 | 23.4923 |
| tip radius | 6.4500 | 10.8900 (nominal 11.0000) | 24.4700 |
| root radius, nominal → as the Ø1 cutter leaves it | 4.2200 → 4.2200 (`RootCircle`) | 8.7500 → 8.7500 (`RootCircle`) | 26.7200 → 26.5584 (`Flanks`) |
| form (fillet tangency) radius | 4.6934 | 9.2365 | 26.2787 |
| space width at the root | 0.9365 | **1.1399** | 0.4523 |
| tooth thickness at pitch | 1.8829 | 1.5408 | 1.1987 |
| tip land | 0.2250 | **0.7860** | 0.8284 |
| **reachability at Ø1** | OK, margin **+0.067466** | OK, margin **+0.178061** | OK, margin **+0.000189** |
| flank deviation at the contact limit | 0.000000 | 0.000000 | 0.000000 |
| largest cutter that still clears | Ø1.1095 | Ø1.2369 | **Ø1.0002** |

All three members are reachable with a Ø1 end mill, well inside the 5 µm of
flank deviation the milestone allows — in fact with *zero* deviation: no
fillet crosses its contact limit at all. **The ring clears by 0.19 µm of
radius**, which is the tightest number in the train: the largest cutter that
still works on it is Ø1.0002, so Ø1 is not a choice, it is the only choice.

### Measurements to take off the finished part

| | |
|---|---|
| over pins Ø1.4 | **M = 21.0738 mm** (pin centre radius 9.8369, contact sensitivity dM/dt = 3.1776) |
| base tangent over 3 teeth | **W = 7.6322 mm** (anvils touch at r 10.1422) |
| tip diameter | Ø21.78 |
| bore | Ø4 H7 after reaming |

The recommended pin for this gear is Ø1.3711; Ø1.4 is what the shop has, and it
still seats on live involute, so it is what the fixture asks for.

### The ring's 1.36 µm, confirmed through the CLI

The roadmap recorded the ring as *graded* at 1.36 µm of flank deviation. That
number belongs to the ring **without backlash thinning**, and the CLI
reproduces it exactly:

```bash
# ring, no backlash thinning, strict → exit 2
vcad cam gear --request <ring-strict.json>
#   reachability: NOT REACHABLE — margin -0.033641 mm,
#   flank deviation 0.001356 mm at the contact limit, Exceeds
# the same request with "flank_tolerance": 0.005 → exit 0
#   reachability: OK — … WithinTolerance
```

With the 0.03 mm/gear backlash thinning the design actually calls for, the
thinner tooth widens the space, the fillet retreats, and the same ring comes
back **Clear with zero deviation and +0.000189 mm of margin**. Both verdicts are
right; they answer different questions, and the request has to say which.
`crates/vcad-cli/tests/cam.rs::the_ring_verdict_flips_on_the_flank_tolerance`
pins both.

> **Fixed along the way.** `flank_tolerance` was accepted by the `gear` request
> and then echoed back without being used: the verdict was always the strict
> one. `crates/vcad-cam-api/src/gearing.rs` now routes it to
> `PlanetaryTrain::reports_within`. Without that there is no way to ask the CLI
> for a graded verdict at all.

### The sun and the ring, reports only (no job)

| | sun — 10 blind pockets | ring — 50T internal, Ø59 blank, 6 mm |
|---|---|---|
| ideal space width at the nominal root | 0.9365 mm | 0.4523 mm |
| root binding | `RootCircle` (the cutter reaches the full depth) | `Flanks` (the flanks close before the root does) |
| effective root radius | 4.2200 (= nominal) | 26.5584 (0.1616 shy of the nominal 26.72) |
| `cam fit` at Ø1, inside the space the cutter leaves | fits, largest Ø1.6921 | fits, largest Ø1.1312 |
| verdict | cuttable | cuttable, with 0.16 mm of root the cutter cannot reach — which is below the form radius and out of contact, so it does not matter |

Both are reports. Neither is posted as a job here: the ring is not cut from this
setup, and the sun's spaces are blind pockets on a different blank.

---

## 2 · Feeds

`vcad cam recommend` for `brass-c360`, Ø1 2-flute flat end mill, hobby gantry,
dial router:

| | slot (the spaces and the bore) | profile (the outside) |
|---|---|---|
| rpm | 30 000 | 30 000 |
| **router dial** | **6** | **6** |
| surface speed | 94.2 m/min (150 wanted; the spindle tops out) | 94.2 m/min |
| chipload | 0.0087 mm/tooth | 0.0087 mm/tooth |
| feed | 524 mm/min | 600 mm/min |
| plunge | 157 mm/min | 180 mm/min |
| ramp angle | 3.0° | 3.0° |
| stepdown | **0.175 mm** | 0.500 mm |
| stepover | 1.000 mm | — |
| coolant | dry | dry |

The **60 % first-cut derate is the hobby machine class**, applied by the table
itself: 0.0146 mm/tooth × 0.60 = 0.0087. The `S` word does nothing on this
spindle, so the dial position is the recommendation and every feed above assumes
it is set by hand.

> The drawing note says "3 passes" through the 5 mm face. At the recommended
> 0.175 mm that is **30 passes per space**, and the note's plan is 9.9× the
> stepdown a hobby gantry is given for a full-width Ø1 slot in brass. The
> rehearsal uses 0.175.

---

## 3 · The job, and the four refusals

The job: a 30 × 30 × 5.0 mm C360 plate over a 6 mm sacrificial board, part
centred at X15 Y15, one Ø1 2-flute carbide cutter, arc fitting at 0.01 mm,
verification on. 23 operations: the bore, then 20 tooth spaces, then the outside
profile. Each cut goes 0.2 mm past the underside into the board.

### FINDING 1 — a helical bore cannot open Ø3.8 with a Ø1 cutter

```
REFUSED: the job could not be assembled: operation "bore Ø3.8 (Ø4 H7 after
reaming)" could not be generated: a Ø3.800 hole with a Ø1.000 cutter leaves a
Ø1.800 core standing in the middle: a helix only opens a hole under twice the
cutter diameter, so pocket it instead
```

Correct, and it says what to do. **Exit 2, no program written.**

### FINDING 2 — the pocket it told us to use is blocked by a phantom tab

The same opening as a circular pocket is blocked by the tab audit: every pocket
pass ends with a **0.00200 mm lift at the pocket centre**, each one is recorded
as a tab with **−0.998 mm of metal under it**, and the check then reads every
deeper pass as cutting through it. 31 violations on one bore.

A metal width is `lifted_run − tool_diameter`. A negative one means the lifted
stretch is narrower than the cutter, which cannot be a tab — it is a numerical
stub. Guarding the observation on `metal_width > 0` in
`crates/vcad-kernel-cam/src/verify2d.rs` would drop every one of these and keep
every real tab (they measure 2.0 mm). Not fixed here: that file belongs to
another package.

The job bores with a **Ø1.9 helical pilot** (under twice the cutter diameter, so
no core) **opened out to Ø3.8 by an inside contour** instead. Same hole, no tab
observation at all.

### FINDING 3 — the ramp entry mints a phantom tab per tooth space

The job **as designed** — a ramped entry on every space, which is what a Ø1
carbide cutter wants in brass — is **BLOCKED**, and not by the part:

```
REFUSED: tabs
  tabs: 581 violation(s), worst 5.0690 — metal left under each tab must be at
  least 1.50 mm, on every pass that goes below it
```

Twenty phantom tabs, one per space: **0.00728 mm ramp stubs with −0.993 mm of
metal under each**. Every other check passes — 0 gouge, 0 material-left, 0
depth, 0 envelope, 0 rapids. The stub is the level move the ramp lays down at
the previous pass's floor on the way in; `verify2d`'s lifted-run detector drops
such a stub only when it is the *first* run of a pass, and here it is not.

Worse than blocking: with twenty stubs on the profile's own clustering radius,
the three **real** tabs are absorbed into them and disappear from
`tab_placement` entirely, so the report can no longer show the operator the tabs
that are actually cut.

`entry: "plunge"` on the spaces removes the phantom completely (tab count 3,
2.000 mm of metal each). That is the workaround the posted job uses, and it is a
workaround, not a preference: **re-post with `entry: "ramp"` once the guard
lands.** Both entries were run; the ramp is the better cut.

### FINDING 4 — this gear cannot hang on tabs

With the spaces plunged, the audit answers the real question and still blocks:

```
tabs: 20 violation(s) — the pass at Z-5.200 cuts straight through the tab at
(20.69, 5.14), whose top is Z-4.500
```

A tooth space runs out past the tip circle at the same angle as a tab, so the
space pass cuts the tab away. Moving the tabs onto tooth tips does not help: a
tab wants at least 1.50 mm of metal under it and **this gear's tooth tip land is
0.7860 mm**. There is nowhere on a Ø21.78 20-tooth gear to hang a tab.

So the part is held the way the drawing implies: **a Ø4 screw through its own
bore**, which is why the bore is phase 0 and is cut before anything else.

### FINDING 5 — workholding cannot be declared

With no tabs the profile frees the part, and `loose_pieces` is an **error** when
what comes free is the part rather than waste: 294.42 mm² at the centre, "no
tab, no skin". That is the right answer to the question it can ask. What it
cannot be told is that the part is **screwed down** — there is no way to declare
workholding to the oracle. The only way to say "yes, I know, it is bolted
through its own bore" is `verify_policy: {"loose_pieces": "warning"}`, and
**that override is the workholding claim**. It is the one line of the request a
reviewer should look at hardest.

The alternative that needs no override is an onion skin: a positive
`bottom_allowance` on every operation leaves the part on 0.15 mm of brass to be
sanded off. On a 20-tooth gear that is twenty slots to clean up by hand.

### Two more things the schema cannot say

- **A mid-job pause.** The bore has to be cut, then the screw fitted, then the
  spaces cut. `ToolpathSegment::Pause` exists in the kernel, but the only way to
  emit one is a tool change, and this job has one tool. The operator has to stop
  the program by hand between phase 0 and phase 1.
- **`verify_gcode` had no `placement` and no `verify_policy`** — both present on
  `job`. A program posted from a placed job replayed against a part still at the
  origin, so every cut read as a gouge; and a program posted under a policy
  could not be re-verified under that policy. Both are now additive fields on
  the verify request (`crates/vcad-cam-api/src/verify.rs`), defaulting to
  today's behaviour. Without them the two entry points give different answers
  about the same file, which is the one thing that crate exists to stop.

### A design correction the oracle found

The tooth space `gear` hands back is **closed across its mouth** by an arc of the
tip circle. Cut as a closed inside contour it leaves the top of both flanks
standing, because the cutter centre can never come within its own radius of the
mouth. The first run was refused for exactly that: **40 patches of 0.147 mm²,
standing up to 0.578 mm proud**, two per tooth.

`cam fit` says the same thing more quietly on a single space: `fits: true`, and
**2 unreachable corners of 0.152 mm² each at 0.226 mm of stand-off**, both at
r ≈ 10.72 — the two ends of the mouth. The job's `material_left` is the check
that refuses on them.

The fix is a machining one, not a tolerance one: push the two mouth points
radially out to one cutter diameter past the tip circle (r 11.89) and drop the
mouth arc, so each space becomes a channel the cutter drives into from outside
the gear. Radial rays keep the angles the mouth points already had, so the tip
land is untouched, and everything past r 10.89 in a space's own sector is waste
the profile pass takes anyway. After that: `material_left` 0 violations.

### The posted program

Phase 0 bore, phase 1 the twenty spaces, phase 2 the profile.

| | |
|---|---|
| verdict | **not blocked** |
| operations | 23, one tool (T1) |
| passes per space | 30 at 0.175 mm |
| preview moves | 12 100 |
| G-code | 10 394 lines |
| arc fit | 29 578 segments in → 10 959 out, 1 282 arcs, worst deviation **0.00842 mm** (tolerance 0.01) |
| cutting time | 2 656 s ≈ **44 min** (acceleration-aware; naive 2 508 s) |

| check | verdict | n | worst |
|---|---|---|---|
| gouge | pass | 0 | 0.0000 |
| material_left | pass | 0 | 0.0000 |
| rapids | pass | 0 | 0.0000 |
| depth | pass | 0 | 0.0000 |
| tabs | pass | 0 | 0.0000 |
| envelope | pass | 0 | 0.0000 |
| loose_pieces | **fail, demoted** | 21 | 294.42 mm² |
| plunges | pass | 0 | 0.0000 |

`loose_pieces` is the declared workholding (finding 5). Everything else passes
on its own merits.

### `verify`, from disk

The posted `.nc` read straight back off disk and replayed against the same part,
under the same policy: **15 416 moves, accepted, the only demoted check
`loose_pieces`.** The job and a fresh replay of its own output agree.

---

## 4 · The outline, round-tripped

The gear's own profile, extruded 5 mm into an STL, sectioned back out with
`cam outline`, and compared against the profile it came from:

| | |
|---|---|
| section | z 2.5 of a 0.0 … 5.0 part, closed, no healing |
| area | 305.54 mm² |
| prismatic | yes |
| `cam compare` vs the gear's own profile | **agrees**, boundaries at most **4.7 × 10⁻⁷ mm** apart, tolerance 0.02, no unmatched holes |

Seven orders of magnitude inside the 0.02 mm the milestone asks for. The
comparison runs through the real section code rather than comparing a list
against itself.

---

## 5 · The measurement loop

`gear`'s `compensation` turns a reading into the next cut's offset. Worked both
ways off the nominal M = 21.0738 with Ø1.4 pins:

| measured M | thickness error | move the cutter (flank normal) | radially |
|---|---|---|---|
| 21.0938 (+0.0200) | +0.0063 mm (too thick) | **−0.00297 mm** | −0.00869 mm |
| 21.0538 (−0.0200) | −0.0063 mm (too thin) | **+0.00294 mm** | +0.00861 mm |

The sign is the thing: teeth too thick → move the cutter *into* the material.
Feed the offset back as `offset` on each tooth-space operation and re-post.

---

## The shop sheet

**Blank.** 30 × 30 × 5.0 mm C360 plate, over a 6 mm MDF sacrificial board.

**Zero.** Top of the stock; part centre at X15 Y15. Work offset G54.

**Cutter.** Ø1.0 2-flute carbide, 6 mm flute length, 12 mm stickout, 3.175 mm
shank. **Set the router dial to 6 (≈ 30 000 rpm) by hand** — the `S` word in the
program does nothing on this spindle.

**Order.**

1. **Bore** — Ø1.9 helical pilot, then out to Ø3.8 by contour. 0.2 mm through
   into the board.
2. **Stop the program.** Fit a Ø4 screw through the bore into the board. This is
   the only thing holding the part from here on.
3. **Twenty tooth spaces** — 30 passes each at 0.175 mm, 524 mm/min, climb,
   plunge entry (see finding 3).
4. **Outside profile** — Ø21.78, 0.5 mm per pass, 600 mm/min, climb, no tabs.

**After the cut, measure:**

| | nominal | how |
|---|---|---|
| over pins Ø1.4 | **21.0738 mm** | two Ø1.4 pins in opposite spaces, micrometer across |
| span over 3 teeth | **7.6322 mm** | gear-tooth calliper / disc micrometer |
| tip diameter | **21.78 mm** | micrometer across two opposite tips |
| bore | **Ø4 H7** | ream after the mill; check with a plug gauge |
| face | **5.0 mm** | = plate thickness, no facing cut |

**Turning a measured M into the second cut.** Feed the reading back:

```bash
jq '. + {measured: 21.0938}' docs/cam-fixtures/planet-gear.json | vcad cam gear
```

and read `compensation`: `tool_normal_offset` is the number to put in each
tooth-space operation's `offset` field. Negative cuts deeper. A +0.020 mm
reading on M is +0.0063 mm of tooth thickness and asks for −0.00297 mm.

**What the receipt says.** Before the cut, every claim on this part is
*predicted*: the reachability verdicts, M, W, and the verification checks. A
measured M closes the over-pins claim — that is the one number that turns the
milestone from "the program verifies" into "the part is right". Until then the
`vcad.cam-claims/1` family stays Provisional, and the honest reading of this
document is **"vcad says this program makes this part", not "this part exists"**.

---

## Fixtures

| file | what |
|---|---|
| `docs/cam-fixtures/planet-gear.json` | the 6:1 train as a `vcad cam gear` request, graded at 5 µm |
| `docs/cam-fixtures/planet-blank-job.json` | the planet *blank* — Ø21.78 disc + bore, no tooth spaces — small enough to live in a test |
| `docs/cam-fixtures/planet-blank-verify.json` | its companion verify request |
| `docs/cam-fixtures/planet-rehearsal.mjs` | the whole rehearsal, asserting every number above |
| `crates/vcad-cli/tests/cam.rs` | the CLI integration tests, including the exit-2/no-file rule |

The 23-operation job is not checked in: it is 20 congruent copies of one
contour, and the rehearsal builds it from the `gear` answer (and proves the
congruence — space 7 is space 0 turned 126.0°, to 4 × 10⁻¹⁵ mm).

## What is still open

1. **Guard `verify2d`'s tab observations on a positive metal width.** One
   condition. It unblocks the ramped job (finding 3), the circular pocket
   (finding 2), and restores the real tabs to `tab_placement`.
2. **Declare workholding** in the job request — a screw, a clamp, tape — so
   `loose_pieces` can be answered rather than overridden (finding 5).
3. **A mid-job pause** the request can ask for, not only between tools.
4. Re-post the planet with `entry: "ramp"` once (1) lands, and cut it.
5. Measure a real part over Ø1.4 pins and close the claim.
