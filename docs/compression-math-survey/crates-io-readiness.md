# crates.io readiness: tang-expr, tang-compile, tang-compress

2026-10-20. Branch `claude/crates-prep` (worktree `/Users/cam/Developer/tang-crates-prep`),
PR https://github.com/ecto/tang/pull/99 (not merged). Nothing published.
All cargo runs were on `mew` (`~/crates-prep/tang`, rsynced copy of the
worktree, `taskset -c 12-23`, cargo 1.98.0).

## crates.io state (API, 2026-10-20)

| crate | on crates.io | needed by |
| --- | --- | --- |
| tang | 0.2.0, 0.2.1 | tang-expr, tang-compile (`^0.2`) |
| tang-la | 0.1.0, 0.1.1 | tang-compress (`^0.1`) |
| tang-ad | 0.1.0, 0.1.1 | via tang-optim |
| tang-optim | 0.1.0, 0.1.1 | tang-compress (`^0.1`) |
| tang-expr | 0.1.0, 0.1.1 | tang-compile, tang-compress (`^0.2` now) |
| tang-compile | — | — |
| tang-compress | — | — |

`git diff b0f680c` (the 0.2.1/0.1.1 release commit) shows no changes in
tang-la, tang-ad, tang-optim since publish; tang changed only in gpu/splatting
code these crates do not use. So no dependency needs a republish.

(Workspace Cargo.toml comment said tang-optim was a 0.1.0 first publish; it is
actually 0.1.1 on the index and in its manifest. Comment corrected.)

## Versions

- **tang-expr 0.1.1 -> 0.2.0.** Not 0.1.2: besides the additive `int`,
  `reverse`, `split` modules, `compile::CompiledMany` gained `+ Send + Sync`
  (breaking for a non-Send closure) and abs/min/max/tanh now trace to `select`
  graphs (diff/sparsity output changes). 0.x minor = breaking digit.
- **tang-compile 0.1.0**, **tang-compress 0.1.0** (workspace version, first publish).
- Workspace dep: `tang-expr = { version = "0.2", path = ... }`.

## Publish order

1. tang-expr 0.2.0
2. tang-compile 0.1.0
3. tang-compress 0.1.0

(tang-compile and tang-compress are independent of each other; both need
tang-expr 0.2.0 on the index.)

## Licenses

Workspace is MIT (root `LICENSE`, Cam Pedersen 2026). Each crate now ships
`LICENSE` (symlink to root, packaged as a regular file). `cargo tree -e normal`
for all three (incl. `--features gpu`, native and wasm32): 116 packages, all
permissive: MIT, MIT OR Apache-2.0, Apache-2.0, ISC (libloading), Zlib
(foldhash), CC0-1.0 (hexf-parse), Unlicense OR MIT (termcolor),
BSD-2-Clause OR Apache-2.0 OR MIT (zerocopy), Unicode-3.0 (unicode-ident).
No copyleft; compatible with MIT. cargo-deny/cargo-license not installed on mew,
so this was checked via `cargo tree -f "{p}|{l}"`.

## Packages (`cargo package --list`)

| crate | files | size (compressed) | contents |
| --- | --- | --- | --- |
| tang-expr 0.2.0 | 27 | 237.7 KiB (58.4 KiB) | src, README, LICENSE, 2 examples |
| tang-compile 0.1.0 | 26 | 895.8 KiB (219.1 KiB) | src, README, LICENSE, 1 example |
| tang-compress 0.1.0 | 38 | 696.1 KiB (189.3 KiB) | src (+gpuwav.wgsl), README, LICENSE, SHIPPED_DECODER.md, 9 examples |

No bench data or models: `include` is source-only, and the only
`include_str!` is `src/gpuwav.wgsl`. Examples that need data read it from CLI paths at runtime.

## Changes in the PR

- Metadata: `documentation = docs.rs/<crate>`, LICENSE in `include`
  (description/keywords/categories/repository/readme already set).
- tang-compress: new README (encode/decode f32 with MaxAbs bound, bound
  modes, `gpu` feature, the unpublished `tang-compress-wasm` crate);
  lib.rs gained a "Features and targets" section.
- tang-expr: integer-dialect doctest in lib.rs plus a README section;
  `examples/int_dialect.rs` (FNV hash over a u8 buffer, CPU result checked,
  WGSL + Rust printed).
- tang-compile: the `ignore`d PassManager snippet is now a real doctest;
  `examples/passes.rs`.
- 6 broken intra-doc links fixed (1 tang-expr, 5 tang-compile).
- `docs/publishing.md` "Next release" section; `scripts/publish.sh` now
  defaults to the three crates and takes crate names as args.
- Features: tang-compress `default = []`, `gpu = [wgpu, pollster]`;
  tang-expr / tang-compile have no features.

## Verification (mew)

- `cargo test --release -p tang-expr -p tang-compile -p tang-compress`: all
  pass (88 + 84 + 62 unit, doctests 3 + 1 + 2).
- Examples run: int_dialect, passes, roundtrip (MaxAbs 1e-3 -> 36x, bound held).
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` for all three and for
  tang-compress `--features gpu`: clean.
- `cargo clippy --no-deps --all-targets` with CI's allow-list, plus
  `--features gpu`: clean.
- `cargo check -p tang-compress --features gpu --target wasm32-unknown-unknown`: ok.
- `cargo check --workspace` (minus the wasm/iOS cdylibs): ok after the bump.
- `cargo fmt --check`: clean.

## Dry runs

- `cargo publish --dry-run -p tang-expr`: **pass**, verified against crates.io
  (tang 0.2.1 from the index).
- tang-compile / tang-compress: a plain dry run fails with
  "failed to select a version for `tang-expr = ^0.2`". That's expected before
  publish. Ran `cargo package -p tang-expr`, then patched crates.io to the
  unpacked tarball:
  `--config 'patch.crates-io.tang-expr.path="<target>/package/tang-expr-0.2.0"'`.
  The tarball's normalized manifest takes `tang` from crates.io, so the
  dependency graph matches the real one. Both dry runs **pass** this way.
  The packaged tang-compress also passes `cargo check --features gpu --all-targets`
  and its doctests.
  (Patching to `crates/tang-expr` doesn't work: it pulls in a second, path-sourced
  `tang` and fails with a spurious E0599 "multiple versions of tang".)

## Open items / blockers

- No hard blockers.
- Published crates that require `tang-expr ^0.1` (tang-gpu 0.1.0, and
  tang-3dgs / tang-mesh when published) keep resolving to 0.1.1. Their next
  publish takes 0.2. Non-blocking.
- tang-llm and tang-compute (owned by the zest session) inherit the
  workspace `tang-expr = "0.2"` requirement through `workspace = true`. I
  didn't edit them. Before publishing they also need tang-compress 0.1 /
  tang-expr 0.2 on the index.
- clippy 1.98 (mew stable) without `--no-deps` flags new lints in **tang-la**
  (`chunks_exact` with a constant size, stats.rs:314/331) and **tang**
  (too_many_arguments without the allow). CI pins 1.94.1, so it is unaffected.
  Out of scope.
- `README.md` code blocks aren't doctested (no `#![doc = include_str!]`). They
  mirror the lib.rs doctests, which are tested.

## Publish-day commands (from the tang repo root, on main after the PR merges)

```sh
git checkout main && git pull
cargo publish --dry-run -p tang-expr
cargo publish -p tang-expr
sleep 30   # index propagation
cargo publish --dry-run -p tang-compile && cargo publish -p tang-compile
sleep 30
cargo publish --dry-run -p tang-compress && cargo publish -p tang-compress
# or: scripts/publish.sh   (defaults to tang-expr tang-compile tang-compress)
```
