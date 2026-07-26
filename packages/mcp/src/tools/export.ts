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
        "Output filename with extension (.stl, .glb, or .step/.stp), " +
        "relative to the server working directory (or VCAD_MCP_EXPORT_DIR if set). " +
        "STEP is a BRep AP214 export: booleans, transforms, fillets, and sweeps keep true " +
        "analytic faces (the format CNC vendors quote from). Sheet-metal documents export the " +
        "FOLDED body with cylindrical bend faces auto-detected by 3D fab pipelines (e.g. " +
        "SendCutSend). Parts built from imported meshes have no BRep and are refused by name — " +
        "export those as STL.",
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

  // STEP: BRep-preserving export. Sheet-metal documents export the FOLDED
  // body (true cylindrical bend faces — the zero-data-entry upload path for
  // 3D fab pipelines); everything else evaluates the scene roots through
  // the kernel, where booleans, transforms, fillets, and sweeps all keep
  // analytic BRep faces, and serializes one AP214 body per root.
  const stepExt = filename.toLowerCase().split(".").pop();
  if (stepExt === "step" || stepExt === "stp") {
    const foldedStep = engine.foldedSheetMetalStep(ir);
    if (foldedStep !== null) {
      const bytes = new TextEncoder().encode(foldedStep);
      return deliver(filename, bytes, {
        format: stepExt,
        parts: 1,
        note: "Folded sheet-metal body (AP214) with cylindrical bend faces — bends/angles/directions auto-detect in 3D fab pipelines.",
      });
    }
    // General path: BRep evaluation of scene roots. Throws with the
    // offending root names when a part is mesh-only (e.g. imported mesh).
    const bytes = engine.documentStep(ir);
    return deliver(filename, bytes, {
      format: stepExt,
      note: "BRep AP214 export — analytic faces (planes/cylinders/spheres/cones/tori/NURBS) preserved through booleans; ready for CNC quoting.",
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
      throw new Error(`Unsupported format: .${ext}. Use .stl, .glb, or .step`);
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
      "Export a CAD document to a file. Supports STL (3D printing), GLB (visualization), and STEP AP214 (CNC quoting: BRep with true analytic faces, preserved through booleans/transforms/fillets/sweeps; sheet-metal documents export the FOLDED body with cylindrical bend faces that 3D fab pipelines like SendCutSend auto-detect). Mesh-only parts (imported meshes) can't go to STEP and are refused by name — use STL for those. Format is determined by file extension. Pass `joint_state` to export a jointed assembly at a real pose (joint id or name → degrees, or mm for sliders) instead of its zero pose.",
    inputSchema: exportCadSchema,
    handler: (a, c) => exportCad(a, c.engine),
    behavior: behavior({}),
  },
];
