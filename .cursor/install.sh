#!/usr/bin/env bash
# Cloud Agent install script for vcad.
#
# Idempotent bootstrap that prepares both the TypeScript workspace (web app,
# packages) and the Rust workspace (kernel, CLI). See CLAUDE.md for background
# on the sibling `tang`/`phyz`/`loon` workspaces and the checked-in kernel WASM.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARENT_DIR="$(cd "$REPO_ROOT/.." && pwd)"

log() { printf '\n[install] %s\n' "$*"; }

# --- System deps ------------------------------------------------------------
# The native `clipper-sys`/`geo-clipper` crates compile C++ with the default
# clang toolchain, which selects the newest installed GCC (14) for its libstdc++
# headers and link libraries. Make sure those are present.
if ! printf '#include <vector>\nint main(){return 0;}\n' | c++ -x c++ - -o /tmp/.cxxcheck 2>/dev/null; then
  log "Installing C++ standard library headers (libstdc++-14-dev)"
  sudo apt-get update -qq
  sudo apt-get install -y --no-install-recommends build-essential pkg-config libstdc++-14-dev
fi
rm -f /tmp/.cxxcheck

# --- Rust toolchain ---------------------------------------------------------
# Several dependencies (via the pinned phyz git rev) require edition2024, which
# is only stable on Rust >= 1.85.
if command -v rustup >/dev/null 2>&1; then
  log "Updating Rust stable toolchain"
  rustup update stable
  rustup default stable
fi

# --- Sibling workspaces -----------------------------------------------------
# The Cargo workspace path-depends on tang, phyz and loon checked out next to
# the repo root (../tang, ../phyz, ../loon). Access is granted via the
# repositoryDependencies field in environment.json.
ORIGIN_URL="$(git -C "$REPO_ROOT" remote get-url origin)"
BASE_URL="${ORIGIN_URL%/ecto/vcad*}"

clone_sibling() {
  local name="$1"
  local dest="$PARENT_DIR/$name"
  if [ -d "$dest/.git" ]; then
    log "Sibling '$name' already present at $dest"
    return 0
  fi
  log "Cloning sibling workspace '$name' into $dest"
  if ! mkdir -p "$dest" 2>/dev/null; then
    sudo mkdir -p "$dest"
    sudo chown "$(id -u):$(id -g)" "$dest"
  fi
  git clone --depth 1 "$BASE_URL/ecto/$name" "$dest"
}

clone_sibling tang
clone_sibling phyz
clone_sibling loon

# --- JavaScript / TypeScript ------------------------------------------------
cd "$REPO_ROOT"
log "Installing npm dependencies"
npm ci

# Build all workspace packages. VCAD_WASM_SKIP=1 reuses the checked-in kernel
# WASM artifacts instead of rebuilding them with wasm-pack.
log "Building TypeScript workspace packages"
VCAD_WASM_SKIP=1 npm run build --workspaces --if-present

log "Install complete"
