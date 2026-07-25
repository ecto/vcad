# M7 — universal pair coupling and the SI receipt

> **Correction (2026-07-25).** The results below were measured against a DRC
> baseline our own pad-rotation bug had inflated (PR #684), and the "40-net
> subset — receipt Pass" headline does not reproduce. Corrected figures are in
> [the correction section](#correction-2026-07-25--re-measured-after-the-pad-rotation-fix)
> at the end; the numbers in the body are left as originally published so the
> record shows what moved. Claims move only with runs; a correction is a run.

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

## Correction (2026-07-25) — re-measured after the pad-rotation fix

vcad's KiCad importer stored pad angles as read, but KiCad's pad angle is
absolute (it includes the footprint's orientation) while eleven consumers
compose `fp.rotation + pad.rotation`. Rotated fine-pitch packages therefore had
**overlapping pads**, inflating the DRC baseline every attribution claim in this
document is measured against. Fixed in PR #684; see
[gpu-router-m6-results.md](gpu-router-m6-results.md#correction-2026-07-25--the-drc-baseline-was-inflated)
for the full baseline correction.

Everything below is re-measured on merged main with #684 in. DRC and SI are
deterministic on a fixed board; every figure was run twice with identical
results.

### The 40-net subset was not a Pass

The headline "40-net subset — receipt Pass (ALL HOLD)" is **withdrawn**. It also
failed to reproduce on merged main *before* this fix (measured 2026-07-25:
`worst_intra_pair_skew` 1.347mm against the 1.1mm bound, i.e. 3/4), so the
original Pass was a property of that one branch state, not a reproducible
result.

| 40-net subset | published | corrected (2 runs, identical) |
|---|---|---|
| worst_group_skew | 0.000 HOLDS | 0.000 HOLDS |
| **worst_intra_pair_skew** | **0.983 HOLDS** | **1.353 BROKEN** (bound 1.100) |
| min_pair_coupled_fraction | 0.777 HOLDS | 0.582 HOLDS |
| vias_per_si_net | 2.727 HOLDS | 2.474 HOLDS |
| **verdict** | **ALL HOLD** | **3 of 4** |

Routability is unchanged at 1.000 (4.7–5.1 s over two runs).

**The claim that survives**: the finishing pass still introduces no clearance
violations. Published as "the board's total `Clearance` count is identical with
the finishing pass on and off (74)". The 74 was the inflated floor; corrected,
the 40-net board scores `Clearance` **23** — *exactly* the stripped-fixture
floor. Same finding, honest number.

### The full board is 1 of 4, not 2 of 4

Full-board route on the corrected importer, two runs: routability **0.994**
both times, byte-identical board (3,791 segments / 895 vias) — the route is
deterministic given the tree. Wall-clock 895.5 / 958.9 s.

| full board | published (routed + si_finish) | corrected |
|---|---|---|
| worst_group_skew | 9.297 HOLDS | **8.397** HOLDS |
| worst_intra_pair_skew | 37.853 BROKEN | **38.521** BROKEN |
| min_pair_coupled_fraction | 0.000 BROKEN | **0.101** BROKEN |
| **vias_per_si_net** | **2.766 HOLDS** | **3.128 BROKEN** (bound 3.000) |
| **verdict** | **2 of 4** | **1 of 4** |

`vias_per_si_net` crossed the bound: 294 vias over 94 routed SI nets. The board
now routes more connections (0.994 vs 0.983) and pays for them in vias, so this
claim broke as coverage improved — worth stating plainly rather than reporting
only the routability gain.

`min_pair_coupled_fraction` moved off exactly zero (0.101) because pair-first
now couples **47** pairs in round 0, up from 43 — overlapping pads had been
sealing the BGA fields pairs escape through. Still far under the 0.5 bound, and
the short `/HS.*` pairs diagnosed above remain the pin.

### DRC attribution, corrected

| | published | corrected |
|---|---|---|
| stripped-fixture floor, `Clearance` | 74 | **23** |
| our full board, `Clearance` | — | **173** |
| **route-attributable `Clearance`** | claimed **0** | **150** |

The note "the import carries ~906 pad-level shorts, and routing only 40 nets
adds ~409" is corrected: the import carries **258** pad-level shorts, and the
40-net route adds **27** (285 total). The *conclusion* is unchanged and still
correct — `Short` counts are not route-attributable on this fixture, because
every routed trace transitively merges same-net copper clusters. Use `Clearance`
for attribution.

### The bounds are untouched, and still valid

The human board's anchor re-measures **bit-identical** after the fix —
9.756 / 1.074 / 0.857 / 2.265, ALL HOLD. Pad angles feed pad geometry, not
trace lengths, so the calibration anchor is unaffected and the envelope
argument stands. No bound was adjusted.

### The open A/B is now closed on the honest baseline

The "no new route-attributable DRC — router: **not A/B'd**" gap above is
answered: against the corrected floor of 23, the router's own delta is
**+150 `Clearance`**. That is the missing evidence, and it is not zero.

## Next

0. Reduce the 150 route-attributable clearance violations — now measurable
   against an honest floor, and the router's real debt.
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
