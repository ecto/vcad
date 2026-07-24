import type { EvaluatedScene } from "@vcad/engine";
import { getKernelWasmSync } from "@vcad/engine";

/**
 * Export an evaluated scene as a binary STL ArrayBuffer.
 *
 * Thin wrapper over the kernel WASM writer (`vcad-kernel-export` via
 * `buildStlBytes`) — the single source of truth for STL serialization.
 * All parts are merged into one triangle soup. Requires the kernel WASM
 * module to be initialized (always true once a scene has been evaluated).
 */
export function exportStlBuffer(scene: EvaluatedScene): ArrayBuffer {
  const mod = getKernelWasmSync();
  if (!mod) {
    throw new Error(
      "kernel WASM not initialized — await getKernelWasm() before exporting",
    );
  }
  const wasm = mod as unknown as {
    buildStlBytes(
      specJson: string,
      f32Data: Float32Array,
      u32Data: Uint32Array,
    ): Uint8Array;
  };

  let f32Len = 0;
  let u32Len = 0;
  for (const part of scene.parts) {
    f32Len += part.mesh.positions.length;
    u32Len += part.mesh.indices.length;
  }
  const f32Data = new Float32Array(f32Len);
  const u32Data = new Uint32Array(u32Len);
  let f32Off = 0;
  let u32Off = 0;
  const meshes = scene.parts.map((part) => {
    f32Data.set(part.mesh.positions, f32Off);
    u32Data.set(part.mesh.indices, u32Off);
    const spec = {
      positions: [f32Off, part.mesh.positions.length],
      indices: [u32Off, part.mesh.indices.length],
    };
    f32Off += part.mesh.positions.length;
    u32Off += part.mesh.indices.length;
    return spec;
  });

  const stl = wasm.buildStlBytes(
    JSON.stringify({ name: "vcad binary STL export", meshes }),
    f32Data,
    u32Data,
  );
  return stl.buffer.slice(
    stl.byteOffset,
    stl.byteOffset + stl.byteLength,
  ) as ArrayBuffer;
}

/**
 * Export an evaluated scene as a binary STL Blob (browser only).
 */
export function exportStlBlob(scene: EvaluatedScene): Blob {
  const buffer = exportStlBuffer(scene);
  return new Blob([buffer], { type: "application/octet-stream" });
}
