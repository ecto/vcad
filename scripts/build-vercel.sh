#!/usr/bin/env bash
set -e

# Build app
npm run build -w @vcad/ir
npm run build -w @vcad/engine
npm run build -w @vcad/core
npm run build -w @vcad/app

# Merge app output into dist/
mkdir -p dist
cp -r packages/app/dist/. dist/

# Build docs (static export)
# Vercel's Turbo pruning may skip docs deps — ensure they're installed
npm install -w @vcad/docs --ignore-scripts 2>/dev/null || true
cd packages/docs
NEXT_BIN=$(node -e "console.log(require.resolve('next/dist/bin/next'))")
node "$NEXT_BIN" build
cd ../..

# Merge docs output into dist/docs/
mkdir -p dist/docs
cp -r packages/docs/out/. dist/docs/

# Copy fonts to root so /fonts/* resolves from SPA root
cp -r packages/docs/out/fonts dist/fonts 2>/dev/null || true
