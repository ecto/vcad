/**
 * import_step tool — import geometry from STEP files.
 */

import type { Document, Node, NodeId, ImportedMeshOp, Vec3 } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { basename } from "node:path";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineArtifactBytes } from "./remote.js";
import { storeArtifact } from "./artifact-store.js";
import { registerSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

// Cap STEP imports at 100 MB to prevent a remote caller from pinning memory.
const MAX_STEP_BYTES = 100 * 1024 * 1024;

interface ImportStepInput {
  filename?: string;
  content_base64?: string;
  name?: string;
  material?: string;
}

export const importStepSchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description:
        "Path to the STEP file (.step or .stp) on the server filesystem, relative to the " +
        "server working directory (or VCAD_MCP_EXPORT_DIR if set). On hosted servers pass " +
        "content_base64 instead.",
    },
    content_base64: {
      type: "string" as const,
      description:
        "Base64-encoded STEP file contents. Use instead of `filename` when the " +
        "server has no access to your filesystem (hosted/remote deployments).",
    },
    name: {
      type: "string" as const,
      description: "Part name (default: filename without extension)",
    },
    material: {
      type: "string" as const,
      description: "Material key (default: 'steel')",
    },
  },
};

export function importStep(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const { filename, content_base64, name, material } = input as ImportStepInput;

  let fileBuffer: Buffer;
  if (content_base64) {
    fileBuffer = Buffer.from(content_base64, "base64");
    if (fileBuffer.length === 0) {
      throw new Error("content_base64 decoded to zero bytes");
    }
    if (fileBuffer.length > MAX_STEP_BYTES) {
      throw new Error(`STEP content exceeds ${MAX_STEP_BYTES} byte limit`);
    }
  } else if (filename) {
    if (isRemoteDeployment()) {
      throw new Error(
        "This hosted server has no access to your filesystem — pass the STEP " +
          "file contents as `content_base64` instead of `filename`.",
      );
    }
    // Resolve against the export dir (VCAD_MCP_EXPORT_DIR or cwd) and reject
    // any path that escapes it.
    const filepath = resolveWithinRoot(
      filename,
      process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd(),
    );

    if (!existsSync(filepath)) {
      throw new Error("STEP file not found");
    }

    const stat = statSync(filepath);
    if (!stat.isFile()) {
      throw new Error("STEP path is not a regular file");
    }
    if (stat.size > MAX_STEP_BYTES) {
      throw new Error(`STEP file exceeds ${MAX_STEP_BYTES} byte limit`);
    }

    fileBuffer = readFileSync(filepath);
  } else {
    throw new Error("Provide either `filename` or `content_base64`");
  }

  // Copy into a plain ArrayBuffer (Buffer.buffer may be a pooled
  // SharedArrayBuffer slice; the engine API takes ArrayBuffer).
  const arrayBuffer = new ArrayBuffer(fileBuffer.byteLength);
  new Uint8Array(arrayBuffer).set(fileBuffer);

  // Import using the engine
  const meshes = engine.importStep(arrayBuffer);

  if (meshes.length === 0) {
    throw new Error("No geometry found in STEP file");
  }

  // Create a document with ImportedMesh nodes
  const doc = createDocument();
  const sourceLabel = filename ?? "step-import";
  const partName =
    name ??
    basename(
      sourceLabel,
      /\.(step|stp)$/i.test(sourceLabel) ? sourceLabel.slice(-5) : "",
    );
  const partMaterial = material ?? "steel";

  let nextId = 1;

  for (let i = 0; i < meshes.length; i++) {
    const mesh = meshes[i];
    const nodeName = meshes.length === 1 ? partName : `${partName}_${i + 1}`;

    const op: ImportedMeshOp = {
      type: "ImportedMesh",
      positions: Array.from(mesh.positions),
      indices: Array.from(mesh.indices),
      normals: mesh.normals ? Array.from(mesh.normals) : undefined,
      source: sourceLabel,
    };

    const nodeId = nextId++;
    doc.nodes[String(nodeId)] = {
      id: nodeId,
      name: nodeName,
      op,
    };

    doc.roots.push({
      root: nodeId,
      material: partMaterial,
    });

    doc.part_materials[nodeName] = partMaterial;
  }

  // Add default materials
  doc.materials = {
    steel: {
      name: "Steel",
      color: [0.6, 0.6, 0.65],
      metallic: 0.9,
      roughness: 0.3,
      density: 7850,
    },
    aluminum: {
      name: "Aluminum",
      color: [0.8, 0.8, 0.85],
      metallic: 0.9,
      roughness: 0.2,
      density: 2700,
    },
    default: {
      name: "Default",
      color: [0.8, 0.8, 0.8],
      metallic: 0,
      roughness: 0.5,
    },
  };

  const summary = {
    bodies: meshes.length,
    total_triangles: meshes.reduce((sum, m) => sum + m.indices.length / 3, 0),
    total_vertices: meshes.reduce((sum, m) => sum + m.positions.length / 3, 0),
  };

  // A real STEP import is megabytes of mesh JSON — far past the tool-output
  // token budget if echoed inline. Over the cap, keep the IR out of context:
  // register it as a session (the agent continues by document_id, which every
  // CAD tool accepts) and offload the full IR to the artifact store for
  // download. Small imports keep the inline `document` for backward compat.
  const inline = JSON.stringify({ document: doc, summary }, null, 2);
  const cap = maxInlineArtifactBytes();
  if (Buffer.byteLength(inline, "utf8") > cap) {
    const documentId = registerSession(doc);
    const handle = storeArtifact([
      { name: `${partName}.vcad`, content: JSON.stringify(doc) },
    ]);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            {
              document_id: documentId,
              summary,
              artifact_id: handle.artifact_id,
              artifact_url: handle.artifact_url,
              manifest: handle.manifest,
              expires_at: handle.expires_at,
              note:
                "Imported geometry is large; the full IR was kept out of context. " +
                "Continue editing via document_id (every CAD tool accepts it), or " +
                "download the document at artifact_url. The inline `document` was " +
                "omitted to stay under the tool-output limit.",
            },
            null,
            2,
          ),
        },
      ],
    };
  }

  return {
    content: [
      {
        type: "text",
        text: inline,
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "import_step",
    pack: null,
    description:
      "Import geometry from a STEP file (.step or .stp). Returns an IR document with ImportedMesh nodes. " +
      "Supports AP203/AP214 STEP files commonly exported from Fusion 360, SolidWorks, Onshape, etc.",
    inputSchema: importStepSchema,
    handler: (a, c) => importStep(a, c.engine),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
