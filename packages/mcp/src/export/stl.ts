/**
 * Binary STL export for 3D printing.
 *
 * Thin wrapper over the kernel WASM writer (`vcad-kernel-export` via
 * `buildStlBytes`) — the single source of truth for STL serialization.
 */

import type { EvaluatedScene, TriangleMesh } from "@vcad/engine";
import { exportKernel } from "./glb.js";
import { sceneExportUnits, worldMesh } from "./scene-units.js";

/** Convert evaluated scene to binary STL bytes (merged triangle soup).
 *
 * STL has no scene graph, so assembly instances are baked into world space —
 * the merged soup matches what `inspect_cad` measures for the same document. */
export function toStlBytes(scene: EvaluatedScene, name: string): Uint8Array {
  const bodies: TriangleMesh[] = sceneExportUnits(scene).map(worldMesh);

  let f32Len = 0;
  let u32Len = 0;
  for (const mesh of bodies) {
    f32Len += mesh.positions.length;
    u32Len += mesh.indices.length;
  }

  const f32Data = new Float32Array(f32Len);
  const u32Data = new Uint32Array(u32Len);
  let f32Off = 0;
  let u32Off = 0;
  const meshes = bodies.map((mesh) => {
    f32Data.set(mesh.positions, f32Off);
    u32Data.set(mesh.indices, u32Off);
    const spec = {
      positions: [f32Off, mesh.positions.length],
      indices: [u32Off, mesh.indices.length],
    };
    f32Off += mesh.positions.length;
    u32Off += mesh.indices.length;
    return spec;
  });

  return exportKernel().buildStlBytes(
    JSON.stringify({ name, meshes }),
    f32Data,
    u32Data,
  );
}
