#!/usr/bin/env bash
# Build the Typst plugin WASM (crates/vcad-typst) and install it into the
# Typst package at typst/vcad/vcad.wasm.
#
# Requires: rustup target wasm32-unknown-unknown; wasm-opt (binaryen) is
# used when available and skipped (with a warning) otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --profile typst-wasm --target wasm32-unknown-unknown -p vcad-typst

WASM=target/wasm32-unknown-unknown/typst-wasm/vcad_typst.wasm
OUT=typst/vcad/vcad.wasm

if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int "$WASM" -o "$OUT"
else
  echo "warning: wasm-opt not found, shipping unoptimized wasm" >&2
  cp "$WASM" "$OUT"
fi

ls -lh "$OUT"
