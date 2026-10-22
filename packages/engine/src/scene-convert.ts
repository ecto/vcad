/**
 * Kernel scene → `EvaluatedScene`, shared by the eval worker and the engine
 * host: typed-array conversion, the TS-side post-passes for root kinds the
 * Rust evaluator leaves empty (embroidery, sheet metal), and the buffer list
 * for zero-copy `postMessage`.
 */
import {
  embroideryPatternToMeshWithKernel,
  findEmbroideryPattern,
  transformMeshWithKernel,
  resolveSheetMetalPart,
} from "./evaluate.js";
import type { EvaluatedScene, EvalTimingData, TriangleMesh } from "./mesh.js";
import type { Document } from "@vcad/ir";

/** WASM evaluator result shape (typed arrays from Rust, or plain arrays from legacy) */
export interface WasmMesh {
  positions: Float32Array | number[];
  indices: Uint32Array | number[];
  normals?: Float32Array | number[];
  faceKinds?: Uint8Array | number[];
}

export interface WasmEvaluatedScene {
  parts: Array<{ mesh: WasmMesh; material: string }>;
  partDefs?: Array<{ id: string; mesh: WasmMesh }>;
  instances?: Array<{
    instance_id: string;
    part_def_id: string;
    name?: string;
    mesh: WasmMesh;
    material: string;
    transform?: unknown;
  }>;
  clashes: Array<WasmMesh>;
  failures?: Array<{ scope: string; node_id: number; error: string }>;
  timing?: EvalTimingData;
}

/** The kernel bindings the post-passes call. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type SceneKernel = any;

/** Collect ArrayBuffers from an EvaluatedScene for zero-copy transfer. */
export function collectTransferables(scene: EvaluatedScene): ArrayBuffer[] {
  const buffers: ArrayBuffer[] = [];

  const collectMesh = (m: TriangleMesh) => {
    buffers.push(m.positions.buffer as ArrayBuffer);
    buffers.push(m.indices.buffer as ArrayBuffer);
    if (m.normals) buffers.push(m.normals.buffer as ArrayBuffer);
    if (m.colors) buffers.push(m.colors.buffer as ArrayBuffer);
    if (m.faceKinds) buffers.push(m.faceKinds.buffer as ArrayBuffer);
  };

  for (const p of scene.parts) collectMesh(p.mesh);
  if (scene.partDefs) for (const pd of scene.partDefs) collectMesh(pd.mesh);
  if (scene.instances) for (const inst of scene.instances) collectMesh(inst.mesh);
  for (const c of scene.clashes) collectMesh(c);

  return buffers;
}

/** Convert WASM result to EvaluatedScene. Handles both typed arrays (fast path) and plain arrays. */
export function wasmResultToScene(result: WasmEvaluatedScene): EvaluatedScene {
  const toMesh = (m: WasmMesh): TriangleMesh => ({
    positions: m.positions instanceof Float32Array ? m.positions : new Float32Array(m.positions),
    indices: m.indices instanceof Uint32Array ? m.indices : new Uint32Array(m.indices),
    normals: m.normals
      ? m.normals instanceof Float32Array ? m.normals : new Float32Array(m.normals)
      : undefined,
    faceKinds: m.faceKinds
      ? m.faceKinds instanceof Uint8Array ? m.faceKinds : new Uint8Array(m.faceKinds)
      : undefined,
  });

  return {
    parts: result.parts.map((p) => ({ mesh: toMesh(p.mesh), material: p.material })),
    partDefs: result.partDefs?.map((pd) => ({ id: pd.id, mesh: toMesh(pd.mesh) })),
    instances: result.instances?.map((inst) => ({
      instanceId: inst.instance_id,
      partDefId: inst.part_def_id,
      name: inst.name,
      mesh: toMesh(inst.mesh),
      material: inst.material,
      transform: inst.transform as EvaluatedScene["instances"] extends Array<infer T> ? T extends { transform?: infer X } ? X : never : never,
    })),
    clashes: result.clashes.map(toMesh),
    failures: result.failures,
    timing: result.timing,
  };
}

/** Post-process WASM result: generate TS-side meshes for types the Rust
 *  evaluator doesn't tessellate (e.g. EmbroideryPattern). */
export function postProcessEmbroidery(
  scene: EvaluatedScene,
  doc: Document,
  kernel: SceneKernel,
): EvaluatedScene {
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  let changed = false;
  const parts = scene.parts.map((p, i) => {
    if (p.mesh.positions.length === 0 && i < visibleRoots.length) {
      const emb = findEmbroideryPattern(visibleRoots[i].root, doc.nodes);
      if (emb) {
        changed = true;
        const baseMesh = embroideryPatternToMeshWithKernel(emb.pattern, kernel);
        const mesh = transformMeshWithKernel(baseMesh, emb.transform, kernel);
        return { mesh, material: p.material };
      }
    }
    return p;
  });
  return changed ? { ...scene, parts } : scene;
}

/** Post-process WASM result: route sheet-metal roots (which the Rust
 *  document evaluator leaves empty) through the dedicated kernel binding,
 *  mirroring {@link postProcessEmbroidery}. Without this the worker path —
 *  which the app uses by default — never attaches `part.sheetMetal`, so
 *  the sheet-metal panel/DFM inspector never appears. */
export function postProcessSheetMetal(
  scene: EvaluatedScene,
  doc: Document,
  kernel: SceneKernel,
): EvaluatedScene {
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  let changed = false;
  const parts = scene.parts.map((p, i) => {
    if (p.mesh.positions.length === 0 && i < visibleRoots.length) {
      // Resolve any Translate/Rotate/Scale wrapper to the chain tip and place
      // the folded body, so a positioned bracket (e.g.
      // `Translate(child: EdgeFlange)`) is recognized — not just a bare root.
      try {
        const smPart = resolveSheetMetalPart(visibleRoots[i]!.root, doc.nodes, kernel);
        if (smPart) {
          changed = true;
          return { ...smPart, material: p.material };
        }
      } catch (e) {
        console.warn(`[worker] sheet-metal eval failed at root[${i}]:`, e);
      }
    }
    return p;
  });
  return changed ? { ...scene, parts } : scene;
}

/** Both post-passes, in the order the eval worker has always run them. */
export function postProcessScene(
  scene: EvaluatedScene,
  doc: Document,
  kernel: SceneKernel,
): EvaluatedScene {
  return postProcessSheetMetal(postProcessEmbroidery(scene, doc, kernel), doc, kernel);
}
