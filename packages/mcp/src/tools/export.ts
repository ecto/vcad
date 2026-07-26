/**
 * export_cad tool — export IR document to file.
 */

import type { Engine } from "@vcad/engine";
import { writeFileSync } from "node:fs";
import { toStlBytes } from "../export/stl.js";
import { toGlbBytes } from "../export/glb.js";
import { sceneExportUnits } from "../export/scene-units.js";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineExportBytes } from "./remote.js";
import { storeArtifact } from "./artifact-store.js";
import { applyJointState, jointStateSchemaProp } from "./pose.js";
import { resolveDocInput } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

/**
 * Deliver export bytes: write to disk on local servers, return base64
 * inline on remote deployments. On serverless hosts (Vercel) the
 * function filesystem is read-only — a writeFileSync there throws EROFS
 * and the bytes would be invisible to the caller anyway — so the bytes
 * ride back in the tool result instead. A binary over the inline cap is
 * offloaded to the artifact store and only a { artifact_url, manifest }
 * handle is returned, so it never overflows the model's context.
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
      const handle = storeArtifact([{ name: filename, content: bytes }]);
      payload = {
        filename,
        bytes: bytes.length,
        artifact_id: handle.artifact_id,
        artifact_url: handle.artifact_url,
        manifest: handle.manifest,
        expires_at: handle.expires_at,
        note_delivery:
          `Export is ${bytes.length} bytes — over the ${cap}-byte inline limit; ` +
          "written to the artifact store. Download it at artifact_url, or pass " +
          "artifact_id to quote_manufacturing / place_order so the bytes never " +
          "transit model context.",
        ...extra,
      };
      return { content: [{ type: "text", text: JSON.stringify(payload) }] };
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
    document_id: {
      type: "string" as const,
      description: "Session id from open_document (preferred).",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to export instead of a session. Use this stateless " +
        "path when no `document_id` is resident (e.g. a cold serverless instance).",
    },
    ir: {
      type: "object" as const,
      description: "Inline IR document (legacy alias for `document`).",
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
    joint_state: jointStateSchemaProp,
  },
  required: ["filename"],
};

export function exportCad(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const args = (input ?? {}) as Record<string, unknown>;
  const { doc: stored } = resolveDocInput(args, ["document", "ir"]);
  // Export the posed assembly, not just the zero pose.
  const { doc: ir, pose } = applyJointState(stored, args.joint_state);
  const filename = String(args.filename ?? "");

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

  // Evaluate the document to get meshes. Both document shapes export: scene
  // roots (`doc.roots`) AND assembly instances (`partDefs` + `instances`,
  // placed by the joint tree) — an assembly-authored document has no roots at
  // all, which is normal, not an error.
  const scene = engine.evaluate(ir);
  const units = sceneExportUnits(scene);

  if (units.length === 0) {
    const failures = scene.failures ?? [];
    if (failures.length > 0) {
      throw new Error(
        "Nothing to export: every feature failed to evaluate — " +
          failures.map((f) => `${f.scope}: ${f.error}`).join("; ") +
          ". Fix the failing feature with `update`, then re-export.",
      );
    }
    throw new Error(
      "Nothing to export: the document defines no geometry — it has no scene " +
        "roots (doc.roots) and no assembly instances. Add a part with " +
        "`create_cad_loon` / `create`, then re-export.",
    );
  }

  const triangles = units.reduce((n, u) => n + u.mesh.indices.length / 3, 0);
  if (triangles === 0) {
    throw new Error(
      `Nothing to export: ${units.length} part(s) evaluated but produced zero ` +
        "triangles (empty geometry — e.g. a boolean that removed everything). " +
        "Check the shape with `render_view`, then fix it with `update`.",
    );
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
    parts: units.length,
    ...(pose ? { pose } : {}),
    ...(scene.instances && scene.instances.length > 0
      ? { instances: scene.instances.length }
      : {}),
  });
}

export const toolDefs: ToolDef[] = [
  {
    name: "export_cad",
    pack: null,
    description:
      "Export a CAD document to a file. Supports STL (3D printing), GLB (visualization), and — for sheet-metal documents — STEP AP214 of the FOLDED body with true cylindrical bend faces (fab 3D pipelines like SendCutSend auto-detect bends/angles/directions; zero data entry). Format is determined by file extension. Pass `joint_state` to export a jointed assembly at a real pose (joint id or name → degrees, or mm for sliders) instead of its zero pose.",
    inputSchema: exportCadSchema,
    handler: (a, c) => exportCad(a, c.engine),
    behavior: behavior({}),
  },
];
