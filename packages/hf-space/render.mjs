#!/usr/bin/env node
/**
 * Render VCode to PNG using vcad kernel
 * Usage: node render.mjs "<vcode>" output.png
 */

import { createCanvas } from 'canvas';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

// Load WASM kernel
const __dirname = path.dirname(fileURLToPath(import.meta.url));

async function loadKernel() {
  const wasmModule = await import('@vcad/kernel-wasm');
  const wasmPath = path.join(__dirname, 'node_modules', '@vcad', 'kernel-wasm', 'vcad_kernel_wasm_bg.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  wasmModule.initSync({ module: wasmBuffer });
  return wasmModule;
}

// Simple 3D to 2D projection
function project(x, y, z, angle = 45, elevation = 25) {
  const radAngle = (angle * Math.PI) / 180;
  const radElev = (elevation * Math.PI) / 180;

  // Rotate around Y axis
  const x1 = x * Math.cos(radAngle) - z * Math.sin(radAngle);
  const z1 = x * Math.sin(radAngle) + z * Math.cos(radAngle);

  // Rotate around X axis for elevation
  const y1 = y * Math.cos(radElev) - z1 * Math.sin(radElev);
  const z2 = y * Math.sin(radElev) + z1 * Math.cos(radElev);

  return { x: x1, y: y1, z: z2 };
}

// Render mesh to canvas
function renderMesh(ctx, mesh, width, height) {
  const { positions, indices, normals } = mesh;

  if (!positions || positions.length === 0) {
    return;
  }

  // Find bounding box
  let minX = Infinity, maxX = -Infinity;
  let minY = Infinity, maxY = -Infinity;
  let minZ = Infinity, maxZ = -Infinity;

  for (let i = 0; i < positions.length; i += 3) {
    const p = project(positions[i], positions[i + 1], positions[i + 2]);
    minX = Math.min(minX, p.x);
    maxX = Math.max(maxX, p.x);
    minY = Math.min(minY, p.y);
    maxY = Math.max(maxY, p.y);
    minZ = Math.min(minZ, p.z);
    maxZ = Math.max(maxZ, p.z);
  }

  // Scale to fit canvas
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

    // Simple lighting (light from front-top-right)
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

  // Draw triangles
  for (const tri of triangles) {
    const g = Math.floor(200 * tri.light);
    const b = Math.floor(220 * tri.light);
    ctx.fillStyle = `rgb(${Math.floor(g * 0.4)}, ${g}, ${Math.floor(b * 0.5)})`;
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.3)';
    ctx.lineWidth = 0.5;

    ctx.beginPath();
    ctx.moveTo(tri.points[0].x, tri.points[0].y);
    ctx.lineTo(tri.points[1].x, tri.points[1].y);
    ctx.lineTo(tri.points[2].x, tri.points[2].y);
    ctx.closePath();
    ctx.fill();
    ctx.stroke();
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length < 2) {
    console.error('Usage: node render.mjs "<vcode>" output.png');
    process.exit(1);
  }

  const vcode = args[0];
  const outputPath = args[1];

  try {
    const kernel = await loadKernel();

    // Evaluate VCode
    const solid = kernel.evaluateVCode(vcode);

    if (solid.isEmpty()) {
      console.error('Solid is empty');
      process.exit(1);
    }

    // Get mesh
    const mesh = solid.getMesh(32);

    // Create canvas
    const width = 600;
    const height = 600;
    const canvas = createCanvas(width, height);
    const ctx = canvas.getContext('2d');

    // Dark background
    ctx.fillStyle = '#1a1a1a';
    ctx.fillRect(0, 0, width, height);

    // Render mesh
    renderMesh(ctx, mesh, width, height);

    // Save PNG
    const buffer = canvas.toBuffer('image/png');
    fs.writeFileSync(outputPath, buffer);

    console.log(`Rendered to ${outputPath}`);
    console.log(`Triangles: ${mesh.indices.length / 3}`);
  } catch (err) {
    console.error('Error:', err.message);
    process.exit(1);
  }
}

main();
