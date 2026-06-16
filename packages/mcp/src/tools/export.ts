/**
 * export_cad tool — export IR document to file.
 */

import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import { writeFileSync } from "node:fs";
import { toStlBytes } from "../export/stl.js";
import { toGlbBytes } from "../export/glb.js";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineExportBytes } from "./remote.js";

interface ExportInput {
  ir: Document;
  filename: string;
}

/**
 * Deliver export bytes: write to disk on local servers, return base64
 * inline on remote deployments. On serverless hosts (Vercel) the
 * function filesystem is read-only — a writeFileSync there throws EROFS
 * and the bytes would be invisible to the caller anyway — so the bytes
 * ride back in the tool result instead.
 */
function deliver(
  filename: string,
  bytes: Uint8Array,
  extra: Record<string, unknown>,
): { content: Array<{ type: "text"; text: string }> } {
  let payload: Record<string, unknown>;
  if (isRemoteDeployment()) {
    const cap = maxInlineExportBytes();
    if (bytes.length > cap) {
      throw new Error(
        `Export is ${bytes.length} bytes — over the ${cap} byte inline limit for this hosted server. ` +
          "Use open_in_browser to hand the document to vcad.io and export from there.",
      );
    }
    payload = {
      filename,
      bytes: bytes.length,
      data_base64: Buffer.from(bytes).toString("base64"),
      note_delivery:
        "Hosted server: file contents returned inline as base64 (this server has no access to your disk).",
      ...extra,
    };
  } else {
    // Resolve against the export dir (VCAD_MCP_EXPORT_DIR or cwd) and
    // reject any path that escapes it.
    const path = resolveWithinRoot(
      filename,
      process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd(),
    );
    writeFileSync(path, bytes);
    payload = { path, bytes: bytes.length, ...extra };
  }
  return {
    content: [{ type: "text", text: JSON.stringify(payload) }],
  };
}

export const exportCadSchema = {
  type: "object" as const,
  properties: {
    ir: {
      type: "object" as const,
      description: "IR document from create_cad_document",
    },
    filename: {
      type: "string" as const,
      description:
        "Output filename with extension (.stl, .glb, or — for sheet-metal documents — .step/.stp), " +
        "relative to the server working directory (or VCAD_MCP_EXPORT_DIR if set). " +
        "STEP exports the FOLDED sheet-metal body (AP214) with true cylindrical bend faces sized by " +
        "the document's shop profile, so fab services with a 3D pipeline (e.g. SendCutSend) " +
        "auto-detect bends, angles, and directions with zero data entry.",
    },
  },
  required: ["ir", "filename"],
};

export function exportCad(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const { ir, filename } = input as ExportInput;

  // STEP: only the folded sheet-metal body is exportable (mesh-evaluated
  // documents have no B-rep to write). The folded solid carries real
  // cylindrical bend faces, so a 3D-pipeline fab service detects bends,
  // angles, and directions from the file itself — the zero-data-entry
  // alternative to the DXF path (where bend angles are entered in the
  // service's UI).
  const stepExt = filename.toLowerCase().split(".").pop();
  if (stepExt === "step" || stepExt === "stp") {
    const step = engine.foldedSheetMetalStep(ir);
    if (step === null) {
      throw new Error(
        "STEP export is only available for sheet-metal documents (the folded " +
          "body needs B-rep bend geometry). Use .stl or .glb for mesh exports.",
      );
    }
    const bytes = new TextEncoder().encode(step);
    return deliver(filename, bytes, {
      format: stepExt,
      parts: 1,
      note: "Folded sheet-metal body (AP214) with cylindrical bend faces — bends/angles/directions auto-detect in 3D fab pipelines.",
    });
  }

  // Evaluate the document to get meshes
  const scene = engine.evaluate(ir);

  if (scene.parts.length === 0) {
    throw new Error("Document has no parts to export");
  }

  // Determine format from extension
  const ext = filename.toLowerCase().split(".").pop();
  let bytes: Uint8Array;

  switch (ext) {
    case "stl":
      bytes = toStlBytes(scene, filename);
      break;
    case "glb":
      bytes = toGlbBytes(scene, filename);
      break;
    default:
      throw new Error(`Unsupported format: .${ext}. Use .stl, .glb, or .step (sheet-metal only)`);
  }

  return deliver(filename, bytes, {
    format: ext,
    parts: scene.parts.length,
  });
}
