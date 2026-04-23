import { serializeDocument, getDocumentForDisplay, type VcadFileFormat } from "@vcad/core";
import type { Document } from "@vcad/ir";
import { toVCode } from "@vcad/ir";
import { downloadBlob } from "./download";

/**
 * Download the current document as a `.vcad` file.
 *
 * Always writes the canonical format:
 *  - `.vcad` = CRDT bytes (JSON) when the engine is present
 *  - `.vcad` = loon text when the document is loon-authored
 *
 * Legacy VCode/v1-JSON are read-only formats — see `exportVCode` below for
 * the separate export path.
 */
export function saveDocument(state: {
  crdtBytes?: Uint8Array | null;
  loonSource?: string | null;
}) {
  const text = serializeDocument(state);
  const blob = new Blob([text], { type: "application/json" });
  downloadBlob(blob, "model.vcad");
}

/**
 * Export the current document as VCode text — a read-only, human/LLM-friendly
 * view. VCode is NOT used for persistence; round-tripping through it is what
 * caused the old silent-data-loss bugs (see migrate_v1 bypass).
 */
export function exportVCode(doc: Document, filename = "model.vcode.txt") {
  const text = toVCode(doc);
  const blob = new Blob([text], { type: "text/plain" });
  downloadBlob(blob, filename);
}

/**
 * Copy the current document as VCode text to the clipboard — handy for
 * pasting into an LLM prompt.
 */
export async function copyAsVCode(file: VcadFileFormat): Promise<boolean> {
  const doc = getDocumentForDisplay(file);
  if (!doc) return false;
  try {
    await navigator.clipboard.writeText(toVCode(doc));
    return true;
  } catch (e) {
    console.warn("Failed to copy VCode to clipboard:", e);
    return false;
  }
}

export function downloadDxf(data: Uint8Array, filename: string) {
  const blob = new Blob([new Uint8Array(data)], { type: "application/dxf" });
  const name = filename.endsWith(".dxf") ? filename : `${filename}.dxf`;
  downloadBlob(blob, name);
}
