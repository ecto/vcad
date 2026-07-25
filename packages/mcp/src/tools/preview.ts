/**
 * GLB preview generation for MCP Apps viewer.
 *
 * Evaluates an IR document to produce a base64-encoded GLB that can
 * be rendered inline by the MCP Apps viewer iframe.
 *
 * PCBs get special treatment: the canonical `PcbBoard` evaluation merges the
 * FR4 slab, copper, and component boxes into one gray solid, which renders as
 * a featureless slab. For the preview we swap that part for the kernel's
 * layered, colored preview meshes (green substrate, gold copper, real 3D
 * component bodies, white silkscreen) so the inline viewer shows a recognizable
 * board.
 */

import type { Document } from "@vcad/ir";
import { pcbPreviewMeshes, transformMesh, type Engine } from "@vcad/engine";
import { getNodePcb } from "@vcad/core";
import {
  buildGlb,
  buildPartLabels,
  eulerXyzDegToQuat,
  DEFAULT_MATERIAL,
  type GlbMesh,
} from "../export/glb.js";
import { getSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

// ─── Content-addressed GLB cache ─────────────────────────────────────────────
//
// The viewer's first paint used to pay a full engine.evaluate + tessellation
// on EVERY get_preview_glb call — including the fetch that happens ~1s after
// the mutation tool already evaluated the same document. Cache the built GLB
// keyed by (content hash, mode): the value is a pure function of the document
// content, so the cache is tenant-safe (two users with identical content get
// identical bytes) and never serves stale geometry (any edit flips the hash).
const GLB_CACHE_MAX = 16;
const glbCache = new Map<string, CachedGlb>();

interface CachedGlb {
  glb: string;
  degraded?: boolean;
  oversize?: boolean;
}

function glbCacheGet(key: string): CachedGlb | undefined {
  const hit = glbCache.get(key);
  if (hit !== undefined) {
    // LRU touch: re-insert so the hottest entries survive eviction.
    glbCache.delete(key);
    glbCache.set(key, hit);
  }
  return hit;
}

function glbCachePut(key: string, value: CachedGlb): void {
  glbCache.delete(key);
  glbCache.set(key, value);
  while (glbCache.size > GLB_CACHE_MAX) {
    const oldest = glbCache.keys().next().value;
    if (oldest === undefined) break;
    glbCache.delete(oldest);
  }
}

/**
 * A ready-to-render preview envelope: base64 GLB + change token (+ mode).
 *
 * `mode` is the authoritative statement of what the GLB contains: it is
 * `"instances"` ONLY when the GLB carries one node per assembly instance.
 * Requesting `instances = true` on a document without instances falls back
 * to the merged parts preview and returns `mode: undefined` — callers that
 * need instance nodes (FK replay) must check `mode`, not the flag they
 * passed.
 */
export interface PreviewGlb {
  glb: string;
  version: string;
  mode?: "instances";
  /** The GLB was decimated to fit {@link PREVIEW_MAX_BASE64} — preview
   *  fidelity, not export fidelity. */
  degraded?: boolean;
  /** Even after degradation the payload exceeds the budget. Callers must NOT
   *  ship it through the JSON-RPC channel. */
  oversize?: boolean;
}

/**
 * Hard upper bound (in base64 chars) on ANY preview GLB leaving this server —
 * the `_meta` inline attach AND the `get_preview_glb` fetch fallback alike.
 *
 * Both paths ride the same JSON-RPC transport, so they have the same ceiling;
 * previously only the inline attach checked a limit, which meant every
 * document above that limit silently fell through to a fetch path that was
 * *guaranteed* to blow the host's result ceiling and surface as a bare
 * "preview unavailable". A document over budget is now decimated until it
 * fits (see {@link fitPreviewToBudget}) rather than being sent whole.
 */
export const PREVIEW_MAX_BASE64 = 1_500_000;

/**
 * Build (or fetch from cache) the preview GLB for a document. Single shared
 * path for the `get_preview_glb` tool and the dispatch layer's inline
 * `_meta` preview, so both populate/benefit from the same cache. Returns
 * null when the document has no previewable geometry.
 */
export async function previewGlbFor(
  doc: Document,
  engine: Engine,
  instances = false,
): Promise<PreviewGlb | null> {
  const version = previewVersion(doc);
  if (instances) {
    const key = `${version}:instances`;
    const cached = glbCacheGet(key);
    if (cached !== undefined) {
      return { ...cached, version, mode: "instances" };
    }
    const instGlb = generateInstancesGlbPreview(doc, engine);
    if (instGlb) {
      // Instances mode is NOT degraded: the glTF node layout (one node per
      // instance, meshes shared by `meshKey`) is the replay's FK bind
      // contract, and per-mesh decimation would desync shared meshes. An
      // over-budget instances preview is reported as oversize instead.
      const entry: CachedGlb = {
        glb: instGlb,
        oversize: instGlb.length > PREVIEW_MAX_BASE64 || undefined,
      };
      glbCachePut(key, entry);
      return { ...entry, version, mode: "instances" };
    }
    // No instances — fall through to the parts preview (flag is safe on any doc).
  }
  const key = `${version}:parts`;
  const cached = glbCacheGet(key);
  if (cached !== undefined) return { ...cached, version };
  const built = await generateGlbPreview(doc, engine);
  if (!built) return null;
  glbCachePut(key, built);
  return { ...built, version };
}

/**
 * Generate a base64-encoded GLB preview from an IR document.
 * Returns null only when the document has no previewable geometry at all.
 *
 * Board roots are rendered from the layered PCB preview meshes, which are
 * built straight from the PCB data (outline + footprints + pads) and never
 * touch the BRep boolean pipeline. We build them *before* and independent of
 * the scene eval, so a PCB session always previews — even when the canonical
 * board solid fails to evaluate (e.g. a degenerate `board_from_solid` outline)
 * or `engine.evaluate` throws outright. Otherwise such a session showed the
 * viewer an empty grid despite having a fully-placed board.
 */
export async function generateGlbPreview(
  doc: Document,
  engine: Engine,
  /** Base64-char ceiling to fit the payload into, or `null` for full fidelity.
   *  Tool results ride JSON-RPC and must be bounded; the plain-HTTP live route
   *  serves bytes over a real HTTP body and passes `null`. */
  budget: number | null = PREVIEW_MAX_BASE64,
): Promise<CachedGlb | null> {
  // Part-identity node names let the viewer map a click back to a part_id
  // for selection context and "ask about this part".
  const labels = buildPartLabels(doc);
  // `scene.parts` is index-aligned with the visible roots.
  const visibleRoots = doc.roots.filter((r) => r.visible !== false);

  const meshes: GlbMesh[] = [];
  // Roots already rendered from the PCB-data path — the scene loop skips them.
  const handledBoards = new Set<number>();

  for (let i = 0; i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = doc.nodes[String(rootId)];
    if (node?.op?.type !== "PcbBoard") continue;
    const pcb = getNodePcb(doc, rootId);
    const preview = pcb ? await pcbPreviewMeshes(pcb) : [];
    // Empty preview (older kernel WASM lacks the binding) — leave the board
    // for the scene loop, which renders its neutral merged slab instead.
    if (preview.length === 0) continue;
    handledBoards.add(Number(rootId));
    pushPcbPreview(meshes, labels[i] ?? `part_${i}`, preview);
  }

  // Non-board geometry (and any board whose preview meshes were unavailable)
  // comes from the normal scene eval. Wrapped so a single failing board — or a
  // hard eval throw — can't blank a preview the PCB-data path already filled.
  try {
    const scene = engine.evaluate(doc);
    for (let i = 0; i < (scene?.parts.length ?? 0); i++) {
      const rootId = visibleRoots[i]?.root;
      if (rootId !== undefined && handledBoards.has(Number(rootId))) continue;

      const part = scene!.parts[i];
      const name = labels[i] ?? `part_${i}`;
      const node = rootId !== undefined ? doc.nodes[String(rootId)] : undefined;

      // Old-WASM fallback: a board with no preview meshes still renders as the
      // neutral merged slab rather than being dropped.
      if (node?.op?.type === "PcbBoard" && rootId !== undefined) {
        const pcb = getNodePcb(doc, rootId);
        const preview = pcb ? await pcbPreviewMeshes(pcb) : [];
        if (preview.length > 0) {
          pushPcbPreview(meshes, name, preview);
          continue;
        }
      }

      meshes.push({
        name,
        positions: part.mesh.positions,
        indices: part.mesh.indices,
        normals: part.mesh.normals,
        color: DEFAULT_MATERIAL.color,
        metallic: DEFAULT_MATERIAL.metallic,
        roughness: DEFAULT_MATERIAL.roughness,
      });
    }

    // Assembly instances (part-local mesh + world transform). Bake the
    // transform into the vertices so an assembly-only document previews
    // instead of showing an empty grid.
    for (const inst of scene?.instances ?? []) {
      const mesh = inst.transform
        ? transformMesh(inst.mesh, {
            translate: inst.transform.translation,
            rotate: inst.transform.rotation,
            scale: inst.transform.scale,
          })
        : inst.mesh;
      meshes.push({
        name: `${inst.instanceId}:${inst.name ?? ""}`,
        positions: mesh.positions,
        indices: mesh.indices,
        normals: mesh.normals,
        color: DEFAULT_MATERIAL.color,
        metallic: DEFAULT_MATERIAL.metallic,
        roughness: DEFAULT_MATERIAL.roughness,
      });
    }
  } catch {
    // Evaluation failures should not break the preview — PCB sessions still
    // render from the board path above; a non-PCB doc that can't evaluate
    // falls through to the empty-geometry signal below.
  }

  if (meshes.length === 0) return null;
  return fitPreviewToBudget(meshes, budget);
}

// ─── Bounded preview payloads ────────────────────────────────────────────────
//
// A preview GLB is unbounded in principle: filleted mechanical parts inflate
// triangle counts hard (a 31-part filleted assembly reaches ~81k triangles and
// ~9 MB of base64), and the kernel tessellation is non-indexed, so the naive
// payload is ~84 bytes per triangle. The transport ceiling doesn't care why —
// it just drops the result. So the preview is fitted to the budget here, once,
// on the single path both the inline attach and the fetch fallback share.
//
// Degradation is uniform vertex clustering (a grid over the scene bbox,
// collapsing each cell's vertices to their centroid, dropping the triangles
// that collapse). It's cheap, deterministic, and preserves per-part mesh
// identity — node names, materials, and part→click mapping all survive, which
// quadric decimation across merged meshes would not.

/** Grid resolutions tried, coarsest-last. 256 barely changes a normal part;
 *  4 flattens anything into a coarse blob. The ladder bottoms out rather than
 *  running forever — below ~4 cells across the scene there is no shape left. */
const DECIMATE_GRIDS = [256, 192, 128, 96, 64, 48, 32, 24, 16, 12, 8, 6, 4];

/**
 * Build the GLB for `meshes`, decimating progressively until the base64
 * payload fits {@link PREVIEW_MAX_BASE64}. Documents under budget are built
 * once and returned at full fidelity — degradation only ever kicks in above
 * the ceiling, where the alternative is no preview at all.
 */
export function fitPreviewToBudget(
  meshes: GlbMesh[],
  budget: number | null = PREVIEW_MAX_BASE64,
): CachedGlb {
  const full = uint8ArrayToBase64(buildGlb(meshes, "preview"));
  if (budget === null || full.length <= budget) return { glb: full };

  const box = sceneBounds(meshes);
  // Diagonal-relative cell size: scale-independent, so a 5 mm part and a
  // 500 mm assembly degrade the same way.
  const diag =
    Math.hypot(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]) ||
    1;

  // Surface-ish scaling: clustered triangle count grows ~grid², so start near
  // the resolution the required reduction implies instead of walking the whole
  // ladder from 256 down (each rung costs a full pass over every vertex).
  const startGrid = DECIMATE_GRIDS[0] * Math.sqrt(budget / full.length);
  let best = full;
  for (const grid of DECIMATE_GRIDS) {
    if (grid > startGrid * 1.5) continue;
    const cell = diag / grid;
    const reduced = meshes.map((m) => decimateMesh(m, cell, box.min, grid));
    if (reduced.every((m, i) => m.indices.length === meshes[i].indices.length)) {
      continue; // grid too fine to change anything — skip the rebuild
    }
    // Skip the (expensive) serialize when the buffers alone already blow the
    // budget — the GLB is strictly larger than its vertex/index data.
    if (estimateBase64(reduced) > budget) continue;
    const glb = uint8ArrayToBase64(buildGlb(reduced, "preview"));
    best = glb;
    if (glb.length <= budget) return { glb, degraded: true };
  }
  // Pathological input (e.g. millions of triangles spread so thin that even
  // the coarsest grid keeps them distinct). Report it rather than shipping a
  // payload we know the transport will drop.
  return { glb: best, degraded: true, oversize: true };
}

/** Lower bound on the base64 length of the GLB these meshes would serialize
 *  to: f32 positions + f32 normals + u32 indices, 4/3 base64 expansion. */
function estimateBase64(meshes: GlbMesh[]): number {
  let bytes = 0;
  for (const m of meshes) {
    bytes += m.positions.length * 4 + (m.normals?.length ?? 0) * 4 + m.indices.length * 4;
  }
  return Math.ceil((bytes * 4) / 3);
}

interface Bounds {
  min: [number, number, number];
  max: [number, number, number];
}

function sceneBounds(meshes: GlbMesh[]): Bounds {
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (const m of meshes) {
    const p = m.positions;
    for (let i = 0; i + 2 < p.length; i += 3) {
      for (let a = 0; a < 3; a++) {
        const v = p[i + a];
        if (v < min[a]) min[a] = v;
        if (v > max[a]) max[a] = v;
      }
    }
  }
  for (let a = 0; a < 3; a++) {
    if (!Number.isFinite(min[a])) {
      min[a] = 0;
      max[a] = 0;
    }
  }
  return { min, max };
}

/**
 * Collapse a mesh's vertices onto a uniform grid of `cell`-sized cubes anchored
 * at `origin`, dropping triangles whose corners land in the same cell. Normals
 * are recomputed area-weighted from the surviving triangles, so the result
 * still shades — a decimated mesh carrying stale normals reads as corrupt.
 */
export function decimateMesh(
  mesh: GlbMesh,
  cell: number,
  origin: [number, number, number],
  /** Cells per axis across the scene — only used to pack the cell coordinates
   *  into a collision-free integer key. Must bound the coordinates actually
   *  produced by `cell`; a mesh reaching past it clamps into the edge cell. */
  grid = 4096,
): GlbMesh {
  const pos = mesh.positions;
  const idx = mesh.indices;
  const vertCount = Math.floor(pos.length / 3);
  if (cell <= 0 || vertCount === 0) return mesh;

  // Cell key → new vertex index; accumulate centroids so the collapsed
  // vertex sits on the surface rather than at a grid corner.
  const cells = new Map<number, number>();
  const remap = new Int32Array(vertCount);
  const sums: number[] = [];
  const counts: number[] = [];
  const span = grid + 1;
  const axis = (v: number, o: number): number =>
    Math.min(span - 1, Math.max(0, Math.floor((v - o) / cell)));
  for (let v = 0; v < vertCount; v++) {
    const x = pos[v * 3];
    const y = pos[v * 3 + 1];
    const z = pos[v * 3 + 2];
    const key =
      (axis(x, origin[0]) * span + axis(y, origin[1])) * span + axis(z, origin[2]);
    let ni = cells.get(key);
    if (ni === undefined) {
      ni = counts.length;
      cells.set(key, ni);
      sums.push(0, 0, 0);
      counts.push(0);
    }
    sums[ni * 3] += x;
    sums[ni * 3 + 1] += y;
    sums[ni * 3 + 2] += z;
    counts[ni] += 1;
    remap[v] = ni;
  }

  const newPos = new Float32Array(counts.length * 3);
  for (let i = 0; i < counts.length; i++) {
    const n = counts[i] || 1;
    newPos[i * 3] = sums[i * 3] / n;
    newPos[i * 3 + 1] = sums[i * 3 + 1] / n;
    newPos[i * 3 + 2] = sums[i * 3 + 2] / n;
  }

  const newIdx: number[] = [];
  const newNormals = new Float32Array(counts.length * 3);
  for (let t = 0; t + 2 < idx.length; t += 3) {
    const a = remap[idx[t]];
    const b = remap[idx[t + 1]];
    const c = remap[idx[t + 2]];
    if (a === b || b === c || a === c) continue; // collapsed to a sliver
    newIdx.push(a, b, c);
    // Area-weighted face normal (cross product magnitude IS 2×area).
    const ux = newPos[b * 3] - newPos[a * 3];
    const uy = newPos[b * 3 + 1] - newPos[a * 3 + 1];
    const uz = newPos[b * 3 + 2] - newPos[a * 3 + 2];
    const vx = newPos[c * 3] - newPos[a * 3];
    const vy = newPos[c * 3 + 1] - newPos[a * 3 + 1];
    const vz = newPos[c * 3 + 2] - newPos[a * 3 + 2];
    const nx = uy * vz - uz * vy;
    const ny = uz * vx - ux * vz;
    const nz = ux * vy - uy * vx;
    for (const i of [a, b, c]) {
      newNormals[i * 3] += nx;
      newNormals[i * 3 + 1] += ny;
      newNormals[i * 3 + 2] += nz;
    }
  }
  if (newIdx.length === 0) return mesh; // fully collapsed — keep the original

  for (let i = 0; i < counts.length; i++) {
    const len = Math.hypot(
      newNormals[i * 3],
      newNormals[i * 3 + 1],
      newNormals[i * 3 + 2],
    );
    if (len > 0) {
      newNormals[i * 3] /= len;
      newNormals[i * 3 + 1] /= len;
      newNormals[i * 3 + 2] /= len;
    }
  }

  return {
    ...mesh,
    positions: newPos,
    indices: new Uint32Array(newIdx),
    normals: newNormals,
  };
}

/**
 * Generate a base64 GLB preview with one named node PER ASSEMBLY INSTANCE,
 * for the replay viewer's FK playback. Node names follow
 * `"<instanceId>:<name>"` (mirroring the `"<part_id>:<name>"` root
 * convention) so the viewer can bind per-step transforms from
 * `get_sim_replay` back to nodes. Geometry stays part-local — the FK-solved
 * world pose rides on the glTF node TRS — and instances of one partDef share
 * a single glTF mesh via `meshKey`.
 *
 * Returns null when the scene has no instances (or evaluation fails), so the
 * caller can fall back to the parts path and the flag is safe on any doc.
 */
export function generateInstancesGlbPreview(
  doc: Document,
  engine: Engine,
): string | null {
  try {
    const scene = engine.evaluate(doc);
    const instances = scene?.instances;
    if (!instances || instances.length === 0) return null;

    const meshes: GlbMesh[] = instances.map((inst) => ({
      name: `${inst.instanceId}:${inst.name ?? ""}`,
      positions: inst.mesh.positions,
      indices: inst.mesh.indices,
      normals: inst.mesh.normals,
      color: DEFAULT_MATERIAL.color,
      metallic: DEFAULT_MATERIAL.metallic,
      roughness: DEFAULT_MATERIAL.roughness,
      meshKey: inst.partDefId,
      transform: inst.transform
        ? {
            translation: [
              inst.transform.translation.x,
              inst.transform.translation.y,
              inst.transform.translation.z,
            ],
            rotationQuat: eulerXyzDegToQuat(inst.transform.rotation),
            scale: [
              inst.transform.scale.x,
              inst.transform.scale.y,
              inst.transform.scale.z,
            ],
          }
        : undefined,
    }));

    return uint8ArrayToBase64(buildGlb(meshes, "preview"));
  } catch {
    // Evaluation failures fall back to the parts path (or its own
    // empty-geometry signal) rather than erroring the viewer poll.
    return null;
  }
}

/**
 * Push a board's layered preview meshes onto `meshes`, all carrying the
 * board's part-identity `name` so a click anywhere on the board resolves to
 * the PCB part.
 */
function pushPcbPreview(
  meshes: GlbMesh[],
  name: string,
  preview: Awaited<ReturnType<typeof pcbPreviewMeshes>>,
): void {
  for (const pm of preview) {
    const emissive = pm.emissive ?? [0, 0, 0];
    const glows = emissive[0] > 0 || emissive[1] > 0 || emissive[2] > 0;
    meshes.push({
      name,
      positions: pm.positions,
      indices: pm.indices,
      normals: pm.normals,
      color: pm.color,
      metallic: pm.metalness,
      roughness: pm.roughness,
      emissive,
      // Push LED lenses past white so they read as "on" under the viewer's
      // bright studio IBL.
      emissiveStrength: glows ? 3.0 : 1,
      clearcoat: pm.clearcoat ?? 0,
      clearcoatRoughness: pm.clearcoat_roughness ?? 0,
      alpha: pm.alpha ?? 1,
    });
  }
}

/** Convert Uint8Array to base64 string. */
function uint8ArrayToBase64(bytes: Uint8Array): string {
  // Use Buffer in Node.js for efficiency
  if (typeof Buffer !== "undefined") {
    return Buffer.from(bytes).toString("base64");
  }
  // Fallback for non-Node environments
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

/** Schema for the app-only `get_preview_glb` tool. */
export const getPreviewGlbSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id of the document to preview.",
    },
    instances: {
      type: "boolean" as const,
      description:
        "Return one named node per assembly instance (part-local geometry + " +
        "node transforms) so the replay viewer can bind FK targets. Falls " +
        "back to the merged parts preview when the document has no instances.",
    },
  },
  required: ["document_id"],
};

/**
 * Tool handler for `get_preview_glb`: returns the base64 GLB of a session
 * document wrapped in a `_vcad_glb` JSON envelope the viewer detects.
 *
 * This tool exists for the MCP Apps viewer (`visibility: ["app"]`) — it
 * keeps multi-hundred-KB geometry payloads out of model-visible tool
 * results. Agents wanting geometry should use `export_cad` instead.
 *
 * With `instances: true` the GLB carries one node per assembly instance
 * (see {@link generateInstancesGlbPreview}) and the envelope adds
 * `mode: "instances"`; documents without instances fall back to the
 * normal parts preview, so the flag is safe on any doc.
 */
export async function getPreviewGlb(
  doc: Document,
  engine: Engine,
  instances = false,
): Promise<{ content: Array<{ type: "text"; text: string }> }> {
  const preview = await previewGlbFor(doc, engine, instances);
  if (preview?.oversize) {
    // Explicit, named failure instead of a payload the transport will drop
    // and a viewer status that says only "preview unavailable".
    const mb = (preview.glb.length / 1_000_000).toFixed(1);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            _vcad_glb: null,
            version: preview.version,
            error: "preview_too_large",
            detail:
              `Model too large for inline preview (${mb} MB of GLB, ` +
              `budget ${(PREVIEW_MAX_BASE64 / 1_000_000).toFixed(1)} MB) — ` +
              "open it in vcad.io or use export_cad for the full geometry.",
          }),
        },
      ],
    };
  }
  if (preview) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            _vcad_glb: preview.glb,
            version: preview.version,
            ...(preview.mode ? { mode: preview.mode } : {}),
            ...(preview.degraded ? { degraded: true } : {}),
          }),
        },
      ],
    };
  }
  // No previewable geometry yet (e.g. a freshly opened empty document the
  // agent is about to build into). Return a soft signal, not an error — the
  // viewer shows "no geometry" and the self-refresh poll just waits for the
  // next change, and routine empty previews don't inflate the tool error rate.
  return {
    content: [{ type: "text", text: JSON.stringify({ _vcad_glb: null }) }],
  };
}

/**
 * A cheap, geometry-free change token for a document. FNV-1a over the IR
 * JSON — no kernel evaluation, no tessellation — so the inline viewer can
 * poll it on a timer to learn "did the document change?" without paying for
 * a full GLB rebuild every tick. Stable across server instances because it
 * hashes the (hydrated) IR, not in-process state.
 */
export function previewVersion(doc: Document): string {
  const s = JSON.stringify(doc);
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

/** Schema for the app-only `get_preview_version` tool. */
export const getPreviewVersionSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id of the document to version-check.",
    },
  },
  required: ["document_id"],
};

/**
 * Tool handler for `get_preview_version`: returns a cheap `{document_id,
 * version}` change token (no geometry eval). The inline viewer polls this
 * to self-refresh — it only re-fetches the heavy GLB when `version` changes.
 *
 * Like `get_preview_glb`, this is internal to the viewer (`visibility:
 * ["app"]`) and excluded from usage telemetry so polling can't flood it.
 */
export function getPreviewVersion(
  doc: Document,
  docId: string,
): { content: Array<{ type: "text"; text: string }> } {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ document_id: docId, version: previewVersion(doc) }),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "get_preview_glb",
    pack: null,
    description:
      "Return a base64 GLB preview of an open session document. Internal to the inline 3D viewer — agents should use `export_cad` for geometry exports.",
    inputSchema: getPreviewGlbSchema,
    handler: async (a, c) =>
      getPreviewGlb(
        getSession(String(a.document_id ?? "")),
        c.engine,
        a.instances === true,
      ),
    behavior: behavior({ appOnly: true }),
  },
  {
    name: "get_preview_version",
    pack: null,
    description:
      "Return a cheap {document_id, version} change token for an open session document (no geometry eval). Internal to the inline 3D viewer's self-refresh poll — agents should ignore it.",
    inputSchema: getPreviewVersionSchema,
    handler: (a) =>
      getPreviewVersion(
        getSession(String(a.document_id ?? "")),
        String(a.document_id ?? ""),
      ),
    behavior: behavior({ appOnly: true }),
  },
];
