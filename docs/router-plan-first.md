# Plan-first redesign of the vcad PCB autorouter

This is a read-only design. I read the router at `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src` (abbreviated `S/` below) and both campaign logs.

## 0. Diagnosis: why every round traded one metric for another

The current pipeline in `S/router/auto.rs:485` (`route_all_with_opts`) runs these steps in order:
1. Plane-layer no-route list, opt-in (`auto.rs:517`, set through `maze.rs:115`).
2. Plane prestitch (`auto.rs:585`, `stitch_planes` at `auto.rs:4343`).
3. Optional planned microvia fan-out (`auto.rs:4083`).
4. 2D corridor planning (`plan_with_scarcity`, `auto.rs:1564`).
5. Pairs-first.
6. Optional ring escape (`auto.rs:1724`).
7. Per-connection maze (`maze.rs:323`) with PathFinder history (`congestion.rs`) and a GPU negotiator (`auto.rs:7149`).
8. Rescue, joint-window repair (`complete.rs:256`) and shove.
9. Post-route stitch (`auto.rs:848`) and island stitch.
10. Legalize (`auto.rs:1137`), then SI descent (`descent.rs` / `sco.rs`).

The logs show the same failure every round. Each stage takes a scarce 3D resource greedily, and later stages pay for it:

- **The global model has no layers and no vias.** `CapacityMesh` (`global.rs:44`) stores a single scalar per 4 mm GCell (`global.rs:28`), described as "free-space fraction × layer count". It cannot see that In1/In3/In6/In8 are planes, that only F/In2/In7/B carry signal, or that the escape budget under U1 is a via-site budget rather than a track budget. `Congestion` (`congestion.rs:27`) is also 2D, at 1 mm.
- **Via spans are chosen inside the maze, one hop at a time.** `maze.rs:100` (`micro_bias`) prices microvias inside A*. The log says the bias "DOES NOT WORK with this maze (hop chains through the plane-layer stack get rejected by the oracle)". The span choice is really a fan-out decision (F→In1→In2 staggered), not a per-step cost.
- **Planes are a side effect, not a resource.** Plane connectivity is attempted twice by `stitch_planes`, before and after signals. Island stitch reports "536 stranded, 242 unreachable" (full15). Power pads for In4/In5 need drilled vias whose sites the signals have already taken.
- **Escape is planned per field, on one layer, without ring order.** `PinField.layer` (`escape.rs:44`) is one layer. `plan_multilayer_escapes` (`escape.rs:712`) does stagger escapes, but it runs after pairs and against copper that is already there. Ring escape committed 321 of 393 planned escapes, but the escape copper "walls the BGA channels the singles needed" (vs11nd).
- **Turning on layer roles exposes the real capacity.** With roles enforced (full15), split nets went from 34 to 64. Run unenforced, the router puts 957 PlaneLayerSignal segments on the plane layers.

The fix is to decide the 3D topology (escape layer, via span, plane access, layer assignment, bundles) globally over explicit resources, then make detailed routing realize it.

## 1. Stage A: stackup and resource model (new `router/resources.rs`)

**Inputs:**
- `Pcb` stackup and `LayerRole` (`vcad-ir/src/ecad.rs:646`, inferred at `ecad.rs:1442`).
- `ViaSizing` (`S/via_sizing.rs:47`; `spec` at `:157`, `may_stack` at `:126`, `is_micro_span` at `:116`).
- Rules, footprints, zones.

**Data structures:**
- `LayerClass { Signal, Plane(net), Split(nets) }`. This is a hard constraint, not the `VCAD_ENFORCE_PLANES` opt-in. Signal edges simply do not exist on plane layers.
- `SpanCatalog`: the legal via spans and their geometry. On CM5 these are F–In1 and In8–B microvias, the second-level In1–In2 and In7–In8 microvias, buried In1–In8, and through. Each span has a size from `ViaSizing::spec`, a stagger rule from `may_stack`, and a relative cost.
- `Region` grid. The GCell size adapts: pitch-aligned (0.65 mm) under BGAs, about 2 mm elsewhere. Each region carries:
  - `track_cap[layer][dir]` for signal layers only, computed from free width / (w + s) minus fixed copper. Fixed-copper probes reuse `RouteSession::copper_area_in` / `obstacles_in` (`S/session.rs:824`, `:745`).
  - `via_cap[span]`: the number of drill sites that fit after hole-to-hole and existing drills. This uses `probe_via_span` (`session.rs:715`). Under a BGA it is counted per interstitial site, so capacity is the lattice of dog-bone sites.
  - `plane_access[layer][net]`: whether the plane pour of `net` exists here and has room for anti-pads. This comes from zones and pour synthesis.
- `PlaneGraph`: per plane net, the pour components per layer and the pads that must reach them. This is plane connectivity as a first-class demand: each pad needs one plane-access path consisting of a pad, a span to a plane layer, and a component.

**Reuse:** `ViaSizing`, `RouteSession` probes, `copper_pour`/`pour_synth`, and `escape::detect_pin_fields` (`escape.rs:104`).
**Cost:** O(regions × layers) to build.
**Determinism:** pure function of the board, iterated in sorted order.

## 2. Stage B: global planning

Run B1 to B4 in order, all writing into one `GlobalPlan`. Each later step sees the resources committed by the earlier ones.

### B1. Plane reservation, before anything else
For every plane-net pad, pick its access span (pad → nearest legal plane layer of that net), and on HDI pick a site: via-in-pad, or a dog-bone into a specific interstitial site.

Formulate this as min-cost assignment of pads to sites over `via_cap`. Costs are span cost plus stub length. A site serves one via, a pad needs exactly one site, and microvias are preferred when `is_micro_span`.

The output is a list of reserved `ViaReservation{pad, site, span}`. These are committed as fixed copper exactly as planned microvia fan-out is today (`auto.rs:4083`), but now they include the drilled In4/In5 power vias. The full16 log names those as what kept VDD_BCM_CORE, VDD_0V6 and CM5_3V3 split.

Split-plane nets reserve on In4/In5 only inside their pour polygon. Islands left unreachable are reported up front as *plan-infeasible* rather than discovered after routing.

**Reuse:** `stitch_spans` (`auto.rs:3828`) for span ranking, and the microvia fan-out code. **Replaces:** prestitch, post-route stitch and island stitch as primary mechanisms. They remain only as fallback.

### B2. Pin-field escape planning, per BGA, ring by ring
For each `PinField`, take the signal balls (plane balls are already served by B1). Then assign each ball an escape layer from the signal-role layers {F, In2, In7, B}, a span chain, and an egress side.

Formulate one min-cost flow per field per layer, as in the network-flow BGA escape literature (Yan and Chen's ordered escape, and Chin et al.'s MCMF escape with via constraints).
- Nodes: ball sites, interstitial via sites (capacity 1, minus B1 reservations), and channel edges between balls (capacity = tracks per gap, which is `MAX_GAP_TRACKS` today at `escape.rs:30`).
- Rings are processed outside-in. Outer rings escape on the pad layer (F for U1 top). The next rings drop by staggered F→In1→In2 microvia to In2. Bottom-side and deep balls go B→In8→In7.
- Ring order is driven by `LayerClass`, not hard-coded: the escape layers are the signal layers reachable through `SpanCatalog` chains whose intermediate layers are planes, and the stagger is legal by `may_stack`.
- The egress side is biased by B3's coarse direction toward each net's other terminal. B2 and B3 iterate twice (see B3).
- Pair and bundle members are forced to the same layer, the same egress side and adjacent channels (a pair is one flow unit of width 2).

**Output:** `EscapeAssignment{ball → (layer, span chain, egress region)}`, with no geometry yet. The geometry comes later from `plan_multilayer_escapes` constrained to this assignment.

**Reuse:** the flow network in `escape.rs` (the grid probing from d697045e) and `plan_multilayer_escapes`. **Replaces:** the greedy "as many as fit on the first layer" ordering and `ring_escape` (`auto.rs:3919`).
**Cost:** MCMF on about 10^4 nodes per field, which takes milliseconds to seconds.

### B3. 3D global routing with layer assignment
- Graph: regions × signal layers. Planar edges carry `track_cap`; via edges carry `via_cap[span]`.
- Terminals are B2 egress regions, or pads outside fields.
- Plane layers have no planar edges. They appear only as pass-through in via spans.

The solver is negotiated PathFinder over this 3D graph (McMurchie and Ebeling): present-congestion times history cost, with deterministic net order and a fixed number of iterations. It generalizes `CapacityMesh` and its 3 sweeps, and can reuse the GPU negotiator (`gpu_bridge.rs`) once its graph is generalized.

Option B3+, behind a flag: soft candidate selection. Generate K candidate paths per connection (from PathFinder history snapshots) and relax them to weights `x_c ∈ [0,1]` with softmax per connection. Minimize wirelength plus via cost plus overflow hinge as Σ demand − cap, then round. This mirrors DREAMPlace's differentiable relaxation and DRG-3D-style differentiable candidate selection. It fits the tang-ir `ExprGraph` / `ResidualSystem` already in `claims_ir.rs` / `sco.rs` (hinge rows and AL multipliers). I'd ship PathFinder first and treat the relaxation as an experiment, because an LP/softmax relaxation does not guarantee integral feasibility.

**Output:** `Route3D{conn → [(region, layer)], via transitions with span}`, plus per-region residual capacity.
**Iteration with B2:** after B3, recompute each ball's preferred egress side and rerun B2 on the fields that changed. Cap this at 2 iterations.

### B4. Bundles: diff pairs and length groups
- Pairs are one demand of width 2·w + gap, with a shared layer and synchronized transitions (vias in pairs, so span capacity is consumed as 2).
- Length-matched groups (LPDDR4 CA/DQ byte lanes, RGMII) are a bundle: same layer set, adjacent corridors, a group corridor wide enough for serpentine slack. The slack area is reserved as extra `track_cap` demand ≈ (target − estimated length) × pitch.
- Groups come from net classes (`classes.rs`), with naming heuristics as fallback.
- Planned within B3 as composite demands, routed first in each PathFinder iteration.

**Reuse:** `pair.rs` realize-legs for geometry later.

## 3. Stage C: detailed routing inside assigned corridors

For each connection, the maze (`maze.rs:323`) gets three restrictions:
1. **Layer mask:** only its assigned layers per region. This means a per-region layer mask, not one global `set_no_route_layers`.
2. **Corridor:** the union of its assigned regions, inflated by one region, replacing the bbox corridor.
3. **Via moves:** only to reserved or capacity-counted spans at the planned transition regions. Soft-allow elsewhere at high cost, so that small deviations do not fail closed.

Escape geometry is realized first from B2, using `plan_multilayer_escapes` with the fixed layer and span. Then pairs (`pair.rs`), then groups, then singles. Rip-up, shove (`shove.rs` / `push_shove.rs`) and joint windows (`complete.rs:256`) are kept, but they are scoped to a region and its layers: they may only rip connections that share the region in the plan. This bounds the rescue arsenal, which is what made full17 and vs3nd run unbounded.

The gate does not change: session clearance oracle, legalize (`legalize.rs`), DRC (`drc.rs`) including PlaneLayerSignal (`drc.rs:58`, `:836`) and MicroviaStack (`drc.rs:64`, `:1161`), and the connectivity gate. All of these fail closed.

Then SI comes from tang-route descent (`descent.rs`, SQP in `sco.rs`), with pair skew claims (`claims_ir.rs:435`) and group length claims (`length_match.rs`, `length_tune.rs`) inside the slack area B4 reserved.

**Cost:** detailed searches become small (corridor plus layer mask), so expect fewer, cheaper searches than today's 3193.
**Determinism:** keep the work budgets (31560c34) and no clock reads.

## 4. Stage D: feedback from detail failures to the plan

When a connection fails in Stage C (or legalize demotes it), the failure is turned into an update on the plan:
- The failing regions and layers get **capacity corrections**: `track_cap -= observed shortfall`, or `via_cap[span] = sites actually realized`. This is a learned derating per region type, for example "BGA interior In2 gap fits 1 track not 2".
- The PathFinder history in B3 is raised on those 3D edges.
- **Duals from descent (the tang-route M4 idea):** the final augmented-Lagrangian shifts and multipliers σ_i in `sco.rs` (documented at `sco.rs:23` as "the input M4 would turn into history costs") are mapped back to the regions and layers of their claims. Large duals on clearance or skew claims are added as history on those B3 edges. Large skew duals for a group widen its B4 slack reservation.
- Then re-plan *incrementally*: only the affected nets plus any whose B3 path crosses the updated edges, and B2 for the touched fields. Stage C re-runs on just those.
- Bound the outer loop at 3 iterations. Keep the best iteration by lexicographic score (split nets, then DRC, then SI), as `auto.rs` does today.

## 5. Module reuse summary

| Status | Modules |
|---|---|
| Keep | `session.rs`, `via_sizing.rs`, `maze.rs` (adds layer mask and region corridor), `pair.rs`, `shove.rs` / `push_shove.rs`, `complete.rs`, `legalize.rs`, `drc.rs`, `descent.rs` / `sco.rs` / `claims_ir.rs` / `si_claims.rs`, `length_tune.rs`, `escape.rs` geometry |
| Generalize | `global.rs` → 3D region graph; `congestion.rs` → per-layer history; `gpu_bridge.rs` → 3D graph |
| Replace as primary path (keep as fallback) | `stitch_planes` pre/post/island, `ring_escape`, `micro_bias`, the 2D `plan_with_scarcity` |
| New | `resources.rs` (Stage A), `plane_plan.rs` (B1), `escape_plan.rs` (B2 assignment), `global3d.rs` (B3/B4), `plan_feedback.rs` (D) |

`auto.rs` (7224 lines) gets a new top-level `route_planned()` beside `route_all_with_opts`, selected by `VCAD_PLAN=1`.

## 6. Incremental migration plan

Every milestone ships behind its own flag and is measured with `bench-out/via-sizing/run.sh`: cm5_bench, drc_delta, si_descent, census.py, plus the same on `hardware/fpga-dev-board`.

The baseline is vs10nd:
- 365 routed / 43 unrouted, UnconnectedNet 53
- PlaneLayerSignal 957
- vias: micro 491, blind 568, through 335, buried 35
- pairs: 41/47 coupled, worst skew 7.43 mm
- 2525 s

The human reference:
- 9 split nets
- vias: 1867 micro, 803 through, 232 buried
- about 0 signal on plane layers

**M0, measurement (no flag).** Add census outputs for:
- plane groups per net
- via mix by span
- PlaneLayerSignal mm
- group skew per LPDDR4 / RGMII group
- the "plan vs realized" divergence (fraction of connections whose realized layers match the plan)

Success: deterministic numbers on both boards.

**M1, resource model plus 3D global plan, advisory only (`VCAD_PLAN_GLOBAL3D`).** Stage A and B3 feed corridors and per-region layer preferences, as costs, into today's pipeline.
- Success: on CM5 with roles enforced, split nets at or below the 34 seen with roles off (versus 64 in full15), PlaneLayerSignal = 0, time ≤ baseline. On the FPGA board, no regression.

**M2, plane reservation B1 (`VCAD_PLAN_PLANES`).**
- Success: plane groups for GND go from 45 to ≤ 5. In4/In5 power nets are connected or flagged plan-infeasible up front. Island-stitch stranded count drops by at least 5×. Via mix shows drilled In4/In5 access vias.

**M3, escape assignment B2 (`VCAD_PLAN_ESCAPE`).**
- Success: on U1/U2/U3/U9, at least 95% of planned escapes are committed (ring escape managed 321 of 393). The first-batch commit count does not drop below 401. The microvia share moves toward the human's: CM5 micro vias ≥ 1200, staggered, MicroviaStack 0.

**M4, hard layer masks plus region-scoped rescue (`VCAD_PLAN_DETAIL`).** Stage C enforces the plan.
- Success: split nets ≤ 20, time ≤ 0.7× baseline, maze searches bounded per region, byte-identical reruns.

**M5, bundles B4.** Pairs and groups are planned as composite demands.
- Success: pairs ≥ 45/47 coupled, worst pair skew ≤ 1.25 mm (descent tuning not blocked by octilinear rejects), LPDDR4 byte-lane group skew within its class target, RGMII group skew within its class target.

**M6, feedback D with duals.**
- Success: split nets ≤ 12 within 3 outer iterations, and the plan/realized divergence falls with each iteration.

**M7 (experimental), soft candidate selection in tang-ir.**
- Success: it beats PathFinder on overflow and split nets at equal time. If it does not, drop it.

## 7. Risks

1. **Capacity estimates are wrong at BGA scale.** Interstitial sites, anti-pads on plane layers and teardrops all distort capacity. M1's divergence metric and the derating learned in D are the mitigation. Seed the derating by measuring capacities on the human CM5, since its own via sites give `via_cap` ground truth.
2. **Hard layer masks remove the slack that currently rescues nets.** Keep a soft escape hatch: a high-cost off-plan edge on signal layers only, never on planes.
3. **The B2/B3 coupling oscillates.** Use a fixed iteration count with damping, and accept the best iteration.
4. **Plan-infeasible boards.** If the human used something outside the catalog (for example 82 stacked microvia pairs), B1/B2 must honour `allowStackedMicrovias` through `may_stack`.
5. **Wall time of the 3D PathFinder.** Regions × 4 signal layers is small (about 10^5 edges), so this is likely acceptable.
6. **Group detection on CM5 depends on net naming or classes.** Wrong groups waste slack reservation.

## 8. What to cut first if it's too big

1. M7 (differentiable selection). Pure research.
2. Dual-driven feedback (keep only capacity corrections from D).
3. B4 group slack reservation (keep pairs as width-2 demands; leave groups to descent).
4. The B2/B3 iteration (one pass: B3's coarse direction from straight-line estimates).

The core that must survive is A plus B1 plus B2 plus the hard layer masks of M4. Those four directly attack the measured failures: plane fragmentation, signal on planes, the wrong via mix, and BGA walling.

## 9. Prior art (brief, not overclaimed)

> Citations below are from memory and not yet verified against the papers; check before quoting.

- **PathFinder** (McMurchie and Ebeling 1995): the negotiation scheme already used in `congestion.rs`; here it is lifted to 3D with via-span edges.
- **Layer assignment after 2D global routing** (for example COLA, and NVM-style via-minimizing assignment): an alternative to full 3D search if B3 is too slow.
- **BGA escape as network flow:** Yan and Chen on ordered escape routing; Chin, Kuan and Chang on min-cost-flow escape with staggered and multi-layer vias. These motivate B2's per-ring MCMF.
- **SURF / rubber-band routing** (Dai et al.): topological-then-geometric routing. It is a candidate for Stage C geometry if the grid maze proves too jagged; I'm not proposing it now.
- **DREAMPlace / DRG-3D-style differentiable relaxations:** motivation for M7 only.

## Critical files for implementation
- `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/router/global.rs` (2D `CapacityMesh` at :44 and `plan_corridors` at :299; becomes the 3D region graph)
- `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/router/escape.rs` (`PinField` :44, `plan_multilayer_escapes` :712, flow network; becomes B2)
- `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/router/auto.rs` (pipeline :485–1180, `stitch_planes` :4343, `ring_escape` :3919, fan-out :4083; new `route_planned` entry)
- `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/via_sizing.rs` and `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/session.rs` (span catalog and capacity probes :715, :745, :824)
- `/Users/cam/Developer/vcad/.claude/worktrees/agent-a9f16100e22282cf2/crates/vcad-ecad-pcb/src/router/maze.rs` (:323 maze3d; add per-region layer masks and span gating) and `router/sco.rs` (:23 M4 duals for feedback)