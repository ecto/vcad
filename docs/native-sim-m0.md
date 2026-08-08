# Native simulation — M0 through M4

Physics simulation, policy inference, and RL training inside the native
(macOS / visionOS) app, over a C ABI onto `vcad-kernel-physics` and `vcad-sim`.

Before this, the app's only motion was `vcad_scene_solve_fk` driven by authored
joint tracks — kinematic playback, no dynamics and no contact. The Rust side
already had everything (`RobotEnv`, `SimPipeline`, ARS with an MLP policy); the
gap was exactly one seam, the C ABI.

## What shipped

| Milestone | Contents | State |
|-----------|----------|-------|
| **M0** — the seam | `vcad_gym_*` (env lifecycle, stepping, introspection, render seam), `vcad_policy_*` (inference), `vcad_last_error` (diagnostics), golden-trajectory parity harness | done |
| **M1** — alive in the app | Background sim engine, transport (run / pause / step / reset / shove), live telemetry, auto-configuration from the document. Present in **both** the studio and released-to-desktop modes | done |
| **M2** — policies as artifacts | `.vcadpolicy` bundles with provenance, staleness detection against an edited model, in-Rust inference | done |
| **M3** — GPU batch | `vcad_batch_*` over `BatchSimPipeline` | **not started** — gated on CPU/GPU parity in phyz |
| **M4** — training console | In-process ARS on a worker thread, held-out selection, live reward curve, save / adopt best | done |

M3 was deliberately skipped rather than attempted: GPU contact landed only days
ago (phyz#35 / vcad#772) and CPU/GPU parity is still open there. Everything
above rides the proven CPU path.

## Architecture

```
Document (.vcad JSON)
        │
        ▼
crates/vcad-ffi/src/gym.rs      ── VcadGym  ──► RobotEnv (vcad-kernel-physics)
crates/vcad-ffi/src/train.rs    ── VcadTrainer ► ARS      (vcad-sim::rl)
        │  C ABI
        ▼
apple/VcadApp/Sources/VcadApp/
    Sim.swift            RAII handles + Codable spec mirrors
    SimController.swift  SimEngine (private serial queue) + @Observable model
    SimPanel.swift       transport bar + inspector + training console
    ReleasedDesktop.swift  SimulationWindow — the released twin, wrapping the
                           same SimInspector so the two cannot drift
```

Both modes are first-class. `SimBar` and `SimInspector` are defined once and
hosted twice: in the studio as a bottom transport plus an inspector section, and
in released mode as a floating transport plus a BCB tool window. Every readout,
control and fail-closed message is therefore identical between them by
construction rather than by discipline.

The render seam is the load-bearing design decision: `vcad_gym_scene_transforms`
writes **column-major millimetre 4×4s in scene-instance order** — byte-identical
in layout to what `vcad_scene_solve_fk` already produces. So the renderer draws a
physics rollout and a kinematic playback through the same code path without
knowing which it got. Both land in `EditorModel.instanceTransforms`.

## Things that are easy to get wrong here

Each of these is a bug that produces *plausible motion* rather than an error,
which is why they are enforced in code rather than documented as advice.

**Units.** `PhysicsWorld` works in metres, the vcad scene in millimetres. Forget
the conversion and the whole robot renders as a speck at the origin — which
reads as "the mesh failed to load", not as a unit bug. Pinned by
`body_transforms_are_millimetres_and_track_the_base`.

**dt.** A policy trained at one timestep and replayed at another sees a
different plant: the stiff leg gains a humanoid needs sit near their
explicit-integration stability limit at 1 kHz and diverge outright at 200 Hz.
`dt` and `substeps` are recorded in every `.vcadpolicy` bundle.

**Fixed base masquerading as floating.** A fixed-base document still has a base
*instance*, so its pose resolves fine — to a constant. Every height and tilt
reading is then constant, no termination can fire, and a run reports
full-length episodes while measuring nothing. `RobotEnv::has_floating_base`
checks for an actual 6-DOF joint; checking that `base_pose` resolved is the
test that looks right and is wrong.

**`termination: null` is not "no termination".** It selects the kernel's legacy
end-effector-below-ground rule, which is true on step one for anything standing
on the ground. With auto-reset on, that presents as a robot doing nothing while
the step counter flickers 0-1-0. Auto-configuration emits an *empty*
`TerminationConfig` instead.

**Two bottom-aligned overlays do not stack, they coincide.** `ViewportView`
and `EditorView` each attach one. `EditorView`'s is applied outermost, so the
transport bar drawn inside the viewport rendered *underneath* the composer and
the tool palette — every piece of state correct, nothing visible. The
pre-existing `PlaybackBar` had the same defect. Both transports now live in
`EditorView`'s bottom stack.

**A `guard ... else { return }` on a button's action is a button that does
nothing.** `enableSimulation` had three silent early returns and the inspector
was gated on `sim.isAvailable` — which `prepare` sets to `false` on failure,
making the error it had just recorded unreachable. Pressing Simulate could fail
in four distinct ways with no feedback at all. Every path now reports, and the
panel is gated on `canSimulate` (is this document simulable) rather than on
`isAvailable` (did it work).

**PD gains on a zero-DOF joint panicked the kernel.** A Fixed joint still has an
entry in `joint_v_offsets`, pointing one past the end of the control vector for
the last joint in the model, and the inertia probe indexed it. The ABI made it
reachable by validating gains against *all* joints rather than actuated ones —
the check that reads more naturally is the one that lets it through. Fixed on
both sides.

**A spawn height in the air makes every episode a fall.** The first shipped
sample was imported with `--spawn-height-mm 600`, so auto-configuration derived
a height floor of `0.55 x 0.6 = 0.33 m` and the robot crossed it within a
handful of steps — episodes ended at step 4–7, the viewport showed a robot
falling on loop, and training reported a collapsed `sigma` of 0.004 because
every direction scored identically on an env that terminated before the policy
had done anything. Measured across spawn heights from 0 to 600 mm, the arm
survives 400/400 at *every* one with tilt-only termination: it was never
tipping, only crossing a floor derived from where it happened to be dropped.
The sample is now authored resting on the ground, and the height floor is only
derived when the base genuinely starts elevated (> 0.2 m). Tilt is the signal
that generalizes — a robot past 45 degrees has fallen whether it stands two
metres tall or sits on the floor.

**Released mode rendered no assembly at all.** `ReleasedARView` walked only
`scene.meshes` — the root parts — and `scene.meshes` is empty for an assembly,
whose content is entirely `scene.instances`. Every robot document showed an
empty desktop. Separately, `updateNSView` only runs when one of the
representable's properties changes, so even once instances were built the view
would render them at their rest pose and never re-pose; a `poseTick` input
carries each published solve. The studio gets that for free from
`RealityView`'s update closure.

**Chrome in released mode must report its frame.** The window passes the mouse
through to the desktop wherever the hit test says the pixel is not ours, so a
panel without a `ChromeRegion` renders normally and then silently ignores every
click.

**A flag that does not do what its name says is worse than no flag.**
`--spawn-height-mm` applied only to a *synthesized* floating joint; a URDF that
already declares one kept its authored origin. The Booster K1's floating variant
authors `xyz="0 0 0"`, so the requested height was silently discarded, the robot
spawned at the world origin below its own termination floor, every episode ended
on step 1 — and the CLI printed `synthesized a 6-DOF root joint at z = 550 mm`
the whole time. The height now applies either way, and the CLI reports the
document's actual anchor rather than the request.

**Relative mesh paths are resolved at the boundary, not in the evaluator.**
`MeshImport` opens its path verbatim, so a relative path resolves against the
*process working directory*. `vcad_eval::resolve_mesh_paths` normalizes a
document once, where it is loaded from a known location, rather than threading a
base directory through evaluation — physics evaluates parts by its own path, and
every consumer would otherwise have to remember to pass it.

**Inference must not be reimplemented in Swift.** The forward pass has to match
training exactly — whitening, output clamp, default-pose offset, degree
conversion. A drift of one clamp gives a robot that almost stands, the hardest
kind of bug to attribute. `vcad_gym_policy_step` keeps the whole
observation → features → action chain inside Rust;
`a_zero_policy_reproduces_the_hold_rest_pose_trajectory` pins it.

**The trainer's own eval return is not a measure of an iterate.** On a
randomized env it selects for lucky draws — measured on the K1 standing task, it
picked an iterate worth 10.84 over one worth 35.40. Held-out selection is done
inside `vcad_train_start` rather than left to each caller, and the UI shows the
held-out score as the headline with `train-eval` visually demoted.

**A cancel flag that only skips bookkeeping cancels nothing.** `train_curriculum`
kept iterating regardless; `on_iteration` now returns `ControlFlow`, and
breaking is what actually stops the run.

**`document_hash` must not hash a `HashMap`'s iteration order.** `Document`
holds `HashMap` fields and Rust gives each map instance its own `RandomState`
ordering, so hashing a plain `to_string` made a document hash differently from
*itself* — every loaded policy would report Stale at random, an alarm that fires
constantly and is therefore ignored. Keys are sorted before digesting.

## Verification

- **Rust** — `crates/vcad-ffi/tests/gym_parity.rs` (13 tests) records a golden
  100-step trajectory of a 23-DOF floating-base humanoid falling under gravity;
  `train_smoke.rs` (6 tests) runs real ARS end to end.
- **Swift** — `apple/VcadApp/Tests/VcadAppTests/` (29 tests). `SimTests` loads
  the *same* golden fixture through the Swift wrapper, so a Swift-only failure
  localizes the bug to marshalling rather than the kernel. `SimControllerTests`
  drives the real app model, background queue included.

### On the golden's tolerance

Compared at `1e-8` relative, and the reason is not that the simulator is flaky —
bit-exactness *is* asserted, in `stepping_is_deterministic_for_a_fixed_seed`,
which compares two in-memory runs with no file in the path. The golden's
tolerance covers two measured effects:

- `serde_json` serializes an f64 faithfully but its **parser** does not
  round-trip subnormal-magnitude values (`-7.510773185222099e-19` reads back as
  `-7.5107731852221e-19`). Several base DOFs sit at 1e-17..1e-19. Worst
  same-profile deviation: **9.8e-17**.
- macOS/aarch64 golden vs Linux/glibc/aarch64, measured in a container against
  a clean clone: **3.8e-9**. That is the OS + libm dimension isolated
  (`sin`/`cos`/`atan2` differ by ~1 ulp between Apple's libm and glibc's,
  amplified over the 21 frames). The x86_64 dimension — FMA contraction and
  reassociation — is still untested; `1e-6` leaves ~260x headroom for it.
- Optimization changes floating-point codegen, and a falling humanoid is a
  divergent system that amplifies early rounding. Debug golden vs release run:
  **1.5e-9**. Pinning a build profile would only move the problem to the first
  machine with a different architecture — this repo already has a torture-track
  baseline that differs between x86_64 and aarch64 for exactly this reason.

`1e-8` on a 0.78 m height is 8 nanometres, with ~6× headroom over the observed
cross-profile figure. The assertion prints the worst deviation it saw, so drift
creeping toward the limit is visible before it trips.

## The K1 sample

`examples/k1-floating.vcad` is the 22-DOF Booster K1, with its meshes vendored
under `third_party/booster-k1` (BSD 3-Clause, 24 meshes, 12 MB). It renders —
236,523 triangles — and it simulates: 22 actuated joints, 58 policy features,
real foot-contact forces on both feet, and roughly 60 uncontrolled steps of
standing before it goes down, which is the standing task rather than a fall from
a bad spawn.

`examples/k1-stand.vcadpolicy` is a balance policy trained on it: **held-out
394.87 over 400.0 steps, 10/10 full episodes**, against a hold-rest-pose
baseline of 9.55 over 18.8. In the app it holds 0.5504 m and 1.1 degrees of
tilt indefinitely, redistributing weight between the feet (contact readings
swing roughly 90-190 N against a ~280 N total, which is the K1's 28.5 kg).
Retrain with:

```bash
K1_CURVE=5 K1_OUT=examples/k1-stand.vcadpolicy \
  cargo run --release -p vcad-sim --example k1_stand -- examples/k1-floating.vcad 150
```

That the vendored import reproduces the reference run's baseline exactly (9.55
over 18.8 steps) is the strongest evidence available that the meshes, inertias
and joint frames came across faithfully.

One gap: `k1_stand` writes its own `{policy, kept, log, config}` shape rather
than the `PolicyBundle` the in-app trainer emits, so a CLI-trained policy loads
and runs but carries no provenance — no `document_hash`, and therefore no
staleness check. Unifying them means moving `PolicyBundle` (and `GymSpec`) down
into `vcad-sim`, where the trainer that defines the artifact lives. Not done.

Two things to know about it:

- **It runs at real time (RTF 1.00x)** in both the studio and released modes.
  Getting there is written up below, because the obvious explanation was wrong.
- The document references its meshes **relatively**, which is what makes it
  committable. Re-import with `--relative-meshes` or it will only resolve on the
  machine that produced it.

## The XLeRobot sample

`examples/xlerobot.vcad` is [XLeRobot](https://github.com/Vector-Wangel/XLeRobot):
an IKEA RÅSKOG cart carrying two 6-DOF SO-101 arms and a pan/tilt head, meshes
vendored under `third_party/xlerobot` (Apache-2.0, 29 meshes, 4.8 MB). It
renders — 106,480 triangles — and it simulates: **17 actuated DOF**, 86
observation features, ~20k steps/s at dt = 1/200 s with 4 substeps.

It is the counterpart to the K1 rather than a second copy of it. The K1 is a
floating-base humanoid whose whole problem is not falling over; XLeRobot's base
is a *planar* chain (prismatic X, prismatic Y, continuous yaw) on a world-welded
root, so it cannot tip, cannot leave the ground plane, and never touches the
ground collider. The wheels are visual-only. Balance and tip-over questions are
outside what this model can answer; manipulation is the point.

```bash
cargo run --release -p vcad-kernel-physics --example xlerobot_probe -- examples/xlerobot.vcad
cargo run --release -p vcad-sim --example xlerobot_reach -- examples/xlerobot.vcad
```

**The URDF declares `effort="0"` on all twelve arm joints, so as imported the
arms are inert** — a zero effort limit saturates every controller output to
zero torque. Measured: a 40 N·m command moves the `Elbow` by exactly 0.0000°.
Upstream never notices because ManiSkill supplies actuator limits from its own
controller config and never reads the URDF's. The importer now warns on any
`effort="0"` joint; it does not invent a replacement. `xlerobot_reach` supplies
2.94 N·m — the Feetech STS3215's 12 V stall torque, corroborated by upstream's
own `gripper_force_limit = 2.8` — and then tracks a six-joint pose to **0.000°**
steady-state error. Starved to 0.02 N·m the same reach misses by **90.3°**,
which is what makes the first number mean something.

Two more caveats live in `third_party/xlerobot/README.md`: `base_link` is
authored at 70.13 kg against a real robot of roughly 10 kg (left as upstream
wrote it, so vcad and ManiSkill agree), and eleven links carry no `<inertial>`
— all of them pure sensor/TCP frames on fixed joints, which is ordinary URDF.

## Why the K1 ran at 0.29x, and what it actually was

Worth recording because the plausible answer was wrong, and acting on it would
have meant a substantial IR change for no gain.

The visible symptom was `RTF 0.29x` on the 22-DOF K1. The obvious cause: the
URDF importer takes each link's *visual* geometry and only falls back to
`<collision>`, so physics collides against 236,523 triangles of full-detail
visual mesh while the URDF's 12 collision primitives go unused. That is true,
and it is not the bottleneck.

Measured instead of assumed:

| | per control step | budget | ratio |
|---|---|---|---|
| Rust physics alone (release) | 3.32 ms | 20 ms | **6.0x** |
| Full app-model tick, headless (step + reward + scene transforms) | 6.74 ms | 20 ms | **3.0x** |
| Same tick, in the running app | ~69 ms | 20 ms | 0.29x |

So the simulation was never the problem — it had 3x headroom. The ~62 ms gap was
entirely in the app, from two independent ceilings that each capped the frame
rate at roughly the same place, so fixing either alone changed nothing:

1. **The app linked a *debug* kernel.** `build-ffi.sh` built and staged
   `target/debug/libvcad_ffi.a`. Debug physics steps the K1 at roughly 1/20th
   speed — about 66 ms against a 20 ms budget on its own. It now builds release
   by default (`CONFIG=debug` to opt out); the staged archive went from 555 MB
   to 74 MB as a side effect.
2. **A 50 Hz counter was read in the overlay's `body`.** `poseTick` exists so
   the released `NSViewRepresentable` re-poses each frame, but reading it in
   `ReleasedOverlayView.body` re-evaluated the *entire* overlay on every physics
   step — every tool window, the 24-row feature tree, the inspector, the
   training chart. SwiftUI's observation is per-view, so the read is now
   confined to a `ReleasedScene` wrapper around the ARView alone. Separately,
   `applyInstancePoses` called `findEntity(named:)` once per instance per frame;
   that walks the whole descendant tree, so it was quadratic in the model. The
   entities are captured at build time now.

The lesson is the one this repo already had written down: measure before
concluding. `apple/VcadApp/Tests/VcadAppTests/K1PerfTests.swift` keeps the
headless number honest, and `VCAD_SIM_PROFILE=1` breaks a live tick into
step / reward / publish.

Collision-vs-visual geometry remains a real (unfixed) modelling gap — the IR
carries one geometry per part — but it is a fidelity question, not a
performance one.

## Fixtures

`crates/vcad-ffi/tests/fixtures/` holds a 23-DOF Unitree G1 imported from the
upstream Unitree URDF, in floating-base and fixed-base variants, plus the
golden trajectory. Its meshes are not vendored, so masses and inertias are the
authored ones but the colliders are placeholders — see the note above. Both Rust and Swift read these same bytes.
The app's shipped sample is `examples/floating-arm.vcad` instead — imported
from the genuinely primitives-only `robot-arm-2dof.urdf`, so it renders. Regenerate with:

```bash
cargo run -p vcad-cli -- import-urdf examples/unitree-g1.urdf \
  crates/vcad-ffi/tests/fixtures/g1_floating.vcad --floating-base --spawn-height-mm 780
cargo run -p vcad-cli -- import-urdf examples/robot-arm-2dof.urdf \
  examples/floating-arm.vcad --floating-base --spawn-height-mm 600
UPDATE_GOLDEN=1 cargo test -p vcad-ffi --test gym_parity
```

Regenerate the golden **only** for an intended physics change, and say so in the
commit — an unexplained golden update is how a regression gets blessed.

## C header

`crates/vcad-ffi/include/vcad_ffi.h` is canonical. The two Swift-package copies
are **generated** by `scripts/sync-ffi-header.py`, which each `build-ffi.sh`
runs before building. Hand-syncing three copies of a C ABI is a miscompile
waiting to happen: C does no signature checking across the boundary, so a mirror
that lags the Rust declarations does not fail to link, it corrupts.

## Not done

- **M3, GPU batch.** Gated on CPU/GPU parity in phyz. The unfair advantage worth
  building when it lands: wgpu-on-Metal plus unified memory means batch state
  and render buffers share physical RAM, so a wall of simulated robots can be
  visualized with no readback.
- **Design↔behaviour loop.** `ScrubField` already drives
  `vcad_doc_set_param_cheap`; hooking parameter edits to env rebuild plus an
  automatic policy re-rollout would let a user drag a dimension and watch
  stability degrade live, with the policy receipt flipping Stale → Violated.
  `vcad-kernel-diff` + `phyz::diff` M11 already provide exact ∂J/∂p through
  contact, so a per-parameter sensitivity badge is reachable from here.
**A URDF whose meshes are missing simulates but does not render.** The kernel
takes mass, COM and inertia from the authored `<inertial>` blocks and falls back
to a placeholder only for the *collider*, so the dynamics stay faithful — but
every instance mesh is empty, `assemblyInstanceCount` drops to 0, and the
Simulate affordance vanishes with no explanation. `examples/UNITREE.md` claimed
both Unitree URDFs were primitives-only and self-contained; they reference 50
and 17 meshes respectively. Corrected there. Consequence for the fixtures below:
the G1 golden trajectory has faithful masses and inertias but contact against
placeholder colliders — fine for "does it fall", not a faithful foot contact
model.

## Verified in the app

Confirmed by eye with `examples/floating-arm.vcad`, in **both** modes:

- The robot falls under gravity, the render tracks it frame by frame, and the
  transport reads `RTF 1.00×` — real time.
- Auto-configuration derived, correctly and unaided: 2 actuated joints, 14
  policy features (10 base + 2×2 joints + 0 end effectors), 50 Hz control,
  1 kHz physics / 20 substeps, terminate below 0.33 m (= 0.6 × 0.55) and beyond
  45°.
- Training ran to 12/12 iterations with the held-out score as the headline, a
  rendered reward curve, and `train-eval` / σ / |Δθ| / α visually demoted.
- Released mode: the robot renders over the desktop, the Simulation tool window
  carries the full inspector, and the transport sustains 50 Hz (193→349 steps in
  3 s) at `RTF 1.00×` with the overlay re-rendering each frame.
- With the sample authored at rest: `STEP 357/400`, height −0.000 m, tilt 0.00°,
  and `RETURN 357.0` — exactly 1.0 per step, the pure alive bonus, so the height
  and tilt terms are both contributing zero. It is not falling.

Two dev hooks were added for this, alongside the existing `VCAD_GRIPPER` /
`VCAD_ROUTE` ones: `VCAD_SIM=1` builds and runs the simulation on launch,
`VCAD_TRAIN=1` additionally starts a short training run. Both exist because
synthetic mouse events did not reach the app's window content during
verification (the menu bar accepted them; the window did not), and driving the
feature from the environment is how this repo already solves that.

- **Not verified by eye:** loading a `.vcadpolicy` bundle through the file
  picker, and the Stale banner. Both are covered by tests
  (`an_edited_document_marks_a_policy_stale`,
  `testTrainingRunsAndProducesASaveableBundle`) but nobody has clicked them.
