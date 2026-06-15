# OHM
### *The router that decides which side of the world a wire belongs on — before it draws a single line.*

> Every other autorouter paints copper, then asks DRC whether the copper was legal. OHM plans the **topology** of the whole board as data, proves it routable before any coordinate is fixed, geometrizes it correct-by-construction, pours its own planes and stitches its own vias with a real solid-modeling kernel, and polishes it with physics that has a **gradient** — so the board is right by construction, and can explain every micron in terms of the cost that put it there.

---

## 1. Name + Tagline

**OHM** — *Optimal Homotopy Maestro*. (Also: the unit every trace in it is trying to hit.)

**Manifesto:** *Topology first, geometry second, physics in the gradient, legality in the oracle, and a reason for every wire.*

---

## 2. The Thesis

Altium, KiCad's PNS, Allegro, FreeRouting, and the new black boxes (Quilter, DeepPCB, Flux) all share one structural commitment that OHM refuses: **they commit to metric geometry — exact x,y polylines — before they decide topology, and they treat physics as a judge that scores finished copper rather than a gradient that shapes it.** They route one net at a time against geometric DRC, then hand the copper to a field solver that says *whether* it works, never *which way to nudge a trace to make it work*. Two consequences fall out of that one mistake: the net you route first gets the highway and the net you route last gets a DRC violation (vcad's current `route_nets` literally inflates other nets to static bounding boxes and Dijkstras around them — `push_shove.rs::route_net` builds a visibility graph of inflated obstacle corners and **never relocates a single existing trace**, despite its own doc comment at line 136 promising it "can displace existing traces to make room"); and the physics and the geometry live in different worlds, bridged only by re-simulation.

OHM inverts both. It is **structurally unlike the incumbents on two axes at once:**

1. **Topology-as-data, not geometry-first.** OHM first solves the *homotopy class of the entire board* — which side of every obstacle each net passes, what order pins escape a field, which layer carries which signal — as a combinatorial object living in vcad's nets-as-data IR, **before a single coordinate is fixed.** This is not a bolt-on. vcad's BRep kernel (`vcad-kernel-topo`) is an arena of half-edge combinatorial adjacency with `slotmap` IDs and Shewchuk exact-orientation predicates — *faces and edges as combinatorial adjacency first, coordinates second.* A homotopy sleeve graph is the same kind of object. **The company that shipped a robust half-edge kernel with `orient2d`/`incircle` is the one company on Earth positioned to ship a robust rubber-band router**; the C++ EDA incumbents, sitting on 30-year metric kernels, structurally cannot copy this without a rewrite.

2. **Physics with a gradient, not a score.** vcad's `impedance.rs` is genuinely `tang::Scalar`-generic — `microstrip_z0<S: Scalar>` (`impedance.rs:42`) with a symbolic gradient proven against finite-difference (`impedance.rs:338`). The *same* closed form that computes 50Ω in `f64` for verification traces through `tang-expr` to yield `dZ0/dw` symbolically. So OHM picks trace width to hit impedance by a **Newton step, not a search** — and that derivative is real, in the codebase, today. Quilter *scores*; OHM *steers*.

Only vcad has all six ingredients in one substrate: the half-edge kernel that thinks topologically, the differentiable physics engine (`tang-expr` + `vcad-kernel-constraints`' symbolic-sparse-Jacobian LM, already 21–84× over finite-difference), the GPU compute path (`tang-expr → WGSL`), the real 3D BRep world (`board_from_solid`) **with a boolean engine that can pour and clip copper exactly**, nets-as-data IR, and the agent-native MCP loop with `render_view`/`verify_part`. **The historically-distrusted black box becomes a glass box: same IR carries intent, same `run_drc` judges legality, same differentiable objective both scores *and* steers, same agent loop runs it.**

Two honest boundaries are stated up front and never crossed. The first: **no gradient crosses a homotopy jump, a layer change, or a via's existence.** "Fully differentiable routing from scratch" is snake oil. OHM ships a *discrete topology engine*; gradients refine *within* a frozen topology and never choose one. The second, equally load-bearing: **today's `run_drc` is a real but incomplete oracle, and OHM is honest about exactly where it is blind.** Its `DrcRuleType` enum (`drc.rs:17`) covers Clearance, MinTraceWidth, MinDrill, AnnularRing, EdgeClearance, HoleToHole, UnconnectedNet, SilkscreenClearance, CourtyardOverlap, AcidTrap, Keepout, Short — genuinely excellent legality for *geometric* rules, with real net-tie groups and layer-aware flood-fill. But it has **no diff-pair-gap check, no length/skew check, no creepage/HV-spacing check, no return-path/split-crossing check, and no copper-sliver/min-island check.** The `NetClassRules` IR already carries `diff_pair_gap` and `diff_pair_width` as data (`ecad. rs:369`) — and `drc.rs` never reads them; every match is a `None` field-init. So for precisely the premium regimes OHM sells, the oracle as it stands is blind, and a differentiable loss that "tuned" an uncertified constraint would be marking its own homework. **OHM therefore makes a DRC extension a NOW-stage prerequisite, not a someday-nice — because the whole architecture rests on `run_drc` being the legality authority, and that authority must actually cover what OHM optimizes.** That line is drawn in steel through the entire architecture.

---

## 3. The North Star — what it feels like

You hand OHM a placed board and a paragraph of intent: *"USB is a 90Ω diff pair, keep it off the switching node. DDR byte-lane 0 is 40Ω±10%, length-matched to 5ps, every segment over solid ground. VBUS carries 3A, keep it under a 20°C rise. Pour a stitched ground plane on L2. Everything else, your call."*

OHM doesn't start drawing. It **plans the topology and shows it to you as data** — sleeves, sides, layers, crossings resolved, vias placed as topology events, plane regions and stitch-via fences as zones — *with no copper yet.* You see "USB_D+ passes left of U7, right of via V12, on L1; crosses to L3 at V_new to dodge the SW plane split." You can approve the plan, or `hint("route the address bus on L2")` and watch it re-plan that region. Topology is a thing you *argue with* before it costs you anything.

When you say "go," OHM geometrizes — pulls the sleeves taut to clearance-exact copper, shoves neighbors aside only where it must, **pours the L2 ground plane with the kernel's boolean engine** so every pad gets a correct thermal-relief spoke and every void is a real subtracted polygon — and then **polishes the physics under gradient**: it widens DDR0 to land exactly on 40Ω, balances the diff-pair skew at the source, stitches a via where USB crosses the plane void to kill the loop, widens VBUS until the thermal model says the rise is under 20°C, and drops teardrop fillets at every pad junction. Before it returns, **it runs the extended `run_drc` on itself** — clearance *and* diff-pair gap *and* split-crossing *and* sliver — so the human never sees illegal copper.

Then you ask the question no other router can answer: *"Why is DDR0 routed this way?"* And OHM tells you, reconstructed from its own cost ledger, not from an LLM narrating after the fact: *"Detoured 1.2mm north because the straight path crossed the GND plane split (loop +0.4nH, your `loop_area` weight). Cost you +0.3Ω of Z0 error — within your 10% tolerance. The L1-direct alternative was rejected: it ran 4mm parallel to the SW node, +6dB crosstalk."* Every "because" dereferences a named cost term. You don't debug the tool's interpretation of your rules; you read the ledger.

That's the feel: **plan you can inspect, copper that's legal by construction, planes poured by a real kernel, physics that's tuned not just checked, and a reason for every wire.**

---

## 4. Architecture — the spine

The spine is **INTENT → PLAN → REALIZE → POLISH → VERIFY**, where PLAN splits into the move no current vcad router has: *topology planning* (discrete, combinatorial, no coordinates) followed by *geometrization* (taut copper, poured planes, stitched vias, teardrops). The discrete half chooses; the continuous half refines; the oracle decides truth — and the oracle is extended to actually cover what OHM steers.

```
┌─ INTENT ──────────────────────────────────────────────────────────────────┐
│ in:  NL + RouteIntent IR (per-net hard|soft+weight, physics targets,       │
│      bus groups, corridors, pour/stitch directives, inter-board links)     │
│ out: compiled, ERC-bound RouteIntent (net-name binding validated, echoed)  │
└────────────────────────────────────────────┬───────────────────────────────┘
                                             │
┌─ PLAN · A: TOPOLOGY (discrete, no coords) ─▼──────────────────────────────┐
│ in:  RouteIntent + placed pads + board solid (board_from_solid keepouts)   │
│ 1. Connection graph: real MST/Steiner ratsnest (fixes ratsnest.rs:80)     │
│ 2. PathFinder negotiated-congestion over the CHANNEL-FACE dual graph:      │
│    route EVERY net each round as shortest SLEEVE; price by present+history │
│    congestion; rip up ALL, re-route. Layer/via-span/side = discrete topo.  │
│ 3. Plane + stitch planning: pour regions as zones, stitch/shield-via       │
│    fences as generated arrays (pitch = lambda/20 or edge rule)             │
│ out: RouteTopology — sleeves, crossings resolved, vias as span-typed topo  │
│      events, pour zones, stitch fences. PROVABLY embeddable. Pure data.    │
└────────────────────────────────────────────┬───────────────────────────────┘
                          ▲ rip-up & re-plan │ (negotiated congestion loop)
┌─ PLAN · B: GEOMETRIZE (legal-by-construction) ▼───────────────────────────┐
│ in:  RouteTopology + SpatialIndex (exact CopperGeom) + BRep boolean engine │
│ 4. Rubber-band pull-taut: each sleeve → clearance-exact polyline+arcs,     │
│    degeneracies resolved by orient2d/incircle (not epsilon)                │
│ 5. Push-and-shove (REAL: relocates copper within its homotopy class),      │
│    bounded effort budget + transactional undo, incremental oracle gates    │
│ 6. POUR + STITCH + TEARDROP via poly2d booleans: knockout planes around    │
│    copper, generate thermal-relief spokes, drop tangent-arc teardrops,     │
│    realize stitch-via arrays — correct-by-construction, not post-cleanup   │
│ out: RoutedPath + FilledZone + ViaArray objects WITH IDENTITY, each owning │
│      its sleeve + cost_ledger. Replaces the flat bag of Trace segments.    │
└────────────────────────────────────────────┬───────────────────────────────┘
                                             │
┌─ POLISH (differentiable, <64-DOF windows) ─▼──────────────────────────────┐
│ in:  RoutedPath (topology FROZEN) + physics targets                        │
│ 7. Waypoints/widths/spoke-params/teardrop-radii → tang-expr Vars. ONE      │
│    smooth loss. Symbolic sparse Jacobian → reuse constraint-solver LM →    │
│    (CPU now / WGSL GPU later). Anneal soft→hard clearance. Project legal.   │
│ out: tuned RoutedPath/zones, snapped to grid                               │
└────────────────────────────────────────────┬───────────────────────────────┘
                                             │
┌─ VERIFY (self-grading, before return) ─────▼──────────────────────────────┐
│ in:  tuned board                                                           │
│ 8. EXTENDED run_drc (region-scoped, f64 — sole legality authority, now     │
│    incl. diff-gap, length/skew, creepage, split-crossing, sliver) +        │
│    physics tuners on ITSELF. Fail → re-POLISH / re-PLAN bumped history /    │
│    escalate. render_view + PCB renderer (pcb.rs) = agent eyes. verify_part │
│ out: {result, reasoning_trace from cost_ledger, drc_delta, physics_report} │
└────────────────────────────────────────────────────────────────────────────┘
```

The load-bearing invariant, enforced by the stage boundaries: **PLAN owns discrete decisions (which-side, which-layer, via-span, via-existence, pour-region, stitch-fence). POLISH only moves continuous coordinates within a frozen topology. VERIFY (f64, extended `run_drc`) is the only thing that decides legality.** No gradient ever crosses a homotopy jump — and no constraint OHM optimizes is left to a DRC rule that doesn't exist.

---

## 5. The Routing Engine — the algorithmic core

### 5.1 The keystone enabler: an incremental legality oracle

Everything downstream asks, millions of times, **"is THIS candidate segment legal, and what is its clearance margin to the nearest other-net copper?"** Today both `drc.rs` and `spatial.rs::from_pcb` (`spatial.rs:377`) rebuild from scratch every call — `from_pcb` does `RTree::bulk_load` (`spatial.rs:447`), and `SpatialIndex` has `insert` (`spatial.rs:452`) but **no `remove`.** In a rip-up loop that rebuilds-per-move is fatal.

OHM wraps the existing, excellent oracle in an **incremental session**:

```rust
RouteSession {
    rtree:    RStar<CopperGeom>,        // gains remove() + rebuild-on-threshold
    uf:       LayerAwareUnionFind,      // incremental connectivity, via spans
    net_ties: NetTieGroups,             // wye/star/center-tap exemptions (unchanged)
}
  probe(candidate)      -> { legal: bool, min_clearance: f64, blockers: Vec<(NetId, geom)> }
  commit(span)          -> SpanId       // transactional
  remove(span_id)       // rip-up without full rebuild
  rollback(checkpoint)  // for bounded shove
```

`probe` is `drc.rs`'s exact clearance pass refactored to per-candidate form: broadphase via the R-tree, narrowphase via `CopperGeom::distance_to` (`spatial.rs:70` — exact capsule/disc/rect), net-tie exemptions via the existing `NetTieGroups` (`drc.rs:82`) so star/wye points never read as shorts. The full multi-pass `run_drc` stays the f64 final gate; `probe` is its hot-loop sibling. **This single change converts DRC from a post-hoc detector into an in-loop avoidance constraint — the structural inversion the whole project is about.**

### 5.2 Extending the oracle to cover what OHM steers

Before any physics gets "tuned," the rules that certify it must exist — otherwise POLISH optimizes a constraint nothing checks. OHM adds five exact-geometry rule types to `DrcRuleType`, each a query the existing spatial index already answers, none of them research:

- **`DiffPairGap`** — reads the `diff_pair_gap`/`diff_pair_width` fields `NetClassRules` already carries (`ecad.rs:369`) and that `drc.rs` currently ignores; checks coupled-segment spacing against the declared gap. *Pure plumbing of existing data into an existing distance test.*
- **`LengthSkew`** — per-net and per-match-group routed length vs target/tolerance; the length math (`length_tune::path_length`) already exists.
- **`Creepage`** — over-surface (not just through-air) spacing for HV nets, slot/cutout-aware via the board-edge geometry the kernel already models. Required for any power/HV claim.
- **`SplitCrossing` / return-path** — flags a signal segment whose reference polygon underneath changes plane or crosses a void with no stitch via within radius. Built on `segment_polygon_intersects` (`spatial.rs:299`) — the kernel already answers this query.
- **`CopperSliver` / min-island** — minimum copper feature and minimum poured-island area, a manufacturability rule the pour engine (§5.5) feeds directly.

This is cheap, geometry-exact work on a proven index, and it is **load-bearing**: it is what lets the doc's central invariant — *f64 `run_drc` decides truth* — remain true for diff pairs, DDR length-matching, HV creepage, and return paths. Until a given rule lands, OHM states the honest fallback explicitly: for that regime the differentiable loss is the only judge, and the agent is told so in the `physics_report`, never silently.

### 5.3 The topology representation: the sleeve

A net's route during planning is **not coordinates.** It is a **sleeve** — a homotopy class stored as pure IR:

```
Sleeve         = ordered list of TangencyEvent
TangencyEvent  = { obstacle: ObstacleId, side: Left | Right, layer: LayerId }
ObstacleId     = Pad | Via | KeepoutVertex | other-net Sleeve
```

"Pass left of U3.7, right of via V12, left of C4.1, cross to L2 at V_new." Two sleeves *cross* iff their tangency sequences interleave inconsistently on a shared layer — a **combinatorial test, no geometry.** The planner's job is to choose sides and layers so no two same-layer sleeves cross: a **planar-embedding problem on the obstacle arrangement**, exactly the structure the half-edge kernel manipulates for BRep faces, with `orient2d` giving left/right orientation exactly. This is the Dayan RBS / TopoR lineage, but expressed *natively* in vcad's IR rather than bolted onto a metric kernel.

Crucially, a via in a sleeve is **not** monolithic. The `Via` IR already carries `start_layer`/`end_layer` as data (`ecad.rs:670`), so blind and buried spans are representable *today*. OHM's planner exploits this: a layer-change event in a sleeve is a *typed via span*, and PathFinder can choose a blind via to save a layer transition or a buried via to free surface real estate, pricing each span's fab cost into the cost field. (Microvia stacks and via-in-pad are a forward extension of the same span model — named in §6.3, not pretended-complete here.)

### 5.4 PLAN: PathFinder negotiated-congestion over sleeves

The net-ordering bug behind today's greedy `route_nets` is **dissolved structurally, not patched**, by adopting McMurchie–Ebeling **PathFinder (1995)** — the algorithm that made FPGAs routable — as the planning loop, but routing *sleeves* over a *congestion* graph instead of wires over a fine grid:

- **Resources are topological channel-faces, not grid cells.** The gap between two adjacent obstacles on a layer (a face in the obstacle arrangement) has a capacity = how many traces fit at min-width+clearance. A via site has capacity 1. A BGA escape corridor has capacity = the rows it can drain. The arrangement is built once via `spatial.rs` R-tree neighbor queries; `query_region` finds which faces a candidate sleeve threads.
- **Every iteration, route ALL nets** as independent shortest-sleeve A* searches over the channel-face dual graph (admissible heuristic = Euclidean pad-to-pad + layer-change penalty). No net is privileged by ordering — *that is the entire point.*
- **Cost of a face** = `base · (1 + history·hist_factor) · (1 + present·present_factor)`. Overfull faces price up immediately (`present`); chronically-contested faces ratchet up permanently (`history`), forcing nets to *find another homotopy class*. This is the principled cure for FreeRouting-class thrashing — a history penalty, not an ad-hoc victim heuristic.
- **The cost field is a `tang-expr` expression.** Congestion pricing, layer-change/via-span penalty, and a *physics prior* (a net with a tight Z0 target pre-prices the narrow channels it can't widen later; an HV net pre-prices channels that violate creepage) all live in one substrate, so the planner and the polisher speak the same language. The `Select` node (`graph.rs:140`) makes the branchy parts of the field smooth where it matters.

PLAN's output is a `RouteTopology`: sleeves, crossings resolved, span-typed vias, pour regions, stitch fences. On dense multi-layer boards, minimum-crossing layer assignment is NP-hard — so PathFinder is honestly a *strong, well-proven heuristic*, with `reroute_region` + escalate-to-human as the escape valve when negotiation stalls, never a false claim of optimality.

### 5.5 REALIZE: geometrize, then shove, then pour

`RouteTopology` is topological; now make it copper. **This is where topological routers historically die** (Toporouter's pull-taut bugginess is legendary), so this is where the engineering budget goes — *not* the elegant planner.

1. **Rubber-band pull-taut.** Each sleeve is a path through a sequence of tangent obstacles; pull it geodesically tight to a sequence of straight segments and arcs tangent to the obstacles it hugs, offset by `width/2 + clearance`. `CopperGeom::distance_to` gives the offsets. **OHM's defense against the graveyard:** vcad has an *exact-predicate kernel*. The degenerate cases that make other rubber-band routers flaky — three collinear obstacles, tangent-to-tangent pinch points — resolve combinatorially with `orient2d`/`incircle` via the `robust` crate, not with epsilon fudging. This is the single best defense available, and no incumbent on a float kernel has it.

2. **Real push-and-shove.** Where taut sleeves over-tighten against each other, OHM invokes a *true* push-and-shove (the KiCad PNS model) that **relocates neighboring copper within its own homotopy class** — replacing today's `push_shove.rs::route_net`, which (confirmed) only Dijkstras around static inflated bboxes and never moves anything, the doc comment notwithstanding. Each shove is **transactional**: propose victim geometry, `probe` it, `commit` (cascading further shoves up to a bounded effort budget) or `rollback` the whole transaction atomically. `spatial.rs` is the shove-collision broadphase; `RouteSession.rollback` makes it cheap.

3. **Pour, stitch, and teardrop with the real kernel — a first-class regime, not a side effect.** This is where OHM steps decisively past today's code: `copper_pour.rs::fill_zone` is a *stub* — it computes clearance cutouts in `collect_clearance_cutouts()` and then literally throws them away (`let _ = cutouts;`, line 60), returning the raw zone outline with no knockout and no thermal-relief spokes, despite the `Zone` IR carrying `thermal_relief`/`thermal_gap`/`thermal_spoke_width` (`ecad.rs:724`). A "world's best autorouter" that cannot pour a ground plane is not credible — and plane fill is *higher-frequency on real boards than any signal-routing flourish.* OHM makes pour correct-by-construction by handing it to the **BRep boolean engine** (`vcad-kernel-booleans` / `poly2d`, already the proven engine behind sheet-metal SCS export): the pour outline minus exact clearance cutouts is a polygon boolean, and thermal-relief spokes are generated geometry, not discarded intent. The same engine generates **stitch-via and shield-via arrays** (pour-to-reference-plane stitching at λ/20, via-fences flanking high-speed traces) emitted by the planner and consumed here, and drops **tangent-arc teardrop fillets** at every pad/via junction — a cheap, kernel-native, fab-table-stakes win that KiCad and Altium do by default and OHM's exact-arc kernel does *natively*. A stitched, poured plane is not cosmetic: it **is** the return path the §7 loop-inductance term reasons about — pour and signal integrity are the same physics.

REALIZE emits **`RoutedPath`**, **`FilledZone`**, and **`ViaArray`** objects — and this fixes the IR's original sin. Today a net's copper is a flat bag of `Trace { start, end, width, layer, net: NetId }` with no path identity; rip-up is filter-by-net and connectivity is reconstructed from geometry every time. A `RoutedPath { id, net, layers, waypoints, vias, sleeve, cost_ledger }` makes rip-up O(drop path), gives POLISH a real object to parameterize, and gives the reasoning trace something to name ("the detour on `RoutedPath[CLK]` between V3 and U2.4"). The `cost_ledger` — per-decision record of *which cost terms drove this routing* — is what the reasoning trace reads from later.

### 5.6 POLISH: differentiable refinement, strictly within a frozen topology

Freeze the topology. **Now and only now** is everything continuous. Each `RoutedPath`'s waypoints and widths, plus pour spoke widths/counts and teardrop radii, become `tang-expr` `Var`s, and OHM descends on **one smooth loss** over a local window:

```
L(window) =  w_len·Σ length
           + w_clr·Σ Select(d>ε, 0, softplus(ε − d))        ← clearance, branchless via Select
           + w_z0·Σ (microstrip_z0(w,t,h,er) − Z_target)²   ← impedance.rs, dZ0/dw PROVEN
           + w_skew·(len(P) − len(N))²                       ← diff-pair / match, AT THE SOURCE
           + w_via·smooth_via_count + w_corner·Σ corner
           + w_xtalk·Σ crosstalk(spacing, parallel_len)      ← estimate_crosstalk, lifted to Scalar
           + w_loop·Σ loop_area(seg, ref_net_polygon)        ← the silent-killer term (§7)
           + w_ir·IR_drop(widths)                            ← pdn.rs, IFT gradient
           + w_temp·Σ temp_rise(J(width), enclosure_sink)    ← thermal.rs, the unexploited asset
           + w_bal·Σ (R_phase_i − R̄)²                        ← motor per-phase resistance balance
           + w_therm·Σ (spoke_R − R_target)²                 ← pour thermal-relief electrical/thermal tradeoff
```

Five disciplines keep this honest, each grounded in a verified constraint:

1. **<64-DOF windows, by hard necessity.** `tang-expr`'s sparsity bitmask is a *runtime assert* — `assert!(idx < 64, "deps() supports at most 64 variables")` (`sparsity.rs:18`). Whole-board differentiation is **impossible**, full stop. OHM decomposes into local windows — a diff-pair corridor, a BGA quadrant, a phase-tie cluster, a pour's spoke set — each under 64 Vars, stitched at fixed boundary waypoints. "Differentiate locally, chain globally" is the same discipline the constraint solver already lives under. **Honest limit:** a dense BGA escape or long coupled bus that won't tile under 64 DOF simply doesn't get differentiable polish — it stays discrete-only. That is a real coverage gap on exactly the hardest boards, and we say so.

2. **Reuse the constraint solver, port not plug.** `vcad-kernel-constraints` already ships LM + symbolic sparse Jacobian (21–84× over finite-diff) + a WGSL backend. OHM writes a *new routing residual* for that proven machinery. Honest scoping: `CompiledSystem::build` today takes sketch-specific `Constraint`/`EntityRef` types, so this is *porting the pattern* to a routing residual, not a free plug-in — weeks of real work, not a config change.

3. **Anneal + snap + re-verify.** Soft clearance → hard, high → low temperature (lets traces tunnel through each other to escape local minima), then **snap to grid and re-verify on f64 extended `run_drc`.** WGSL is f32-only (`wgsl.rs:58`), so the GPU result is always a *proposal*; the f64 oracle is *truth*. This is exactly the `size_impedance` snap-and-re-verify template, generalized.

4. **Project onto the legal set every step.** After each descent step, any waypoint that `probe` says crossed into illegality is clamped. Gradients refine *within* the frozen homotopy; they never silently flip which-side or via-existence.

5. **The terms are honestly staged.** `w_z0`, `w_len`, `w_clr` are gradient-ready **today** (impedance is Scalar-generic, Select is real). `w_temp` wires the existing `thermal.rs` (`analyze_thermal`, `via_thermal_resistance`) to the `pdn.rs` Joule source coupled to the BRep enclosure as a heat sink — turning the North Star's "VBUS 3A under 20°C rise" from a *hard width clamp* into a real gradient-steered objective, grounded in assets that already exist. `w_xtalk` requires lifting `estimate_crosstalk` from `f64` to `Scalar` first (mechanical, mirrors what `impedance.rs` already proves). `w_loop` requires the reference-net abstraction, which is unbuilt (§7). The loss above is the *destination*; §10 sequences which terms light up when.

### 5.7 Where GPU plugs in

Two distinct uses, neither overclaimed. **(a) PLAN:** the N independent shortest-sleeve searches per PathFinder round are independent — a natural fit for parallel dispatch on `vcad-kernel-gpu`. *Honest flag: GPU graph search (irregular frontiers, priority queues, warp divergence) is genuinely hard and is NOT the proven `tang-expr → WGSL` elementwise path — it's a perf moonshot, prove the CPU-rayon version first.* **(b) POLISH:** the loss compiles `tang-expr → WGSL` (`wgsl.rs:28`) and the gradient descends on the existing GPU constraint-solver backend — the path that genuinely matches what that backend already does. The bet is hedged: *the algorithm wins on quality even if the silicon doesn't win on speed.*

---

## 6. vcad's Unfair Advantages

### 6.1 The differentiable engine — `tang-expr`

The whole POLISH stage *is* this asset, and three uses are kept separate. The **PLAN cost field** is a `tang-expr` graph (congestion + history + via-span + physics prior, CSE'd). The **POLISH loss** is symbolic-diff'd exactly as the constraint solver already does (symbolic sparse Jacobian, 21–84× over finite-diff). And `impedance.rs` being `Scalar`-generic (`microstrip_z0<S: Scalar>`, gradient tested `impedance.rs:338`) means **width-to-hit-Z0 is a Newton step, not a search.** The `Select` node (`graph.rs:140`) makes `min`/`max`/`clamp`/clearance-penalty *both* differentiable *and* WGSL-emittable as a branchless ternary — the keystone that lets `softplus(clearance)` carry a real gradient onto the GPU. The hard limit (≤64 vars, `sparsity.rs:18`) is designed *around*, never papered over.

### 6.2 GPU compute — `vcad-kernel-gpu`

PathFinder's per-round parallelism and POLISH's gradient descent both target the existing wgpu stack that already hosts the GPU constraint-solver backend. The `tang-expr → WGSL` path means the POLISH objective and gradient are *generated*, not hand-written. Honest caveat carried through: WGSL is f32, so f64 `run_drc` always certifies; and the *graph-search* GPU use (PLAN) is research-grade, sequenced behind a proven CPU version.

### 6.3 BRep / 3D world, the boolean engine, and mechanical co-design

The board is a real solid in a real assembly, and that is OHM's **uniquest** weapon — spent in *three* places, not one. (a) The **return-path advantage** (§7): the reference plane below each trace segment is a known BRep face, split-crossing detection is `segment_polygon_intersects` (`spatial.rs:299`), loop area is a differentiable functional. (b) **Copper pour and teardrops done by a real solid-modeling boolean engine** (§5.5): `poly2d` knockout and spoke generation are *exact*, the way no float-EDA pour ever is. (c) **Mechanical context flat EDA structurally cannot see:** `board_from_solid` hands the router enclosure inner walls as keepouts, mounting bosses as obstacles, and thermal mass as a heat sink for the `w_temp` term. The `Via` span model (`ecad.rs:670`) is the seed of layer-span-optimal via planning (blind to save layers, buried to free surface, microvia stacks as a forward extension). No flat router can do any of this, because no flat router has the 3D solid.

And the **flagship of this advantage is flex / rigid-flex** — the one regime where "no flat router has the 3D solid" stops being a boast and becomes undeniable. Today's stackup IR (`StackupLayer`/`LayerStackup`, `ecad.rs:304`) is rigid-only: no bend region, no neutral axis, no curved substrate. But OHM's board *is a BRep solid*, so a bend region is a real curved face with a real neutral axis. Flex routing rules — *no trace crosses a bend line except perpendicular, arcs-only inside bends, hatched (not solid) pour in flex zones, signal copper biased toward the neutral axis to survive flex cycles* — are geometric queries the kernel can answer and a flat tool fundamentally cannot. This is staked honestly as a **Legendary-stage moonshot grounded in the kernel** (§10), not claimed as shipping — but it is the regime that makes the 3D claim killer, so OHM owns it rather than over-claiming 3D and spending it only on return paths.

### 6.4 Physics in the loop

`impedance.rs` (Z0, gradient-ready, Scalar-generic), `pdn.rs` (resistor-mesh IR-drop with an implicit-function-theorem gradient `d(drop)/d(width)` — the template for chaining a gradient *through* an embedded solve), `thermal.rs` (`analyze_thermal` + `via_thermal_resistance` — an existing solver, never yet wired into a loss, that the `w_temp` term finally exploits), `signal_integrity.rs::estimate_crosstalk` (NEXT/FEXT — `f64` today, lift to Scalar), `calc_rf` (lumped RLC, PDN target-Z), and `codesign.rs` (differentiable stator+controller rollout). OHM optimizes against *real physics with analytic gradients*, not a scored oracle. **Honest fidelity boundary:** these are closed-form IPC-2141-class models plus a loop-area/slot-resonance and lumped-thermal *proxy* — **not** a Simbeor-grade 2.5D field solver, **not** compliance-grade dBµV/m. The claim is *gradient-steerable approximate physics*, which is unprecedented; the claim is never *certified EMC*.

### 6.5 Agent-native MCP

`RouteIntent`, `RouteTopology`, `RoutedPath`, `FilledZone`, and the reasoning ledger are all IR/JSON — diffable, composable, agent-authored, human-read, and they survive between MCP calls in the `document_id` session. The whole loop runs over MCP with two kinds of eyes: generic `render_view` for assembly sanity, and the **existing PCB renderer** (`vcad-render/src/pcb.rs`) for *routing-specific* vision — layer-colored copper, congestion heatmaps, plane-void overlays — far richer than a generic isometric PNG for an agent reasoning about routes. Vision is for sanity, *not* a clearance check — `run_drc` owns legality. `verify_part`/mecheval is the whole-board grade. The verified-agent-loop thesis — *author → see → measure → verify → iterate* — applied to copper, run by the router *on itself.*

### 6.6 Nets-as-data + IR

The sleeve, `RouteTopology`, `FilledZone`, and `ViaArray` *are* the embodiment of the topology-first thesis as data. Because intent and result are JSON like `create_schematic`'s `nets`, a topology plan can be inspected, edited, and re-geometrized without re-solving. Rip-up is "drop `RoutedPath` P." A net that **spans two boards through a mated connector** is just net continuity declared in the same IR — the seed of inter-board routing (§7). Everything is a diff.

---

## 7. Physics-Correct Routing

**Controlled impedance Z0 — solved, gradient-ready.** `impedance.rs::microstrip_z0`/`stripline_z0` are Scalar-generic with proven gradients. POLISH's `(Z0(w) − Z_target)²` term lands the trace on 50Ω by Newton step.

**Differential pairs — and the missing checker that makes them real.** `Zdiff` via `diff_coupling_k` (`impedance.rs:67`), constant gap held as a sleeve constraint, intra-pair skew minimized **at the source, not end-of-trace**, in the `w_skew` term. The `diff_pair_gap`/`diff_pair_width` intent already lives in `NetClassRules` as data — OHM wires it into the new `DiffPairGap` DRC rule (§5.2) so the gap is *certified*, not merely re-declared in `RouteIntent`. One important correction to the project's own folklore: the meander/length-tuning machinery is **not stranded** — `length_tune::generate_meanders` is already reachable through `push_shove.rs::route_net` (line 208) and `diff_pair.rs::route_pair` (line 99); the real gap is that *neither is exposed over MCP*, and the meander style is trombone-only. So the NOW-list item is "expose and generalize the half-wired length tuner," not "wire a dead one" — precision here protects the doc's "every claim grounded" brand.

**Return paths & loop inductance — the silent killer, and OHM's highest-value missing piece.** A DRC-legal trace that crosses a reference-plane split with no stitch via is *electrically broken and invisible to clearance DRC*. Every flat router misses it. OHM doesn't, because the board is a real BRep solid: the reference polygon under each segment is a known geometric query (`segment_polygon_intersects`, `spatial.rs:299`), and loop area is a differentiable functional. OHM adds a **reference-net abstraction** (per-segment plane assignment + loop-inductance estimator), the `SplitCrossing` DRC rule (§5.2), and the `w_loop` term — so OHM doesn't merely *avoid* split crossings, it *minimizes loop inductance under gradient* and forces a stitch via where a crossing is unavoidable. And because §5.5's pour engine makes the reference plane a *real, stitched poured polygon*, the return path is physically present, not assumed. **Build the pour and the `w_loop` term together, or the "physics-correct" claim has a literal hole.**

**Power copper / PDN, now thermally closed-loop.** `pdn.rs` sizes trace width to a target IR-drop by gradient (the IFT chain through a linear solve), and `size_pdn` is the agent-facing tuner. High-current rails get current-density-sized copper (IPC-2152 class) — but instead of treating `max_temp_rise` as a hard clamp, OHM's `w_temp` term couples `pdn.rs`'s Joule heating into `thermal.rs` with the BRep enclosure as heat sink, so VBUS width is *steered* to the thermal target, not just clamped to a table value.

**High voltage.** The `Creepage` DRC rule (§5.2) enforces over-surface spacing (slot- and cutout-aware via board-edge geometry) — the rule no current vcad DRC has, and a hard requirement before any power/HV-board claim is credible.

**The motor winding / bus-ring case.** `winding_layout` hands OHM the hardest input *for free* — per-coil phase (A/B/C) and polarity (+/−) as data (verified: 9-slot/12-pole needs +/+/+). On the annular stator (`board_from_solid`), the crossing-free interconnect is **nested concentric bus-rings + radial jumpers** — a *provably planar* embedding, which is precisely what the sleeve engine solves natively, and a far safer first target than a dense digital board. POLISH's `w_bal` term widens/narrows ring copper to **equalize per-phase resistance** (torque-ripple-free), and `codesign.rs` folds that balance into the same gradient that tunes Kt/Ke. *Honest scope flag: `codesign.rs` today is a low-DOF motor model — the per-phase-resistance-balance term is net-new modeling, not a free integration. It is legendary if it lands, and it is real work, not a wiring job.*

**Multi-board / connectors / harness — the most differentiated regime, named not faked.** vcad uniquely has the assembly, enclosure, and connectors as a real 3D world, so a net that spans two boards through a mated connector, or a wire harness threaded through an enclosure, is something flat EDA cannot represent at all. OHM models **inter-board net continuity** in the same nets-as-data IR (a net whose terminals live on two different board parts joined by a connector mate) — staked here as a regime the IR should carry and a Legendary-stage target (§10), grounded in the assembly world that already exists, not pretended-shipped.

---

## 8. The Agent-in-the-Loop UX

**Constraints-as-data; NL is the front door but compiles to IR.** A `RouteIntent` block carries per-net `hard | soft+weight` priorities plus physics targets that *point at the tuners*: `{net:"DDR_DQ0", z0:{target:40, tol:0.1, ref:"GND"}, match_group:"DQ", max_skew_ps:5}`, plus pour directives (`{pour:"GND", layer:"L2", stitch:"lambda/20"}`) and inter-board links. An **ERC-style binding-validation pass** (reusing `run_erc`) catches the classic LLM failure — net-name mis-binding, dangling reference, contradictory hard constraints — *before a single trace*, and **echoes the compiled intent back** so the agent debugs its own claims, never the tool's misreading of them.

**The Reasoning Trace — the headline moat, with the discipline that keeps it honest.** Every trace/via/zone emits, *reconstructed from the router's own `cost_ledger`, not from LLM prose*:

```json
{ "net": "DDR_DQ0", "decision": "route L3, detour +1.2mm N around U7",
  "because": ["intent.z0=40Ω", "loss.loop_area", "congestion.face_F8.history=high"],
  "traded_off": [{ "term": "length", "delta_mm": 1.2, "delta_cost": 0.03 }],
  "alternatives": [{ "alt": "L1 direct", "rejected_because": "crosses GND split, +0.4nH loop" }] }
```

**The non-negotiable discipline:** every `because` must dereference a *named cost term* — `congestion.face_F8.history`, `intent.z0`, `loss.loop_area`, `loss.temp_rise` — in the PathFinder field or the POLISH loss. If a justification can't be traced to a real cost gradient, **it doesn't ship.** The moment a `because` becomes LLM narration, the glass box becomes manufactured false trust — *worse* than honest opacity. This is the property Quilter's RL and Flux's LLM-mimicry **structurally cannot** produce: OHM's decisions *are* shortest-sleeve searches and gradient descents, so the explanation *is* the ledger, mechanically.

**The evolved MCP surface — interactive verbs, not one blunt `route_nets`:**

| Tool | Returns | Why it's a trust unlock |
|---|---|---|
| `route_intent(document_id, RouteIntent)` | `{topology_summary, drc_delta, reasoning_trace}` | Full spine; compiled intent echoed + ERC-bound. NL is the door, IR is the contract. |
| `plan_topology(document_id, nets?)` | `RouteTopology` (sleeves, crossings resolved, pour regions, **no copper**) | See which-side/which-layer/which-via-span decisions *as data before geometry*. Approve, then geometrize. |
| `critique_route(net)` | grade + margins, **mutates nothing** | The trust unlock — audit before you commit. "CLK is 48Ω vs 50Ω, 0.08mm margin, 1 unstitched plane crossing." |
| `geometrize(topology, nets?)` | `{paths, zones, drc_delta, physics_report}` | Turn an approved topology into legal copper *and poured planes*. |
| `pour_zone(zone, stitch?)` | `{filled_zone, drc_delta}` | Kernel-boolean pour + thermal relief + optional stitch fence — the regime no current vcad router has. |
| `tune_route(net, targets)` | `{path, physics_report, drc_delta}` | Run POLISH only, on one net's window (incl. `w_temp`, `w_z0`). |
| `reroute_region(bbox\|nets, depth)` | strategic rip-up, depth-bounded + **escalate-to-human exit** | Re-plan a sub-region without touching the rest. |
| `hint(corridor\|layer\|side\|bus_group)` | accepts a topological constraint | Steer the *homotopy*, not the pixels: "address bus on L2," "keep DDR together." |
| `lock_route(net)` / `override(polyline)` | locks / re-verifies | Even a human override re-runs extended `run_drc` — the human can always win, but never illegally. |
| `explain_route(net)` | drills into the cost ledger | "Why is this here?" answered from named cost terms. |

Every mutating verb returns `{result, reasoning_trace, drc_delta}`. `critique_route` and `explain_route` are read-only. That read/write split is the conversational core: **you argue with the router before you let it commit.**

---

## 9. Killer Demos

**1. The FSCW PCB-motor winding interconnect — the closed loop nobody else can close.** An agent calls `winding_layout(slots=9, poles=12)`; vcad returns per-coil phase and polarity *as data*. `add_coil_array` realizes the coils on an annular stator from `board_from_solid`. Today the phase ties are straight lines that cross and `run_drc` flags a forest of shorts. OHM recognizes the annular obstacle arrangement, plans **concentric bus-rings + radial jumpers** (provably crossing-free — same-layer sleeves can't interleave on nested annuli), inserts span-typed vias as topology events where a jumper must hop a ring, pours a stitched return plane, and POLISH balances per-phase resistance (`w_bal`) folded into the magnetics gradient via `codesign.rs`. Extended `run_drc` returns clean (net-ties exempt the star point), the PCB renderer shows a beautiful concentric-ring stator, `calc_motor` confirms torque. **No commercial EDA tool takes winding phase as data, co-designs copper with magnetics, or solves the annular embedding topologically.**

**2. The DDR4 fly-by byte-lane — physics the oracle can finally see.** Eight DQ + strobe, 40Ω±10%, length-matched to 5ps, every segment over a continuous GND reference. OHM plans the topology to keep the lane together (a `bus_group` hint), geometrizes, pours and stitches the L2 ground plane, then POLISH hits 40Ω by Newton step, matches skew *at the source*, and the `w_loop` term **forces a stitch via at every plane split** the lane crosses. The new `DiffPairGap`, `LengthSkew`, and `SplitCrossing` DRC rules *certify* every one of those — the silent killers made visible, fixed, *and checkable*. The reasoning trace reports each Z0 and skew margin as a number. Quilter would *score* this after the fact; OHM *steers* it correct and then *proves* it legal.

**3. The BGA escape + 3-phase GaN inverter, in one engine.** The channel-face capacity model *is* the BGA escape problem — draining N rows through M channels with via-in-pad as span-typed topology events, negotiated-congestion dissolving the net-ordering thrash that defeats greedy routers. The same engine routes the inverter's six gate-drive signals (crosstalk-aware) and three high-current rails (`pdn.rs` current-density-sized, `w_temp` thermally-steered copper) while minimizing the commutation-loop inductance against `calc_rf`, pouring a stitched power plane and dropping teardrops at every via-in-pad. One router, two problems flat EDA face-plants on, both explained.

---

## 10. Staged Roadmap — Now / Next / Legendary

**The non-negotiable sequencing rule:** ship the isolated, low-risk wins and a genuinely-better-than-today classical router *first*. Each step is independently valuable and de-risks the next. The differentiable and GPU moonshots are earned, not assumed. *Invert the doc's own glamour: the elegant planner and the sexy gradient are cheap; the geometrizer and the pour engine are where the project lives or dies, so prove the spine before you spend a year on pull-taut.*

### NOW (buildable on Monday — best-in-class classical core, no new geometry research)
1. **Real MST/Steiner ratsnest.** Replace the sequential chain at `ratsnest.rs:80` with Prim/Kruskal + Steiner for 3+ pin nets. Isolated, tested-in-place, improves every downstream stage. *Days.*
2. **`RoutedPath` IR object.** Path identity to replace the flat `Trace` bag — rip-up becomes O(drop path), and it's the prerequisite for the cost ledger. *Pure data-model change.*
3. **Incremental `RouteSession`.** Add `remove` to `spatial.rs` + the `probe(candidate)` feasibility query refactored out of `drc.rs`'s clearance pass. The keystone enabler. *Bounded, obvious test: incremental result == `from_pcb` rebuild.*
4. **The DRC extension.** Add `DiffPairGap`, `LengthSkew`, `Creepage`, `SplitCrossing`, `CopperSliver` to `DrcRuleType` — wiring the unread `NetClassRules.diff_pair_*` fields and the existing `segment_polygon_intersects` query. **Without this, "f64 `run_drc` decides truth" is false for every premium regime.** Cheap, exact-geometry, load-bearing.
5. **Fix the copper-pour stub.** Make `copper_pour.rs::fill_zone` actually subtract clearance cutouts and generate thermal-relief spokes via `poly2d` booleans (today it discards its own cutouts at line 60). A router that can't pour a ground plane isn't "best." Unlocks the return-path story physically.
6. **A real maze router that *avoids*.** The honest intermediate between straight-line `route_nets` and full topological routing: a gridless single-net A* with real obstacle avoidance (NOT visibility-graph-around-static-bboxes) over the incremental `RouteSession`, auditing and replacing the unproven existing `grid.rs`. Ships avoidance value *without* the sleeve engine or pull-taut, and de-risks the oracle independently of the topology moat.
7. **Expose + generalize the half-wired routers.** `diff_pair.rs::route_pair` is genuinely MCP-unexposed; `length_tune::generate_meanders` is *reachable internally* (`push_shove.rs:208`) but trombone-only and unexposed. Surface both over MCP+wasm; add meander styles. *Plumbing, two real features — stated accurately.*
8. **PathFinder negotiated-congestion** on top of the maze router from #6 — route all nets per round, present+history pricing, rip-up via `RoutedPath`. **The single biggest quality lever, zero new geometry.** CPU-rayon first; prove convergence before any GPU work.
9. **Teardrops + `critique_route` + self-verify loop.** Tangent-arc teardrops via `poly2d` (cheap, kernel-native, fab table-stakes); router runs region-scoped extended `run_drc` on itself before returning `{result, reasoning_trace, drc_delta}`. Near-free trust unlock.

### NEXT (the topology engine + the first real differentiable polish)
10. **Sleeve / `RouteTopology` homotopy planner** — the novel, IR-native moat. Channel-face dual graph over the obstacle arrangement; planar-embedding via PathFinder; span-typed vias exploiting the existing `start_layer`/`end_layer` model.
11. **Rubber-band geometrization** — the historical graveyard. Prototype on the **provably-planar annular motor case first** (low-risk territory) before dense digital boards. Exact predicates are the defense. *Best engineer; most time here.*
12. **Real push-and-shove** replacing the static-obstacle detour — KiCad-PNS-class, transactional, bounded-effort. Needs the incremental oracle first.
13. **Via stitching / shield-via generators + pour-to-plane stitch arrays** — the planner emits, the pour engine consumes. λ/20 fences, edge stitching, high-speed via guards.
14. **Single-net differentiable POLISH** (Z0 + length + clearance) on a <64-DOF window — `size_impedance` generalized to a few waypoints, on CPU closures. A legitimate world-first *scoped to one net*. Lift `estimate_crosstalk` to Scalar to add `w_xtalk`.
15. **Reference-net abstraction + `w_loop`** — per-segment plane assignment, the `SplitCrossing` pass, loop-inductance estimator. The highest-value physics, uniquely vcad's.
16. **Thermal `w_temp` term** — wire `thermal.rs` + `pdn.rs` Joule source to the BRep enclosure heat sink. Turns "VBUS under 20°C rise" from clamp to gradient. Grounded in existing assets — NEXT, not Legendary.

### LEGENDARY (the moonshots, earned)
17. **GPU POLISH** — `tang-expr → WGSL` gradient on the GPU constraint-solver backend, once the CPU loss is proven and f32→f64 snap survives DRC on real boards.
18. **GPU-parallel PLAN** — the negotiated-congestion inner loop as parallel dispatch. Genuine GPU-graph-search research; a perf moonshot. *Hedged: the algorithm already won on quality at step 8.*
19. **Flex / rigid-flex bend-aware routing** — the regime *only* the BRep-3D world can do: neutral-axis copper, perpendicular-only bend crossings, arc-only-in-bend, hatched flex-zone pour. Requires a bend-region extension to the stackup IR. The advantage that makes "no flat router has the 3D solid" undeniable.
20. **Multi-board / connector / harness routing** — inter-board net continuity through mated connectors and harnesses threaded through the 3D enclosure. The most differentiated regime vcad's assembly world enables.
21. **The full motor co-design** — multi-phase resistance-balance model in `codesign.rs`, the annular planar embedding, the return-net plumbing, composed into the single-descent demo. Legendary if they land.

---

## 11. Honest Risk Ledger

| Risk | Severity | Reality | Mitigation |
|---|---|---|---|
| **Geometrization (pull-taut to clearance-exact copper)** | **Make-or-break** | Where every topological router in history dies (Toporouter). 60–70% of real implementation pain lives here, *not* in the elegant planner. | vcad's exact-predicate kernel (`orient2d`/`incircle`) resolves degeneracies combinatorially — the best defense available. Prototype on the *provably-planar annular motor case* first. Bounded transactional shove as fallback. Staff the best engineer here. |
| **The oracle is blind to premium-regime rules** | **Load-bearing** | `DrcRuleType` (`drc.rs:17`) has no diff-gap/length/creepage/return-path/sliver check; `NetClassRules.diff_pair_*` is carried but unread. POLISH would "tune" constraints nothing certifies — the central invariant is false until fixed. | Make the DRC extension a NOW-stage prerequisite (#4) — exact-geometry checks on the existing index, cheap. Until a rule lands, state honestly in `physics_report` that the differentiable loss is the only judge for that regime. Never imply coverage that doesn't exist. |
| **Copper pour is a stub** | High | `fill_zone` discards its own cutouts (`copper_pour.rs:60`); no knockout, no thermal relief. A router that can't pour a plane isn't credible, and the return-path physics depends on a real poured plane. | Fix with `poly2d` booleans (NOW #5) — the kernel to do exact knockout + spokes already ships (sheet-metal SCS export proves it). Pour and `w_loop` ship together. |
| **≤64-var sparsity is a hard `assert`** (`sparsity.rs:18`) | High | Whole-board differentiation is *impossible*. POLISH only applies to subproblems that tile under 64 DOF — and dense BGA / long coupled buses, where physics matters most, are exactly where tiling fails. | Honest coverage gap: those regions stay discrete-only (still excellent — negotiated-congestion + Newton-step width). Never claim board-wide differentiable routing. "Differentiate locally, chain globally" is a thesis, not a theorem. |
| **Real push-and-shove is greenfield** | High | Today's `push_shove.rs` never shoves (confirmed). A real shove engine is KiCad's hardest module; took them years. Two graveyard-grade subsystems (this + geometrization) on the critical path is the classic way these projects die. | Sequence behind the incremental oracle and the proven CPU planner. Bounded effort budget + escalate-to-human prevents the PNS infinite-shove pathology. The NOW core ships value *without* it. |
| **GPU graph search (PLAN)** | Research gamble | Irregular frontiers, priority queues, warp divergence — NOT the proven `tang-expr → WGSL` elementwise path. | Structurally hedged: negotiated-congestion wins on *quality* at NOW-step 8 regardless of silicon. GPU is a perf moonshot, not a dependency. |
| **f32 WGSL vs f64 legality** | Medium | GPU descent in f32; tight-clearance boards can ping-pong "GPU says legal, f64 says no." | f64 `run_drc` is *always* the final arbiter. Route to clearance + ε on the GPU so the snap has slack. Never ship f32 as legal. |
| **Planar embedding is NP-hard on dense boards** | Medium | PathFinder is a *heuristic*, not optimal; a dense DDR/BGA board may return a topology the geometrizer can't realize, re-opening the PLAN/REALIZE seam. | Say "heuristic," not "optimal." `reroute_region` + escalate-to-human is the escape valve, not a pretense of solving NP-hard. |
| **Flex / multi-board are unbuilt regimes** | Honesty | The stackup IR is rigid-only; inter-board continuity isn't modeled. These are the *most* differentiated claims and the *least* shipped. | Stake them as Legendary moonshots grounded in the BRep solid and assembly world — named, scoped, not pretended-done. Don't invoke "3D advantage" and then ignore the regimes that prove it; own flex explicitly or drop the boast. |
| **Physics-model fidelity** | Reputational | IPC-2141 closed-form, not 2.5D field-solved; EMI is a loop-area *proxy*; thermal is lumped, not CFD. A board that passes OHM and fails on a VNA destroys the trust OHM is built to earn. | Claim *gradient-steerable approximate physics*, never *certified EMC*. `critique_route` numbers are margins against analytic models, stated as such. Quilter's Simbeor is a *better forward model* — don't let "differentiable" paper over "lower-fidelity forward model." |
| **The differentiable Rust path isn't on the agent critical path yet** | Honesty | Shipped `size_impedance` solves in TypeScript LM, *not* the `tang-expr` gradient. The Rust differentiable path is tested-but-unwired. | Don't cite `size_impedance` as proof the agent path is already differentiable. Wiring it through MCP is real NEXT-stage work. POLISH is what finally *puts it there*. |
| **Reasoning-trace integrity** | Subtle/reputational | The instant a `because` is LLM-narrated rather than dereferenced from a cost term, the moat inverts to manufactured false trust — worse than no explanation. | Engineering discipline: every justification carries a cost-term pointer or it doesn't emit. Easy to let slip under deadline; non-negotiable. |
| **The AlphaChip trap** | Strategic | Black-box RL as the *core* is contested, unreproducible on public benchmarks, needs proprietary data. | Refuse it. RL/generative may *propose* a topology seed into PLAN; legality and physics guarantees come from the differentiable loss + extended `run_drc`, **never a trusted black box.** The moment the guarantee lives in the net, OHM has become the thing it replaces. |

---

**The bottom line.** OHM plans like a kernel, proves like a solver, pours like a solid-modeler, and explains like a ledger. It refuses to draw a line until it knows which side of the world that line belongs on — and because vcad already thinks in combinatorial topology, exact predicates, differentiable physics, a real boolean engine, and nets-as-data, vcad is the one organization on Earth that can build it. It ships as best-in-class classical routing the moment the NOW list lands (real MST, incremental oracle, the DRC extension, a real maze router that avoids, a real ground pour, negotiated-congestion, self-verify) — already strictly better than today's flag-after-the-fact `route_nets` — closes the motor-winding loop nobody else can close, and stands as the solid ground the topological, differentiable, GPU, flex, and multi-board moonshots are earned upon. Build the boring keystone first. Pour the plane. Then build the legend.