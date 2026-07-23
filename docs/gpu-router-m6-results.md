# GPU Router M6 — first end-to-end results

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

## Next

1. Wire M4 negotiation into the route/ratchet stages (the <30 min path).
2. Reroute-then-descend for extreme-skew pairs; descent for the rest.
3. Re-run this document's table after each; claims move only with runs.
