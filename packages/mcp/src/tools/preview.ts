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
  DEFAULT_MATERIAL,
  type GlbMesh,
} from "../export/glb.js";

/**
 * Generate a base64-encoded GLB preview from an IR document.
 * Returns null if the document cannot be evaluated or has no geometry.
 */
export async function generateGlbPreview(
  doc: Document,
  engine: Engine,
): Promise<string | null> {
  try {
    const scene = engine.evaluate(doc);
    if (!scene || scene.parts.length === 0) return null;

    // Part-identity node names let the viewer map a click back to a
    // part_id for selection context and "ask about this part".
    const labels = buildPartLabels(doc);
    // `scene.parts` is index-aligned with the visible roots.
    const visibleRoots = doc.roots.filter((r) => r.visible !== false);

    const meshes: GlbMesh[] = [];
    for (let i = 0; i < scene.parts.length; i++) {
      const part = scene.parts[i];
      const name = labels[i] ?? `part_${i}`;
      const rootId = visibleRoots[i]?.root;
      const node = rootId !== undefined ? doc.nodes[String(rootId)] : undefined;

      // A board root: replace the merged gray slab with colored layers.
      if (node?.op?.type === "PcbBoard" && rootId !== undefined) {
        const pcb = getNodePcb(doc, rootId);
        const preview = pcb ? await pcbPreviewMeshes(pcb) : [];
        if (preview.length > 0) {
          for (const pm of preview) {
            meshes.push({
              // Keep the board's part identity on every sub-mesh so a click
              // anywhere on the board still resolves to the PCB part.
              name,
              positions: pm.positions,
              indices: pm.indices,
              normals: pm.normals,
              color: pm.color,
              metallic: pm.metalness,
              roughness: pm.roughness,
            });
          }
          continue;
        }
        // Preview meshes unavailable (e.g. older kernel WASM) — fall through
        // to the neutral merged board rather than dropping it.
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

    return uint8ArrayToBase64(buildGlb(meshes, "preview"));
  } catch {
    // Evaluation failures should not break tool responses
    return null;
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
 */
export async function getPreviewGlb(
  doc: Document,
  engine: Engine,
): Promise<{ content: Array<{ type: "text"; text: string }> }> {
  const glbBase64 = await generateGlbPreview(doc, engine);
  if (!glbBase64) {
    throw new Error("document produced no previewable geometry");
  }
  return {
    content: [{ type: "text", text: JSON.stringify({ _vcad_glb: glbBase64 }) }],
  };
}
