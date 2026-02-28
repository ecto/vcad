#!/usr/bin/env node
/**
 * Render API - HTTP server that renders VCode to SVG
 * POST /render with body: { ir: "<vcode>" }
 * Returns: SVG image
 */

import { createServer } from 'node:http';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PORT = process.env.PORT || 7860;

// Load WASM kernel
let kernel = null;

async function loadKernel() {
  if (kernel) return kernel;

  const wasmModule = await import('./kernel-wasm/vcad_kernel_wasm.js');

  const possiblePaths = [
    path.join(__dirname, 'node_modules', '@vcad', 'kernel-wasm', 'vcad_kernel_wasm_bg.wasm'),
    path.join(__dirname, 'kernel-wasm', 'vcad_kernel_wasm_bg.wasm'),
  ];

  let wasmBuffer = null;
  for (const wasmPath of possiblePaths) {
    if (fs.existsSync(wasmPath)) {
      wasmBuffer = fs.readFileSync(wasmPath);
      console.log(`WASM loaded from: ${wasmPath}`);
      break;
    }
  }

  if (!wasmBuffer) {
    throw new Error('Could not find WASM file');
  }

  wasmModule.initSync({ module: wasmBuffer });
  kernel = wasmModule;
  console.log('WASM kernel initialized');
  return kernel;
}

// 3D to 2D projection
function project(x, y, z, angle = 45, elevation = 25) {
  const radAngle = (angle * Math.PI) / 180;
  const radElev = (elevation * Math.PI) / 180;

  const x1 = x * Math.cos(radAngle) - z * Math.sin(radAngle);
  const z1 = x * Math.sin(radAngle) + z * Math.cos(radAngle);
  const y1 = y * Math.cos(radElev) - z1 * Math.sin(radElev);
  const z2 = y * Math.sin(radElev) + z1 * Math.cos(radElev);

  return { x: x1, y: y1, z: z2 };
}

// Render mesh to SVG
function renderMeshToSVG(mesh, width = 600, height = 600) {
  const { positions, indices } = mesh;

  if (!positions || positions.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect fill="#1a1a1a" width="100%" height="100%"/><text x="50%" y="50%" fill="white" text-anchor="middle">Empty mesh</text></svg>`;
  }

  // Find bounding box
  let minX = Infinity, maxX = -Infinity;
  let minY = Infinity, maxY = -Infinity;

  for (let i = 0; i < positions.length; i += 3) {
    const p = project(positions[i], positions[i + 1], positions[i + 2]);
    minX = Math.min(minX, p.x);
    maxX = Math.max(maxX, p.x);
    minY = Math.min(minY, p.y);
    maxY = Math.max(maxY, p.y);
  }

  // Scale to fit
  const rangeX = maxX - minX || 1;
  const rangeY = maxY - minY || 1;
  const scale = Math.min((width - 100) / rangeX, (height - 100) / rangeY);
  const offsetX = width / 2 - ((minX + maxX) / 2) * scale;
  const offsetY = height / 2 + ((minY + maxY) / 2) * scale;

  // Collect triangles with depth
  const triangles = [];
  for (let i = 0; i < indices.length; i += 3) {
    const i0 = indices[i], i1 = indices[i + 1], i2 = indices[i + 2];

    const v0 = project(positions[i0 * 3], positions[i0 * 3 + 1], positions[i0 * 3 + 2]);
    const v1 = project(positions[i1 * 3], positions[i1 * 3 + 1], positions[i1 * 3 + 2]);
    const v2 = project(positions[i2 * 3], positions[i2 * 3 + 1], positions[i2 * 3 + 2]);

    // Calculate normal for lighting
    const nx = (v1.y - v0.y) * (v2.z - v0.z) - (v1.z - v0.z) * (v2.y - v0.y);
    const ny = (v1.z - v0.z) * (v2.x - v0.x) - (v1.x - v0.x) * (v2.z - v0.z);
    const nz = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
    const len = Math.sqrt(nx * nx + ny * ny + nz * nz) || 1;

    // Simple lighting
    const light = Math.max(0.3, (nx / len * 0.3 + ny / len * 0.5 + nz / len * 0.8));
    const avgZ = (v0.z + v1.z + v2.z) / 3;

    triangles.push({
      points: [
        { x: v0.x * scale + offsetX, y: -v0.y * scale + offsetY },
        { x: v1.x * scale + offsetX, y: -v1.y * scale + offsetY },
        { x: v2.x * scale + offsetX, y: -v2.y * scale + offsetY },
      ],
      z: avgZ,
      light,
    });
  }

  // Sort by depth (painter's algorithm)
  triangles.sort((a, b) => a.z - b.z);

  // Build SVG
  let svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">\n`;
  svg += `<rect fill="#1a1a1a" width="100%" height="100%"/>\n`;
  svg += `<g stroke="rgba(255,255,255,0.3)" stroke-width="0.5">\n`;

  for (const tri of triangles) {
    const g = Math.floor(200 * tri.light);
    const b = Math.floor(220 * tri.light);
    const r = Math.floor(g * 0.4);
    const fill = `rgb(${r},${g},${Math.floor(b * 0.5)})`;

    const pts = tri.points.map(p => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ');
    svg += `<polygon points="${pts}" fill="${fill}"/>\n`;
  }

  svg += `</g>\n</svg>`;
  return svg;
}

async function renderIR(vcode) {
  const k = await loadKernel();

  const solid = k.evaluateVCode(vcode);

  if (solid.isEmpty()) {
    throw new Error('Solid is empty');
  }

  const mesh = solid.getMesh(32);
  return renderMeshToSVG(mesh);
}

// HTTP server
const server = createServer(async (req, res) => {
  // CORS headers
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'POST, GET, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type');

  if (req.method === 'OPTIONS') {
    res.writeHead(204);
    res.end();
    return;
  }

  if (req.method === 'GET' && req.url === '/health') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ status: 'ok' }));
    return;
  }

  if (req.method === 'GET' && req.url === '/') {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(`<!DOCTYPE html>
<html>
<head><title>vcad Render API</title></head>
<body style="background:#1a1a1a;color:white;font-family:sans-serif;padding:2rem">
<h1>vcad Render API</h1>
<p>POST /render with {"ir": "C 50 30 10"} to render VCode to SVG</p>
<p><a href="/health" style="color:#4ade80">/health</a> - Health check</p>
</body>
</html>`);
    return;
  }

  if (req.method === 'POST' && req.url === '/render') {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', async () => {
      try {
        const { ir } = JSON.parse(body);
        if (!ir) {
          res.writeHead(400, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ error: 'Missing ir field' }));
          return;
        }

        const svg = await renderIR(ir);
        res.writeHead(200, { 'Content-Type': 'image/svg+xml' });
        res.end(svg);
      } catch (err) {
        console.error('Render error:', err.message);
        res.writeHead(500, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: err.message }));
      }
    });
    return;
  }

  res.writeHead(404, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ error: 'Not found' }));
});

// Start server
await loadKernel();
server.listen(PORT, () => {
  console.log(`Render API listening on port ${PORT}`);
});
