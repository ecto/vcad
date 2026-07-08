#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd ../.. && pwd)"

echo "[vcad-mcp] Building workspace packages..."

# ── 1. Build workspace packages ──────────────────────────────
cd "$REPO_ROOT"
npm run build -w @vcad/ir
npm run build -w @vcad/engine
npm run build -w @vcad/core
npm run build -w @vcad/mcp

cd "$REPO_ROOT/services/mcp"

# ── 2. Prepare Build Output API structure ────────────────────
OUT=".vercel/output"
rm -rf "$OUT"
mkdir -p "$OUT/functions/mcp.func"

echo "[vcad-mcp] Bundling with esbuild..."

# ── 3. Bundle entry.ts with esbuild ─────────────────────────
# Uses npx esbuild@latest because the repo's esbuild (0.14) is too
# old for `import ... with { type: "json" }` (needs 0.21+).
# @resvg/resvg-js ships native .node binaries that esbuild cannot
# bundle; keep it external and ship the package alongside the bundle.
#
# The banner imports createRequire under a private alias (__vcadCreateRequire)
# so it never collides with the top-level `import { createRequire } from
# "node:module"` that bundled sources (server.ts, wasm/ecad-diff.ts) emit —
# two unaliased `createRequire` bindings would throw
# `SyntaxError: Identifier 'createRequire' has already been declared` at load.
#
# __VCAD_VERSION__ is baked in from @vcad/mcp's package.json: the bundle flattens
# server.ts away from its sibling package.json, so its source-relative require
# resolves to nothing and server_info would otherwise report 0.0.0.
MCP_VERSION="$(node -p "require('$REPO_ROOT/packages/mcp/package.json').version")"

# Build identity baked at build time so server_info, the MCP `initialize`
# handshake, and /health can fingerprint the exact running commit. On Vercel
# VERCEL_GIT_COMMIT_SHA is set during the build; fall back to git, then a
# sentinel. Baking (vs reading process.env at runtime) is deterministic — it
# doesn't depend on the project's "expose system env vars at runtime" setting,
# whose absence is what made a stale serverless instance indistinguishable
# from a fresh one.
BUILD_SHA="${VERCEL_GIT_COMMIT_SHA:-$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}"
BUILD_TIME="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "[vcad-mcp] Baking version $MCP_VERSION build ${BUILD_SHA:0:7} ($BUILD_TIME) into bundle"

npx esbuild@latest entry.ts \
  --bundle \
  --platform=node \
  --target=node20 \
  --format=esm \
  --external:@resvg/resvg-js \
  --define:__VCAD_VERSION__="\"$MCP_VERSION\"" \
  --define:__VCAD_BUILD_SHA__="\"$BUILD_SHA\"" \
  --define:__VCAD_BUILD_TIME__="\"$BUILD_TIME\"" \
  --outfile="$OUT/functions/mcp.func/index.mjs" \
  --banner:js="import { createRequire as __vcadCreateRequire } from 'module'; const require = __vcadCreateRequire(import.meta.url);"

# ── 3b. Bundle edge middleware (export-control geo-block) ───
# Build Output API deploys don't get Vercel's automatic middleware.ts
# detection, so the geo-block middleware is bundled explicitly as an edge
# function and wired in via a `middlewarePath` route below.
mkdir -p "$OUT/functions/_middleware.func"
npx esbuild@latest middleware.ts \
  --bundle \
  --platform=browser \
  --format=esm \
  --outfile="$OUT/functions/_middleware.func/index.js"

cat > "$OUT/functions/_middleware.func/.vc-config.json" << 'EOF'
{
  "runtime": "edge",
  "entrypoint": "index.js"
}
EOF

# ── 4. Copy WASM binary next to the bundle ───────────────────
cp "$REPO_ROOT/packages/kernel-wasm/vcad_kernel_wasm_bg.wasm" \
   "$OUT/functions/mcp.func/"

# ── 4b. Ship resvg (native PNG rasterizer) next to the bundle ─
# render.ts degrades to raw SVG if the import fails, so a missing
# package is non-fatal — but include it when installed.
if [ -d "$REPO_ROOT/node_modules/@resvg" ]; then
  mkdir -p "$OUT/functions/mcp.func/node_modules/@resvg"
  cp -R "$REPO_ROOT/node_modules/@resvg/." \
        "$OUT/functions/mcp.func/node_modules/@resvg/"
fi

# ── 5. Function config ──────────────────────────────────────
cat > "$OUT/functions/mcp.func/.vc-config.json" << 'EOF'
{
  "runtime": "nodejs20.x",
  "handler": "index.mjs",
  "maxDuration": 60,
  "memory": 1024,
  "launcherType": "Nodejs",
  "supportsResponseStreaming": true
}
EOF

# ── 6. Output config (routing) ──────────────────────────────
cat > "$OUT/config.json" << 'EOF'
{
  "version": 3,
  "routes": [
    {
      "src": "/.*",
      "middlewarePath": "_middleware",
      "continue": true
    },
    {
      "src": "^/$",
      "status": 308,
      "headers": { "Location": "https://docs.vcad.io/reference/mcp/overview" }
    },
    {
      "src": "/mcp",
      "methods": ["POST", "GET", "DELETE", "OPTIONS"],
      "dest": "/mcp"
    },
    {
      "src": "/health",
      "methods": ["GET"],
      "dest": "/mcp"
    },
    {
      "src": "/artifacts/(.*)",
      "methods": ["GET", "HEAD", "OPTIONS"],
      "dest": "/mcp"
    },
    {
      "src": "/live/(.*)",
      "methods": ["GET", "POST", "OPTIONS"],
      "dest": "/mcp"
    },
    {
      "src": "/oauth/(register|authorize|start|callback|token)",
      "methods": ["GET", "POST", "OPTIONS"],
      "dest": "/mcp"
    },
    {
      "src": "/\\.well-known/oauth-authorization-server(/.*)?",
      "methods": ["GET", "OPTIONS"],
      "dest": "/mcp"
    },
    {
      "src": "/\\.well-known/oauth-protected-resource(/.*)?",
      "methods": ["GET", "OPTIONS"],
      "dest": "/mcp"
    }
  ]
}
EOF

# ── 7. Publish this commit as `expected_build_sha` to Edge Config ───
# Lets a RUNNING instance learn a newer build exists and flag itself stale
# (server_info.is_stale, every tool result's _meta, and the initialize banner) —
# the fix for warm-instance staleness masking a fresh deploy. Edge Config is a
# globally-replicated runtime KV, so this reaches every warm lambda within its
# read TTL without a redeploy or a drain.
#
# Best-effort and fully gated: skipped (never fails the build) unless both
# VERCEL_API_TOKEN and VERCEL_EDGE_CONFIG_ID are set in the project's env.
# VERCEL_TEAM_ID is appended when present (required for team-scoped configs).
if [ -n "${VERCEL_API_TOKEN:-}" ] && [ -n "${VERCEL_EDGE_CONFIG_ID:-}" ]; then
  echo "[vcad-mcp] Publishing expected_build_sha=${BUILD_SHA:0:7} to Edge Config..."
  EDGE_URL="https://api.vercel.com/v1/edge-config/${VERCEL_EDGE_CONFIG_ID}/items"
  if [ -n "${VERCEL_TEAM_ID:-}" ]; then
    EDGE_URL="${EDGE_URL}?teamId=${VERCEL_TEAM_ID}"
  fi
  set +e
  curl -sS -X PATCH "$EDGE_URL" \
    -H "Authorization: Bearer ${VERCEL_API_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "{\"items\":[{\"operation\":\"upsert\",\"key\":\"expected_build_sha\",\"value\":\"${BUILD_SHA}\"}]}" \
    -o /dev/null -w "  Edge Config PATCH → HTTP %{http_code}\n" \
    || echo "  [vcad-mcp] Edge Config publish failed (non-fatal) — staleness detection degrades to env-only."
  set -e
else
  echo "[vcad-mcp] Edge Config publish skipped (VERCEL_API_TOKEN / VERCEL_EDGE_CONFIG_ID unset)."
fi

echo "[vcad-mcp] Build complete."
echo "  Bundle: $(ls -lh "$OUT/functions/mcp.func/index.mjs" | awk '{print $5}')"
echo "  WASM:   $(ls -lh "$OUT/functions/mcp.func/vcad_kernel_wasm_bg.wasm" | awk '{print $5}')"
