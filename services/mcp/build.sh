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
npx esbuild@latest entry.ts \
  --bundle \
  --platform=node \
  --target=node20 \
  --format=esm \
  --external:@resvg/resvg-js \
  --outfile="$OUT/functions/mcp.func/index.mjs" \
  --banner:js="import { createRequire } from 'module'; const require = createRequire(import.meta.url);"

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
    }
  ]
}
EOF

echo "[vcad-mcp] Build complete."
echo "  Bundle: $(ls -lh "$OUT/functions/mcp.func/index.mjs" | awk '{print $5}')"
echo "  WASM:   $(ls -lh "$OUT/functions/mcp.func/vcad_kernel_wasm_bg.wasm" | awk '{print $5}')"
