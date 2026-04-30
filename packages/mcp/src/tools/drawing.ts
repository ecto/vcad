/**
 * drawing (v2) — generate 2D drawings (orthographic + iso views) as SVG.
 *
 * Phase-1 scope: project the evaluated mesh into the requested views and
 * draw triangle edges in stroke. The full drafting pipeline (sections,
 * dimensions, GD&T, BOMs) is the Phase 6 follow-up; this tool ships
 * enough to give the agent a reviewable result on every doc.
 */

import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef, ResourceRef } from "../types.js";

export const drawingSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle." },
    views: {
      type: "array" as const,
      description:
        "Drawing views: { kind: 'ortho', angle: 'front'|'top'|'right'|... } or { kind: 'iso' }.",
    },
    sheet: {
      type: "object" as const,
      properties: {
        size: { type: "string" as const, enum: ["A4", "A3", "A2", "letter"] },
        orientation: { type: "string" as const, enum: ["portrait", "landscape"] },
      },
    },
  },
  required: ["doc"],
};

interface OrthoView {
  kind: "ortho";
  angle: "front" | "top" | "right" | "left" | "back" | "bottom";
  at?: { x: number; y: number };
  scale?: number;
}
interface IsoView {
  kind: "iso";
  at?: { x: number; y: number };
  scale?: number;
  from?: "ne" | "nw" | "se" | "sw";
}
type DrawingView = OrthoView | IsoView;

interface DrawingInput {
  doc: DocRef;
  views?: DrawingView[];
  sheet?: { size?: string; orientation?: string };
}

const SHEETS = {
  A4: { width: 297, height: 210 },
  A3: { width: 420, height: 297 },
  A2: { width: 594, height: 420 },
  letter: { width: 279.4, height: 215.9 },
} as const;

export function drawing(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as DrawingInput;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");

  const { doc, handle } = resolveRef(args.doc);
  let scene;
  try {
    scene = engine.evaluate(doc);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return fail("eval_failed", msg);
  }
  if (scene.parts.length === 0) return fail("empty_document", "No parts to draw.");

  const sheetKey = (args.sheet?.size ?? "A4") as keyof typeof SHEETS;
  const sheet = SHEETS[sheetKey] ?? SHEETS.A4;
  let landscape = (args.sheet?.orientation ?? "landscape") === "landscape";
  let W = landscape ? Math.max(sheet.width, sheet.height) : Math.min(sheet.width, sheet.height);
  let H = landscape ? Math.min(sheet.width, sheet.height) : Math.max(sheet.width, sheet.height);

  const views =
    args.views && args.views.length > 0
      ? args.views
      : ([
          { kind: "ortho", angle: "front" },
          { kind: "ortho", angle: "top" },
          { kind: "ortho", angle: "right" },
          { kind: "iso" },
        ] as DrawingView[]);

  // Compute scene bbox for auto-framing.
  const bbox = sceneBox(scene);
  const span = Math.max(
    bbox.max.x - bbox.min.x,
    bbox.max.y - bbox.min.y,
    bbox.max.z - bbox.min.z,
    1,
  );
  // Pack views in a 2×2 grid by default.
  const cellW = W / 2;
  const cellH = H / 2;
  const margin = Math.min(cellW, cellH) * 0.1;
  const drawScale = (Math.min(cellW, cellH) - 2 * margin) / span;

  let svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}mm" height="${H}mm" viewBox="0 0 ${W} ${H}" font-family="sans-serif" font-size="3">`;
  svg += `<rect x="0" y="0" width="${W}" height="${H}" fill="white" stroke="black" stroke-width="0.3"/>`;

  for (let i = 0; i < views.length; i++) {
    const v = views[i];
    const col = i % 2;
    const row = Math.floor(i / 2);
    const cx = col * cellW + cellW / 2;
    const cy = row * cellH + cellH / 2;
    svg += renderView(v, scene, bbox, cx, cy, drawScale * (v.scale ?? 1));
  }

  svg += "</svg>";

  const resource: ResourceRef = {
    kind: "embedded",
    mime: "image/svg+xml",
    data_base64: Buffer.from(svg, "utf-8").toString("base64"),
  };

  return ok({
    result: {
      svg,
      resource,
      sheet: { size: sheetKey, orientation: landscape ? "landscape" : "portrait", width_mm: W, height_mm: H },
      views: views.map((v) => v.kind + ("angle" in v ? `:${v.angle}` : "")),
    },
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}

interface Box {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
}

function sceneBox(scene: { parts: Array<{ mesh: { positions: number[] | Float32Array } }> }): Box {
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (const part of scene.parts) {
    const p = part.mesh.positions;
    for (let i = 0; i < p.length; i += 3) {
      if (p[i] < minX) minX = p[i];
      if (p[i + 1] < minY) minY = p[i + 1];
      if (p[i + 2] < minZ) minZ = p[i + 2];
      if (p[i] > maxX) maxX = p[i];
      if (p[i + 1] > maxY) maxY = p[i + 1];
      if (p[i + 2] > maxZ) maxZ = p[i + 2];
    }
  }
  if (!isFinite(minX)) {
    return { min: { x: 0, y: 0, z: 0 }, max: { x: 1, y: 1, z: 1 } };
  }
  return { min: { x: minX, y: minY, z: minZ }, max: { x: maxX, y: maxY, z: maxZ } };
}

function renderView(
  view: DrawingView,
  scene: { parts: Array<{ mesh: { positions: number[] | Float32Array; indices: number[] | Uint32Array } }> },
  bbox: Box,
  cx: number,
  cy: number,
  scale: number,
): string {
  const cxw = (bbox.min.x + bbox.max.x) / 2;
  const cyw = (bbox.min.y + bbox.max.y) / 2;
  const czw = (bbox.min.z + bbox.max.z) / 2;

  let label: string;
  let project: (p: { x: number; y: number; z: number }) => { x: number; y: number };
  if (view.kind === "iso") {
    label = "ISO";
    const dir = norm({ x: 1, y: 1, z: 0.7 });
    const up = norm({ x: 0, y: 0, z: 1 });
    const right = norm(crossV(dir, up));
    const camUp = norm(crossV(right, dir));
    project = (p) => {
      const dx = p.x - cxw, dy = p.y - cyw, dz = p.z - czw;
      return {
        x: cx + (dx * right.x + dy * right.y + dz * right.z) * scale,
        y: cy - (dx * camUp.x + dy * camUp.y + dz * camUp.z) * scale,
      };
    };
  } else {
    label = view.angle.toUpperCase();
    switch (view.angle) {
      case "front":
        project = (p) => ({ x: cx + (p.x - cxw) * scale, y: cy - (p.z - czw) * scale });
        break;
      case "back":
        project = (p) => ({ x: cx - (p.x - cxw) * scale, y: cy - (p.z - czw) * scale });
        break;
      case "top":
        project = (p) => ({ x: cx + (p.x - cxw) * scale, y: cy + (p.y - cyw) * scale });
        break;
      case "bottom":
        project = (p) => ({ x: cx + (p.x - cxw) * scale, y: cy - (p.y - cyw) * scale });
        break;
      case "right":
        project = (p) => ({ x: cx + (p.y - cyw) * scale, y: cy - (p.z - czw) * scale });
        break;
      case "left":
        project = (p) => ({ x: cx - (p.y - cyw) * scale, y: cy - (p.z - czw) * scale });
        break;
    }
  }

  let g = `<g><text x="${cx}" y="${cy + 50 * scale}" text-anchor="middle">${label}</text>`;

  // Edge map for outline — count unique edges; edges appearing exactly
  // once are silhouette/boundary edges.
  for (const part of scene.parts) {
    const idx = part.mesh.indices;
    const pos = part.mesh.positions;
    g += '<g stroke="black" stroke-width="0.15" fill="none">';
    for (let t = 0; t < idx.length; t += 3) {
      const a = idx[t] * 3, b = idx[t + 1] * 3, c = idx[t + 2] * 3;
      const va = { x: pos[a], y: pos[a + 1], z: pos[a + 2] };
      const vb = { x: pos[b], y: pos[b + 1], z: pos[b + 2] };
      const vc = { x: pos[c], y: pos[c + 1], z: pos[c + 2] };
      const pa = project(va), pb = project(vb), pc = project(vc);
      g += `<polyline points="${fmt(pa)} ${fmt(pb)} ${fmt(pc)} ${fmt(pa)}" />`;
    }
    g += "</g>";
  }
  g += "</g>";
  return g;
}

const fmt = (p: { x: number; y: number }) =>
  `${p.x.toFixed(2)},${p.y.toFixed(2)}`;

interface V3 { x: number; y: number; z: number }
const crossV = (a: V3, b: V3): V3 => ({
  x: a.y * b.z - a.z * b.y,
  y: a.z * b.x - a.x * b.z,
  z: a.x * b.y - a.y * b.x,
});
const norm = (v: V3): V3 => {
  const l = Math.hypot(v.x, v.y, v.z) || 1;
  return { x: v.x / l, y: v.y / l, z: v.z / l };
};
