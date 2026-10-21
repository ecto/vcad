#!/usr/bin/env bash
# Live end-to-end test of vcad's built-in agent loop against frog api, run on
# mew (Linux). Builds crates/vcad-app/examples/agent_live.rs and appends its
# JSON report to ~/agent-live/results.jsonl.
#
# Key: ~/.config/zest/frog-test-key.env (ZEST_BASE_URL / ZEST_API_KEY), copied
# from the Mac with `scp ~/.config/zest/frog-test-key.env mew:.config/zest/`
# then `chmod 600`. Never commit or echo it.
#
#   scripts/agent-live-mew.sh            # FROG_WAIT_SECS etc. pass through
#
# Daily timer (optional): see the "agent-live timer" section at the bottom.
set -euo pipefail

REPO="${VCAD_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
KEY_FILE="${FROG_KEY_FILE:-$HOME/.config/zest/frog-test-key.env}"
OUT_DIR="$HOME/agent-live"
mkdir -p "$OUT_DIR"

if [[ ! -r "$KEY_FILE" ]]; then
  echo "agent-live: missing $KEY_FILE" >&2
  exit 1
fi
set -a
# shellcheck disable=SC1090
source "$KEY_FILE"
set +a
export FROG_BASE_URL="${FROG_BASE_URL:-${ZEST_BASE_URL:-}}"
export FROG_API_KEY="${FROG_API_KEY:-${ZEST_API_KEY:-}}"
unset ZEST_API_KEY

export CARGO_INCREMENTAL=0
cd "$REPO"
cargo build -q -p vcad-app --features agent-live --example agent_live >&2
BIN="${CARGO_TARGET_DIR:-$REPO/target}/debug/examples/agent_live"

set +e
REPORT="$("$BIN")"
CODE=$?
set -e
echo "$REPORT" >> "$OUT_DIR/results.jsonl"
echo "$REPORT"
exit "$CODE"

# --- agent-live timer -------------------------------------------------------
# Installed on mew as a user systemd timer (agent-live.timer) firing 15:05 UTC
# daily; the run polls /status for up to 4h (FROG_WAIT_SECS=14100) so it
# catches the 15:00–19:00 UTC window. Disable:
#   systemctl --user disable --now agent-live.timer
