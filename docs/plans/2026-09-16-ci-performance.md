# CI performance: first pass

## Baseline

PR #888's stable job took 47m50s on run 35118982750. Its cargo test
step took 41m19s; nightly's took 42m34s. TypeScript took 4m52s and
cached WASM took 24s. The preceding stable run (35117391848) missed its
Rust cache, spent 20m22s compiling tests and approximately 15 minutes
executing them, then uploaded a roughly 3 GB cache. Its helix_sweep
binary alone took 309 seconds.

These are GitHub-hosted Linux measurements, not local Mac benchmarks.

## Changes

- Cancel superseded PR runs of CI and Kernel Torture Track. Main,
  scheduled, and manual runs have unique concurrency groups and are retained.
- Run stable on every PR and main push. Run both stable and nightly on
  the daily schedule and manual dispatch. Nightly remains informational.
- Save the Rust compilation cache when later validation fails.
- Use pinned nextest 0.9.122 to schedule tests across binaries. Preserve
  all non-ignored workspace tests and no-fail-fast behavior; no retries or
  new test exclusions. Keep the optimized Cargo profile unchanged.
- Retain cargo doctests separately, including after nextest assertion failures.
  Keep example builds, documentation, IR drift checks, the full torture corpus,
  KiCad export verification, TypeScript checks, and artifact/audit gates.
- Separate test compilation from execution, and upload Cargo timing HTML
  and nextest JUnit XML for seven days, including on failures.

## Validation and measurement

`actionlint` validates both changed workflows. The pinned local nextest runner
passed 145 CAM/FFI tests across five binaries (one existing ignored test),
and generated the configured JUnit report. CAM/FFI doctests pass separately.
All six helix_sweep integration tests also pass under nextest (196 seconds
execution on the local Mac). Full Linux results and wall-clock improvement
must be measured on the PR.

Compare warm runs with warm runs and cold runs with cold runs. The first pass
reduces duplicate runner work and serial test scheduling; it does not claim
10x lower latency. Moving nightly off PRs primarily reduces runner cost.

## Next steps after measurements

1. Inspect timing reports for expensive compilation and the JUnit distribution
   for long-running tests. Investigate warm-cache misses before adding caches.
2. Consider a build-once archive plus balanced test shards if execution remains
   dominant; measure the cost of transferring multi-GB archives first.
3. Add conservative changed-crate/reverse-dependency selection only with tests
   for renamed/deleted files, shared fixtures, build scripts, feature changes,
   and fallback-to-full behavior. Keep full stable coverage on main.
4. A Swift-only fast lane needs native build/test coverage before skipping
   Linux checks; current Linux CI does not validate the Swift app.

References:
- https://nexte.st/docs/installation/pre-built-binaries/
- https://nexte.st/docs/ci-features/partitioning/
- https://github.com/Swatinem/rust-cache
