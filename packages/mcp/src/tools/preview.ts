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
import { pcbPreviewMeshes, type Engine } from "@vcad/engine";
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
const glbCache = new Map<string, string>();

function glbCacheGet(key: string): string | undefined {
  const hit = glbCache.get(key);
  if (hit !== undefined) {
    // LRU touch: re-insert so the hottest entries survive eviction.
    glbCache.delete(key);
    glbCache.set(key, hit);
  }
  return hit;
}

function glbCachePut(key: string, glb: string): void {
  glbCache.delete(key);
  glbCache.set(key, glb);
  while (glbCache.size > GLB_CACHE_MAX) {
    const oldest = glbCache.keys().next().value;
    if (oldest === undefined) break;
    glbCache.delete(oldest);
  }
}

/** A ready-to-render preview envelope: base64 GLB + change token (+ mode). */
export interface PreviewGlb {
  glb: string;
  version: string;
  mode?: "instances";
}

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
    if (cached !== undefined) return { glb: cached, version, mode: "instances" };
    const instGlb = generateInstancesGlbPreview(doc, engine);
    if (instGlb) {
      glbCachePut(key, instGlb);
      return { glb: instGlb, version, mode: "instances" };
    }
    // No instances — fall through to the parts preview (flag is safe on any doc).
  }
  const key = `${version}:parts`;
  const cached = glbCacheGet(key);
  if (cached !== undefined) return { glb: cached, version };
  const glb = await generateGlbPreview(doc, engine);
  if (!glb) return null;
  glbCachePut(key, glb);
  return { glb, version };
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
): Promise<string | null> {
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
  } catch {
    // Evaluation failures should not break the preview — PCB sessions still
    // render from the board path above; a non-PCB doc that can't evaluate
    // falls through to the empty-geometry signal below.
  }

  if (meshes.length === 0) return null;
  return uint8ArrayToBase64(buildGlb(meshes, "preview"));
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
  if (preview) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            _vcad_glb: preview.glb,
            version: preview.version,
            ...(preview.mode ? { mode: preview.mode } : {}),
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
