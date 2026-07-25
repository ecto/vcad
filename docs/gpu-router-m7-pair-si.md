# M7 — universal pair coupling and the SI receipt

Follow-on to [gpu-router-m6-results.md](gpu-router-m6-results.md), whose
scoreboard closed with `receipt Pass | 1/4 HOLDS` and named the levers:
coupled routing during construction, and reroute-then-descend for the
extreme-skew tail. This document records what those levers actually bought,
including where they did not reach.

Claims move only with runs. Every number below is measured.

## The calibration anchor (unchanged)

The human-routed CM5 scores `Pass`, which is what makes the bounds an
*envelope* argument rather than a target. Re-verified at the start and end of
this work, identical both times:

```
worst_group_skew           9.756 mm  <= 10.0   HOLDS
worst_intra_pair_skew      1.074 mm  <=  1.1   HOLDS
min_pair_coupled_fraction  0.857     >=  0.5   HOLDS
vias_per_si_net            2.265     <=  3.0   HOLDS
verdict: ALL HOLD
```

**The bounds were not touched.** Any change to them would invalidate the
argument the receipt exists to make.

## Lever 1 — coupled construction: 17 → 39 of 49 pairs

Method: typed bail reasons (`PairBail`) plus `census_pairs` / the `si_census`
example, which runs the pair stage alone on a copper-stripped board. The
census takes ~15s where a full route takes ~2.7h, and because the board is
empty it isolates geometry and logic failures from congestion.

| census | coupled / 49 | dominant bail |
|---|---|---|
| baseline | 17 | `centerline-search` 28 of 32 |
| + USB `DP`/`DM` in `pair_partner` | 20 | `centerline-search` 28 |
| + narrow-centerline fallback | 22 | `centerline-search` 23 |
| + measured neck-down | 26 | `centerline-search` 22 |
| **+ inner-layer centerline endpoints** | **41** | `centerline-search` 6 |
| + connector pad-layer constraint | 39 | `centerline-search` 5 |

The decisive one is the fifth row. The phantom centerline was pinned to start
and end on the **outer layer**. Inside a BGA the outer layer is solid pads, so
no fat capsule can ever sit there and the search failed outright — 28 of 32
census bails and 48 of 56 in a full route. Pairs escape a BGA the way the
human board does: down a via, coupled on an inner layer *under* the field,
where there are no pads at all. Allowing that took the census 26 → 41 and made
it 9x faster, because failed searches stopped burning the entire retreat
ladder.

The last row is a deliberate loss. Allowing inner-layer legs exposed a latent
connectivity bug — pad connectors searched from *all* copper layers, so one
could start at a pad's XY on a layer the pad is not on and never touch it
electrically. Nothing catches that: the copper is same-net, so the clearance
probe is happy. Constraining connectors to the pad's own layers costs two
pairs and buys correctness.

In a full route the pair-first stage went from **22 to 43** pairs routed
coupled.

## Lever 2 — the SI finishing pass

`router::si_finish` runs three non-regressive, oracle-gated stages:
`polish_pairs` (rip and re-route coupled) → `descend_board` (differentiable
skew descent) → phase compensation.

### Pair skew is structural

The finishing work is shaped by a fact worth stating plainly: a coupled pair's
skew is not sloppy routing. Both legs are offsets of **one** centerline, so at
every bend the outer leg takes the long way and the inner the short way:

```
skew = (w + gap) · Σ_signed 2·tan(θ/2)
```

At this board's 0.2/0.25 geometry that is **~0.9mm for a single net
right-angle turn**, against a 1.1mm bound. It cannot be tuned out by moving
the pair sideways — a lateral shift changes both legs equally and their
difference depends only on the separation. Length has to be added back.

### Compensation capacity

Adding it back has a closed form. With cells of length `3·h` carrying bumps of
amplitude `h`, gain per cell is `2·(hypot(h, h) − h) = 0.83·h` over `3·h` of
run, so capacity is

```
2·(√2 − 1)/3 ≈ 0.276 mm of added length per mm of run
```

**independent of amplitude.** Three consequences, all of which cost a
measurement to learn:

- Small bumps are strictly better — same capacity, less coupled fraction
  spent, less chance of hitting a neighbour. The amplitude ladder climbs.
- Bumps must be tiled by **arclength, not per segment**. A routed leg is a
  simplified maze staircase of ~0.45mm segments; per-segment tiling caps
  capacity at ~0.03mm per segment, so a 10.66mm run reported "cannot reach
  1.312mm" while sitting on ample copper.
- A generic meander generator is useless on a coupled leg: its bends go into
  the twin one gap away ("meanders do not fit" refused 4 of 7 pairs). Bumps
  bulge away from the twin.

### The two pair claims trade against each other

Length added to a coupled leg moves it away from its twin, which costs
`min_pair_coupled_fraction`. Measured: at full amplitude, coupling fell
0.777 → 0.252. Amplitude is therefore capped by the coupling window on
leg-width runs, and a floor keeps every pair well clear of the 0.5 bound.

## Three defects this surfaced

Each was found by a measurement, not by reading code.

1. **Descent was pinching pairs below the gap.** Its fail-closed check dropped
   *both* nets' copper before probing, so it could not see a leg drifting into
   its own partner — the gap spring is a soft term, not a guarantee. Checking
   each leg against the twin *at its descended position* (new-against-old
   rejects 100%, since the pair moves as a unit) plus a +20µm spring cushion
   took subset clearance violations from **4404 to 77**.
2. **`polish_pairs` committed unchecked assemblies**, and worse, the
   incremental session probe and the DRC disagree at mitred pair corners: the
   probe passed legs 0.191mm apart that the DRC rejects against its 0.245mm
   pair-gap rule. Gating on `check_drc_in_region` over the pair's bbox took
   intra-pair violations from **3 to 0**.
3. **The connector straight-hop shortcut emitted copper without probing it** —
   the one path in the stage that bypassed the oracle outright.

## Results

### 40-net subset — receipt Pass

```
worst_group_skew           0.000 mm  <= 10.0   HOLDS
worst_intra_pair_skew      0.983 mm  <=  1.1   HOLDS   (was 1.499)
min_pair_coupled_fraction  0.777     >=  0.5   HOLDS
vias_per_si_net            2.727     <=  3.0   HOLDS
verdict: ALL HOLD
```

Zero intra-pair DRC violations, and the board's total `Clearance` count is
identical with the finishing pass on and off (74) — the pass introduces none.

### 250-net board — 2 of 4, and the important control

```
worst_group_skew           9.158 mm  <= 10.0   HOLDS
worst_intra_pair_skew     37.525 mm  <=  1.1   BROKEN
min_pair_coupled_fraction  0.000     >=  0.5   BROKEN
vias_per_si_net            2.405     <=  3.0   HOLDS
```

This run is what stops the 40-net result from being over-read. Routability
0.957, 36 pairs measured, 17 of 37 re-coupled by polish — and the *same* two
blocker groups as the full board, with `/ETH.3_P` the worst pair on both
(37.5mm here, 37.9mm there).

So the 40-net Pass reflects an **uncongested** board rather than a solved
pipeline: `polish_pairs` can rip and re-route a pair freely when there is
space, and its success rate falls as congestion rises. The finishing pass is
sound and non-regressive at every size; what does not yet scale is its ability
to *land* a re-route on a full board.

### Full board — still 2 of 4

Full route: 2h41m, routability 0.983, 393/408 nets, 830 vias (0.29x human).

```
                            routed      + si_finish     bound
worst_group_skew            9.204 mm     9.297 mm      <= 10.0   HOLDS
worst_intra_pair_skew      38.443 mm    37.853 mm      <=  1.1   BROKEN
min_pair_coupled_fraction   0.000        0.000         >=  0.5   BROKEN
vias_per_si_net             2.830        2.766         <=  3.0   HOLDS
```

The finishing pass re-coupled 19 of 49 pairs on the full board and reverted
none, but the two broken claims are worst-case over every pair and are pinned
by two small, distinct groups:

- **Coupling is pinned at 0.000 by the short `/HS.*` pairs.** These are ~1.8mm
  nets between adjacent pads. Routed as independent singles their legs land on
  *different layers* (`/HS.1_P` on In1Cu, `/HS.1_N` on FCu), and
  `coupled_fraction` requires same-layer twin copper, so they score exactly
  zero. The phantom cannot help: `lead` insets the centerline so the fat
  capsule clears the four pads, which on a ~1.8mm span leaves almost no
  centerline to search.
- **Skew is pinned at 37.9mm by four pairs** (`/ETH.3`, `/USB3-0.RX`,
  `/USB3-1.D`, `/MIPI0.D1`) where one leg took a large detour. Polish now
  triggers on skew as well as coupling — a pair can be 100% coupled and still
  carry 38mm of skew, because coupled fraction is measured on P while the
  extra copper sits on N — but on the congested full board the rip-and-reroute
  attempt fails for these.

Compensation cannot reach either group: 38mm of deficit would need ~138mm of
run at 0.276mm/mm.

## Honest gaps

| target | measured | named closer |
|---|---|---|
| receipt Pass on the full board | 2/4 HOLDS | Two specific groups, above. Short pairs need a coupled path that does not inset a lead — the phantom is the wrong tool below ~4mm span. The detour pairs need polish to succeed under full-board congestion, i.e. a wider corridor rip or a higher-effort re-route. |
| receipt Pass on a subset | ✅ ALL HOLD | 40-net board, end to end through `cm5_bench` — but see the 250-net control above: that Pass reflects an uncongested board, not a solved pipeline. |
| no new route-attributable DRC | finisher: proven (74 → 74). router: **not A/B'd** | The pre-change full-board baseline was not kept, so the routing stage's own delta is unmeasured. A clean pre/post full-board pair is the missing evidence. |

Note on DRC accounting: `Short` counts are **not** route-attributable on this
fixture. The import carries ~906 pad-level shorts, and every routed trace
merges copper clusters, multiplying reported net-pair shorts — routing only 40
nets adds ~409. Use the `Clearance` rule for attribution.

## Next

1. A short-pair path that keeps both legs on one layer without the lead inset
   — this alone unpins `min_pair_coupled_fraction` from zero.
2. Higher-effort polish for the detour pairs, or catching them during
   construction so no leg ever takes the detour. `/ETH.3_P` is the worst pair
   on both the 250-net and full boards, so it is a stable, reproducible
   target rather than a full-board-only artifact.
3. Polish's success rate under congestion is the scaling limit: it works on
   an open board and fails on a full one. Wider corridor rip, or ordering
   polish before the board fills.
4. The clean pre/post full-board DRC A/B.

## M8 re-measurement after #684 — the skew claim was measuring stub copper

Everything below is a fresh full-board measurement taken after #684 (absolute
KiCad pad angles) and its companion short-pair centerline fix landed, on
`e1a5d90d`. It was run to check whether the two broken claims had moved. They
had, but the run also invalidated the *reading* of the skew claim, so this
section supersedes M7's "two blocker groups" diagnosis.

### The calibration anchor, re-verified

Unchanged, and the bounds were not touched:

```
worst_group_skew           9.756 mm  <= 10.0   HOLDS
worst_intra_pair_skew      1.074 mm  <=  1.1   HOLDS
min_pair_coupled_fraction  0.857     >=  0.5   HOLDS
vias_per_si_net            2.265     <=  3.0   HOLDS
verdict: ALL HOLD
```

### The census improved; the receipt got worse

The stripped-board census is now **47 of 49** coupled (bails: `/PCIe.TX_P`
connector, `/USB3-1.RX_P` leg-validation), and the full-board pair-first stage
routes **47** coupled, up from 43. Full route: 980s, routability **0.994**,
380 routed / 28 unrouted. Despite that:

```
                            M7 (pre-fix)   M8 (post-#684)    bound
worst_group_skew            9.297 mm        8.397 mm         <= 10.0   HOLDS
worst_intra_pair_skew      37.853 mm       38.521 mm         <=  1.1   BROKEN
min_pair_coupled_fraction   0.000           0.101            >=  0.5   BROKEN
vias_per_si_net             2.766           3.128            <=  3.0   BROKEN
```

Three of four now break: `vias_per_si_net` **regressed through its bound**
(294 vias over 94 routed SI nets). `min_pair_coupled_fraction` came off zero
— the `/HS.*` group is fixed — but the new worst is `/MIPI0.D1_P` at 0.101.

### What the 38mm skew actually is

M7 read the tail as "pairs where one leg took a large detour" and priced the
fix in compensation capacity (138mm of run for 38mm of deficit). That reading
was wrong. The worst pairs are not detoured — **one leg is not routed at all.**

Measured on the routed board, copper length per leg against the straight
pad-to-pad span of that same net. The ratio needs no connectivity model, so it
is immune to how one chooses to define an island:

| board | legs | legs with copper < 50% of span | median ratio | min ratio |
|---|---|---|---|---|
| human | 86 | **0** | 1.23 | 1.03 |
| ours (M8) | 84 | **7** | 1.11 | **0.01** |

The seven:

```
  0.01  /USB3-0.TX_P            copper  0.23mm   span 18.36mm
  0.02  /MIPI0.D3_P             copper  0.54mm   span 26.64mm
  0.03  /PCIe.RX_P              copper  0.68mm   span 20.19mm
  0.06  /RP1 IO/USB3-1-TXC_P    copper  1.02mm   span 17.31mm
  0.06  /MIPI0.D1_N             copper  1.34mm   span 21.85mm
  0.10  /HDMI0.CLK_P            copper  1.44mm   span 15.15mm
  0.19  /MIPI1.D1_P             copper  5.57mm   span 29.54mm
```

`/USB3-0.TX_P` carries 1% of the copper its own pad span requires. These are
not routes; they are pad escapes and orphaned stubs. `/MIPI1.D1_P` is four
disjoint fragments — a 0.739mm escape at U3.V5, a 0.481mm stub, a floating
4.234mm stub, and a **0.113mm** stub at J4.83 — with 29.5mm of nothing between
them, while its twin `/MIPI1.D1_N` is a complete 38.57mm route.

So the reported skew is very nearly *the length of the leg that did route*:
`/MIPI1.D3_P` 38.52mm skew, `/USB3-1.RX_P` 33.50mm against a polish centerline
of 33.5mm. Compensation cannot close this and should never have been aimed at
it — there is no second leg to match.

Why it reached the receipt unflagged is a single gap, and it is not in the DRC:
**`si_claims` never asks whether a leg is connected.** Its test for "measured" is
`net_routed_length > 0`, which sums any copper carrying the net name, so a pad
escape qualifies. Every one of the seven was *already* reported
`UnconnectedNet` by the DRC at the same time as the receipt was scoring it as a
routed pair.

The second-order reason the resulting numbers looked plausible rather than
absurd is `coupled_fraction`, which normalizes by P alone — "fraction of P
length with same-layer N copper within 1.75x pitch". A stub lying beside a full
twin is 100% coupled, so `/MIPI1.D3_P` reported `coupled 1.000` while carrying
38.52mm of skew. The metric rewards the failure.

> Correction to an earlier draft of this section, kept because the mistake is
> instructive: it claimed the DRC missed these legs because each was bridged by
> copper it was shorted to. That was wrong, and it came from grepping
> `drc_json`'s output — which prints only `Short`, `Clearance`, `MinTraceWidth`
> and `NetIslands`, and counts `UnconnectedNet` without ever printing it. The
> shorts-defeat-connectivity mechanism is real, but it lives in the *pruner*,
> not in `UnconnectedNet` — see below.

## The prune was being defeated by shorts

`dangling_copper_mask` is documented as removing copper whose island "touches no
pad and no pour fragment **of their net**", but it judged anchoring over the
board-global touch graph and accepted *any* pad in the component. So a dead
island of net X that merely brushed net Y was swept into Y's component, saw Y's
pads, and read as anchored. On a board carrying 845 shorts that is not a corner
case.

Fixed by judging anchoring over same-net connectivity (`build_same_net_dsu`,
which reuses the contact list the global pass already builds, so it costs one
linear walk and no extra broadphase). Net ties still connect, since intentional
junctions are what `NetTieGroups::exempt` is for. `UnconnectedNet` gets the same
treatment for the same reason — a short is not a substitute for a route.

Measured on the routed CM5, re-pruning the saved board:

| | before | after |
|---|---|---|
| `NetIslands` | 93 | **17** |
| `Clearance` | 173 | **150** |
| `Short` | 845 | 815 |
| `UnconnectedNet` | 253 | **253** |
| total | 3400 | 3156 |

`UnconnectedNet` holding at 253 is the soundness evidence: 393 traces and 52
vias were deleted and **no net lost a connection**. Route-attributable
`Clearance` drops from +150 to +127, and `worst_group_skew` from 8.397mm to
3.579mm, because that dead copper was being counted as routed length.

## Spurs: the other half of the excess copper

Removing dead *islands* is not enough. A dead-end branch hanging off an island
that *does* reach pads is kept by an island-level pruner, and those are large
here — comparing each net's total copper against the shortest pad-to-pad path
through it:

| net | total | shortest pad-to-pad path | spur |
|---|---|---|---|
| `/USB3-1.DP` | 72.86mm | 36.60mm | **36.26mm** |
| `/USB3-0.DP` | 70.11mm | 35.95mm | **34.17mm** |
| `/MIPI1.D2_P` | 68.08mm | 34.67mm | **33.41mm** |

That is a ~34mm unterminated stub on a USB3 differential pair — a real
transmission-line defect, not just a bookkeeping error.

`drc::spur_copper_mask` / `prune_spur_copper` implement the finer pass: iterated
leaf removal on the same-net contact graph, where a node is anchored if it is a
pad or pour. A non-anchored node of degree ≤ 1 cannot lie on a path between two
pads, so its copper goes and its neighbour's degree drops; repeat to a fixpoint.
Removing a leaf can never disconnect what remains, and the measurement agrees:
run over both boards it changed **neither** board's `UnconnectedNet` count
(human 291 → 291, ours 253 → 253) while dropping `NetIslands` on the human board
from 128 to 27.

Applied to the routed board it recovers a claim: `vias_per_si_net` **3.106 →
2.626, HOLDS** — the spurs were carrying the vias that pushed it over.

**It is deliberately not wired into `route_all` yet.** For a net the router only
partially reached, *every* piece of its copper is a dead end, so enabling the
pass reclassifies those nets as unrouted and moves `routability`. The existing
`chained_copper_anchored_through_via_kept` test pins the current island-level
contract precisely there (a chain reaching one pad, kept as a unit), and it
fails under the finer rule — correctly, because that chain really is a stub.
Reconciling the contract needs its own full-board before/after, so it is a
separate change rather than something smuggled in here.

## What is left, and why compensation is still the wrong tool

After both prunes and the receipt gate, `worst_intra_pair_skew` is **unchanged
at 38.521mm**. `/MIPI1.D3_N` holds 77.03mm of copper for a 34.54mm pad span, all
of it on pad-to-pad paths and reported `continuous` with 2/2 pads — so it is
neither dead copper nor a spur but a genuine redundant **cycle**: two parallel
routes between the same points, which leaf-pruning cannot touch because every
node has degree ≥ 2. That is the signature of a re-route that did not remove the
path it replaced.

So the remaining skew is real excess copper on a real route, and the fix is
cycle removal (keep a spanning structure over the net's pads, drop the rest) plus
finding the stage that leaves the loop behind — not compensation, which at
0.276mm/mm would need ~138mm of run to absorb 38mm and would be adding copper to
a net that already has twice what it needs.

## The receipt is now fail-closed on unrouted legs

`si_claims` gained a fifth claim, `si_pairs_incomplete` (bound 0), and now
excludes a pair from the skew and coupling claims when either leg holds less
copper than its own pad span requires. Excluding without reporting would be
worse than the original bug — it would let a board pass by leaving the hard
pairs as stubs — so the exclusions are counted and bounded.

The gate deliberately does **not** use DRC continuity. That was tried first and
is disqualifying: the CM5 fixture is reverse-engineered, its imported copper has
sub-tolerance gaps, and continuity therefore reports **25 of the human board's
49 pairs** as discontinuous, which would break the calibration anchor. The
length test reads only copper length and pad positions, so it is immune to that.

The threshold is `MIN_LEG_SPAN_FRACTION = 0.5`, calibrated like every other
bound to the human board, whose thinnest leg is `/LPDDR4 RAM/DQS1_C_A` at 0.522
(6.81mm across a 13.04mm span). It cannot be 1.0 even though a connected path
must span its pads, because copper runs pad-edge to pad-edge rather than
centre-to-centre and correctly routed legs land just under
(`/LPDDR4 RAM/DQS0_T_B` 0.999, `/HDMI0.TX0_P` 0.994). The ~4% margin is thin and
is the reason this is a screen for *absent* copper, not a length check. It is a
necessary, not sufficient, condition — `UnconnectedNet` remains the
authoritative completeness check; this only stops the receipt from *measuring*
legs that are plainly not routes.

`examples/starved_legs` names the offending legs, the same way
`pair_coupled_fraction` names the pairs behind `min_pair_coupled_fraction`.

### The anchor still passes, unchanged

The whole point of the exercise, re-verified after every change:

```
worst_group_skew           9.756 mm  <= 10.0   HOLDS
worst_intra_pair_skew      1.074 mm  <=  1.1   HOLDS   (over 49 pairs, 0 excluded)
si_pairs_incomplete        0         <=  0     HOLDS
min_pair_coupled_fraction  0.857     >=  0.5   HOLDS
vias_per_si_net            2.265     <=  3.0   HOLDS
verdict: ALL HOLD
```

Every original number is identical to M7's. **No bound was changed**, and the
stripped-fixture DRC baseline is also unchanged at 311 short/clearance, 23
`Clearance` — the connectivity fixes do not move it.

## Scoreboard

| claim | M7 | M8 measured | bound | |
|---|---|---|---|---|
| `worst_group_skew` | 9.297mm | **3.579mm** | ≤ 10.0 | HOLDS |
| `worst_intra_pair_skew` | 37.853mm | 38.521mm | ≤ 1.1 | BROKEN |
| `si_pairs_incomplete` | *(did not exist)* | 9 | ≤ 0 | BROKEN |
| `min_pair_coupled_fraction` | 0.000 | 0.221 | ≥ 0.5 | BROKEN |
| `vias_per_si_net` | 2.766 | 3.106 → **2.626** with spur prune | ≤ 3.0 | BROKEN / HOLDS |

Two claims recovered (`worst_group_skew` decisively, `vias_per_si_net` once the
spur pass lands), one new claim now makes the unrouted pairs visible instead of
silently feeding the other two, and the headline skew is diagnosed to a
reproducible cause with a named fix.

## What did not work

- **`descend_board` is inert on a full board: 0 of 41 pairs tuned, 41
  rejected.** M7 credits descent with driving reachable pairs to ~0.006mm, and
  that reproduces in isolation — but every attempt is rejected by its
  fail-closed clearance check once the board is full. The lever M7 named as the
  answer to residual skew contributes exactly nothing at the size that matters.
- **Phase compensation is real but tiny in reach**: 3 pairs meandered
  (`/ETH.1_P` 0.889 → 0.145mm, `/LPDDR4 RAM/DQS0_C_B` 0.872 → 0.029mm), with
  **38 pairs still over tolerance**. It works only on pairs already close.
- **Coupling degrades after construction.** Construction couples 47 of 49, yet
  by the end of routing 10 pairs sit below 0.5 and polish recovers only 4. The
  loss happens in the negotiation / rip-up stages, which re-route pair legs
  independently. M7 framed the limit as polish's success rate; the earlier and
  larger effect is that the board *un-couples* work already done.
- **DRC continuity as the receipt's routed-ness gate** — disqualifying on this
  fixture, as above (25 of 49 human pairs).
- **Wiring the spur prune into `route_all`** — sound, but it moves the
  routability contract, so it is staged separately.
- **Chasing the four claims to HOLDS on this board would have been false.** With
  legs unrouted and the pair claims blind to it, a green receipt would assert an
  SI envelope over copper that is not a route.

## Next

1. Redundant-cycle removal on signal nets — the last mechanism behind the
   38.5mm skew, and the only one left that compensation would otherwise be
   pointed at. `/MIPI1.D3_N` (77.03mm of copper, 34.54mm span, continuous, 2/2
   pads) is the reproducible target.
2. Find the stage that re-routes a pair leg without removing the path it
   replaced. The cycle is the evidence; the culprit is upstream of the pruners.
3. Wire in `prune_spur_copper` with the routability contract reconciled, which
   lands `vias_per_si_net` for real.
4. Stop the negotiation / rip-up stages from un-coupling pairs that construction
   already coupled — 47 coupled becomes 10 below 0.5 with no stage claiming
   responsibility.
