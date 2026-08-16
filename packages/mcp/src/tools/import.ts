/**
 * import_step tool — import geometry from STEP files.
 *
 * Imports are B-rep by default. The document gets one lazy
 * `step_import` node per body — the same node the CLI writes — so analytic
 * faces (cylinder axes and radii, planes, cones) survive into booleans,
 * fillets, measurement, and STEP export. `as_mesh: true` opts back into the
 * old tessellated `ImportedMesh` form, which is still what you want for
 * rendering-only or physics-collider use.
 *
 * The WASM kernel has no filesystem, so the bytes behind each node are
 * registered with the kernel (see `Engine.registerStepSource`). That registry
 * is per-process: a document reopened in a later server run re-registers from
 * the stored path, and a document whose STEP arrived as base64 with no
 * writable disk is session-bound — both are reported rather than silently
 * evaluating to nothing.
 */

import type { Document, ImportedMeshOp, StepImportOp } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import type { Engine, RegisterStepSourceResult } from "@vcad/engine";
import { createHash } from "node:crypto";
import { readFileSync, existsSync, statSync, mkdirSync, writeFileSync } from "node:fs";
import { basename, join, resolve, sep } from "node:path";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineArtifactBytes } from "./remote.js";
import { storeArtifact } from "./artifact-store.js";
import { registerSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

// Cap STEP imports at 100 MB to prevent a remote caller from pinning memory.
const MAX_STEP_BYTES = 100 * 1024 * 1024;

/** Directory sidecar STEP files are written to, relative to the export dir. */
const STEP_CACHE_DIR = ".vcad-step-cache";

interface ImportStepInput {
  filename?: string;
  content_base64?: string;
  name?: string;
  material?: string;
  as_mesh?: boolean;
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
    as_mesh: {
      type: "boolean" as const,
      description:
        "Import as a baked triangle mesh instead of B-rep (default false). " +
        "Mesh-only parts cannot be exported to STEP and have no analytic faces; " +
        "use this only when a tessellation is what you actually want (rendering, " +
        "physics colliders).",
    },
  },
};

/** Where sidecar STEP files are written so a document stays portable. */
function exportRoot(): string {
  return process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd();
}

/** Whether an absolute path lies inside the directory imports are confined to.
 *  `resolveWithinRoot` can't be used here — it rejects absolute paths, and a
 *  stored node path is absolute by construction. */
function isWithinExportRoot(path: string): boolean {
  if (path.includes("\0")) return false;
  const root = resolve(exportRoot());
  const abs = resolve(path);
  return abs === root || abs.startsWith(root + sep);
}

/**
 * Choose the path a `step_import` node will store, and make the bytes
 * resolvable from it later.
 *
 * A real file on disk is used as-is (absolute, so it survives a change of
 * working directory — the same rule `resolve_mesh_paths` applies to mesh
 * references). Inline content is written to a sidecar file when the server has
 * somewhere to write, which keeps the document re-openable; when it doesn't,
 * the node gets a `step:` key that only resolves inside this process, and the
 * caller is told so.
 */
function stepNodePath(
  bytes: Buffer,
  diskPath: string | null,
  label: string,
): { path: string; portable: boolean } {
  if (diskPath) return { path: diskPath, portable: true };

  const digest = createHash("sha256").update(bytes).digest("hex").slice(0, 16);
  try {
    const dir = join(exportRoot(), STEP_CACHE_DIR);
    mkdirSync(dir, { recursive: true });
    const file = join(dir, `${digest}.step`);
    if (!existsSync(file)) writeFileSync(file, bytes);
    return { path: resolve(file), portable: true };
  } catch {
    // Read-only or ephemeral filesystem (hosted deploys): fall back to a
    // process-scoped key. Evaluation still works for this session because the
    // bytes are registered under exactly this string.
    return { path: `step:${digest}/${label}`, portable: false };
  }
}

/**
 * Re-register the STEP contents behind every `step_import` node in `doc`.
 *
 * Call this whenever a document enters the process from outside (a `.vcad`
 * loaded from disk, a restored session): the kernel's registry lives in WASM
 * memory and does not survive a restart, so without this the nodes evaluate to
 * an error. Returns the paths that could not be restored — a caller should
 * surface them rather than let the parts quietly disappear.
 */
export function registerDocumentStepSources(
  doc: Document,
  engine: Engine,
): { registered: string[]; missing: Array<{ path: string; reason: string }> } {
  const registered: string[] = [];
  const missing: Array<{ path: string; reason: string }> = [];
  const seen = new Set<string>();

  for (const node of Object.values(doc.nodes ?? {})) {
    const op = node?.op as { type?: string; path?: string } | undefined;
    if (op?.type !== "step_import" || typeof op.path !== "string") continue;
    const path = op.path;
    if (seen.has(path)) continue;
    seen.add(path);

    if (engine.stepSourceRegistered(path)) {
      registered.push(path);
      continue;
    }
    if (path.startsWith("step:")) {
      missing.push({
        path,
        reason:
          "session-bound import (the STEP arrived inline and this server has no " +
          "writable filesystem) — re-run import_step with the file contents",
      });
      continue;
    }
    // A document is caller-supplied data, so its paths are untrusted: confine
    // the read to the same root `import_step` writes into. Every path this
    // tool mints lives there, and a document naming something else must not
    // turn a reopen into an arbitrary-file read.
    if (!isWithinExportRoot(path)) {
      missing.push({
        path,
        reason: `outside the server's STEP directory (${exportRoot()}) — re-import the file`,
      });
      continue;
    }
    try {
      const bytes = readFileSync(path);
      const buf = new ArrayBuffer(bytes.byteLength);
      new Uint8Array(buf).set(bytes);
      engine.registerStepSource(path, buf);
      registered.push(path);
    } catch (e) {
      missing.push({ path, reason: (e as Error).message });
    }
  }

  return { registered, missing };
}

function readStepInput(input: ImportStepInput): {
  bytes: Buffer;
  diskPath: string | null;
} {
  const { filename, content_base64 } = input;

  if (content_base64) {
    const bytes = Buffer.from(content_base64, "base64");
    if (bytes.length === 0) {
      throw new Error("content_base64 decoded to zero bytes");
    }
    if (bytes.length > MAX_STEP_BYTES) {
      throw new Error(`STEP content exceeds ${MAX_STEP_BYTES} byte limit`);
    }
    return { bytes, diskPath: null };
  }

  if (!filename) {
    throw new Error("Provide either `filename` or `content_base64`");
  }

  if (isRemoteDeployment()) {
    throw new Error(
      "This hosted server has no access to your filesystem — pass the STEP " +
        "file contents as `content_base64` instead of `filename`.",
    );
  }
  // Resolve against the export dir (VCAD_MCP_EXPORT_DIR or cwd) and reject
  // any path that escapes it.
  const filepath = resolveWithinRoot(filename, exportRoot());

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

  return { bytes: readFileSync(filepath), diskPath: resolve(filepath) };
}

/** Default document materials shared by both import routes. */
function defaultMaterials(): Document["materials"] {
  return {
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
}

/** Flatten a skipped-face report into the shape the tool result reports. */
function skippedFaceSummary(
  report: RegisterStepSourceResult["report"],
  warnings: string | null,
): Record<string, unknown> {
  const skipped = report.reduce((sum, s) => sum + s.skipped_faces.length, 0);
  if (skipped === 0) return {};
  return {
    warning:
      `${skipped} face(s) skipped (unsupported surface types) — the imported geometry ` +
      `has holes where they were.\n${warnings ?? ""}`,
    skipped_faces: report.flatMap((s) =>
      s.skipped_faces.map((f) => ({
        solid_id: s.solid_id,
        face_id: f.face_id,
        surface_id: f.surface_id,
        reason: f.reason,
      })),
    ),
  };
}

export function importStep(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const parsed = input as ImportStepInput;
  const { filename, name, material, as_mesh } = parsed;

  const { bytes: fileBuffer, diskPath } = readStepInput(parsed);

  // Copy into a plain ArrayBuffer (Buffer.buffer may be a pooled
  // SharedArrayBuffer slice; the engine API takes ArrayBuffer).
  const arrayBuffer = new ArrayBuffer(fileBuffer.byteLength);
  new Uint8Array(arrayBuffer).set(fileBuffer);

  const sourceLabel = filename ?? "step-import";
  const partName =
    name ??
    basename(
      sourceLabel,
      /\.(step|stp)$/i.test(sourceLabel) ? sourceLabel.slice(-5) : "",
    );
  const partMaterial = material ?? "steel";

  const doc = createDocument();
  doc.materials = defaultMaterials();

  const addRoot = (
    nodeId: number,
    nodeName: string,
    op: StepImportOp | ImportedMeshOp,
  ) => {
    doc.nodes[String(nodeId)] = { id: nodeId, name: nodeName, op };
    doc.roots.push({ root: nodeId, material: partMaterial });
    doc.part_materials[nodeName] = partMaterial;
  };

  let summary: Record<string, unknown>;

  if (as_mesh) {
    const { meshes, report, summary: importWarnings } =
      engine.importStepWithReport(arrayBuffer);
    if (meshes.length === 0) {
      throw new Error("No geometry found in STEP file");
    }

    meshes.forEach((mesh, i) => {
      const op: ImportedMeshOp = {
        type: "ImportedMesh",
        positions: Array.from(mesh.positions),
        indices: Array.from(mesh.indices),
        normals: mesh.normals ? Array.from(mesh.normals) : undefined,
        source: sourceLabel,
      };
      addRoot(i + 1, meshes.length === 1 ? partName : `${partName}_${i + 1}`, op);
    });

    summary = {
      representation: "mesh",
      bodies: meshes.length,
      total_triangles: meshes.reduce((sum, m) => sum + m.indices.length / 3, 0),
      total_vertices: meshes.reduce((sum, m) => sum + m.positions.length / 3, 0),
      note:
        "Imported as triangle meshes: no analytic faces, and these parts cannot be " +
        "exported to STEP. Drop `as_mesh` for a B-rep import.",
      ...skippedFaceSummary(report, importWarnings),
    };
  } else {
    const label = basename(sourceLabel).replace(/[^\w.-]/g, "_") || "import.step";
    const { path, portable } = stepNodePath(fileBuffer, diskPath, label);

    // Registering parses the file once: it hands the kernel the bytes (so the
    // lazy nodes resolve) and gives back the per-body B-rep stats and the
    // skipped-face report, which is otherwise silent.
    let registered: RegisterStepSourceResult;
    try {
      registered = engine.registerStepSource(path, arrayBuffer);
    } catch (e) {
      throw new Error(`STEP import failed: ${(e as Error).message}`);
    }
    if (registered.solids.length === 0) {
      throw new Error("No geometry found in STEP file");
    }

    registered.solids.forEach((solid, i) => {
      const op: StepImportOp = {
        type: "step_import",
        path,
        ...(solid.index === 0 ? {} : { solid_index: solid.index }),
      };
      addRoot(
        i + 1,
        registered.solids.length === 1 ? partName : `${partName}_${i + 1}`,
        op,
      );
    });

    const meshOnly = registered.solids.filter((s) => s.faces === 0);
    summary = {
      representation: "brep",
      bodies: registered.solids.length,
      source_path: path,
      total_faces: registered.solids.reduce((sum, s) => sum + s.faces, 0),
      total_volume_mm3: registered.solids.reduce((sum, s) => sum + s.volume, 0),
      bodies_detail: registered.solids.map((s, i) => ({
        node: i + 1,
        solid_index: s.index,
        faces: s.faces,
        volume_mm3: s.volume,
        bbox: s.bbox,
      })),
      step_exportable: meshOnly.length === 0,
      ...(meshOnly.length > 0
        ? {
            mesh_only_bodies: meshOnly.map((s) => s.index),
          }
        : {}),
      ...(portable
        ? {}
        : {
            session_bound:
              "This server has no writable filesystem, so the import is bound to " +
              "this process: the nodes reference `" +
              path +
              "`, which only resolves until the server restarts. Export the part " +
              "(export_cad) if you need it to outlive the session.",
          }),
      ...skippedFaceSummary(registered.report, registered.summary),
    };
  }

  // A mesh import is megabytes of JSON — far past the tool-output token budget
  // if echoed inline. Over the cap, keep the IR out of context: register it as
  // a session (the agent continues by document_id, which every CAD tool
  // accepts) and offload the full IR to the artifact store for download.
  // Small imports keep the inline `document` for backward compat — which a
  // B-rep import essentially always is, since it stores paths, not geometry.
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
      "Import geometry from a STEP file (.step or .stp), keeping B-rep: the document gets one " +
      "lazy `step_import` node per body, so analytic faces survive into booleans, fillets, and " +
      "STEP export. Pass `as_mesh: true` for the old tessellated import (rendering / colliders " +
      "only — mesh parts cannot be exported to STEP). Supports AP203/AP214 STEP files commonly " +
      "exported from Fusion 360, SolidWorks, Onshape, etc.",
    inputSchema: importStepSchema,
    handler: (a, c) => importStep(a, c.engine),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
