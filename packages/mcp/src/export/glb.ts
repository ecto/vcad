/**
 * GLB (binary glTF) export for visualization.
 *
 * Ported from crates/vcad/src/export/gltf.rs
 */

import type { EvaluatedScene, TriangleMesh } from "@vcad/engine";
import type { Document } from "@vcad/ir";

/**
 * Build `"<part_id>:<name>"` labels for every visible root, index-aligned
 * with `EvaluatedScene.parts` (the evaluator maps visible roots 1:1 onto
 * parts, padding failures with empty meshes — so the alignment holds even
 * when a root fails to evaluate). The viewer parses these node names back
 * into part identity for click-to-select.
 */
export function buildPartLabels(doc: Document): string[] {
  return doc.roots
    .filter((entry) => entry.visible !== false)
    .map((entry) => {
      const name = doc.nodes[String(entry.root)]?.name;
      return `${entry.root}:${name ?? ""}`;
    });
}

/** Default material for parts without material assignment. */
const DEFAULT_MATERIAL = {
  name: "default",
  color: [0.8, 0.8, 0.8] as [number, number, number],
  metallic: 0.1,
  roughness: 0.5,
};

/** Convert evaluated scene to binary GLB bytes.
 *
 * `partLabels` (index-aligned with `scene.parts`) become glTF node names —
 * the viewer parses them back into part identity for click-to-select, so
 * they follow the `"<part_id>:<name>"` convention from
 * {@link buildPartLabels}. Omitted entries fall back to `part_<idx>`. */
export function toGlbBytes(
  scene: EvaluatedScene,
  name: string,
  partLabels?: string[],
): Uint8Array {
  // Collect unique materials
  const materialMap = new Map<string, number>();
  const materials: Array<{
    name: string;
    color: [number, number, number];
    metallic: number;
    roughness: number;
  }> = [];

  for (const part of scene.parts) {
    if (!materialMap.has(part.material)) {
      materialMap.set(part.material, materials.length);
      materials.push({
        name: part.material,
        color: DEFAULT_MATERIAL.color,
        metallic: DEFAULT_MATERIAL.metallic,
        roughness: DEFAULT_MATERIAL.roughness,
      });
    }
  }

  if (materials.length === 0) {
    materials.push(DEFAULT_MATERIAL);
  }

  // Build binary buffer for all meshes
  const bufferChunks: Uint8Array[] = [];
  const bufferViews: BufferView[] = [];
  const accessors: Accessor[] = [];
  const meshes: Mesh[] = [];
  const nodes: GltfNode[] = [];

  let bufferOffset = 0;

  for (let meshIdx = 0; meshIdx < scene.parts.length; meshIdx++) {
    const part = scene.parts[meshIdx];
    const mesh = part.mesh;

    const vertexCount = mesh.positions.length / 3;
    const indexCount = mesh.indices.length;

    // Calculate bounds
    let minX = Infinity,
      minY = Infinity,
      minZ = Infinity;
    let maxX = -Infinity,
      maxY = -Infinity,
      maxZ = -Infinity;

    for (let i = 0; i < vertexCount; i++) {
      const x = mesh.positions[i * 3];
      const y = mesh.positions[i * 3 + 1];
      const z = mesh.positions[i * 3 + 2];
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      minZ = Math.min(minZ, z);
      maxX = Math.max(maxX, x);
      maxY = Math.max(maxY, y);
      maxZ = Math.max(maxZ, z);
    }

    // Write indices as u32
    const indicesBytes = new Uint8Array(indexCount * 4);
    const indicesView = new DataView(indicesBytes.buffer);
    for (let i = 0; i < indexCount; i++) {
      indicesView.setUint32(i * 4, mesh.indices[i], true);
    }

    // Pad to 4-byte alignment
    const indicesPadded = padTo4(indicesBytes);
    bufferChunks.push(indicesPadded);

    // Index buffer view
    const indicesBvIdx = bufferViews.length;
    bufferViews.push({
      buffer: 0,
      byteOffset: bufferOffset,
      byteLength: indexCount * 4,
      target: 34963, // ELEMENT_ARRAY_BUFFER
    });
    bufferOffset += indicesPadded.length;

    // Index accessor
    const indicesAccIdx = accessors.length;
    accessors.push({
      bufferView: indicesBvIdx,
      componentType: 5125, // UNSIGNED_INT
      count: indexCount,
      type: "SCALAR",
    });

    // Write positions as f32
    const positionsBytes = new Uint8Array(mesh.positions.buffer.slice(0));
    const positionsPadded = padTo4(positionsBytes);
    bufferChunks.push(positionsPadded);

    // Position buffer view
    const positionsBvIdx = bufferViews.length;
    bufferViews.push({
      buffer: 0,
      byteOffset: bufferOffset,
      byteLength: vertexCount * 12,
      target: 34962, // ARRAY_BUFFER
    });
    bufferOffset += positionsPadded.length;

    // Position accessor
    const positionsAccIdx = accessors.length;
    accessors.push({
      bufferView: positionsBvIdx,
      componentType: 5126, // FLOAT
      count: vertexCount,
      type: "VEC3",
      min: [minX, minY, minZ],
      max: [maxX, maxY, maxZ],
    });

    // Get material index
    const matIdx = materialMap.get(part.material) ?? 0;

    // Mesh
    meshes.push({
      name: `mesh_${meshIdx}`,
      primitives: [
        {
          attributes: { POSITION: positionsAccIdx },
          indices: indicesAccIdx,
          material: matIdx,
        },
      ],
    });

    // Node — named with part identity when the caller provides it.
    nodes.push({
      mesh: meshIdx,
      name: partLabels?.[meshIdx] ?? `part_${meshIdx}`,
    });
  }

  // Build JSON
  const json = {
    asset: { version: "2.0", generator: "vcad-mcp" },
    scene: 0,
    scenes: [{ name, nodes: nodes.map((_, i) => i) }],
    nodes,
    meshes,
    materials: materials.map((m) => ({
      name: m.name,
      pbrMetallicRoughness: {
        baseColorFactor: [...m.color, 1.0],
        metallicFactor: m.metallic,
        roughnessFactor: m.roughness,
      },
    })),
    accessors,
    bufferViews,
    buffers: [{ byteLength: bufferOffset }],
  };

  const jsonStr = JSON.stringify(json);
  const jsonBytes = new TextEncoder().encode(jsonStr);
  const jsonPadded = padTo4(jsonBytes, 0x20); // Pad with spaces

  // Merge buffer chunks
  const binBuffer = new Uint8Array(bufferOffset);
  let binOffset = 0;
  for (const chunk of bufferChunks) {
    binBuffer.set(chunk, binOffset);
    binOffset += chunk.length;
  }

  // Build GLB
  const totalLength = 12 + 8 + jsonPadded.length + 8 + binBuffer.length;
  const glb = new Uint8Array(totalLength);
  const glbView = new DataView(glb.buffer);

  let offset = 0;

  // GLB header
  glb.set(new TextEncoder().encode("glTF"), offset);
  offset += 4;
  glbView.setUint32(offset, 2, true); // version
  offset += 4;
  glbView.setUint32(offset, totalLength, true); // length
  offset += 4;

  // JSON chunk
  glbView.setUint32(offset, jsonPadded.length, true); // chunk length
  offset += 4;
  glbView.setUint32(offset, 0x4e4f534a, true); // "JSON"
  offset += 4;
  glb.set(jsonPadded, offset);
  offset += jsonPadded.length;

  // BIN chunk
  glbView.setUint32(offset, binBuffer.length, true); // chunk length
  offset += 4;
  glbView.setUint32(offset, 0x004e4942, true); // "BIN\0"
  offset += 4;
  glb.set(binBuffer, offset);

  return glb;
}

/** Pad bytes to 4-byte alignment. */
function padTo4(bytes: Uint8Array, padByte = 0): Uint8Array {
  const paddedLength = (bytes.length + 3) & ~3;
  if (paddedLength === bytes.length) return bytes;

  const padded = new Uint8Array(paddedLength);
  padded.set(bytes);
  for (let i = bytes.length; i < paddedLength; i++) {
    padded[i] = padByte;
  }
  return padded;
}

interface BufferView {
  buffer: number;
  byteOffset: number;
  byteLength: number;
  target: number;
}

interface Accessor {
  bufferView: number;
  componentType: number;
  count: number;
  type: string;
  min?: number[];
  max?: number[];
}

interface Mesh {
  name: string;
  primitives: Array<{
    attributes: { POSITION: number };
    indices: number;
    material: number;
  }>;
}

interface GltfNode {
  mesh: number;
  name: string;
}
