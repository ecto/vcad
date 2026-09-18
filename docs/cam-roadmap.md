# CAM roadmap

Working spec for taking vcad's CAM from "a contour came back" to "a part came
off the machine". Born from the first real cut (2026-09-17, see
`docs/native-app-friction-log.md` items 32–54): everything that mattered that
day was caught by two scripts outside the app. The rule this roadmap serves:

> **The app has to be able to say no by itself.** Every job is replayed against
> what it is meant to make before it can run, and what cannot be verified is
> refused, not assumed.

Integration branch: `claude/cam-roadmap` (stacked on #905). Work packages land
on `cam/<wave>-<name>` branches and are merged here by the integrator.

## House rules (every work package)

1. **Tests assert geometry, never existence.** "The toolpath is non-empty" is
   how an inside contour that cut outward shipped. Assert sides, distances,
   depths, metal left. Mutation-check the key test of each feature: break the
   implementation on purpose, confirm the test fails, restore.
2. **Fail closed.** A case the code cannot handle is an error with a message a
   machinist can act on, never a silently shorter or different path.
3. **Own your files.** Touch only the files listed for your package, plus
   one-line `pub mod` / `pub use` additions in `lib.rs` / `operation.rs`. New
   error kinds go in your own module's error type unless told otherwise.
4. **Build only your crate** (`cargo test -p <crate>`, never `--workspace`);
   disk is tight. Lint in the CI form:
   `cargo clippy -p <crate> -- -D warnings` and `cargo fmt -p <crate>`.
5. Never commit `packages/kernel-wasm/vcad_kernel_wasm*`. Do not push. Commit
   on your branch with a message that says what and why.
6. Units are mm, Z up, stock top Z0, stock frame XY lower-left at 0. Feeds are
   mm/min. `#![warn(missing_docs)]` applies. Match the surrounding style:
   comments explain *why*, not what.
7. Fixtures: `docs/cam-fixtures/stator-outline.dxf` (1 outer + 4 holes; the
   bore-and-12-slots loop has 3.87 mm slot mouths and R1.05 fillets) and
   `docs/cam-fixtures/gears-60-cnc.json` (module 1.0, 10/20/50 T, cut with a
   Ø1 end mill).

## Wave 1 — kernel (parallel; `crates/vcad-kernel-cam` unless noted)

| Package | Owns | Delivers |
|---|---|---|
| `w1-contour` | `operation/contour.rs`, `error.rs` | Rough + finish (stock-to-leave roughing, full-depth finish pass, optional spring pass), climb/conventional choice per side, ramp-along-contour entry and tangential lead-in/out for the finish pass, tabs on inside contours, bottom allowance (onion skin, +/-: negative = break-through into a declared spoilboard), thin-slot centreline fallback when the inward offset collapses because the cutter is about as wide as the slot |
| `w1-arcfit` | new `arcfit.rs` | `fit_arcs(&Toolpath, tolerance) -> Toolpath`: runs of collinear/cocircular G1s become G1/G2/G3 within tolerance, never across a Z change, never widening a gouge; posts unchanged |
| `w1-drill` | new `operation/drill.rs`, new `job.rs`, `tool.rs` | Drill / peck / spot ops; multi-tool job assembly (tool order, `M0` pause + re-zero prompt between tools for machines without a changer, one spindle start per tool with a spin-up dwell); tool library fields (flute length, stickout, holder) and the checks that use them (cut depth vs flute length, holder vs stock top) |
| `w1-verify` | new `verify2d.rs`, new `fit.rs` | 2D replay oracle for prismatic jobs: gouge, material left, rapids below the stock top with XY motion, depth vs stock (+ bottom allowance), envelope vs machine travel given a work offset, tab audit (where, metal width, height, on every pass that reaches them). Cutter-fit report: unreachable corners (count, area, max stand-off), necks the cutter cannot pass, slot clearance per side. Serializable report structs |
| `w1-outline` | new `outline.rs` | Contours from a solid: section a triangle mesh (welded or soup) at a Z plane, chain to closed loops with tolerance, classify outer/holes by nesting, simplify, detect circles/arcs; plus silhouette-from-above for parts that are not constant-section. Output feeds `Contour` directly |
| `w1-materials` | new `materials.rs` | Material table (6061, 7075, C360 brass, copper, mild steel, MDF, plywood, hardwood, acrylic, POM, FR4) × tool diameter/flutes → chipload; recommended feed, plunge, stepdown, stepover for a machine rigidity class and an rpm; router-dial ↔ rpm tables; warnings (e.g. FR4 dust, copper/aluminium need lubricant, Ø ≤ 1 mm depth limits) |
| `w1-gear` | new `gear.rs` | From (module, teeth, profile shift, pressure angle, backlash, cutter Ø, internal/external): exact involute tooth-space contours with the root the cutter leaves, reachability check (form radius vs lowest contact radius, space width vs cutter), tooth-thickness compensation from a measurement over pins, contact-ratio / interference claims. Must reproduce `gears-60-cnc.json` |
| `w1-union` | `crates/vcad-kernel-booleans/**` (research) | The stator's open item: unioning a many-lump operand into the ring loses caps on the analytic path, so the part falls back to triangle soup. Exit: stator solves to 7848 mm³ ± 0.5 % as a B-rep (not soup) with no regression in the boolean/torture suites. If not achievable, a written diagnosis is the deliverable |

## Wave 2 — plumbing

- Pure-Rust polygon offsetting so `wasm32` stops using the bounding-box
  fallback in `Contour2D::offset_contour` (MCP/web get real contours).
- `crates/vcad-ffi/src/cam.rs` request schema for everything in wave 1;
  header sync; `vcad-kernel-stocksim` 3D verify wired for non-prismatic jobs.
- `vcad.cam-claims/1` receipts (predicted → Provisional; a measured part
  closes it).

## Wave 3 — app + MCP

Inspector/ops model (rough/finish, entries, direction, skin, inside tabs,
material + apply-to-all, real op names, reorder, pilots first, re-evaluate
skipped holes on tool change) · verify gate + cutter-fit UI in the run blocker ·
contour-from-solid import · stock / zero / placement / rotation / clamps as
keep-outs, sweep envelope drawn with the tool · tabs visible and draggable ·
machine profile (`$$` baseline diff, envelope vs travel pre-check) · trace
bounds + dip-at-corners · probing with plate thickness, two-point skew probe →
rotate the job · send-to-ncSender + camera tile · menu items, accessible
popovers and number fields, status dump · MCP CAM tools (generate, verify,
export).

## Wave 4 — reach

Prerequisite before any 3D or indexed work: `verify2d::parse_gcode` refuses
`G18`/`G19` (fail closed — a 2D oracle cannot vouch for an out-of-plane arc,
and `job.rs` pins the refusal). The posts already emit the plane word when an
arc leaves XY; the reader, or the 3D oracle in `vcad-kernel-stocksim`, has to
learn to replay them first.


3D finishing in the app (kernel roughing/drop-cutter exist) · gear measurement
loop (pins → compensation → re-cut) · indexed 4th axis (design first).

## First milestone: one brass planet

20T module-1.0 planet from C360 plate, Ø1 cutter, verified in-app, measured
over pins within tolerance.

## Status (2026-09-17)

- **Wave 1 — merged:** contour strategies, arc fitting, drill/helical bore +
  tool checks + job assembly, 2D verify oracle + cutter fit (+ multi-op
  follow-up), contours from the solid, materials/feeds, gears (+ flank-deviation
  grading), union diagnosis.
- **Wave 2 — merged:** consolidation (pure-Rust offsetting everywhere, one
  stock model, `Pause`, honest arc lengths, accel-aware time), FFI job schema
  (fail closed: a blocked job has no G-code), pocketing, `vcad.cam-claims/1` +
  3D job verification. **Open:** export-repair shape guard + tangent seam fix
  (`cam/w2a-union-shape`, `cam/w2b-union-seam`).
- **Wave 3 — running:** app job pipeline + verify gate; shared `vcad-cam-api`
  crate + WASM + MCP `cam` tools. Queued behind them: materials/from-solid/
  stock-and-zero UI, machine profile + trace/probe/skew, ncSender + camera,
  menus/accessibility/status dump, receipts through MCP `build_receipt`.
- Found along the way, by running real jobs rather than package tests: a
  simplifier whose tolerance was not a bound, a 77k-move stator job, phantom
  tabs at ramp starts, an oracle that failed the job that was actually cut, an
  export repair that tore the stator by 0.68 mm under a 1 % volume check.

## Status (2026-09-18, afternoon)

- **Merged today:** web CAM panel (typed `@vcad/engine` client, a refused job
  types its `gcode` as `never`); receipt follow-ups (deposits refuse a missing
  basis, one re-state path for both receipt tools); planet rehearsal +
  `vcad cam <job|verify|fit|outline|compare|materials|recommend|check-feeds|gear>`
  (exit 2 is an honest no; nothing exiting 2 leaves a program on disk);
  two-sided export shape guard; union round 2 (twin-pair flap collapse,
  over-used edges 95 → 55); app follow-ups (tool list + drills + one `M0`,
  NSAccessibility bridge, readiness in the Machine stage); the integrator's
  live look (friction 68–70: refusal wording, thickness from the solid).
- **The real stator, at this tip:** `vcad cam outline stator.loon` refuses at
  every height — two 0.014–0.015 mm gaps at the tab roots, a full-height
  crack in the wall — so the DXF path is the only way to machine it until the
  seam lands (union round 3). Through the DXF the app builds 5530 moves,
  18:13, replayed clean; `verify_nc` on the headless dump passes.
- **Planet milestone** (`docs/planet-milestone.md`): the 20T brass planet
  cannot hang on tabs (tip land 0.79 mm) and must be held by a screw through
  the bore, which there is no way to declare except downgrading
  `loose_pieces` to a warning. Peck drilling is refused by the oracle (G83's
  rapid back down the hole is a "rapid below stock top"). Both are oracle
  work, in the review-fix round.
- **Also merged before the pause:** the review fixes (16 of 17 findings, each
  with a mutation-checked test; the oracle now clips rapids against the
  blank, allows a rapid back down a hole this program already cut, drops
  micro-lifts that are not tabs, reads the real depth past a break-through;
  `M6` is refused on Grbl; a refused answer carries no program anywhere —
  including the claim deposit's basis, which is now the program's digest)
  and union round 3 (seam collapse for curved-vs-curved tangencies; the
  retraction that the tab corner *is* the cylinder–plane family).
- **Gates at the pause (`ca1b2331`):** cam 348, cam-api 35, cli 40, ffi 63,
  registry 16, tessellate 74, booleans green, Swift 184, engine 138, app 92,
  MCP 1165, `ir:check` current; torture track 709/752 with 25 improvements
  and one regression (`chain-13` bad-geometry → timeout, present since union
  round 2, measured under load).

## What is left (2026-09-18, at the pause)

Kernel: the tab-root seam (give planar carriers a real span, then re-run
`zz_seam_probe::probe_stator_l2`; until then the stator is machined through
its DXF only), `MERGE_GENERATORS` → lens-depth epsilon, a bounded union
referee with a cannot-judge outcome, one tangency epsilon, arc fitting that
knows its side (finding 17: mechanism written and reverted for want of a
fixture), `chain-13` re-timed alone. CAM: workholding as something a job can
declare (the planet is held by a screw through its bore; today that is a
policy downgrade), a mid-job pause, `cam_verify_gcode` forwarding
`placement` / `verify_policy`, the planet actually cut and measured over pins.
Receipts: the seven solver tools' deposits with real basis keys (`spec` is a
placeholder). App/MCP: `volumeReport()` at every call site that reports a
volume, stock thickness from the solid is in, the web CAM panel has not been
looked at live. Wave 4: 3D finishing in the app; the indexed 4th axis waits
on the hardware gate (is A driven, does a rotary fit under 100 mm of Z).
Process: a PR from `claude/cam-roadmap` to `main` (CI will show `chain-23`
regressed by design under the shape guard; the agent branches were never
pushed).

## Review (2026-09-18)

An independent read of `crates/` on the branch (~42k lines) found 23 items.
Two critical, both confirmed by reproduction and fixed on the integration
branch the same morning:

- **Grbl dwell was posted in milliseconds.** Grbl's `G4 P` is seconds, so the
  3 s spin-up after every `M3` was a 50-minute park with the spindle on, and
  the two tests that touched the line pinned the wrong number (`"G4 P3000"`,
  `contains("G4")`). The oracle discards `P` on `G4`, so it could not see it.
  House rule 1, again: the assertion was of existence.
- **A request without an `options` map skipped verification.** serde only
  runs field defaults when the map is present; the derived `Default` said
  `verify: false`, and the job came back `blocked: false` with G-code nothing
  had replayed. Unknown keys on the request are now refused too.

The rest are on `cam/w3-review-fixes` (rapids clipped against the stock, the
tool gate reading the real depth past a break-through, `centre_cutting`
reaching the plunge check, G18/G19 arc words, `M6` refused on Grbl, no `moves`
on a blocked job, NaN spoilboard, DXF bulge parse, side-aware arc fitting,
`G91.1`, one sag-adaptive linearizer, dropped errors in materials, the
`Wall::escape` perf cliff, and the vacuous tests). Held for the booleans
round, since that code is under change: the export shape guard is one-sided
(surface added is not bounded, so a slit bridge can cap a real 1.5 mm slot
mouth), `parallel_cylinders` merges below an absolute 0.05 mm with no
`DegradeReason`, the union referee's reference-uncertainty term can
desensitise it ~7×, and the tangency epsilons are absolute, duplicated and
untested.
