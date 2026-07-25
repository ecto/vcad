#!/usr/bin/env bash
# Build vcad-ffi for the Vision Pro simulator and stage it for SwiftPM.
# aarch64-apple-visionos-sim is a tier-3 Rust target: nightly + build-std, and
# the vendored third_party/clipper-sys (workspace [patch]) supplies the libc++
# link arm the registry crate is missing.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

echo "Building vcad-ffi for aarch64-apple-visionos-sim (nightly, build-std)…"
SDKROOT="$(xcrun --sdk xrsimulator --show-sdk-path)" \
  cargo +nightly build -p vcad-ffi \
  --manifest-path "$repo_root/Cargo.toml" \
  --target aarch64-apple-visionos-sim \
  -Zbuild-std=std,panic_abort --release

mkdir -p "$here/Libs"
cp "$repo_root/target/aarch64-apple-visionos-sim/release/libvcad_ffi.a" "$here/Libs/"
echo "Staged: $here/Libs/libvcad_ffi.a"
