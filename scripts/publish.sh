#!/usr/bin/env bash
# Publish the vcad crate closure to crates.io, leaves first.
#
# The order below is the topological order of the publishable closure that
# vcad and kosm consume, taken from `cargo metadata` (see docs/publishing.md).
# `vcad-loon` is not in it: it depends on `loon-lang`, a git rev in a repo that
# has never published, so it stays `publish = false`.
#
# Prerequisites — all of these must already be on crates.io, or the first
# crate that needs them fails:
#   tang 0.2.1, tang-la 0.1.1, tang-expr 0.1.1  (then delete the interim
#                                                [patch.crates-io] in Cargo.toml)
#   phyz 0.4.0 (phyz, -gpu, -model, -math, -md)
#   kosm-render 0.2.0
#
# Usage:
#   CARGO_REGISTRY_TOKEN=... scripts/publish.sh          # publish
#   DRY_RUN=1 scripts/publish.sh                         # cargo publish --dry-run
#
# Stops on the first failure. `scripts/publish-crates.mjs` does the same walk
# with resume, skip-if-already-published and index polling; this script is the
# blunt, auditable version.

set -euo pipefail

cd "$(dirname "$0")/.."

SLEEP_SECS="${SLEEP_SECS:-30}"
EXTRA=()
[ "${DRY_RUN:-}" = "1" ] && EXTRA+=(--dry-run)

CRATES=(
  stepperoni
  vcad-tool-derive
  vcad-ir
  vcad-receipt
  vcad-kernel-acoustics
  vcad-kernel-antenna
  vcad-kernel-math
  vcad-kernel-geom
  vcad-kernel-topo
  vcad-kernel-primitives
  vcad-kernel-tessellate
  vcad-kernel-booleans
  vcad-kernel-calibration
  vcad-kernel-cam
  vcad-kernel-sketch
  vcad-kernel-constraints
  vcad-kernel-cost
  vcad-kernel-raytrace
  vcad-kernel-dfm
  vcad-kernel-em
  vcad-kernel-enclosure
  vcad-kernel-fea
  vcad-kernel-nurbs
  vcad-kernel-fillet
  vcad-kernel-adjoint
  vcad-kernel-gpu
  vcad-kernel-thermal
  vcad-kernel-flow
  vcad-kernel-naming
  vcad-kernel-neutronics
  vcad-kernel-particle
  vcad-gdsii
  vcad-kernel-photonics
  vcad-kernel-qcd
  vcad-kernel-sheet
  vcad-kernel-shell
  vcad-kernel-step
  vcad-kernel-stocksim
  vcad-kernel-sweep
  vcad-kernel-text
  vcad-kernel-tolerance
  vcad-kernel-topopt
  vcad-kernel
  vcad-kernel-drafting
  vcad-kernel-export
  vcad
  vcad-ecad-package
  vcad-ecad-schematic
  vcad-ecad-sim
  vcad-ecad-pcb
  vcad-kernel-diff
  vcad-eval
  vcad-kernel-optics
  vcad-parts
  vcad-render
)

total=${#CRATES[@]}
i=0
for crate in "${CRATES[@]}"; do
  i=$((i + 1))
  echo "==> [$i/$total] cargo publish -p $crate"
  cargo publish -p "$crate" "${EXTRA[@]}"
  if [ "$i" -lt "$total" ]; then
    echo "    waiting ${SLEEP_SECS}s for the index to settle"
    sleep "$SLEEP_SECS"
  fi
done

echo "==> published $total crates"
