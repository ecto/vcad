#!/usr/bin/env bash
# Photoreal image-quality gate.
#
# Renders each scene twice — a high-spp reference and a candidate at the
# normal sample count — and compares them with the `psnr` example. Sampling
# changes in crates/vcad-kernel-raytrace/src/pathtrace.rs must keep every
# scene above MIN_PSNR.
#
# References are large PNGs and are deliberately kept OUT of the repo, under
# $REF_DIR (default /tmp/vcad-photoreal-ref). They are regenerated only when
# missing, or when --regen is passed, so a candidate sweep costs one cheap
# render per scene.
#
# Usage:
#   scripts/photoreal-quality.sh                 # gate current build
#   scripts/photoreal-quality.sh --regen         # rebuild references first
#   REF_SPP=1024 CAND_SPP=32 scripts/photoreal-quality.sh
#
# The candidate render command can be extended with EXTRA_ARGS, which is how
# a flag under test (e.g. --no-adaptive) gets swept against the same
# references.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REF_DIR="${REF_DIR:-/tmp/vcad-photoreal-ref}"
OUT_DIR="${OUT_DIR:-/tmp/vcad-photoreal-cand}"
REF_SPP="${REF_SPP:-1024}"
CAND_SPP="${CAND_SPP:-32}"
SIZE="${SIZE:-800}"
SEED="${SEED:-7}"
MIN_PSNR="${MIN_PSNR:-35}"
EXTRA_ARGS="${EXTRA_ARGS:-}"

REGEN=0
[ "${1:-}" = "--regen" ] && REGEN=1

# BIN can point at a binary built from another revision, which is how a
# candidate is A/B'd against a baseline over one shared set of references.
# Setting it also skips the build, so the comparison binary is not clobbered.
BIN="${BIN:-}"
PSNR="$ROOT/target/release/examples/psnr"

if [ -z "$BIN" ]; then
  echo "building..."
  cargo build --release -p vcad-render --bin vcad-render --example psnr >/dev/null 2>&1
  BIN="$ROOT/target/release/vcad-render"
fi

mkdir -p "$REF_DIR" "$OUT_DIR"
LOG="$OUT_DIR/render.log"
: >"$LOG"

# scene name : path : size
SCENES=(
  "rose-pro:hardware/rose-pro/rose-pro.loon:$SIZE"
  "plate:examples/parametric-plate.vcad:400"
)

status=0
for entry in "${SCENES[@]}"; do
  name="${entry%%:*}"
  rest="${entry#*:}"
  path="${rest%%:*}"
  size="${rest#*:}"

  # References come in two flavours. The un-denoised one is the honest
  # ground truth for the integrator; the denoised one is what a user
  # actually looks at, and is the flavour the gate scores, because a
  # sampling change that the denoiser papers over is not a regression the
  # user can see.
  for variant in raw denoised; do
    ref="$REF_DIR/$name-$variant.png"
    # `dn=()` plus `set -u` is an unbound expansion on bash 3.2 (macOS), so
    # keep the array non-empty by carrying --seed inside it.
    if [ "$variant" = raw ]; then
      dn=(--no-denoise --seed "$SEED")
    else
      dn=(--seed "$SEED")
    fi

    if [ $REGEN -eq 1 ] || [ ! -f "$ref" ]; then
      echo "reference: $name/$variant @ ${REF_SPP}spp ${size}px"
      "$BIN" "$path" --photoreal --spp "$REF_SPP" --size "$size" \
        "${dn[@]}" -o "$ref" >>"$LOG" 2>&1
    fi

    cand="$OUT_DIR/$name-$variant.png"
    # shellcheck disable=SC2086
    "$BIN" "$path" --photoreal --spp "$CAND_SPP" --size "$size" \
      "${dn[@]}" $EXTRA_ARGS -o "$cand" >>"$LOG" 2>&1

    # The raw film at CAND_SPP is pure Monte Carlo noise measured against a
    # 1024spp reference; report it for information, gate only on `denoised`.
    if [ "$variant" = denoised ]; then
      floor=(--min-psnr "$MIN_PSNR")
    else
      floor=(--min-psnr 0)
    fi
    # Capture rather than pipe: a pipeline's exit status is the *last*
    # command's, which would swallow the gate's failure.
    if report="$("$PSNR" "$ref" "$cand" "${floor[@]}")"; then
      verdict=ok
    else
      verdict=FAIL
      status=1
    fi
    printf '%-12s %-9s %-32s %s\n' "$name" "$variant" \
      "$(echo "$report" | tr '\n' ' ')" "$verdict"
  done
done

exit $status
