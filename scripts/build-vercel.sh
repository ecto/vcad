#!/usr/bin/env bash
set -e

# Ensure hoisted binaries (next, etc.) are in PATH
export PATH="$PWD/node_modules/.bin:$PATH"

# Build app
npm run build -w @vcad/ir
npm run build -w @vcad/engine
npm run build -w @vcad/core
npm run build -w @vcad/app

# Merge app output into dist/
mkdir -p dist
cp -r packages/app/dist/. dist/

# Build docs (static export)
cd packages/docs
../../node_modules/.bin/next build
cd ../..

# Merge docs output into dist/docs/
mkdir -p dist/docs
cp -r packages/docs/out/. dist/docs/

# Copy fonts to root so /fonts/* resolves from SPA root
cp -r packages/docs/out/fonts dist/fonts 2>/dev/null || true
