/**
 * GPU-accelerated geometry processing utilities.
 *
 * This module provides GPU compute shader acceleration for:
 * - Creased normal computation
 * - Mesh decimation for LOD generation
 */

let wasmModule: typeof import("@vcad/kernel-wasm") | null = null;
let gpuAvailable = false;
let gpuInitPromise: Promise<boolean> | null = null;
let rayTracerAvailable = false;
let rayTracerInitPromise: Promise<boolean> | null = null;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let rayTracerInstance: any = null;

/**
 * Result of GPU geometry processing.
 */
export interface GpuGeometryResult {
  /** Vertex positions (flat array: x, y, z, ...) */
  positions: Float32Array;
  /** Triangle indices */
  indices: Uint32Array;
  /** Vertex normals (flat array: nx, ny, nz, ...) */
  normals: Float32Array;
}

/**
 * Initialize the GPU context for accelerated geometry processing.
 *
 * Should be called once at application startup, after the WASM module is loaded.
 * Safe to call multiple times - subsequent calls return the cached result.
 *
 * @returns true if WebGPU is available and initialized
 */
export async function initializeGpu(): Promise<boolean> {
  // Already initialized
  if (gpuAvailable) return true;

  // Init in progress - return existing promise to avoid double init
  if (gpuInitPromise) return gpuInitPromise;

  gpuInitPromise = (async () => {
    try {
      if (!wasmModule) {
        wasmModule = await import("@vcad/kernel-wasm");
      }

      if (typeof wasmModule.initGpu === "function") {
        gpuAvailable = await wasmModule.initGpu();
      } else {
        gpuAvailable = false;
      }

      return gpuAvailable;
    } catch {
      // GPU init can fail due to missing WebGPU support or
      // wasm-bindgen closure bugs in Safari. Non-critical —
      // the app works without GPU acceleration.
      return false;
    }
  })();

  return gpuInitPromise;
}

/**
 * Check if GPU processing is currently available.
 */
export function isGpuAvailable(): boolean {
  return gpuAvailable;
}

/**
 * Process geometry with GPU acceleration.
 *
 * Computes creased normals and optionally generates LOD meshes.
 *
 * @param positions - Flat array of vertex positions (x, y, z, ...)
 * @param indices - Triangle indices
 * @param creaseAngle - Angle in radians for creased normal computation (default: PI/6 = 30 degrees)
 * @param generateLod - If true, returns multiple LOD levels
 * @returns Array of geometry results. If generateLod is true, returns [full, 50%, 25%].
 * @throws Error if GPU is not available
 */
export async function processGeometryGpu(
  positions: Float32Array,
  indices: Uint32Array,
  creaseAngle: number = Math.PI / 6,
  generateLod: boolean = false
): Promise<GpuGeometryResult[]> {
  if (!gpuAvailable) {
    throw new Error("GPU not available - call initializeGpu() first");
  }

  if (!wasmModule) {
    throw new Error("WASM module not loaded");
  }

  const results = await wasmModule.processGeometryGpu(
    positions,
    indices,
    creaseAngle,
    generateLod
  );

  return results.map((r: { positions: number[]; indices: number[]; normals: number[] }) => ({
    positions: new Float32Array(r.positions),
    indices: new Uint32Array(r.indices),
    normals: new Float32Array(r.normals),
  }));
}

/**
 * Compute creased normals using GPU acceleration.
 *
 * @param positions - Flat array of vertex positions (x, y, z, ...)
 * @param indices - Triangle indices
 * @param creaseAngle - Angle in radians; faces meeting at sharper angles get hard edges
 * @returns Flat array of normals (nx, ny, nz, ...), same length as positions
 * @throws Error if GPU is not available
 */
export async function computeCreasedNormalsGpu(
  positions: Float32Array,
  indices: Uint32Array,
  creaseAngle: number
): Promise<Float32Array> {
  if (!gpuAvailable) {
    throw new Error("GPU not available - call initializeGpu() first");
  }

  if (!wasmModule) {
    throw new Error("WASM module not loaded");
  }

  const normals = await wasmModule.computeCreasedNormalsGpu(
    positions,
    indices,
    creaseAngle
  );

  return new Float32Array(normals);
}

/**
 * Decimate a mesh to reduce triangle count using GPU acceleration.
 *
 * @param positions - Flat array of vertex positions
 * @param indices - Triangle indices
 * @param targetRatio - Target ratio of triangles to keep (0.5 = 50%)
 * @returns Decimated mesh with positions, indices, and normals
 * @throws Error if GPU is not available
 */
export async function decimateMeshGpu(
  positions: Float32Array,
  indices: Uint32Array,
  targetRatio: number
): Promise<GpuGeometryResult> {
  if (!gpuAvailable) {
    throw new Error("GPU not available - call initializeGpu() first");
  }

  if (!wasmModule) {
    throw new Error("WASM module not loaded");
  }

  const result = await wasmModule.decimateMeshGpu(
    positions,
    indices,
    targetRatio
  );

  return {
    positions: new Float32Array(result.positions),
    indices: new Uint32Array(result.indices),
    normals: new Float32Array(result.normals),
  };
}

/**
 * Merge multiple triangle meshes into a single mesh.
 *
 * This is a CPU operation but useful as a pre-processing step before GPU processing.
 *
 * @param meshes - Array of meshes to merge
 * @returns Single merged mesh
 */
export function mergeMeshes(
  meshes: Array<{ positions: Float32Array; indices: Uint32Array }>
): { positions: Float32Array; indices: Uint32Array } {
  // Calculate total sizes
  let totalVertices = 0;
  let totalIndices = 0;
  for (const mesh of meshes) {
    totalVertices += mesh.positions.length;
    totalIndices += mesh.indices.length;
  }

  // Allocate output arrays
  const positions = new Float32Array(totalVertices);
  const indices = new Uint32Array(totalIndices);

  // Copy data
  let vertexOffset = 0;
  let indexOffset = 0;
  let baseVertex = 0;

  for (const mesh of meshes) {
    // Copy positions
    positions.set(mesh.positions, vertexOffset);

    // Copy indices with offset
    for (let i = 0; i < mesh.indices.length; i++) {
      indices[indexOffset + i] = mesh.indices[i] + baseVertex;
    }

    vertexOffset += mesh.positions.length;
    indexOffset += mesh.indices.length;
    baseVertex += mesh.positions.length / 3;
  }

  return { positions, indices };
}

/**
 * Initialize the GPU ray tracer for direct BRep rendering.
 *
 * Requires WebGPU to be available (call initializeGpu first).
 * Safe to call multiple times - subsequent calls return the cached result.
 *
 * @returns true if ray tracer is available and initialized
 */
export async function initializeRayTracer(): Promise<boolean> {
  // Already initialized
  if (rayTracerAvailable) return true;

  // Init in progress - return existing promise
  if (rayTracerInitPromise) return rayTracerInitPromise;

  // GPU must be initialized first
  if (!gpuAvailable) {
    console.log("[RayTracer] GPU not available, skipping ray tracer init");
    return false;
  }

  rayTracerInitPromise = (async () => {
    try {
      if (!wasmModule) {
        wasmModule = await import("@vcad/kernel-wasm");
      }

      if (typeof wasmModule.RayTracer?.create === "function") {
        rayTracerInstance = wasmModule.RayTracer.create();
        rayTracerAvailable = true;
        console.log("[RayTracer] Ray tracer initialized successfully");
      } else {
        console.log("[RayTracer] Ray tracing feature not compiled into WASM module");
        rayTracerAvailable = false;
      }

      return rayTracerAvailable;
    } catch (e) {
      console.warn("[RayTracer] Init failed:", e);
      return false;
    }
  })();

  return rayTracerInitPromise;
}

/**
 * Get the ray tracer instance.
 *
 * Returns null if ray tracer is not initialized.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getRayTracer(): any {
  return rayTracerInstance;
}

/**
 * Check if the ray tracer is currently available.
 */
export function isRayTracerAvailable(): boolean {
  return rayTracerAvailable;
}
