# Kernel torture track

A public robustness scoreboard for the vcad BRep kernel. The track runs a
**752-case deterministic adversarial corpus** — coincident/tangent boolean
configurations, near-degenerate slivers, chained curved booleans, seeded
random primitive pairs, STEP round-trips, and tessellation watertightness —
and grades every case:

| class | meaning |
|---|---|
| **pass** | operation succeeded and the result is watertight with a volume inside op-specific sanity bounds |
| **graceful-refusal** | operation returned a structured error (fail-closed — acceptable, not a pass) |
| **bad-geometry** | operation "succeeded" but the result is non-watertight or volume-insane |
| **timeout** | case exceeded the per-case timeout (20 s) |
| **crash** | panic or abort |

The corpus lives in [`crates/vcad-torture`](../crates/vcad-torture) and is
generated from constants plus a seeded splitmix64 PRNG — no time- or
platform-dependent randomness, so every run everywhere sees identical cases.
Each case executes in its own subprocess (panic + timeout isolation), so one
kernel abort can't take down the run.

## Running it

```bash
# Full corpus (~1–2 s wall on a laptop after the build)
cargo run --release -p vcad-torture --features vcad-kernel/no-builtin-font -- \
  run --baseline crates/vcad-torture/baseline.json --check

# Fast PR subset (362 cases)
cargo run --release -p vcad-torture --features vcad-kernel/no-builtin-font -- \
  run --subset pr --baseline crates/vcad-torture/baseline.json --check

# One case, in-process (for debugging under a debugger / RUST_BACKTRACE=1)
cargo run --release -p vcad-torture --features vcad-kernel/no-builtin-font -- \
  run-case rand-011
```

CI (`.github/workflows/torture-track.yml`) runs the PR subset on any pull
request touching `crates/vcad-kernel*`, failing on **any per-case class
regression** vs the checked-in baseline
(`crates/vcad-torture/baseline.json`); pushes to `main` and a nightly
schedule run the full corpus and publish the scoreboard to the job summary.

After a deliberate kernel change shifts results (in either direction),
refresh the baseline:

```bash
cargo run --release -p vcad-torture --features vcad-kernel/no-builtin-font -- \
  run --write-baseline crates/vcad-torture/baseline.json
```

Baseline policy: for a case that flip-flops between classes (see kernel
nondeterminism below), record its **worse** class so CI never fails on the
lucky/unlucky flip; the runner additionally re-confirms any regression with
two isolated retries before failing.

## Scoreboard (baseline, 2026-07-17)

| category | cases | pass | refusal | bad-geo | timeout | crash | pass rate |
|---|---:|---:|---:|---:|---:|---:|---:|
| boolean-chain | 24 | 0 | 0 | 24 | 0 | 0 | 0.0% |
| boolean-coincident | 93 | 91 | 0 | 2 | 0 | 0 | 97.8% |
| boolean-random | 400 | 299 | 0 | 101 | 0 | 0 | 74.8% |
| boolean-sliver | 33 | 20 | 0 | 13 | 0 | 0 | 60.6% |
| boolean-tangent | 57 | 42 | 0 | 15 | 0 | 0 | 73.7% |
| step-roundtrip | 60 | 59 | 0 | 1 | 0 | 0 | 98.3% |
| tessellation | 85 | 65 | 0 | 20 | 0 | 0 | 76.5% |
| **total** | **752** | **576** | **0** | **176** | **0** | **0** | **76.6%** |

## Fixed by the first run

- **Crash: `pipeline.rs` index-out-of-bounds on one-sample SSI curves** —
  12/752 cases panicked at `crates/vcad-kernel-booleans/src/pipeline.rs`
  (`evaluate_curve`): a degenerate `Sampled`/`TwoSampled` intersection curve
  with exactly one point underflowed `len - 2` and indexed `points[1]`.
  Fixed (return the single sample); crashes went 12 → 0.

## Triaged findings (open)

All remaining findings are **bad-geometry** — no crashes or timeouts survive
in the corpus. Grouped by root-cause family, roughly ordered by impact:

1. **Boolean seam watertightness on curved results** (~101 `rand-*`, all 24
   `chain-*`, 15 `tan-*`, 13 `sliv-*`, 2 `coin-cube-cylcap-*`): boolean
   results involving trimmed curved faces (especially *rotated* cylinders/
   cones and chained curved cuts) tessellate with open boundary edges —
   T-junctions along trim seams. This is the known boolean seam-conformity
   gap (PR #469 landed T-junction repair for part of it; the
   freeze-at-boolean-entry completion is the open work). Every `chain-*`
   case fails at its first rotated-tool step, which is why that category
   sits at 0% — it's one root cause, not 24.
2. **Torus primitive tessellation is empty** (`tess-torus-*`,
   `tess-torus-fat-*`, 10 cases): `tessellate_brep` (the `Solid::to_mesh`
   path) has no `SurfaceKind::Torus` arm — toroidal faces fall into the
   planar fallback and produce nothing. Adding the obvious arm (mirroring
   `tessellate_face`) is *not* safe on its own: the boolean pipeline leaves
   torus faces untrimmed and currently relies on them being dropped
   (`test_torus_boolean_subtract` codifies this). Needs torus trim-curve
   support in the splitter first, then the arm.
3. **Inverted/degenerate cone tessellation leaks** (`tess-cone-inverted-*`,
   5 cases): a cone with `r_top > r_bottom` tessellates with open boundary
   edges at every segment count.
4. **Tiny-solid tessellation is empty** (`tess-sphere-tiny-*`, 5 cases): a
   0.1 µm-radius sphere produces an empty mesh — segment-count sizing
   truncates to zero for sub-tolerance radii.
5. **Kernel nondeterminism** (`step-bool-40`, flaky): the same
   boolean + STEP round-trip alternates between pass and a 6% volume drift
   run-to-run — `HashMap` (random-state) iteration order affects a
   pipeline decision somewhere. Worth hunting independently of the drift
   itself: a deterministic kernel is a prerequisite for byte-reproducible
   receipts.

The per-case details for every non-pass case are in
`crates/vcad-torture/baseline.json` (`details` map).
