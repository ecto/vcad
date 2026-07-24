import type { EvaluatedScene } from "@vcad/engine";
import { getKernelWasmSync } from "@vcad/engine";

/**
 * Export an evaluated scene as a GLB (binary glTF 2.0) ArrayBuffer.
 *
 * Thin wrapper over the kernel WASM writer (`vcad-kernel-export` via
 * `buildGlbBytes`) — the single source of truth for GLB serialization.
 * One glTF node per part with the neutral default material. Requires the
 * kernel WASM module to be initialized (always true once a scene has been
 * evaluated).
 */
export function exportGltfBuffer(scene: EvaluatedScene): ArrayBuffer {
  const wasm = requireWasm();

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
  const meshes = scene.parts.map((part, i) => {
    f32Data.set(part.mesh.positions, f32Off);
    u32Data.set(part.mesh.indices, u32Off);
    const spec = {
      name: `part_${i}`,
      positions: [f32Off, part.mesh.positions.length],
      indices: [u32Off, part.mesh.indices.length],
      color: [0.8, 0.8, 0.8],
      metallic: 0.1,
      roughness: 0.5,
    };
    f32Off += part.mesh.positions.length;
    u32Off += part.mesh.indices.length;
    return spec;
  });

  const glb = wasm.buildGlbBytes(
    JSON.stringify({ name: "vcad", meshes }),
    f32Data,
    u32Data,
  );
  return glb.buffer.slice(
    glb.byteOffset,
    glb.byteOffset + glb.byteLength,
  ) as ArrayBuffer;
}

function requireWasm(): {
  buildGlbBytes(
    specJson: string,
    f32Data: Float32Array,
    u32Data: Uint32Array,
  ): Uint8Array;
} {
  const mod = getKernelWasmSync();
  if (!mod) {
    throw new Error(
      "kernel WASM not initialized — await getKernelWasm() before exporting",
    );
  }
  return mod as unknown as ReturnType<typeof requireWasm>;
}

/**
 * Export an evaluated scene as a GLB Blob (browser only).
 */
export function exportGltfBlob(scene: EvaluatedScene): Blob {
  const buffer = exportGltfBuffer(scene);
  return new Blob([buffer], { type: "model/gltf-binary" });
}
