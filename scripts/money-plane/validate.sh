#!/usr/bin/env bash
# Validate the Fabricate money-plane migration (027) against an ephemeral
# Postgres: applies it twice (re-runnability) and runs the behavior matrix
# (idempotency, every authz-failure mode, daily cap, allowlists, drift,
# service-role guard). Requires docker + psql. No effect on any real DB.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MIG="$HERE/../../supabase/migrations/027_fabricate_money_plane.sql"
PORT="${PORT:-55440}"
CID=$(docker run -d --rm -e POSTGRES_PASSWORD=pw -p "$PORT:5432" postgres:16-alpine)
trap 'docker stop "$CID" >/dev/null 2>&1 || true' EXIT
export PGPASSWORD=pw
URL="postgresql://postgres@localhost:$PORT/postgres"
for _ in $(seq 1 30); do psql "$URL" -c "select 1" >/dev/null 2>&1 && break; sleep 1; done
psql "$URL" -q -f "$HERE/stub.sql"
psql "$URL" -v ON_ERROR_STOP=1 -q -f "$MIG" >/dev/null   # apply
psql "$URL" -v ON_ERROR_STOP=1 -q -f "$MIG" >/dev/null   # re-apply (idempotency)
psql "$URL" -q -f "$HERE/tests.sql" 2>&1 | grep -iE "NOTICE:|ERROR|FAIL"
