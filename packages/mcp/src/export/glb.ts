/**
 * GLB (binary glTF) export for visualization.
 *
 * Ported from crates/vcad/src/export/gltf.rs
 */

import type { EvaluatedScene, TriangleMesh } from "@vcad/engine";
import type { Document, Vec3 } from "@vcad/ir";

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
export const DEFAULT_MATERIAL = {
  name: "default",
  color: [0.8, 0.8, 0.8] as [number, number, number],
  metallic: 0.1,
  roughness: 0.5,
};

/**
 * A glTF node transform for {@link GlbMesh}: translation in mm, rotation as a
 * glTF quaternion `[x, y, z, w]` (see {@link eulerXyzDegToQuat}), per-axis
 * scale. Geometry stays part-local; the viewer applies the node TRS.
 */
export interface GlbNodeTransform {
  translation: [number, number, number];
  rotationQuat: [number, number, number, number];
  scale: [number, number, number];
}

/**
 * One renderable mesh for {@link buildGlb}: geometry plus an explicit PBR
 * material. Positions/indices/normals accept either typed arrays (the scene
 * path) or plain number arrays (the PCB-preview path). When `normals` is
 * omitted the GLB carries no NORMAL attribute and the viewer flat-shades.
 */
export interface GlbMesh {
  /** glTF node name — `"<part_id>:<name>"` for click-to-select. */
  name: string;
  positions: Float32Array | number[];
  indices: Uint32Array | number[];
  normals?: Float32Array | number[];
  color: [number, number, number];
  metallic: number;
  roughness: number;
  /** Emissive RGB 0..1 (linear); omitted/`[0,0,0]` = not emissive. */
  emissive?: [number, number, number];
  /** KHR_materials_emissive_strength multiplier (>1 = glows past white). */
  emissiveStrength?: number;
  /** KHR_materials_clearcoat factor 0..1 (glossy soldermask wet-look). */
  clearcoat?: number;
  /** Clearcoat roughness 0..1. */
  clearcoatRoughness?: number;
  /** Node TRS applied to part-local geometry (assembly instances). Identity
   *  components are omitted from the emitted node, so an identity transform
   *  produces byte-identical output to no transform. */
  transform?: GlbNodeTransform;
  /** Geometry-dedup key (e.g. a partDefId): inputs sharing a `meshKey` emit
   *  ONE glTF mesh referenced by multiple nodes. The first input carrying a
   *  key supplies the geometry and material for all of them. */
  meshKey?: string;
}

/** A PBR material resolved from a {@link GlbMesh}, deduped across meshes. */
interface GlbMaterial {
  name: string;
  color: [number, number, number];
  metallic: number;
  roughness: number;
  emissive: [number, number, number];
  emissiveStrength: number;
  clearcoat: number;
  clearcoatRoughness: number;
}

const isEmissive = (e: [number, number, number]): boolean =>
  e[0] > 0 || e[1] > 0 || e[2] > 0;

const f32 = (a: Float32Array | number[]): Float32Array =>
  a instanceof Float32Array ? a : new Float32Array(a);

/**
 * Convert a `Transform3D` Euler rotation in degrees (`rotation: Vec3`, Euler
 * XYZ deg) to the glTF node quaternion `[x, y, z, w]`.
 *
 * Composition AUTHORITY: the kernel applies Transform3D rotations as
 * `R = Rz·Ry·Rx` on column vectors — rotate about world X first, then world
 * Y, then world Z (extrinsic XYZ). See crates/vcad-eval/src/kinematics.rs
 * `euler_to_matrix` ("// Rz * Ry * Rx"), the identical matrix in
 * packages/engine/src/evaluate.ts `transformMesh`, and evaluate.rs's
 * `rx.then(ry).then(rz)`. In three.js Euler-order terms this is "ZYX"
 * (three's "XYZ" is the intrinsic Rx·Ry·Rz — the OPPOSITE order), so the
 * quaternion below is q = qz ⊗ qy ⊗ qx.
 */
export function eulerXyzDegToQuat(
  rotation: Vec3,
): [number, number, number, number] {
  const rad = Math.PI / 180;
  const hx = (rotation.x * rad) / 2;
  const hy = (rotation.y * rad) / 2;
  const hz = (rotation.z * rad) / 2;
  const c1 = Math.cos(hx);
  const s1 = Math.sin(hx);
  const c2 = Math.cos(hy);
  const s2 = Math.sin(hy);
  const c3 = Math.cos(hz);
  const s3 = Math.sin(hz);
  // q = qz ⊗ qy ⊗ qx (X applied first) — the "ZYX" quaternion.
  return [
    s1 * c2 * c3 - c1 * s2 * s3,
    c1 * s2 * c3 + s1 * c2 * s3,
    c1 * c2 * s3 - s1 * s2 * c3,
    c1 * c2 * c3 + s1 * s2 * s3,
  ];
}

/** Build binary GLB bytes from an explicit list of meshes + PBR materials.
 *
 * Writes POSITION, NORMAL (when present), and u32 indices per mesh, and
 * de-dupes materials by `(color, metallic, roughness)` so a layered board
 * (board / copper / components / silk) stays at a handful of materials. */
export function buildGlb(inputMeshes: GlbMesh[], name: string): Uint8Array {
  // Collect unique materials keyed by their full PBR values.
  const materialMap = new Map<string, number>();
  const materials: GlbMaterial[] = [];

  const materialIndexFor = (m: GlbMesh): number => {
    const emissive = m.emissive ?? [0, 0, 0];
    const emissiveStrength = m.emissiveStrength ?? 1;
    const clearcoat = m.clearcoat ?? 0;
    const clearcoatRoughness = m.clearcoatRoughness ?? 0;
    const key = `${m.color[0]},${m.color[1]},${m.color[2]},${m.metallic},${m.roughness},${emissive[0]},${emissive[1]},${emissive[2]},${emissiveStrength},${clearcoat},${clearcoatRoughness}`;
    const existing = materialMap.get(key);
    if (existing !== undefined) return existing;
    const idx = materials.length;
    materialMap.set(key, idx);
    materials.push({
      name: m.name.includes(":") ? m.name.split(":")[0] : m.name,
      color: m.color,
      metallic: m.metallic,
      roughness: m.roughness,
      emissive,
      emissiveStrength,
      clearcoat,
      clearcoatRoughness,
    });
    return idx;
  };

  if (inputMeshes.length === 0) {
    materials.push({
      ...DEFAULT_MATERIAL,
      emissive: [0, 0, 0],
      emissiveStrength: 1,
      clearcoat: 0,
      clearcoatRoughness: 0,
    });
  }

  // Build binary buffer for all meshes
  const bufferChunks: Uint8Array[] = [];
  const bufferViews: BufferView[] = [];
  const accessors: Accessor[] = [];
  const meshes: Mesh[] = [];
  const nodes: GltfNode[] = [];

  let bufferOffset = 0;

  // Geometry dedup: inputs sharing a `meshKey` (assembly instances of one
  // partDef) emit one glTF mesh, referenced by one node per input.
  const meshKeyToIdx = new Map<string, number>();

  for (let inputIdx = 0; inputIdx < inputMeshes.length; inputIdx++) {
    const input = inputMeshes[inputIdx];

    const dedupIdx =
      input.meshKey !== undefined ? meshKeyToIdx.get(input.meshKey) : undefined;
    if (dedupIdx !== undefined) {
      nodes.push(makeNode(input, dedupIdx));
      continue;
    }

    const positions = f32(input.positions);
    const normals =
      input.normals && input.normals.length === positions.length
        ? f32(input.normals)
        : undefined;

    const vertexCount = positions.length / 3;
    const indexCount = input.indices.length;

    // Calculate bounds
    let minX = Infinity,
      minY = Infinity,
      minZ = Infinity;
    let maxX = -Infinity,
      maxY = -Infinity,
      maxZ = -Infinity;

    for (let i = 0; i < vertexCount; i++) {
      const x = positions[i * 3];
      const y = positions[i * 3 + 1];
      const z = positions[i * 3 + 2];
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
      indicesView.setUint32(i * 4, input.indices[i], true);
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

    // Write positions as f32 (copy into a fresh, tightly-packed buffer so a
    // subarray view of a larger backing buffer doesn't leak extra bytes).
    const positionsBytes = new Uint8Array(
      positions.buffer.slice(
        positions.byteOffset,
        positions.byteOffset + vertexCount * 12,
      ),
    );
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

    const attributes: { POSITION: number; NORMAL?: number } = {
      POSITION: positionsAccIdx,
    };

    // Normals (optional) — gives the viewer proper smooth/flat shading.
    if (normals) {
      const normalsBytes = new Uint8Array(
        normals.buffer.slice(
          normals.byteOffset,
          normals.byteOffset + vertexCount * 12,
        ),
      );
      const normalsPadded = padTo4(normalsBytes);
      bufferChunks.push(normalsPadded);

      const normalsBvIdx = bufferViews.length;
      bufferViews.push({
        buffer: 0,
        byteOffset: bufferOffset,
        byteLength: vertexCount * 12,
        target: 34962, // ARRAY_BUFFER
      });
      bufferOffset += normalsPadded.length;

      const normalsAccIdx = accessors.length;
      accessors.push({
        bufferView: normalsBvIdx,
        componentType: 5126, // FLOAT
        count: vertexCount,
        type: "VEC3",
      });
      attributes.NORMAL = normalsAccIdx;
    }

    // Mesh
    const meshIdx = meshes.length;
    meshes.push({
      name: `mesh_${meshIdx}`,
      primitives: [
        {
          attributes,
          indices: indicesAccIdx,
          material: materialIndexFor(input),
        },
      ],
    });
    if (input.meshKey !== undefined) meshKeyToIdx.set(input.meshKey, meshIdx);

    // Node — named with part identity when the caller provides it.
    nodes.push(makeNode(input, meshIdx));
  }

  // Build JSON. Materials may carry KHR extensions (clearcoat for glossy
  // soldermask, emissive_strength for LEDs that glow past white). GLTFLoader
  // applies both automatically onto a MeshPhysicalMaterial.
  let usesClearcoat = false;
  let usesEmissiveStrength = false;
  const materialJson = materials.map((m) => {
    const mat: Record<string, unknown> = {
      name: m.name,
      pbrMetallicRoughness: {
        baseColorFactor: [...m.color, 1.0],
        metallicFactor: m.metallic,
        roughnessFactor: m.roughness,
      },
    };
    if (isEmissive(m.emissive)) {
      mat.emissiveFactor = m.emissive;
    }
    const extensions: Record<string, unknown> = {};
    if (m.clearcoat > 0) {
      usesClearcoat = true;
      extensions.KHR_materials_clearcoat = {
        clearcoatFactor: m.clearcoat,
        clearcoatRoughnessFactor: m.clearcoatRoughness,
      };
    }
    if (isEmissive(m.emissive) && m.emissiveStrength !== 1) {
      usesEmissiveStrength = true;
      extensions.KHR_materials_emissive_strength = {
        emissiveStrength: m.emissiveStrength,
      };
    }
    if (Object.keys(extensions).length > 0) mat.extensions = extensions;
    return mat;
  });

  const extensionsUsed: string[] = [];
  if (usesClearcoat) extensionsUsed.push("KHR_materials_clearcoat");
  if (usesEmissiveStrength)
    extensionsUsed.push("KHR_materials_emissive_strength");

  const json: Record<string, unknown> = {
    asset: { version: "2.0", generator: "vcad-mcp" },
    scene: 0,
    scenes: [{ name, nodes: nodes.map((_, i) => i) }],
    nodes,
    meshes,
    materials: materialJson,
    accessors,
    bufferViews,
    buffers: [{ byteLength: bufferOffset }],
  };
  if (extensionsUsed.length > 0) json.extensionsUsed = extensionsUsed;

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

/**
 * Build a glTF node for one input mesh: `{mesh, name}` plus TRS fields when a
 * transform is present. Identity components are omitted (glTF defaults), so
 * an identity transform emits byte-identical JSON to no transform at all.
 */
function makeNode(input: GlbMesh, meshIdx: number): GltfNode {
  const node: GltfNode = { mesh: meshIdx, name: input.name };
  const t = input.transform;
  if (!t) return node;
  const [tx, ty, tz] = t.translation;
  if (tx !== 0 || ty !== 0 || tz !== 0) node.translation = t.translation;
  const [qx, qy, qz, qw] = t.rotationQuat;
  if (qx !== 0 || qy !== 0 || qz !== 0 || qw !== 1) node.rotation = t.rotationQuat;
  const [sx, sy, sz] = t.scale;
  if (sx !== 1 || sy !== 1 || sz !== 1) node.scale = t.scale;
  return node;
}

/** Convert an evaluated scene to binary GLB bytes.
 *
 * `partLabels` (index-aligned with `scene.parts`) become glTF node names —
 * the viewer parses them back into part identity for click-to-select, so
 * they follow the `"<part_id>:<name>"` convention from
 * {@link buildPartLabels}. Omitted entries fall back to `part_<idx>`.
 *
 * Parts keep the neutral default material; for colored PCB previews the
 * caller builds {@link GlbMesh}es directly and calls {@link buildGlb}. */
export function toGlbBytes(
  scene: EvaluatedScene,
  name: string,
  partLabels?: string[],
): Uint8Array {
  const meshes: GlbMesh[] = scene.parts.map((part, i) => ({
    name: partLabels?.[i] ?? `part_${i}`,
    positions: part.mesh.positions,
    indices: part.mesh.indices,
    normals: part.mesh.normals,
    color: DEFAULT_MATERIAL.color,
    metallic: DEFAULT_MATERIAL.metallic,
    roughness: DEFAULT_MATERIAL.roughness,
  }));
  return buildGlb(meshes, name);
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
    attributes: { POSITION: number; NORMAL?: number };
    indices: number;
    material: number;
  }>;
}

interface GltfNode {
  mesh: number;
  name: string;
  /** Node translation (mm), omitted at identity. */
  translation?: [number, number, number];
  /** Node rotation quaternion [x, y, z, w], omitted at identity. */
  rotation?: [number, number, number, number];
  /** Node per-axis scale, omitted at identity. */
  scale?: [number, number, number];
}
