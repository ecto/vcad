# GPU Router — the preeminent GPU-accelerated autorouter

**Thesis.** No open or commercial autorouter runs its *entire* inner loop —
search, legality, negotiation, and signal-integrity optimization — on the GPU
with machine-checkable outputs. vcad already owns the three ingredients nobody
else has together: an exact-oracle router with fail-closed receipts, a
production-hard benchmark it can fully account for (the CM5), and `tang` — an
expression-graph stack that JIT-compiles *differentiable* kernels to
Metal/CUDA/WGSL. The plan fuses them.

**Ground truth today.** CPU chain on the CM5: fresh route ~2h, resume ratchet
~1–2h, verdict ladder ~1h, tune+DRC ~15min. Existing GPU spike
(`vcad-kernel-gpu::wavefront`): single-search compacted-frontier relaxation
beats CPU (60ms vs 114ms at 400×400×10) but is unbatched, transfers rasters
per call, and is not wired into `search_route`. tang provides: `tang-gpu`
(expr→WGSL JIT, fused autodiff, GpuAdam trainer), `tang-compute` (native
Metal/CUDA codegen from `tang-expr`), `tang-ad`, `tang-sparse`, `tang-optim`.

**Design invariant (non-negotiable).** The GPU computes *candidates*; the CPU
exact oracle (pair-aware session probe) validates every commit, exactly as the
speculative-batch pattern does today. GPU acceleration can therefore never
change what is legal — only how fast we find it. Receipts stay fail-closed.

---

## Milestone ladder

### M0 — Resident board state (the substrate)
GPU-resident session mirror: per-layer occupancy rasters (BLOCKED/TIGHT/WIDE)
as textures plus the congestion/history field as a tensor, updated
*incrementally* from the session's dirty-grid epochs (upload deltas, never the
board). Deliverable: `GpuSession` in `vcad-kernel-gpu` with an
`apply_dirty(&session)` diff path; parity test raster-vs-raster.
**Exit:** raster upload cost amortized to <1ms/commit on CM5 scale.

### M1 — Batched wavefront farm (search throughput)
One dispatch stream relaxes N independent searches against the frozen resident
state: per-net masks, per-search source/goal sets (route-to-tree seeds), grain
and history costs read from M0 tensors. Wire into `route_batch`'s speculative
search phase and the 49-pair phantom prologue. CPU validates/commits serially,
unchanged. **Exit:** round-0 CM5 search phase ≥5x vs rayon CPU; identical
routability (candidates differ, legality identical by construction).

### M2 — Probe/DRC farm (legality throughput)
Batch narrowphase on GPU sharing M0 rasters: `map_elementwise` kernels for
segment-vs-copper distance over candidate sets, full-board DRC clearance pass,
and raster rebuilds. CPU oracle remains the authority; the GPU pass is a
*prefilter* (anything GPU-flagged legal is re-probed exactly on commit).
**Exit:** full-board DRC <30s (today ~10min); validate-heavy stages 3x.

### M3 — Expression-defined cost model (the tang twist)
Replace hand-WGSL cost math with a `tang-expr` graph compiled through
`tang-compute`: base length + bend + via(span,haul) + grain(layer,dir) +
history + pair-coupling bias + plane-adjacency — all as editable expression
terms. Costs become data: an experiment is an expression edit, not a shader
rewrite. **Exit:** M1 kernels regenerated from expressions with parity on the
CM5 scoreboard; one novel cost term (plane adjacency, task #23) shipped purely
as an expression.

### M4 — GPU PathFinder (negotiation at scale)
Full negotiated-congestion rounds on GPU: all pending nets' fields relax
concurrently; history/present-sharing updates are tensor ops between rounds;
CPU applies keep-best and commits. InstantGR/GAMER-class throughput with our
hard-legality semantics. **Exit:** effort-10 resume negotiation ≤5min (today
~1h); scoreboard parity or better.

### M5 — Differentiable copper (the legendary part)
Routes as polyline/spline control points θ (layers fixed, geometry free).
Board objective built symbolically:
`L(θ) = Σ length + Σ barrier(clearance, pair-gap) + Σ (group skew)² +
Σ (intra-pair skew)² + coupling springs toward the gap`
— i.e. **the si-claims bounds are the loss terms; the receipt is the
objective.** `tang-ad` fuses ∂L/∂θ; `GpuAdam` descends the whole board at
once. Shove, meander tuning, skew matching, and pinch repair collapse into one
optimizer whose fixed point *is* a passing receipt (barriers keep it legal;
the exact oracle re-validates the final geometry, fail-closed). Seeded from
the routed board — this is a polish stage, not a router replacement.
**Exit:** on a routed CM5, descent drives worst_intra_pair_skew and
min_pair_coupled_fraction into the human envelope with zero oracle
violations; the two BROKEN claims flip on a real board.

### M6 — The claim
CM5 full chain (route → negotiate → verdicts → differentiable polish →
receipt) under 30 minutes end-to-end, every connection routed or certified,
receipt Pass, board rendered human-legible. Publish: benchmark table vs
freerouting/tscircuit (routability, wall-clock, DRC, SI claims — nobody else
can fill the last two columns), `docs/` writeup, and the `gpu` feature flag
default-on for `route_nets` effort ≥2.

---

## Sequencing & risk
- M0→M1→M2 are additive infrastructure (weeks-scale, each independently
  shippable behind a feature flag; CPU path remains the fallback forever).
- M3 is a lateral rewrite of M1's kernels — do it once M1 parity exists.
- M4 depends on M0/M3; highest integration risk (keep-best semantics must
  survive batching — the campaign's negotiation lessons apply verbatim).
- M5 is independent of M4 (needs only M0 + tang-ad) and can start as a
  CPU-`f64` prototype of the objective on small boards first — same code,
  three backends, per tang's Scalar genericity.
- Perf numbers are Apple-Silicon Metal first (dev machine), wgpu keeps
  Vulkan/DX12 within reach; CUDA via tang-compute when a box shows up.

## Relationship to the SI campaign
M5 is not a detour from "verified working CM5" — it is the planned closer for
the two claims that remain BROKEN, replacing per-mechanism geometry surgery
(meanders, staggered vias, corridor rips) with a single optimizer whose
objective is the receipt itself.
