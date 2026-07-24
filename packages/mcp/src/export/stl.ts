/**
 * Binary STL export for 3D printing.
 *
 * Thin wrapper over the kernel WASM writer (`vcad-kernel-export` via
 * `buildStlBytes`) — the single source of truth for STL serialization.
 */

import type { EvaluatedScene } from "@vcad/engine";
import { exportKernel } from "./glb.js";

/** Convert evaluated scene to binary STL bytes (merged triangle soup). */
export function toStlBytes(scene: EvaluatedScene, name: string): Uint8Array {
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

  return exportKernel().buildStlBytes(
    JSON.stringify({ name, meshes }),
    f32Data,
    u32Data,
  );
}
