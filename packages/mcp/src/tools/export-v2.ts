/**
 * export (v2) — handle → embedded resource (default) or local file.
 *
 * Hosted-mode default: returns an embedded base64 resource so any client
 * can consume the bytes without filesystem access. Local-mode (when
 * `target.path` is supplied and `MCP_LOCAL=1`) writes to disk through
 * the existing safe-path gate.
 */

import { writeFileSync } from "node:fs";
import type { Engine } from "@vcad/engine";
import { toStlBytes } from "../export/stl.js";
import { toGlbBytes } from "../export/glb.js";
import { resolveWithinRoot } from "./safe-path.js";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef, ResourceRef } from "../types.js";

export const exportV2Schema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle or inline IR." },
    format: {
      type: "string" as const,
      enum: ["stl", "glb"],
      description: "Export format. Phase-1 covers stl + glb; step/dxf/pdf land later.",
    },
    target: {
      description:
        '"embed" (default) returns an embedded base64 resource; { path: "..." } writes to disk in local mode.',
    },
  },
  required: ["doc", "format"],
};

interface ExportV2Input {
  doc: DocRef;
  format: "stl" | "glb";
  target?: "embed" | { path: string };
}

const MIME = {
  stl: "model/stl",
  glb: "model/gltf-binary",
} as const;

export function exportV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as ExportV2Input;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  if (!args.format) return fail("invalid_input", "Missing `format`.");

  const { doc, handle } = resolveRef(args.doc);
  const scene = engine.evaluate(doc);
  if (scene.parts.length === 0) {
    return fail("empty_document", "Document has no parts to export.");
  }

  const filename = `export.${args.format}`;
  let bytes: Uint8Array;
  switch (args.format) {
    case "stl":
      bytes = toStlBytes(scene, filename);
      break;
    case "glb":
      bytes = toGlbBytes(scene, filename);
      break;
    default:
      return fail("unsupported_format", `Unknown format: ${args.format}`);
  }

  const target = args.target ?? "embed";
  if (typeof target === "object" && target.path) {
    if (process.env.MCP_LOCAL !== "1") {
      return fail(
        "local_disabled",
        "Filesystem export requires MCP_LOCAL=1; pass `target: 'embed'` for hosted use.",
      );
    }
    const path = resolveWithinRoot(target.path);
    writeFileSync(path, bytes);
    return ok({
      result: { path, bytes: bytes.length, format: args.format },
      handle,
      doc,
      engine,
      startedAt,
      skipPreview: true,
    });
  }

  const resource: ResourceRef = {
    kind: "embedded",
    mime: MIME[args.format],
    data_base64: Buffer.from(bytes).toString("base64"),
  };
  return ok({
    result: { resource, bytes: bytes.length, format: args.format },
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}
