/**
 * Universal tool response envelope.
 *
 * Every v2 tool returns the same wire shape: an MCP `content` array
 * containing a `text` block with the JSON envelope and (when the tool
 * produced geometry) an `image` block carrying a preview PNG. Clients
 * that support MCP Apps additionally see the GLB-viewer resource.
 *
 * Tools never craft this shape by hand — they call `ok()` (success) or
 * `fail()` (error) and return the result.
 */

import type { Engine } from "@vcad/engine";
import type { Document, Vec3 } from "@vcad/ir";
import type { DocHandle } from "./handles.js";
import { generateGlbPreview } from "./tools/preview.js";
import { renderPreviewPng } from "./preview-image.js";
import { VIEWER_RESOURCE_URI, MCP_APP_MIME_TYPE } from "./viewer.js";

/** Coarse stats reported on every successful response. */
export interface ResultStats {
  doc: DocHandle;
  parts: number;
  nodes: number;
  volume_mm3?: number;
  bbox?: { min: Vec3; max: Vec3 };
  triangles?: number;
  elapsed_ms: number;
  /** Server implementation tag — agents can detect upgrades w/o parsing semver. */
  version: number;
}

/** Non-fatal issue surfaced alongside a successful result. */
export interface Warning {
  code: string;
  message: string;
  detail?: unknown;
}

/** Envelope shape returned by every tool. */
export interface ToolEnvelope<T> {
  ok: true;
  doc: DocHandle;
  stats: ResultStats;
  warnings: Warning[];
  result: T;
}

/** Error envelope (still wrapped in MCP content). */
export interface ToolError {
  ok: false;
  error: { code: string; message: string; detail?: unknown };
}

const SERVER_VERSION = 2;

/** MCP content-array entry (subset we use). */
type ContentBlock =
  | { type: "text"; text: string; annotations?: unknown }
  | { type: "image"; data: string; mimeType: string; annotations?: unknown }
  | { type: "resource_link"; uri: string; name?: string; mimeType?: string };

/** MCP tool result. */
export interface ToolResult {
  content: ContentBlock[];
  isError?: boolean;
  _meta?: Record<string, unknown>;
}

const VIEWER_META = {
  ui: { resourceUri: VIEWER_RESOURCE_URI },
  "ui/resourceUri": VIEWER_RESOURCE_URI,
};

/** Options driving how `ok()` decorates the response. */
export interface OkOpts {
  /** Tool-specific result payload (envelope's `result`). */
  result: unknown;
  /** Final handle the tool produced (or echoed). */
  handle: DocHandle;
  /** The full doc — used for stats + previews. Skip for read-only tools. */
  doc?: Document;
  /** Engine used to evaluate the doc, when needed. */
  engine?: Engine;
  /** Wall time the operation took. */
  startedAt: number;
  /** Optional warnings to surface to the agent. */
  warnings?: Warning[];
  /** Attach the inline `ui://vcad/viewer` resource (for geometry-producing tools). */
  attachViewerUi?: boolean;
  /** Skip preview generation even if a doc was given (e.g. simulate). */
  skipPreview?: boolean;
  /** Override the preview PNG (e.g. render tool already produced one). */
  previewPng?: string;
}

/** Compose a successful envelope into an MCP tool result. */
export function ok(opts: OkOpts): ToolResult {
  const stats = computeStats(opts.doc, opts.handle, opts.engine, opts.startedAt);
  const envelope: ToolEnvelope<unknown> = {
    ok: true,
    doc: opts.handle,
    stats,
    warnings: opts.warnings ?? [],
    result: opts.result,
  };
  const content: ContentBlock[] = [
    { type: "text", text: JSON.stringify(envelope) },
  ];

  // Agent-visible preview PNG (optional, but the design doc makes this
  // the default for build/edit/render).
  if (opts.previewPng) {
    content.push({ type: "image", data: opts.previewPng, mimeType: "image/png" });
  } else if (!opts.skipPreview && opts.doc && opts.engine) {
    const png = renderPreviewPng(opts.doc, opts.engine);
    if (png) content.push({ type: "image", data: png, mimeType: "image/png" });
  }

  // GLB resource for MCP Apps clients (Claude Desktop). Same payload
  // shape v1 used; we keep the `_vcad_glb` sentinel so the viewer
  // iframe code keeps working.
  if (opts.attachViewerUi && opts.doc && opts.engine) {
    const glb = generateGlbPreview(opts.doc, opts.engine);
    if (glb) {
      content.push({
        type: "text",
        text: JSON.stringify({ _vcad_glb: glb }),
        annotations: { audience: ["user"] },
      });
    }
  }

  const meta: Record<string, unknown> = {};
  if (opts.attachViewerUi) Object.assign(meta, VIEWER_META);

  return Object.keys(meta).length > 0
    ? { content, _meta: meta }
    : { content };
}

/** Compose a failed envelope into an MCP tool result. */
export function fail(code: string, message: string, detail?: unknown): ToolResult {
  const env: ToolError = { ok: false, error: { code, message, detail } };
  return {
    content: [{ type: "text", text: JSON.stringify(env) }],
    isError: true,
  };
}

/** Wrap a sync handler so any thrown error becomes a clean failure envelope. */
export function guard<I>(
  handler: (input: I) => ToolResult,
): (input: I) => ToolResult {
  return (input) => {
    try {
      return handler(input);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      const code =
        e instanceof Error && (e as Error & { code?: string }).code
          ? ((e as Error & { code?: string }).code as string)
          : "internal_error";
      return fail(code, message);
    }
  };
}

/** Async variant of `guard`. */
export function guardAsync<I>(
  handler: (input: I) => Promise<ToolResult>,
): (input: I) => Promise<ToolResult> {
  return async (input) => {
    try {
      return await handler(input);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      const code =
        e instanceof Error && (e as Error & { code?: string }).code
          ? ((e as Error & { code?: string }).code as string)
          : "internal_error";
      return fail(code, message);
    }
  };
}

/** Lightweight stats — falls back gracefully if evaluation fails. */
function computeStats(
  doc: Document | undefined,
  handle: DocHandle,
  engine: Engine | undefined,
  startedAt: number,
): ResultStats {
  const elapsed_ms = Math.round(performance.now() - startedAt);
  if (!doc) {
    return { doc: handle, parts: 0, nodes: 0, elapsed_ms, version: SERVER_VERSION };
  }
  const parts = doc.roots?.length ?? 0;
  const nodes = Object.keys(doc.nodes ?? {}).length;

  if (!engine) {
    return { doc: handle, parts, nodes, elapsed_ms, version: SERVER_VERSION };
  }

  try {
    const scene = engine.evaluate(doc);
    let volume = 0;
    let triangles = 0;
    const bb = {
      min: { x: Infinity, y: Infinity, z: Infinity },
      max: { x: -Infinity, y: -Infinity, z: -Infinity },
    };
    for (const part of scene.parts) {
      const m = part.mesh;
      triangles += m.indices.length / 3;
      for (let i = 0; i < m.indices.length; i++) {
        const vi = m.indices[i] * 3;
        const x = m.positions[vi];
        const y = m.positions[vi + 1];
        const z = m.positions[vi + 2];
        if (x < bb.min.x) bb.min.x = x;
        if (y < bb.min.y) bb.min.y = y;
        if (z < bb.min.z) bb.min.z = z;
        if (x > bb.max.x) bb.max.x = x;
        if (y > bb.max.y) bb.max.y = y;
        if (z > bb.max.z) bb.max.z = z;
      }
      volume += approxVolume(m);
    }
    const finite = isFinite(bb.min.x);
    return {
      doc: handle,
      parts,
      nodes,
      volume_mm3: round3(volume),
      triangles,
      bbox: finite ? bb : undefined,
      elapsed_ms,
      version: SERVER_VERSION,
    };
  } catch {
    return { doc: handle, parts, nodes, elapsed_ms, version: SERVER_VERSION };
  }
}

function approxVolume(mesh: { positions: number[] | Float32Array; indices: number[] | Uint32Array }): number {
  let v = 0;
  for (let i = 0; i < mesh.indices.length; i += 3) {
    const i0 = mesh.indices[i] * 3;
    const i1 = mesh.indices[i + 1] * 3;
    const i2 = mesh.indices[i + 2] * 3;
    const ax = mesh.positions[i0],
      ay = mesh.positions[i0 + 1],
      az = mesh.positions[i0 + 2];
    const bx = mesh.positions[i1],
      by = mesh.positions[i1 + 1],
      bz = mesh.positions[i1 + 2];
    const cx = mesh.positions[i2],
      cy = mesh.positions[i2 + 1],
      cz = mesh.positions[i2 + 2];
    v +=
      (ax * (by * cz - cy * bz) -
        bx * (ay * cz - cy * az) +
        cx * (ay * bz - by * az)) /
      6;
  }
  return Math.abs(v);
}

const round3 = (n: number) => Math.round(n * 1000) / 1000;

export { MCP_APP_MIME_TYPE, VIEWER_RESOURCE_URI };
