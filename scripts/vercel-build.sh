#!/bin/bash
set -e
npm run build -w @vcad/ir
npm run build -w @vcad/engine
npm run build -w @vcad/core
npm run build -w @vcad/mcp
npm run build -w @vcad/app
cp -r dist ../../dist
