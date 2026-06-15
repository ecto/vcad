/**
 * import_step tool — import geometry from STEP files.
 */

import type { Document, Node, NodeId, ImportedMeshOp, Vec3 } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { basename } from "node:path";
import { resolveWithinRoot } from "./safe-path.js";

// Cap STEP imports at 100 MB to prevent a remote caller from pinning memory.
const MAX_STEP_BYTES = 100 * 1024 * 1024;

interface ImportStepInput {
  filename: string;
  name?: string;
  material?: string;
}

export const importStepSchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description:
        "Path to the STEP file (.step or .stp), relative to the server working directory " +
        "(or VCAD_MCP_EXPORT_DIR if set).",
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
  required: ["filename"],
};

export function importStep(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const { filename, name, material } = input as ImportStepInput;

  // Resolve against the export dir (VCAD_MCP_EXPORT_DIR or cwd) and reject any
  // path that escapes it.
  const filepath = resolveWithinRoot(filename, process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd());

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

  // Read the file
  const fileBuffer = readFileSync(filepath);
  const arrayBuffer = fileBuffer.buffer.slice(
    fileBuffer.byteOffset,
    fileBuffer.byteOffset + fileBuffer.byteLength,
  );

  // Import using the engine
  const meshes = engine.importStep(arrayBuffer);

  if (meshes.length === 0) {
    throw new Error("No geometry found in STEP file");
  }

  // Create a document with ImportedMesh nodes
  const doc = createDocument();
  const partName = name ?? basename(filename, /\.(step|stp)$/i.test(filename) ? filename.slice(-5) : "");
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
      source: filename,
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

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document: doc,
          summary: {
            bodies: meshes.length,
            total_triangles: meshes.reduce((sum, m) => sum + m.indices.length / 3, 0),
            total_vertices: meshes.reduce((sum, m) => sum + m.positions.length / 3, 0),
          },
        }, null, 2),
      },
    ],
  };
}
