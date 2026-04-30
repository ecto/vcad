/**
 * import (v2) — STEP/STL → doc handle.
 *
 * Accepts an embedded resource (base64 in `source.data_base64`) or a
 * local file path (`source.path`, requires MCP_LOCAL=1). Auto-detects
 * format from extension or magic bytes when `kind` is omitted. Caps
 * STEP imports at 100 MB.
 */

import {
  createDocument,
  type ImportedMeshOp,
} from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { basename } from "node:path";
import { resolveWithinRoot } from "./safe-path.js";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { commitDoc } from "../handles.js";
import type { ResourceRef } from "../types.js";

const MAX_BYTES = 100 * 1024 * 1024;

export const importV2Schema = {
  type: "object" as const,
  properties: {
    source: {
      description:
        'Either { kind: "embedded", mime, data_base64 } or { path: "local-file.step" } (local mode only).',
    },
    kind: {
      type: "string" as const,
      enum: ["step", "stl", "auto"],
      description: "Format hint. Default `auto` detects from path or mime.",
    },
    name: { type: "string" as const, description: "Part name override." },
    material: { type: "string" as const, description: "Material key (default 'steel')." },
  },
  required: ["source"],
};

interface ImportV2Input {
  source: ResourceRef | { path: string };
  kind?: "step" | "stl" | "auto";
  name?: string;
  material?: string;
}

export function importV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as ImportV2Input;
  if (!args.source) return fail("invalid_input", "Missing `source`.");

  // Read bytes.
  let bytes: Uint8Array;
  let originalName = args.name ?? "imported";
  if ("kind" in args.source && args.source.kind === "embedded") {
    bytes = Buffer.from(args.source.data_base64, "base64");
  } else if ("kind" in args.source && args.source.kind === "url") {
    return fail("unsupported", "URL sources not yet supported — embed the bytes directly.");
  } else if ("path" in args.source) {
    if (process.env.MCP_LOCAL !== "1") {
      return fail(
        "local_disabled",
        "Filesystem import requires MCP_LOCAL=1; pass an embedded resource for hosted use.",
      );
    }
    const filepath = resolveWithinRoot(args.source.path);
    if (!existsSync(filepath)) return fail("not_found", "Source file not found.");
    const stat = statSync(filepath);
    if (!stat.isFile()) return fail("invalid_source", "Path is not a regular file.");
    if (stat.size > MAX_BYTES) return fail("too_large", `File exceeds ${MAX_BYTES} byte cap.`);
    bytes = readFileSync(filepath);
    originalName = args.name ?? basename(filepath).replace(/\.[^.]+$/, "");
  } else {
    return fail("invalid_source", "Unrecognised `source` shape.");
  }

  if (bytes.length > MAX_BYTES) return fail("too_large", `Input exceeds ${MAX_BYTES} byte cap.`);

  // Format detect (Phase 1 covers STEP only — STL goes through the kernel
  // separately; we surface a clear "not yet" for it).
  const kind = args.kind ?? autoDetect(bytes, args.source);
  if (kind === "stl") {
    return fail("unsupported", "STL import not wired into the v2 surface yet — Phase 4 follow-up.");
  }
  if (kind !== "step") return fail("unsupported", `Unknown import kind: ${kind}`);

  // Run STEP import via the engine.
  const ab = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const meshes = engine.importStep(ab);
  if (meshes.length === 0) {
    return fail("empty_import", "No geometry found in input.");
  }

  const doc = createDocument();
  const partName = originalName;
  const partMaterial = args.material ?? "steel";
  let nextId = 1;
  for (let i = 0; i < meshes.length; i++) {
    const mesh = meshes[i];
    const nodeName = meshes.length === 1 ? partName : `${partName}_${i + 1}`;
    const op: ImportedMeshOp = {
      type: "ImportedMesh",
      positions: Array.from(mesh.positions),
      indices: Array.from(mesh.indices),
      normals: mesh.normals ? Array.from(mesh.normals) : undefined,
      source: originalName,
    };
    const nodeId = nextId++;
    doc.nodes[String(nodeId)] = { id: nodeId, name: nodeName, op };
    doc.roots.push({ root: nodeId, material: partMaterial });
    doc.part_materials[String(nodeId)] = partMaterial;
  }

  doc.materials = {
    steel: { name: "Steel", color: [0.6, 0.6, 0.65], metallic: 0.9, roughness: 0.3, density: 7850 },
    aluminum: { name: "Aluminum", color: [0.8, 0.8, 0.85], metallic: 0.9, roughness: 0.2, density: 2700 },
    default: { name: "Default", color: [0.8, 0.8, 0.8], metallic: 0, roughness: 0.5 },
  };

  const handle = commitDoc(doc);
  return ok({
    result: {
      added_nodes: doc.roots.map((r) => r.root),
      bodies: meshes.length,
      total_triangles: meshes.reduce((s, m) => s + m.indices.length / 3, 0),
    },
    handle,
    doc,
    engine,
    startedAt,
  });
}

function autoDetect(bytes: Uint8Array, source: ResourceRef | { path: string }): "step" | "stl" | "unknown" {
  // STEP files start with `ISO-10303-` after some whitespace.
  const head = Buffer.from(bytes.slice(0, 64)).toString("utf-8").trim();
  if (head.startsWith("ISO-10303-")) return "step";
  if (head.startsWith("solid ")) return "stl";
  if ("path" in source) {
    const ext = source.path.toLowerCase().split(".").pop();
    if (ext === "step" || ext === "stp") return "step";
    if (ext === "stl") return "stl";
  }
  if ("mime" in source) {
    if (source.mime.includes("step")) return "step";
    if (source.mime.includes("stl")) return "stl";
  }
  return "unknown";
}
