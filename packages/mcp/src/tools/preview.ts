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

/**
 * Append a GLB preview content block to a tool result.
 *
 * The preview is added as a text content block with a JSON wrapper
 * containing `_vcad_glb` that the viewer iframe can detect.
 * The block has `audience: ["user"]` so it is not sent to the model,
 * reducing token usage.
 */
export function appendGlbPreview(
  result: { content: Array<{ type: string; text: string; annotations?: unknown }> },
  doc: Document,
  engine: Engine,
): void {
  const glbBase64 = generateGlbPreview(doc, engine);
  if (!glbBase64) return;

  result.content.push({
    type: "text",
    text: JSON.stringify({ _vcad_glb: glbBase64 }),
    annotations: { audience: ["user"] },
  });
}
