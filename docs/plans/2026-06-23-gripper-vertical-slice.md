# First vertical slice — the minimal Connector Drag

*The smallest honest proof of cross-domain co-design in the native app. 2026-06-23.*
*North star: [2026-06-23-cross-domain-codesign-vision.md](2026-06-23-cross-domain-codesign-vision.md).*

## Goal

Prove **80/20 #1 — a live cross-domain re-solve on one gesture** — at the
smallest scale that is still genuinely the signature. The full Connector Drag
couples four domains (copper, wall, bracket, price). This slice couples **two**:

> Drag a connector on the PCB; the enclosure's cutout follows live, the wall
> re-solves, a min-wall verdict flips green→red→green, and you feel a detent per
> millimetre and a firm "wall" the instant the cutout threatens the housing.

Two domains, one gesture, verified and felt — in the native app, at 120fps.
Copper routing (OHM) and the sheet-metal bracket are explicitly deferred to
slices 2–3; this slice de-risks the one claim everything else rests on: that one
finger can legally re-solve more than one domain together, fluidly, on-device.

## Why this is the right first slice

It is the hero gesture minus two domains, so it proves the load-bearing thesis
(coupled live re-solve + felt constraint + visible verdict) without the two
hardest engines (the autorouter and sheet-metal unfold). If the cutout can't
follow the connector fluidly with an honest min-wall check, the whole vision is
in trouble — so build exactly that, first, and nothing else.

## The coupling mechanism (already in the kernel)

The kernel resolves a parametric DAG today: `vcad_eval::resolve_document`
(`crates/vcad-eval/src/resolve.rs:68`) evaluates `doc.parameters` into an env and
applies `doc.bindings` onto concrete node fields, then `evaluate_document`
tessellates. **A shared parameter bound to two nodes is the coupling**:

```
parameters:  connector_x = 12.0
bindings:
  (board_connector_node, "translate.x")   <- connector_x
  (enclosure_cutout_node, "translate.x")  <- connector_x
```

One parameter drives the board's connector position *and* the enclosure's cutout
position. Setting `connector_x`, re-resolving, and re-evaluating re-solves both
domains in lockstep. This is not new kernel work — it is the existing parameter
+ binding system pointed at two domains at once. The slice's job is to make that
re-solve **live under a drag**, in the native app, with a verdict and a haptic.

## The worked example (slice-1 subset of the gripper)

A hand-authored IR fixture `apple/VcadApp/Resources/gripper-slice1.vcad`:

- **Enclosure** — a box (the housing), minus a **connector cutout** (a small box)
  whose `translate.x` is bound to `connector_x`. Material: aluminum.
- **Board** — a thin PCB plate inside the enclosure, with a **USB-C connector**
  body whose `translate.x` is bound to `connector_x`. A couple of component
  blocks for context. Material: a board green.
- **Parameter** `connector_x` with a min/max range (the legal travel along the
  board edge).

Authored as **IR JSON, not loon** — loon is static authoring and doesn't emit
`parameters`/`bindings` today. (Teaching loon/the AI composer to emit coupled
parametric docs is a parallel track, not on this slice's critical path.)

The full gripper — sheet-metal bracket, the revolute joint + motor — arrives in
later slices and reuses everything below.

## The native FFI additions (small)

Keep a `Document` resident and re-evaluate on a parameter change — the document
analogue of the existing `StreamingMesh` hot loop:

```c
typedef struct VcadDoc VcadDoc;
VcadDoc *vcad_doc_load(const uint8_t *json, size_t len);      // parse + hold
VcadScene *vcad_doc_set_param(VcadDoc *doc, const char *name, double value);
                                  // set param -> resolve_document -> evaluate -> scene
double vcad_doc_get_param(const VcadDoc *doc, const char *name);
void vcad_doc_free(VcadDoc *doc);
```

`vcad_doc_set_param` is the hot path: mutate `doc.parameters[name]`, call
`resolve_document(&mut doc)` then `evaluate_document(&doc, &EvalOptions)`, return
a `VcadScene` (reusing the existing `vcad_scene_part_count` / `vcad_scene_part_mesh`
buffers). Mirrors `vcad_scene_from_json`; adds residency + the param set. All
behind `catch_unwind`, per the crate's rules.

## The min-wall verdict (honest, simple)

The slice's single cross-domain check: does the cutout leave enough wall? Two
options, both real geometry:

1. **Cheap, native:** compute the clearance from the cutout edges to the
   enclosure's outer/rib faces from the evaluated meshes' bounds — flag when it
   drops below a rule (e.g. 1.2 mm). Fast, good enough to prove the loop.
2. **Real DFM:** expose `vcad-kernel-dfm`'s min-wall rule as
   `vcad_dfm_min_wall(scene) -> f64` and surface the actual rule-pack number.

Ship #1 for slice 1 (keeps the FFI tiny), wire #2 right after — the UI is
identical (a number + a green/red verdict). The rule is honest either way; the
verdict must be able to go **red and stay red** while you hold the connector in
an illegal spot.

## The Swift side

- A `CoupledDoc` `@Observable` holding the `VcadDoc` handle + the current
  `connector_x`, with `setConnectorX(_:) -> RenderScene` calling
  `vcad_doc_set_param` and streaming **both** parts (board + enclosure) into the
  existing `LowLevelMesh` buffers (extend `StreamingMesh`/`sceneFromHandle` to
  multiple resident parts).
- A **connector handle** entity on the board (reuse the floating-handle pattern,
  generalised off the sandbox cube): drag along the board edge → map to
  `connector_x` within its range → re-solve → restream.
- A **min-wall verdict pill** (the slice's Receipt): `min-wall 1.4 mm ✓` →
  `0.9 mm ✗` in the warning colour as you cross the limit. This is the persistent
  frame, not a popup — the seed of the always-on Receipt.
- **Haptics:** `.alignment` detent per mm of travel; `.levelChange` thud the
  instant min-wall goes red — the felt "wall". (The verified chime via
  `AVAudioEngine` is a nice-to-have here, core in a later slice.)
- **ProMotion:** drive the re-solve off a `CADisplayLink` at the display's
  refresh rate (also replace the idle turntable's `1/60` `Timer` while here), so
  four-domains-later has headroom and the drag reads as continuous matter.

## The smallest provable demo

Open `gripper-slice1.vcad`. Grab the connector and drag it across the board:

- the enclosure cutout tracks it live, the wall healing behind it;
- the min-wall pill flips `✓ → ✗ → ✓` as you cross a rib;
- you feel a tick per millimetre and a wall when it goes illegal.

Screenshot-verifiable: capture three frames (connector left / mid-illegal /
right) and confirm the cutout moved with it and the verdict flipped. That image
*is* the proof the thesis is real.

## Explicitly deferred (later slices)

- **Slice 2 — copper.** Bring a minimal OHM re-route into the loop: the
  connector's traces re-route as it moves (`crates/vcad-ecad-pcb/src/router/`).
- **Slice 3 — sheet metal.** The bracket tab refolds to clear the cutout
  (`crates/vcad-kernel-sheet/src/unfold.rs`), snapping to the bend grid.
- **Slice 4 — the full hero + money.** Four domains, the real Receipt wired to
  `run_drc`/`dfm_check`/bend-relief/`quote_manufacturing`, the verified chime;
  then The Reckoning / Air-Gap Pull / Intent Cascade / Drone-Arm reuse the spine.

## Risks

- **Re-solve fluidity.** Re-evaluating the whole doc per drag frame may not hold
  120fps as parts grow. Mitigate: re-solve incrementally / on commit-able
  checkpoints, stream a "solving" shimmer only on the dragged domain, and keep
  slice-1 geometry small. This is the perf claim the slice exists to test — if it
  stutters here on two simple parts, that's the signal to invest in incremental
  re-solve before adding domains.
- **Authoring the coupled fixture.** Hand-authoring valid IR JSON with
  parameters/bindings is fiddly; write a tiny generator + a kernel unit test that
  asserts setting `connector_x` moves both the cutout and the connector.
- **Min-wall fidelity.** The cheap clearance check can disagree with the real DFM
  rule near ribs; ship the real `vcad-kernel-dfm` number as soon as the loop works
  so the verdict is never "honest-looking but wrong".

## First PR scope

1. FFI: `vcad_doc_load` / `vcad_doc_set_param` / `vcad_doc_get_param` /
   `vcad_doc_free` + a unit test (set `connector_x` → both nodes move).
2. The `gripper-slice1.vcad` IR fixture + its generator/test.
3. Swift `CoupledDoc` + multi-part `StreamingMesh` + the connector handle + drag →
   re-solve → restream.
4. The min-wall verdict pill (cheap clearance check) + the `.alignment` detent and
   `.levelChange` wall haptic.
5. Build, run, screenshot the cutout-follows-connector + the verdict flip.

Ship that and the vision stops being a deck — one finger re-solves two domains,
legally, in the real app.
