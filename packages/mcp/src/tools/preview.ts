/**
 * GLB preview generation for MCP Apps viewer.
 *
 * Evaluates an IR document to produce a base64-encoded GLB that can
 * be rendered inline by the MCP Apps viewer iframe.
 */

import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import { toGlbBytes } from "../export/glb.js";

/**
 * Generate a base64-encoded GLB preview from an IR document.
 * Returns null if the document cannot be evaluated or has no geometry.
 */
export function generateGlbPreview(
  doc: Document,
  engine: Engine,
): string | null {
  try {
    const scene = engine.evaluate(doc);
    if (!scene || scene.parts.length === 0) return null;

    const glbBytes = toGlbBytes(scene, "preview");
    return uint8ArrayToBase64(glbBytes);
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
export function getPreviewGlb(
  doc: Document,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const glbBase64 = generateGlbPreview(doc, engine);
  if (!glbBase64) {
    throw new Error("document produced no previewable geometry");
  }
  return {
    content: [{ type: "text", text: JSON.stringify({ _vcad_glb: glbBase64 }) }],
  };
}
