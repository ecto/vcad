/**
 * GLB (binary glTF) export for visualization.
 *
 * Thin wrapper over the kernel WASM writer (`vcad-kernel-export` via
 * `buildGlbBytes`) — the single source of truth for GLB serialization.
 * This module only packs mesh/animation data into the flat buffers the
 * kernel expects; all byte layout, material dedup, KHR extensions, and
 * animation encoding happen in Rust.
 *
 * Requires the kernel WASM module to be initialized (`getKernelWasm()`)
 * before any build call — the MCP server does this at startup.
 */

import { getKernelWasmSync } from "@vcad/engine";
import type { EvaluatedScene } from "@vcad/engine";
import type { Document, Vec3 } from "@vcad/ir";

interface ExportWasm {
  buildGlbBytes(
    specJson: string,
    f32Data: Float32Array,
    u32Data: Uint32Array,
  ): Uint8Array;
  buildStlBytes(
    specJson: string,
    f32Data: Float32Array,
    u32Data: Uint32Array,
  ): Uint8Array;
  eulerXyzDegToQuat(xDeg: number, yDeg: number, zDeg: number): Float64Array;
}

/** The initialized kernel WASM module, or throw with a actionable message. */
export function exportKernel(): ExportWasm {
  const mod = getKernelWasmSync();
  if (!mod) {
    throw new Error(
      "kernel WASM not initialized — await getKernelWasm() before exporting",
    );
  }
  return mod as unknown as ExportWasm;
}

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
  /** Base-color alpha 0..1; below 1 the material is alpha-BLENDed
   *  (translucent soldermask shell). Defaults to opaque. */
  alpha?: number;
  /** Node TRS applied to part-local geometry (assembly instances). Identity
   *  components are omitted from the emitted node, so an identity transform
   *  produces byte-identical output to no transform. */
  transform?: GlbNodeTransform;
  /** Geometry-dedup key (e.g. a partDefId): inputs sharing a `meshKey` emit
   *  ONE glTF mesh referenced by multiple nodes. The first input carrying a
   *  key supplies the geometry and material for all of them. */
  meshKey?: string;
}

/**
 * One animation channel for {@link buildGlb}: keyframed TRS on a named node.
 */
export interface GlbAnimationChannel {
  /** Target node name — must match a {@link GlbMesh} node name (or the
   *  animation's `rootNodeName`) exactly. Unknown names are skipped. */
  nodeName: string;
  path: "translation" | "rotation" | "scale";
  /** Keyframe times in seconds, ascending. */
  times: number[];
  /** Flat keyframe values: VEC3 per key for translation/scale, VEC4
   *  quaternion `[x, y, z, w]` per key for rotation. */
  values: number[];
  /** Sampler interpolation; defaults to LINEAR. */
  interpolation?: "LINEAR" | "STEP";
}

/** Animation options for {@link buildGlb}. */
export interface GlbAnimationOptions {
  /** Animation name; defaults to `"timeline"`. */
  name?: string;
  channels: GlbAnimationChannel[];
  /** If set, a new parent node with this name wraps ALL scene nodes and
   *  becomes the sole scene root; channels may target it (turntable). */
  rootNodeName?: string;
  /** Extra empty (meshless) nodes added as scene roots; channels may
   *  target them. Used for out-of-band motion carriers like the
   *  `__camera` orbit node the viewer reads instead of rendering. */
  extraNodes?: string[];
}

/**
 * Convert a `Transform3D` Euler rotation in degrees (`rotation: Vec3`, Euler
 * XYZ deg) to the glTF node quaternion `[x, y, z, w]`.
 *
 * Composition AUTHORITY is the kernel (`R = Rz·Ry·Rx`, extrinsic XYZ) —
 * this delegates to the Rust implementation in `vcad-kernel-export`.
 */
export function eulerXyzDegToQuat(
  rotation: Vec3,
): [number, number, number, number] {
  const q = exportKernel().eulerXyzDegToQuat(rotation.x, rotation.y, rotation.z);
  return [q[0], q[1], q[2], q[3]];
}

/** A span `[offset, len]` into one of the packed flat buffers. */
type Span = [number, number];

/** Build binary GLB bytes from an explicit list of meshes + PBR materials.
 *
 * Packs geometry and keyframe data into two flat buffers and hands them to
 * the kernel WASM writer, which owns all serialization behavior (POSITION /
 * NORMAL / u32 indices, material + geometry dedup, KHR extensions, node
 * TRS, animations). */
export function buildGlb(
  inputMeshes: GlbMesh[],
  name: string,
  animation?: GlbAnimationOptions,
): Uint8Array {
  // Size the flat buffers.
  let f32Len = 0;
  let u32Len = 0;
  for (const m of inputMeshes) {
    f32Len += m.positions.length;
    u32Len += m.indices.length;
    if (m.normals && m.normals.length === m.positions.length) {
      f32Len += m.normals.length;
    }
  }
  for (const ch of animation?.channels ?? []) {
    f32Len += ch.times.length + ch.values.length;
  }

  const f32Data = new Float32Array(f32Len);
  const u32Data = new Uint32Array(u32Len);
  let f32Off = 0;
  let u32Off = 0;
  const pushF32 = (data: Float32Array | number[]): Span => {
    f32Data.set(data, f32Off);
    const span: Span = [f32Off, data.length];
    f32Off += data.length;
    return span;
  };
  const pushU32 = (data: Uint32Array | number[]): Span => {
    u32Data.set(data, u32Off);
    const span: Span = [u32Off, data.length];
    u32Off += data.length;
    return span;
  };

  const meshes = inputMeshes.map((m) => ({
    name: m.name,
    positions: pushF32(m.positions),
    indices: pushU32(m.indices),
    normals:
      m.normals && m.normals.length === m.positions.length
        ? pushF32(m.normals)
        : undefined,
    color: m.color,
    metallic: m.metallic,
    roughness: m.roughness,
    emissive: m.emissive,
    emissiveStrength: m.emissiveStrength,
    clearcoat: m.clearcoat,
    clearcoatRoughness: m.clearcoatRoughness,
    alpha: m.alpha,
    transform: m.transform,
    meshKey: m.meshKey,
  }));

  const spec = {
    name,
    meshes,
    animation: animation
      ? {
          name: animation.name,
          channels: animation.channels.map((ch) => ({
            nodeName: ch.nodeName,
            path: ch.path,
            times: pushF32(ch.times),
            values: pushF32(ch.values),
            interpolation: ch.interpolation,
          })),
          rootNodeName: animation.rootNodeName,
          extraNodes: animation.extraNodes,
        }
      : undefined,
  };

  return exportKernel().buildGlbBytes(JSON.stringify(spec), f32Data, u32Data);
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
