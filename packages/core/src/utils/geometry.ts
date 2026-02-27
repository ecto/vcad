/**
 * Geometry utilities for volume and mass calculations.
 *
 * Uses Rust WASM for volume computation when available.
 */

import type { TriangleMesh } from "@vcad/engine";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let wasmModule: any = null;

async function loadWasm(): Promise<typeof wasmModule | null> {
  if (wasmModule) return wasmModule;
  try {
    wasmModule = await import("@vcad/kernel-wasm");
    return wasmModule;
  } catch {
    return null;
  }
}

// Eagerly start loading
loadWasm();

/**
 * Compute volume of a closed triangle mesh using the divergence theorem.
 * @param mesh - Triangle mesh with positions and indices
 * @returns Volume in mm³ (assumes mm units)
 */
export function computeVolume(mesh: TriangleMesh): number {
  if (wasmModule?.computeMeshVolume) {
    try {
      return wasmModule.computeMeshVolume(mesh.positions, mesh.indices) as number;
    } catch {
      // Fall through to TS implementation
    }
  }
  return computeVolumeTS(mesh);
}

/** TypeScript fallback for volume computation. */
function computeVolumeTS(mesh: TriangleMesh): number {
  const numTriangles = mesh.indices.length / 3;
  let volume = 0;

  for (let t = 0; t < numTriangles; t++) {
    const i0 = mesh.indices[t * 3]!;
    const i1 = mesh.indices[t * 3 + 1]!;
    const i2 = mesh.indices[t * 3 + 2]!;

    const p1 = getVertex(mesh, i0);
    const p2 = getVertex(mesh, i1);
    const p3 = getVertex(mesh, i2);

    volume += signedVolumeOfTriangle(p1, p2, p3);
  }

  return Math.abs(volume);
}

/** Calculate signed volume of a tetrahedron formed by a triangle and origin. */
function signedVolumeOfTriangle(
  p1: [number, number, number],
  p2: [number, number, number],
  p3: [number, number, number]
): number {
  return (
    (p1[0] * (p2[1] * p3[2] - p3[1] * p2[2]) -
      p2[0] * (p1[1] * p3[2] - p3[1] * p1[2]) +
      p3[0] * (p1[1] * p2[2] - p2[1] * p1[2])) /
    6.0
  );
}

/** Get vertex position from mesh as tuple. */
function getVertex(
  mesh: TriangleMesh,
  index: number
): [number, number, number] {
  const i = index * 3;
  return [mesh.positions[i]!, mesh.positions[i + 1]!, mesh.positions[i + 2]!];
}

/**
 * Compute mass from volume and density.
 * @param volumeMm3 - Volume in mm³
 * @param densityKgM3 - Density in kg/m³
 * @returns Mass in kg
 */
export function computeMass(volumeMm3: number, densityKgM3: number): number {
  // Convert mm³ to m³ (divide by 1e9), then multiply by density
  return (volumeMm3 / 1e9) * densityKgM3;
}

/**
 * Format mass for display with appropriate units.
 * @param massKg - Mass in kg
 * @returns Formatted string with units (mg, g, or kg)
 */
export function formatMass(massKg: number): string {
  if (massKg < 0.001) {
    return `${(massKg * 1e6).toFixed(1)} mg`;
  }
  if (massKg < 1) {
    return `${(massKg * 1000).toFixed(1)} g`;
  }
  return `${massKg.toFixed(2)} kg`;
}

/**
 * Format volume for display with appropriate units.
 * @param volumeMm3 - Volume in mm³
 * @returns Formatted string with units (mm³, cm³, or L)
 */
export function formatVolume(volumeMm3: number): string {
  if (volumeMm3 < 1000) {
    return `${volumeMm3.toFixed(1)} mm³`;
  }
  const cm3 = volumeMm3 / 1000;
  if (cm3 < 1000) {
    return `${cm3.toFixed(1)} cm³`;
  }
  return `${(cm3 / 1000).toFixed(2)} L`;
}
