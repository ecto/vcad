#!/usr/bin/env bash
set -e

echo "=== build-vercel.sh ==="
echo "PWD: $(pwd)"
echo "node_modules exists: $(test -d node_modules && echo yes || echo no)"
echo "next in node_modules: $(test -f node_modules/next/dist/bin/next && echo yes || echo no)"
echo "next in .bin: $(test -f node_modules/.bin/next && echo yes || echo no)"
ls node_modules/.bin/next* 2>/dev/null || echo "no next in .bin"

# Build app
npm run build -w @vcad/ir
npm run build -w @vcad/engine
npm run build -w @vcad/core
npm run build -w @vcad/app

# Merge app output into dist/
mkdir -p dist
cp -r packages/app/dist/. dist/

# Build docs (static export)
echo "=== building docs ==="
echo "PWD before cd: $(pwd)"
cd packages/docs
echo "PWD after cd: $(pwd)"
echo "Resolving next..."
NEXT_BIN=$(node -e "console.log(require.resolve('next/dist/bin/next'))")
echo "NEXT_BIN: $NEXT_BIN"
node "$NEXT_BIN" build
cd ../..

# Merge docs output into dist/docs/
mkdir -p dist/docs
cp -r packages/docs/out/. dist/docs/

# Copy fonts to root so /fonts/* resolves from SPA root
cp -r packages/docs/out/fonts dist/fonts 2>/dev/null || true
