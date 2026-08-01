/**
 * import_urdf tool — import a URDF robot description as a session document.
 *
 * The kernel's URDF reader (via the WASM seam, same parser the browser
 * drag-drop uses) builds the kinematic tree: part_defs per link, instances,
 * joints, and a ground instance at the root link. `<mesh>` references come
 * back as `MeshImport` nodes carrying the URDF filename verbatim; this tool
 * resolves them against the URDF's directory and any `package_roots` and
 * inlines STL data Node-side (the browser flow does the same swap with
 * three.js loaders in `urdf-meshes.ts`). Unresolved references are reported,
 * not fatal — the kernel falls back to a placeholder cube per link, so joint
 * topology and inertials still simulate correctly.
 *
 * Local-mode-first: the hosted server has no client filesystem, so `path`
 * errors there. `content_base64` still works remotely, but mesh references
 * cannot be resolved without a filesystem.
 */

import type { Document, Node, NodeId } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { basename, dirname, isAbsolute, resolve, sep } from "node:path";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment } from "./remote.js";
import { registerSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

// URDF descriptors are tiny; the kernel rejects >8 MB XML anyway.
const MAX_URDF_BYTES = 8 * 1024 * 1024;
// Cap individual mesh files so a caller can't pin memory with a huge STL.
const MAX_MESH_BYTES = 100 * 1024 * 1024;

interface ImportUrdfInput {
  path?: string;
  content_base64?: string;
  package_roots?: string[];
  name?: string;
  floating_base?: boolean;
  floating_base_link?: string;
  spawn_height_mm?: number;
}

export const importUrdfSchema = {
  type: "object" as const,
  properties: {
    path: {
      type: "string" as const,
      description:
        "Path to the .urdf file on the server filesystem, relative to the server " +
        "working directory (or VCAD_MCP_EXPORT_DIR if set). Relative mesh " +
        "references inside the URDF resolve against the URDF's own directory. " +
        "On hosted servers pass content_base64 instead.",
    },
    content_base64: {
      type: "string" as const,
      description:
        "Base64-encoded URDF XML. Use instead of `path` when the server has no " +
        "access to your filesystem (hosted/remote deployments). Mesh references " +
        "cannot be resolved in this mode — links with meshes get placeholder " +
        "geometry, but joints and inertials import exactly.",
    },
    package_roots: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Directories to search when resolving package://NAME/... mesh URIs, " +
        "relative to the server working directory. Each root is checked for a " +
        "NAME/ subdirectory; first match wins.",
    },
    name: {
      type: "string" as const,
      description: "Robot name (default: filename without extension)",
    },
    floating_base: {
      type: "boolean" as const,
      description:
        "Synthesize a floating (6-DOF) base when the URDF declares none. Most " +
        "humanoid/quadruped URDFs ship the world link and its type=\"floating\" " +
        "joint commented out, on the convention that the simulator supplies the " +
        "free base — without this the root link is grounded and the robot is " +
        "welded to the world, which cannot walk or fall. Set this for any " +
        "locomotion task. No-op when the URDF already has a floating joint.",
    },
    floating_base_link: {
      type: "string" as const,
      description:
        "Link to attach the synthesized floating base to (default: the tree's " +
        "root link, i.e. the one that is never a joint's child). Requires " +
        "floating_base.",
    },
    spawn_height_mm: {
      type: "number" as const,
      description:
        "Initial base height in mm for the synthesized floating base (default 0). " +
        "A Free joint's `state` is a scalar and meaningless for 6 DOF, so this " +
        "is written as the joint's parentAnchor.z. Spawn just above the settled " +
        "standing height so the robot drops onto the ground instead of starting " +
        "interpenetrated (Booster K1: 620 mm against a 549.8 mm stand).",
    },
  },
};

/** Minimal mesh payload swapped into the document for a resolved reference. */
interface LoadedMesh {
  positions: number[];
  indices: number[];
}

/**
 * Parse an STL file (binary or ASCII) into a triangle soup. Vertices are in
 * meters (the ROS convention) and are converted to mm here, matching the
 * kernel's ×1000 on URDF primitive dimensions and the browser mesh loader.
 */
function parseStl(buf: Buffer): LoadedMesh {
  const positions: number[] = [];
  // Binary layout: 80-byte header, uint32 triangle count, 50 bytes/triangle.
  const looksBinary =
    buf.length >= 84 && buf.length === 84 + buf.readUInt32LE(80) * 50;
  if (looksBinary) {
    const count = buf.readUInt32LE(80);
    for (let t = 0; t < count; t++) {
      const base = 84 + t * 50 + 12; // skip the facet normal
      for (let v = 0; v < 3; v++) {
        const off = base + v * 12;
        positions.push(
          buf.readFloatLE(off) * 1000,
          buf.readFloatLE(off + 4) * 1000,
          buf.readFloatLE(off + 8) * 1000,
        );
      }
    }
  } else {
    const text = buf.toString("utf8");
    if (!/^\s*solid/.test(text)) {
      throw new Error("not a recognizable STL file (neither binary nor ASCII)");
    }
    const vertexRe =
      /vertex\s+([-+eE0-9.]+)\s+([-+eE0-9.]+)\s+([-+eE0-9.]+)/g;
    let m: RegExpExecArray | null;
    while ((m = vertexRe.exec(text)) !== null) {
      positions.push(
        parseFloat(m[1]) * 1000,
        parseFloat(m[2]) * 1000,
        parseFloat(m[3]) * 1000,
      );
    }
    if (positions.length === 0 || positions.length % 9 !== 0) {
      throw new Error("malformed ASCII STL");
    }
  }
  const indices = new Array(positions.length / 3);
  for (let i = 0; i < indices.length; i++) indices[i] = i;
  return { positions, indices };
}

/**
 * Resolve a URDF `<mesh filename="...">` value to an absolute on-disk path,
 * mirroring the Rust `UrdfReadOptions::resolve_mesh`. Every candidate is
 * required to stay under `confineRoot` — the same working-directory jail
 * `resolveWithinRoot` enforces on tool inputs, extended here to paths the
 * URDF content (untrusted) supplies.
 */
function resolveMeshPath(
  filename: string,
  urdfDir: string | null,
  packageRoots: string[],
  confineRoot: string,
): string | null {
  const confined = (p: string): string | null => {
    const abs = resolve(p);
    const root = resolve(confineRoot);
    const rootSep = root.endsWith(sep) ? root : root + sep;
    if (abs !== root && !abs.startsWith(rootSep)) return null;
    return existsSync(abs) && statSync(abs).isFile() ? abs : null;
  };

  if (filename.includes("\0")) return null;

  const pkgMatch = filename.match(/^package:\/\/([^/]+)\/?(.*)$/);
  if (pkgMatch) {
    for (const root of packageRoots) {
      const hit = confined(resolve(root, pkgMatch[1], pkgMatch[2]));
      if (hit) return hit;
    }
    return null;
  }

  const stripped = filename.startsWith("file://")
    ? filename.slice("file://".length)
    : filename;
  if (isAbsolute(stripped)) return confined(stripped);
  if (urdfDir) return confined(resolve(urdfDir, stripped));
  return null;
}

/**
 * Swap every resolvable `MeshImport` node for an inline `ImportedMesh`
 * (the Node-side analogue of the app's `inlineMeshImports`). Returns the
 * inlined count and the references that could not be resolved or parsed.
 */
function inlineMeshImports(
  doc: Document,
  urdfDir: string | null,
  packageRoots: string[],
  confineRoot: string,
): { inlined: number; unresolved: Array<{ path: string; reason: string }> } {
  let inlined = 0;
  const unresolved: Array<{ path: string; reason: string }> = [];
  for (const key of Object.keys(doc.nodes)) {
    const node = doc.nodes[key] as Node;
    // MeshImport serializes as `"type": "mesh_import"` (the IR's one serde
    // rename); it isn't in the TS CsgOp union since it only exists en route
    // to being swapped, so go through a structural cast.
    const opLoose = node.op as unknown as {
      type: string;
      path?: string;
      scale?: { x: number; y: number; z: number } | null;
    };
    if (opLoose.type !== "mesh_import" && opLoose.type !== "MeshImport") {
      continue;
    }
    const ref = opLoose.path ?? "";
    const filepath = resolveMeshPath(ref, urdfDir, packageRoots, confineRoot);
    if (!filepath) {
      unresolved.push({ path: ref, reason: "file not found" });
      continue;
    }
    if (!/\.stl$/i.test(filepath)) {
      unresolved.push({
        path: ref,
        reason: "unsupported mesh format (only STL is inlined server-side)",
      });
      continue;
    }
    if (statSync(filepath).size > MAX_MESH_BYTES) {
      unresolved.push({ path: ref, reason: "mesh exceeds size limit" });
      continue;
    }
    let mesh: LoadedMesh;
    try {
      mesh = parseStl(readFileSync(filepath));
    } catch (e) {
      unresolved.push({ path: ref, reason: `parse error: ${e}` });
      continue;
    }
    const scale = opLoose.scale;
    let positions = mesh.positions;
    if (scale && (scale.x !== 1 || scale.y !== 1 || scale.z !== 1)) {
      positions = positions.map((v, i) =>
        i % 3 === 0 ? v * scale.x : i % 3 === 1 ? v * scale.y : v * scale.z,
      );
    }
    doc.nodes[key] = {
      id: node.id as NodeId,
      name: node.name,
      op: {
        type: "ImportedMesh",
        positions,
        indices: mesh.indices,
        source: ref,
        // biome-ignore lint/suspicious/noExplicitAny: bridging IR shape
      } as any,
    };
    inlined++;
  }
  return { inlined, unresolved };
}

export function importUrdf(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const {
    path,
    content_base64,
    package_roots,
    name,
    floating_base,
    floating_base_link,
    spawn_height_mm,
  } = input as ImportUrdfInput;

  if (!floating_base && (floating_base_link || spawn_height_mm !== undefined)) {
    throw new Error(
      "floating_base_link / spawn_height_mm require floating_base: true",
    );
  }

  const confineRoot = process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd();
  let fileBuffer: Buffer;
  let urdfDir: string | null = null;
  let sourceLabel: string;

  if (content_base64) {
    fileBuffer = Buffer.from(content_base64, "base64");
    if (fileBuffer.length === 0) {
      throw new Error("content_base64 decoded to zero bytes");
    }
    sourceLabel = "urdf-import";
  } else if (path) {
    if (isRemoteDeployment()) {
      throw new Error(
        "This hosted server has no access to your filesystem — pass the URDF " +
          "XML as `content_base64` instead of `path`. Mesh references cannot " +
          "be resolved in that mode (links fall back to placeholder geometry); " +
          "run the MCP server locally for full mesh import.",
      );
    }
    const filepath = resolveWithinRoot(path, confineRoot);
    if (!existsSync(filepath)) {
      throw new Error(`URDF file not found: ${path}`);
    }
    const stat = statSync(filepath);
    if (!stat.isFile()) {
      throw new Error("URDF path is not a regular file");
    }
    if (stat.size > MAX_URDF_BYTES) {
      throw new Error(`URDF file exceeds ${MAX_URDF_BYTES} byte limit`);
    }
    fileBuffer = readFileSync(filepath);
    urdfDir = dirname(filepath);
    sourceLabel = path;
  } else {
    throw new Error("Provide either `path` or `content_base64`");
  }
  if (fileBuffer.length > MAX_URDF_BYTES) {
    throw new Error(`URDF content exceeds ${MAX_URDF_BYTES} byte limit`);
  }

  const packageRoots = (package_roots ?? []).map((r) =>
    resolveWithinRoot(r, confineRoot),
  );

  // Parse through the WASM seam — the same kernel parser the browser
  // drag-drop uses. Copy into a plain ArrayBuffer (Buffer.buffer may be a
  // pooled slice).
  const arrayBuffer = new ArrayBuffer(fileBuffer.byteLength);
  new Uint8Array(arrayBuffer).set(fileBuffer);
  const doc = JSON.parse(
    engine.importUrdf(arrayBuffer, {
      floatingBase: floating_base,
      floatingBaseLink: floating_base_link,
      spawnHeightMm: spawn_height_mm,
    }),
  ) as Document;

  // A floating joint hiding in a comment means the author expected the
  // simulator to supply the free base — say so rather than silently
  // handing back a robot welded to the world.
  const commentedFloating = floating_base
    ? undefined
    : engine.urdfCommentedFloatingJoint(arrayBuffer);

  const meshes = inlineMeshImports(doc, urdfDir, packageRoots, confineRoot);

  const robotName =
    name ?? basename(sourceLabel).replace(/\.(urdf|xml)$/i, "");

  const parts = Object.values(doc.partDefs ?? {}).map((p) => ({
    id: p.id,
    name: p.name,
  }));
  const joints = (doc.joints ?? []).map((j) => ({
    id: j.id,
    name: j.name,
    kind: j.kind.type,
    parent_instance_id: j.parentInstanceId,
    child_instance_id: j.childInstanceId,
  }));

  const documentId = registerSession(doc);

  const summary = {
    robot: robotName,
    parts: parts.length,
    joints: joints.length,
    instances: (doc.instances ?? []).length,
    ground_instance_id: doc.groundInstanceId ?? null,
    joint_list: joints,
    ...(floating_base
      ? {
          floating_base: {
            synthesized: joints.some((j) => j.kind === "Free"),
            spawn_height_mm: spawn_height_mm ?? 0,
            note:
              "The root link hangs off a 6-DOF Free joint; parentAnchor.z is the " +
              "spawn height. The robot will fall under gravity.",
          },
        }
      : {}),
    ...(commentedFloating
      ? {
          floating_base_warning:
            `This URDF declares a floating joint (${commentedFloating}) inside a ` +
            "comment — the usual convention that the simulator supplies the free " +
            "base. Imported as-is, the root link is grounded and the robot is " +
            "welded to the world (it cannot walk or fall). Re-import with " +
            "floating_base: true (and a spawn_height_mm just above standing " +
            "height) for any locomotion task.",
        }
      : {}),
    ...(meshes.inlined > 0 ? { meshes_inlined: meshes.inlined } : {}),
    ...(meshes.unresolved.length > 0
      ? {
          warning:
            `${meshes.unresolved.length} mesh reference(s) could not be inlined — ` +
            "those links use placeholder geometry. Joint topology and authored " +
            "inertials are exact, so physics still behaves to first order.",
          unresolved_meshes: meshes.unresolved,
        }
      : {}),
  };

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            summary,
            note:
              "The robot is registered as a session document — pass document_id " +
              "to create_robot_env to simulate it, render_view to see it, or any " +
              "CAD tool to edit it.",
          },
          null,
          2,
        ),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "import_urdf",
    pack: null,
    description:
      "Import a URDF (Unified Robot Description Format) robot from a server-local file. " +
      "Builds the full kinematic tree — one part per link, instances FK-placed from joint " +
      "origins, joints (revolute/prismatic/fixed) with limits, and a grounded root link — " +
      "ready for create_robot_env. STL mesh references are resolved against the URDF's " +
      "directory and optional package_roots and inlined; unresolved meshes fall back to " +
      "placeholder geometry with an explicit warning. For locomotion, pass " +
      "floating_base: true — most humanoid/quadruped URDFs leave the world link and " +
      "floating joint commented out, and without them the robot is welded to the world.",
    inputSchema: importUrdfSchema,
    handler: (a, c) => importUrdf(a, c.engine),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
