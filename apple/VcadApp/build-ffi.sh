#!/usr/bin/env bash
# Build the vcad-ffi Rust static library and stage it for the SwiftPM app.
# The .a is gitignored (~140MB); run this once after checkout / kernel changes.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

# The Swift packages' CVcadFFI headers are GENERATED mirrors of the canonical
# crates/vcad-ffi/include/vcad_ffi.h. Regenerating them here means the three
# copies cannot drift — a hand-synced header that lags the Rust signatures is a
# miscompile or, worse, a silently wrong call.
echo "Syncing C header from the canonical crate header…"
python3 "$repo_root/scripts/sync-ffi-header.py"

# RELEASE by default. This is a physics engine, not just a geometry kernel: a
# debug build steps the K1 at roughly 1/20th speed, which shows up as RTF 0.29x
# in the app — a robot moving in slow motion, indistinguishable from a physics
# problem. Measured on the 22-DOF K1: 3.3 ms per control step in release, ~66 ms
# in debug, against a 20 ms budget.
#
# CONFIG=debug for a debuggable kernel (breakpoints, overflow checks); expect
# the simulation to crawl.
config="${CONFIG:-release}"
echo "Building vcad-ffi ($config, CPU-only, no-builtin-font)…"
if [ "$config" = "release" ]; then
  cargo build --release -p vcad-ffi --manifest-path "$repo_root/Cargo.toml"
else
  cargo build -p vcad-ffi --manifest-path "$repo_root/Cargo.toml"
fi

mkdir -p "$here/Libs"
cp "$repo_root/target/$config/libvcad_ffi.a" "$here/Libs/"
echo "Staged: $here/Libs/libvcad_ffi.a"
echo "Now: swift build --package-path $here"
