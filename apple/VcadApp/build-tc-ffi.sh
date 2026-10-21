#!/usr/bin/env bash
# Build the cram-codec field decoder static lib (C ABI, CPU reference decoder) and
# stage it in Libs/ next to libvcad_ffi.a. Built from an ecto/tang checkout
# (crates/cram-codec; default ~/Developer/tang), outside the vcad workspace.
#   ./build-tc-ffi.sh [/path/to/tang]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tc="$(cd "${1:-$HOME/Developer/tang}" && pwd)"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
sed -e "s#@TC@#$tc#" -e "s#@SRC@#$here/tang-compress-ffi/src#" \
  "$here/tang-compress-ffi/Cargo.toml.in" > "$work/Cargo.toml"
cp "$tc/Cargo.lock" "$work/" 2>/dev/null || true
CARGO_TARGET_DIR="$tc/target/tc-ffi" cargo build --release --manifest-path "$work/Cargo.toml"
mkdir -p "$here/Libs"
# Copy into the real directory, never through a symlinked Libs (it once
# overwrote the main checkout's lib).
if [ -L "$here/Libs" ]; then echo "Libs is a symlink — refusing to copy through it" >&2; exit 1; fi
cp "$tc/target/tc-ffi/release/libtang_compress_ffi.a" "$here/Libs/"
echo "Staged: $here/Libs/libtang_compress_ffi.a ($(git -C "$tc" rev-parse --short HEAD))"
