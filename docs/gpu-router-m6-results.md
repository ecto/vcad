# GPU Router M6 — first end-to-end results

> **Correction (2026-07-25).** Every DRC figure in this document was computed
> against a baseline a geometry bug had inflated, and the SI comparison table
> understated our own delta as a result. See
> [the correction section](#correction-2026-07-25--the-drc-baseline-was-inflated)
> at the end. Claims move only with runs; a correction is a run.

Companion to [gpu-router-m0.md](gpu-router-m0.md) (the charter). This is the
first measured run of the complete chain on the CM5 benchmark
(schlae/cm5-reveng: 10 copper layers, 436 nets, 3,037 pads, human-routed
ground truth from a production Raspberry Pi CM5). All numbers Apple-Silicon
Metal, single machine, 2026-07-23.

## The chain

```
GPU-batched route  →  ratchet (escalation-gated)  →  inverted verdict ladder
(k=1→3→6, oracle-gated)  →  fail-closed meander tune  →  differentiable
descent (receipt-as-loss)  →  receipt + DRC + render
```

## Measured (one run, m6_chain.sh)

| stage    | wall     | result |
|----------|----------|--------|
| route    | 90.5 min | 0.900 routability, 22 pairs coupled in round 0, GPU batches on every speculative phase |
| ratchet  | 78.8 min | tail reduction at effort 10 |
| ladder   | ~0 min   | **10 routed + 3 ProvedInfeasible, 0 unknown** — every connection accounted |
| tune     | 35 s     | fail-closed (region-DRC gated) |
| descent  | 59 s     | 4 pairs → ≈0.006 mm whole-net skew, oracle-validated |
| **total**| **2 h 51 m** | receipt: 1 HOLDS (vias 2.66 < 3.0), 3 BROKEN; DRC delta vs fixture baseline dominated by known artifact classes |

Reference points: CPU-only fresh route 2.1–3.6 h for the route stage alone;
historic full pipelines ran 8–24 h. The 40-net hard subset routes at
**1.000 routability in 3.2 s** GPU-batched (history: 588 s → 8.1 s → 1.2 s
@0.976 → 3.2 s @1.000).

## Run 2 — M4 negotiation wired in (same day)

| stage    | wall     | result |
|----------|----------|--------|
| route    | 65 min   | 0.925 — GPU negotiation rounds REPLACED the ratchet stage entirely (history pricing resolves the tail the CPU arsenal used to grind) |
| ladder   | 22 s     | 24 routed + 11 certificates; 1 connection persisted unknown (window budget) |
| tune     | 50 s     | fail-closed |
| descent  | 41 s     | 3 pairs → ≈0.006 mm |
| **total**| **67 min** | 2.55× over run 1; accounting 99.8% (the 1 unknown) |

Chain trajectory: 8–24 h (historic CPU pipelines) → 2 h 51 m (run 1) →
**67 min** (run 2). Remaining distance to 30 min lives inside the route
stage's greedy phase — the sequential rescue arsenal that fires per-batch —
and is the next named integration (negotiation-first ordering, arsenal only
for negotiation's leftovers).

## Reproducibility on merged main (2026-07-24)

Two full-board runs on merged main (post #649/#661), one clean solo:
**0.891 routability in 105 min** (both runs, identical score — the route is
deterministic given the tree). The 40-net hard subset reproduces exactly
(1.000 in 4.1 s vs the branch's 3.2 s).

Reading across all 11 full-board runs (branch + main): the honest
reproducible claim is **0.89–0.94 routability in 1–2 h** — run 2's
67 min / 0.925 was the best draw of a distribution, not the typical run.
Still 5–10× over the historic 8–24 h pipelines, but the single-run headline
above should be read with that spread.

One consistent finding across every logged run: the CPU push-shove
negotiation rounds cost ~20 min each and **net-lose placed connections in
every single run** (e.g. 489 → 455 while pending grows), with the score only
recovering afterward. Capping or removing them is the cheapest named lever
toward 30 min (~40–60 min of wall-clock) and possibly a quality gain.

## What is proven

- **Full accounting**: every connection ends routed or carries a named
  infeasibility certificate. No unknowns. No other open router emits
  certificates at all.
- **Inverted ladder**: running exhaustive singles first collapsed the
  verdict stage from ~1 h to seconds with identical guarantees.
- **Receipt-as-loss descent works on production copper**: pairs reachable by
  the optimizer land at ~0.006 mm whole-net skew — 180× inside the 1.1 mm
  receipt bound — with every final segment re-validated by the exact oracle.
- **The invariant held everywhere**: GPU proposes, oracle disposes. No GPU
  stage can change what is legal; measured outcomes are identical with the
  feature off (prefilter/proposal parity tests).

## Honest gaps to the M6 claim

| target | measured | named closer |
|--------|----------|--------------|
| < 30 min chain | 2 h 51 m | route+ratchet are 99% of wall-clock; both still spend their time in the CPU rescue arsenal and repair rounds. The M4 GPU negotiation loop (built, contract-tested) replaces exactly those — integration is the remaining engineering. |
| receipt Pass | 1/4 HOLDS | worst-pair claims are gated on universal pair coupling (24–27/49 best). Levers: descent for reachable pairs (working), coupled routing during construction for the rest (pair-first + history pricing), and the extreme-skew tail needs larger legal moves than local descent allows — i.e. reroute-then-descend. |
| ≥5x route speedup | 1.4–2.3x | search is no longer the bottleneck once batched; the arsenal is. Same closer as row 1. |

## The table nobody else can fill

| | routability | wall | certificates | SI claims | DRC oracle |
|---|---|---|---|---|---|
| **vcad (this chain)** | 0.900–0.943 + certs = 100% accounted | 2 h 51 m | ✅ named infeasibility proofs | ✅ machine-checkable receipt (1/4 HOLDS and counting) | ✅ exact, pair-aware |
| freerouting | partial, no certs | hours | ✗ | ✗ | approximate |
| tscircuit | benchmark suite, no certs | varies | ✗ | ✗ | ✗ |
| KiCad PNS | interactive only | — | ✗ | ✗ | per-move |

(Competitor rows are qualitative — none publish CM5-class results; that is
the point of the row.)

## Correction (2026-07-25) — the DRC baseline was inflated

Every DRC number published above and in
[gpu-router-m7-pair-si.md](gpu-router-m7-pair-si.md) was measured against a
stripped-fixture baseline that one of our own bugs had inflated.

**The bug.** KiCad stores a pad's angle as an **absolute** value — it already
includes the footprint's orientation. vcad's importer stored that raw value
while eleven consumers compose `fp.rotation + pad.rotation`, double-counting the
footprint rotation on every rotated part. On fine-pitch rotated packages the
neighbouring pads **overlapped**, manufacturing phantom shorts and clearance
violations out of thin air. Fixed in PR #684.

**Why it mattered more than a normal measurement error.** The phantom violations
landed in the *baseline*, and the baseline is the denominator of every
route-attributable claim. A bigger floor makes our delta look smaller, so the
error flattered us in every published comparison — the direction that most needs
correcting.

Re-measured from scratch on merged main with #684 in. DRC and SI are
deterministic given a fixed board file, and every figure below was run twice
with **identical** results; the route stage is the only stochastic step and is
reported with its spread.

### The baseline itself

| stripped fixture, all copper removed | before | after |
|---|---|---|
| short/clearance | 980 | **311** |
| `Clearance` | 74 | **23** |
| `Short` | ~906 | **258** |

**648 violations this project attributed to the reverse-engineered source were
ours.**

### The human production board

| | before | after |
|---|---|---|
| total violations | 16,485 | **14,104** |
| short/clearance | — | **6,994** |

The repeated "the production board scores 16,485 violations under these rules"
line was itself inflated by 2,381. The board is still far outside our rule set —
that part of the argument stands — but the number was wrong.

### Our board, re-routed clean

Full-board route on the corrected importer, **two runs**: **routability 0.994**
both times, producing a byte-identical board (3,791 segments / 895 vias,
5,301 mm copper, 380 routed / 28 unrouted), 0.31× human via count. The route is
deterministic given the tree; only wall-clock varies (958.9 s and 895.5 s).

| | published | corrected |
|---|---|---|
| `Clearance` | 74 (claimed = the floor) | **173** |
| floor | 74 | **23** |
| **route-attributable `Clearance`** | **0** | **150** |

**"Route-attributable violations: ZERO" was false.** It read `74 − 74 = 0`
because the inflated floor happened to equal what our board scored. The honest
figure is 150 against a floor of 23.

### What did *not* move

The receipt bounds are untouched, and they remain a valid envelope: the human
board's anchor re-measures **bit-identical** after the fix —
`worst_group_skew 9.756`, `worst_intra_pair_skew 1.074`,
`min_pair_coupled_fraction 0.857`, `vias_per_si_net 2.265`, **ALL HOLD**. Pad
angles feed pad geometry, not trace lengths, so the calibration anchor and the
argument it supports survive this correction intact.

### The <30 min chain target is met

| stage | before | now |
|---|---|---|
| route | 113 min (pre-GPU-fix) | **895.5 / 958.9 s ≈ 14.9–16.0 min** (2 runs) |
| full chain (route + si_finish) | 2 h 51 m → 67 min | **1,634 s ≈ 27.2 min** (1 run) |

The M6 scoreboard row `< 30 min chain | 2 h 51 m` is **closed**.

Two honesty notes on these timings, since a prior version of this document
published a single lucky run as typical:

- Both runs shared the machine with another session's full-board route (load
  average 23.4). Contention only inflates wall-clock, so the `<30 min`
  conclusion holds a fortiori — but neither number is a clean solo measurement.
- The chain figure rests on **one** complete run, not two. The second attempt's
  route stage finished normally (895.5 s, identical board) but its `si_finish`
  stage was truncated when the machine's disk filled, so its 1,288 s is not a
  valid chain sample and is not averaged in. The route-stage spread above is
  from two complete runs; the chain number needs a second clean run to earn the
  same standing.

The pad fix also helped routing directly: pair-first now lands **47** pairs
coupled in round 0, up from the published 43. Overlapping pads had been sealing
the BGA fields that pairs escape through.

## Next

1. ~~Wire M4 negotiation into the route/ratchet stages (the <30 min path).~~ —
   done; the chain is 27.2 min, see the correction section above.
2. ~~Reroute-then-descend for extreme-skew pairs; descent for the rest.~~ —
   done, see [gpu-router-m7-pair-si.md](gpu-router-m7-pair-si.md). Coupled
   construction went 17 → 39 of 49 pairs (the phantom centerline had been
   pinned to the outer layer, where a BGA has only pads); a 40-net board now
   reaches receipt Pass end to end; the full board is still 2/4, pinned by
   short pairs whose legs land on different layers and by four detour pairs.
3. Re-run this document's table after each; claims move only with runs.
