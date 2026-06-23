#!/usr/bin/env bash
# Build the vcad-ffi Rust static library and stage it for the SwiftPM app.
# The .a is gitignored (~140MB); run this once after checkout / kernel changes.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

echo "Building vcad-ffi (CPU-only, no-builtin-font)…"
cargo build -p vcad-ffi --manifest-path "$repo_root/Cargo.toml"

mkdir -p "$here/Libs"
cp "$repo_root/target/debug/libvcad_ffi.a" "$here/Libs/"
echo "Staged: $here/Libs/libvcad_ffi.a"
echo "Now: swift build --package-path $here"
