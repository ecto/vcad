# vcad — designing the product, not the part

*Cross-domain co-design vision for the native app. 2026-06-23.*

## The big idea

**Stop designing parts. Design the product — all of it, alive, at once.**

Today a mechatronic product is a relay race between four trades and four file
formats: a mechanical engineer in Fusion, an EE in KiCad, a sheet-metal vendor's
portal, a robotics person in URDF — four blind files reconciled by humans over
weeks of *"the connector moved, re-spin the board."*

vcad collapses that into **one parametric DAG, evaluated by one kernel that owns
every domain**. The enclosure wall, the copper trace, the bracket bend, and the
motor's torque are nodes in the same graph — co-solving and self-verifying. You
don't export to manufacturing; the model already knows it is a JLCPCB board, a
SendCutSend bracket, and a moving mechanism, and it keeps all of them legal at
once.

This is the only thing no competitor can answer. Fusion's ECAD is a linked Eagle
round-trip. Onshape is mechanical-only. KiCad has no enclosure, no bracket, no
kinematics. SolidWorks needs three paid add-ins and a PDM server and still
reconciles as linked files *on save*. AI text-to-CAD tools emit a single mesh
with no editable cross-domain graph at all. vcad is the only system where copper
routing, BRep walls, sheet-metal bends, joint kinematics, and live manufacturing
quotes are the **same graph evaluated by the same kernel** — so one gesture can
legally, simultaneously, re-solve all of them.

## The point of view

> A physical product is one coupled system, not a stack of files — and a design
> isn't done when it looks right, it's done when every domain it touches has
> proven itself **legal, costed, and one tap from existing**.

## The signature moment — The Connector Drag

A servo-driven robotic gripper floats in front of you, its enclosure ghosted to
glass so you can see the PCB glowing inside. You pinch the USB-C connector and
drag it across the board, because the cable should exit the back. In the ten
seconds your finger is moving, four domains re-solve under your hand, each with
its own haptic signature:

1. **Copper.** The PCB outline re-flows and the OHM autorouter re-snakes the
   copper live — traces peel off the old pad and re-route, vias hopping layers,
   the ground pour healing around them, never shorting.
2. **Wall.** The enclosure's connector cutout slides with it and the wall
   self-heals where the old hole was — hardening into a felt *wall* the instant
   DFM says the remaining material drops below min-wall.
3. **Bracket.** The sheet-metal bracket clamping the board unfolds-and-refolds
   its tab to clear the cutout, its bend line snapping to the SendCutSend
   bend-relief grid with a detent tick.
4. **Money.** A thin Fabricate receipt ribbon runs along the bottom: the JLCPCB
   + SendCutSend quote ticks green→amber the moment the drag crosses into a
   costlier panel size, then back to green as you settle into the legal spot.

You let go. Everything chimes into **verified**: DRC clean, min-wall held, bend
legal, three quotes locked. One gesture just re-spun a board, an enclosure, and
a bracket — and priced them.

## Why the breadth is the story, not the problem

The obvious fear is diffuseness — six domains in six tabs is a feature parade
that adds up to nothing memorable. The wrong cure is amputation: cut vcad down
to one mechanical part with a draggable dimension, and you have Shapr3D, which
ships and wins nothing, and you've thrown away the only thing no one else has.

The right cure is **coupling**. Six domains sitting in tabs is diffuse. Six
domains *reacting to one finger* is a singular, unforgettable point of view —
because nobody has seen non-shorting copper re-flow inside a metal wall while a
bracket refolds and a price moves, all at once, all legal.

Focus lives in the **interaction**, not the feature set:

- **One gesture, not one domain.** Perfect one worked example end-to-end — the
  servo gripper (mechanical + PCB + bracket + one revolute joint). The signature
  is the connector drag and its honest four-domain wake. We do not try to show
  all six domains coupling at once.
- **Causality you can read.** Stagger the re-solve so the eye follows the chain:
  the dragged domain shimmers "solving," adjacent domains snap crisp as each
  finishes. The brand-pink diff-flash fires only on what actually changed.
- **The receipt is the focusing lens.** Every cross-domain change resolves to a
  verdict you can see and re-run. Breadth without proof is a tech demo; breadth
  that proves itself is a tool.

## The signature set

Five moments, one soul — *correctness propagating across domains, felt in the
hand* — at five scales.

1. **The Connector Drag** *(hero)*. One pinch ripples through copper, wall,
   bracket, and price, settling into a verified state.
2. **The Reckoning.** You hold a finished gadget and ask "is this real?" The
   assembly explodes into its domains; each runs its *actual* verification and
   snaps a verdict-token onto itself. As the last check passes, a physical
   receipt condenses in your palm and a **Make it** button appears — an earned
   consequence of proof, not a menu item.
3. **The Air-Gap Pull.** You widen a motor's stator air gap a half-millimeter
   with two fingers. In the same gesture, flux density drops (the real MEC sim),
   the rotor spins visibly slower under physics, the torque-curve ribbon sags,
   and the coil traces flash amber because the winding can't carry the new
   current. Let go; the gradient step re-routes and settles green.
4. **The Intent Cascade.** You say "swap USB-C for sealed micro-USB and make the
   whole thing waterproof." A consequence-wavefront sweeps the model domain by
   domain — footprint morphs and traces re-route, the wall thickens and a gasket
   groove extrudes, the live clash detector nudges a capacitor inboard before it
   kisses the lid. Then "actually, undo the waterproofing" — and the cascade
   unwinds in reverse, because it was one intent node in the DAG the whole time.
5. **The Drone-Arm Spring-Back.** You say "make this hinge a drone arm with a
   600g motor on the end." The arm sags under real gravity. You push the tip down
   and release; it springs back and oscillates with damped articulated physics,
   and the moment peak stress crosses yield it surfaces "root will fatigue —
   widen fillet to 4.2mm?" You drag the fillet fatter; the next push rings clean.

## Design principles — keeping it honest

The risk is a flashy cross-domain demo that's shallow. Three rules keep it true:

- **If it animates, a domain actually recomputed.** No tweens standing in for
  solvers. The copper re-routes through the OHM autorouter that genuinely never
  shorts; the flux is the real MEC model; the spin is phyz Featherstone
  dynamics; the clash is the live interference detector; min-wall and bend-relief
  are the actual DFM rule packs.
- **The receipt must be able to say "Violated" in red and stop.** A failed check
  snaps the part red, prints the failure with a one-tap fix, and the **Make it**
  button is *genuinely absent*, not greyed. Never imply a firmer price than an
  adapter actually bound. The drama is the utility only because it's genuinely
  correct.
- **Coupling is authored and reversible, never silent.** Every cross-domain link
  is an explicit, visible, pin/unpin-able constraint that writes named nodes into
  the feature tree as a single undo. The AI proposes the coupling; the engineer
  owns what's allowed to move. Commerce never moves on a gesture — `place_order`
  requires the human-signed `authorize_spend` gate.

## The first three things to build

Build these and the vision is *proven real*, not promised. Each de-risks the one
claim a skeptic will test, and together they are The Connector Drag end-to-end.

1. **The live cross-domain re-solve loop on one gesture.** Prove that dragging
   the gripper's connector fires OHM re-route + enclosure cutout heal +
   sheet-metal refold *together, on-device, at interactive speed* through the
   zero-copy FFI hot loop. If it's fluid, everything else is choreography. Earn
   the 120Hz budget by re-solving incrementally with a "solving" shimmer only on
   the dragged domain; adjacent domains snap crisp as each finishes.
2. **The always-visible Receipt, wired to real checks.** `run_drc` /
   `dfm_check` / bend-relief / `quote_manufacturing` flip green/amber/red from
   *actual* kernel calls during the drag. This is what converts spectacle into a
   tool an engineer trusts. Make it the persistent frame, not a popup.
3. **The Core Haptics + ProMotion "felt constraints" layer.** The bend-snap
   detent, the hardening min-wall, the multi-domain verified chime. Cheapest of
   the three, largest contributor to the moment people retell.

Ship those on one worked example — the servo gripper — perfected, and the other
four moments reuse the same engine pointed at new gestures.

## The staged path

The native app today is **mechanical-only**: a loon-authored part rendered in the
studio, with the AI composer, the narrative history sidebar, and the adaptive
inspector. The Connector Drag requires bringing PCB + sheet-metal + the coupling
DAG into that app. That is a multi-week lift, staged:

1. **Foundations** — the design language locked: centered identity + status,
   bottom composer with `+`, the narrative history (origin-aware, time-travel),
   the adaptive inspector, the studio (real IBL, honest materials, weighted
   orbit). *(Mostly designed; some shipped.)*
2. **One domain at a time into the native app** — bring the PCB view and the
   sheet-metal view into the same window as first-class, then the assembly +
   joint view (the gripper's revolute joint).
3. **The coupling DAG** — author the cross-domain constraints (cutout ↔
   connector, bracket tab ↔ board edge) as real nodes; wire the live re-solve.
4. **The Connector Drag** — the hero gesture, end-to-end on the gripper, with
   the Receipt spine and the haptic/ProMotion layer.
5. **The rest of the set** — Reckoning, Air-Gap Pull, Intent Cascade, Drone-Arm,
   each reusing the spine.

## Grounding — the kernels already exist

This vision is buildable because the hard parts are already in the repo:

- **Autorouter** — `crates/vcad-ecad-pcb/src/router/{auto,maze}.rs`,
  `ratsnest.rs`, `session.rs` (OHM: real copper pour, probed vias, never shorts).
- **Motor / flux sim** — `crates/vcad-ecad-sim/src/{airgap,magnetics,motor,thermal}.rs`.
- **Live clash detector** — `packages/app/src/lib/pcb-interference.ts`.
- **DFM + sheet metal** — `crates/vcad-kernel-dfm/src/rules/`,
  `crates/vcad-kernel-sheet/src/unfold.rs`, `crates/vcad-kernel/src/sheet_fold.rs`.
- **Receipt + money gate** — `build_receipt` / `verify_receipt` /
  `authorize_spend` / `place_order` in `packages/mcp/src/tools/`.
- **Physics gym** — `packages/mcp/src/tools/gym.ts` (phyz reset/step/observe).
- **Zero-copy hot loop** — `apple/VcadApp/Sources/CVcadFFI/vcad_ffi.h`,
  `apple/VcadApp/Sources/VcadApp/Kernel.swift` (LowLevelMesh streaming).

The moat is that one kernel owns every domain. The work is making that coupling
**visible, felt, and trustworthy** — so a single gesture re-solves a whole
product, and you watch correctness propagate across the trades that used to take
weeks to reconcile.
