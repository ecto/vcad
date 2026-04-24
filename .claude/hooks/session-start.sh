#!/bin/bash
# SessionStart hook for vcad
#
# In a fresh Claude Code on the web sandbox the sibling `tang` workspace is
# missing, `node_modules` is empty, and workspace package `dist/` outputs are
# absent. `cargo build` and `npm run dev -w @vcad/app` both fail under those
# conditions. This hook provisions everything the preview / test / lint
# commands need so the session is immediately useful.
#
# Local runs (no CLAUDE_CODE_REMOTE) exit fast so contributors don't have their
# environment rewritten.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

log() { printf '[session-start] %s\n' "$*"; }

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"
cd "$PROJECT_DIR"

# Pin tang to a known-good commit so cloud preview builds are reproducible.
# Bump this when vcad needs newer tang APIs; running the hook again will
# fetch and check out the new SHA in place.
TANG_REV="289b4a8871933f6500f150bee4dc0a2c81031607"
TANG_DIR="$(cd "$PROJECT_DIR/.." && pwd)/tang"
if [ ! -d "$TANG_DIR/.git" ]; then
  log "cloning tang into $TANG_DIR"
  git clone https://github.com/ecto/tang.git "$TANG_DIR"
fi
if [ "$(git -C "$TANG_DIR" rev-parse HEAD)" != "$TANG_REV" ]; then
  log "checking out tang @ $TANG_REV"
  git -C "$TANG_DIR" fetch --quiet origin "$TANG_REV"
  git -C "$TANG_DIR" checkout --quiet "$TANG_REV"
else
  log "tang already at pinned rev $TANG_REV"
fi

if [ ! -d "$PROJECT_DIR/node_modules" ]; then
  log "installing npm dependencies (npm ci)"
  npm ci --no-audit --no-fund
else
  log "node_modules present, running npm install to reconcile"
  npm install --no-audit --no-fund
fi

log "building workspace packages via turbo (wasm skipped, checked-in artifacts reused)"
# turbo respects the package dependency graph so @vcad/core/engine/ir build
# before consumers. Skip @vcad/app — its dist isn't consumed by the dev
# server (vite serves source directly) and the tsc pass is multi-minute.
VCAD_WASM_SKIP=1 npx --no-install turbo run build --filter='!@vcad/app'

log "ready"
