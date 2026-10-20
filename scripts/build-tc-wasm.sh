#!/bin/zsh
# Build the tang-compress browser decoder into packages/app/public/tc-wasm/
# (gitignored — wasm-pack writes its own `*` .gitignore there).
#   scripts/build-tc-wasm.sh [/path/to/tang-compress]
# Consumed as a built package, not a cargo dep: vcad's ../tang symlink points
# at the main tang checkout, which need not carry tang-compress.
set -e
TC=${1:-$HOME/Developer/tang-compress}
OUT=$(cd "$(dirname $0)/.." && pwd)/packages/app/public/tc-wasm
cd "$TC/crates/tang-compress-wasm"
wasm-pack build --target web --release --out-dir "$OUT" --no-typescript
