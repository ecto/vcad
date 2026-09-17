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

3D finishing in the app (kernel roughing/drop-cutter exist) · gear measurement
loop (pins → compensation → re-cut) · indexed 4th axis (design first).

## First milestone: one brass planet

20T module-1.0 planet from C360 plate, Ø1 cutter, verified in-app, measured
over pins within tolerance.
