/**
 * Asset resolution for create_cad_loon: the bytes behind `[import-mesh …]`
 * and `[import-step …]`.
 *
 * The WASM kernel has no filesystem, so a loon program's import nodes keep
 * only a path — a `mesh_import` evaluates to nothing and a `step_import`
 * throws until its bytes are registered. The server resolves them here, from
 * one of two places:
 *
 * - **by value** — the caller's `files` map (`{ "<path>": "<base64>" }`),
 *   keyed by the exact path the program names. Works on every deployment,
 *   hosted included.
 * - **from disk** — `<base_dir>/<path>`, only when the server runs on the
 *   caller's machine (stdio / local). A hosted server refuses, since its
 *   disk is not the caller's.
 *
 * A reference neither route can satisfy is reported, never silently dropped:
 * a part that quietly isn't there is the worse failure.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, normalize } from "node:path";
import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import { parseStl } from "./import-urdf.js";
import { isRemoteDeployment } from "./remote.js";
import { resolveWithinRoot } from "./safe-path.js";

/** Per-file cap, matching import_step / import_urdf. */
const MAX_ASSET_BYTES = 100 * 1024 * 1024;

export interface AssetSources {
  files?: Record<string, string>;
  base_dir?: string;
}

export interface AssetReport {
  meshes: number;
  steps: number;
  unresolved: Array<{ path: string; reason: string }>;
}

/** `./a/b.stl`, `a//b.stl` and `a/b.stl` name the same file. */
function key(p: string): string {
  return normalize(p).replace(/\\/g, "/").replace(/^\.\//, "");
}

/** Bytes for `path`, by value first, then (locally) from disk. */
function readAsset(
  path: string,
  { files, base_dir }: AssetSources,
): { bytes: Buffer } | { reason: string } {
  const byValue = Object.entries(files ?? {}).find(([k]) => key(k) === key(path));
  if (byValue) {
    const bytes = Buffer.from(byValue[1], "base64");
    if (bytes.length === 0) return { reason: "`files` entry decoded to zero bytes" };
    if (bytes.length > MAX_ASSET_BYTES) return { reason: "file exceeds size limit" };
    return { bytes };
  }
  if (isRemoteDeployment()) {
    return {
      reason:
        "not in `files` — this hosted server cannot read your filesystem; pass the " +
        "file by value as `files: { \"" + path + "\": \"<base64>\" }`",
    };
  }
  let file: string;
  try {
    // A local server is on the caller's machine, so an absolute path is
    // theirs to name; a relative one is confined to base_dir.
    file = isAbsolute(path)
      ? path
      : resolveWithinRoot(path, base_dir ?? process.cwd());
  } catch (e) {
    return { reason: (e as Error).message };
  }
  if (!existsSync(file) || !statSync(file).isFile()) {
    return {
      reason: base_dir || isAbsolute(path)
        ? `file not found (${file})`
        : "file not found — pass `base_dir` (the project directory) or the bytes in `files`",
    };
  }
  if (statSync(file).size > MAX_ASSET_BYTES) return { reason: "file exceeds size limit" };
  return { bytes: readFileSync(file) };
}

/**
 * Resolve every import node of `doc` in place: `mesh_import` → inline
 * `ImportedMesh` (STL), `step_import` → bytes registered with the kernel
 * under the node's path. Idempotent — an inlined node is no longer an import,
 * and re-registering a STEP path replaces it.
 */
export function resolveLoonAssets(
  doc: Document,
  sources: AssetSources,
  engine: Engine,
): AssetReport {
  const report: AssetReport = { meshes: 0, steps: 0, unresolved: [] };
  for (const [id, node] of Object.entries(doc.nodes)) {
    // `mesh_import` isn't in the TS CsgOp union (it only exists en route to
    // being swapped), so go through a structural cast.
    const op = node.op as unknown as {
      type: string;
      path?: string;
      scale?: { x: number; y: number; z: number } | null;
    };
    const isMesh = op.type === "mesh_import" || op.type === "MeshImport";
    const isStep = op.type === "step_import";
    if (!isMesh && !isStep) continue;
    const path = op.path ?? "";

    if (isStep && engine.stepSourceRegistered(path) && !hasFile(sources, path)) {
      report.steps++;
      continue;
    }
    const got = readAsset(path, sources);
    if ("reason" in got) {
      report.unresolved.push({ path, reason: got.reason });
      continue;
    }

    if (isStep) {
      const buf = new ArrayBuffer(got.bytes.byteLength);
      new Uint8Array(buf).set(got.bytes);
      try {
        engine.registerStepSource(path, buf);
        report.steps++;
      } catch (e) {
        report.unresolved.push({ path, reason: `STEP parse error: ${(e as Error).message}` });
      }
      continue;
    }

    if (!/\.stl$/i.test(path)) {
      report.unresolved.push({ path, reason: "unsupported mesh format (STL only)" });
      continue;
    }
    let mesh: ReturnType<typeof parseStl>;
    try {
      mesh = parseStl(got.bytes);
    } catch (e) {
      report.unresolved.push({ path, reason: `STL parse error: ${(e as Error).message}` });
      continue;
    }
    // Same metres → mm convention and scale as the native evaluator's
    // `load_mesh_import`, so MCP and the CLI agree on the same program.
    const s = op.scale;
    const positions =
      s && (s.x !== 1 || s.y !== 1 || s.z !== 1)
        ? mesh.positions.map((v, i) => v * (i % 3 === 0 ? s.x : i % 3 === 1 ? s.y : s.z))
        : mesh.positions;
    doc.nodes[id] = {
      ...node,
      op: { type: "ImportedMesh", positions, indices: mesh.indices, source: path },
    } as Document["nodes"][string];
    report.meshes++;
  }
  return report;
}

function hasFile({ files }: AssetSources, path: string): boolean {
  return Object.keys(files ?? {}).some((k) => key(k) === key(path));
}

/** One line for the tool result; null when the program imports nothing. */
export function assetNote(r: AssetReport): string | null {
  if (!r.meshes && !r.steps && !r.unresolved.length) return null;
  const done = `Imports resolved: ${r.meshes} mesh(es), ${r.steps} STEP file(s).`;
  if (!r.unresolved.length) return done;
  return (
    `${done} UNRESOLVED (these parts evaluate to nothing until fixed):\n` +
    r.unresolved.map((u) => `- ${u.path}: ${u.reason}`).join("\n")
  );
}
