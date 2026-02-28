import type { PartInfo } from "@vcad/core";
import { serializeDocument } from "@vcad/core";
import type { Document } from "@vcad/ir";
import { downloadBlob } from "./download";

export function saveDocument(state: {
  document: Document;
  parts: PartInfo[];
  consumedParts: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
}) {
  const json = serializeDocument(state);
  const blob = new Blob([json], { type: "application/json" });
  downloadBlob(blob, "model.vcad");
}

export function downloadDxf(data: Uint8Array, filename: string) {
  const blob = new Blob([new Uint8Array(data)], { type: "application/dxf" });
  const name = filename.endsWith(".dxf") ? filename : `${filename}.dxf`;
  downloadBlob(blob, name);
}
