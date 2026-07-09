/**
 * render_view tool — give agents eyes.
 *
 * Renders an open session document to the same drafting-style isometric
 * SVG that the `vcad-render` CLI and the mecheval leaderboard produce
 * (kernel WASM `render_svg`), then rasterizes it to PNG so the model
 * receives an image content block it can actually see. Falls back to
 * returning the raw SVG as text when the optional `@resvg/resvg-js`
 * rasterizer is unavailable.
 *
 * This closes the "blind agent" half of the verify-and-iterate loop:
 * inspect_cad gives numbers, render_view shows the part.
 */

import { getKernelWasm, resetKernelWasm, computeRatsnest } from "@vcad/engine";
import type { NetlistResult } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import type { Document, Pcb } from "@vcad/ir";
import { getSession, resolveDocInput } from "./session.js";
import { validatePcb, pcbValidationError } from "./pcb-validate.js";
import { behavior, type ToolDef } from "./tool-def.js";
import type { ToolResult } from "./tool-result.js";
import {
  makePngRenderAsset,
  renderAssetSummary,
  withRenderAssets,
} from "./render-assets.js";

export const renderViewSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to render instead of a session. Use this stateless " +
        "path when no `document_id` is resident (e.g. a cold serverless instance).",
    },
    view: {
      type: "string" as const,
      enum: ["iso", "isometric", "top", "front", "side"],
      description:
        "Camera view: 'iso' (default, 3/4 isometric), or an orthographic 'top' / 'front' / 'side' elevation. Use 'top' to read a part flat from above.",
    },
    width_px: {
      type: "number" as const,
      description:
        "Target raster width in pixels (default 800, clamped to 64–2048). Ignored when falling back to SVG output.",
    },
  },
};

type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string };

interface RenderViewResult {
  content: ContentBlock[];
  isError?: boolean;
  structuredContent?: Record<string, unknown>;
}

/** px-per-mm passed to the kernel renderer; the raster step handles
 *  final sizing, so this only controls SVG coordinate precision. */
const SVG_SCALE = 2.0;

const DEFAULT_WIDTH_PX = 800;

/** Evaluation runs synchronously inside the shared WASM instance with no
 *  timeout, so runaway documents (a 10^7-count pattern is one tool call
 *  away) must be rejected up front rather than discovered as a hang. */
const MAX_NODES = 10_000;
const MAX_PATTERN_INSTANCES = 50_000;

/** Raw SVG beyond this many bytes is withheld from the fallback text
 *  block — a complex document can otherwise inject megabytes into the
 *  model context in a single tool result. */
const MAX_INLINE_SVG_BYTES = 256 * 1024;

/** Cheap pre-flight complexity bound: node count plus the sum of pattern
 *  counts. Not a tessellation-cost model — just a guard that keeps
 *  obviously-runaway documents out of the synchronous eval path. */
function complexityGuard(doc: Document): string | null {
  const nodes = Object.values(doc.nodes ?? {});
  if (nodes.length > MAX_NODES) {
    return `document has ${nodes.length} nodes (limit ${MAX_NODES})`;
  }
  let patternInstances = 0;
  for (const node of nodes) {
    const op = (node as { op?: { type?: string; count?: number } }).op;
    if (op?.type === "LinearPattern" || op?.type === "CircularPattern") {
      patternInstances += typeof op.count === "number" ? op.count : 0;
    }
  }
  if (patternInstances > MAX_PATTERN_INSTANCES) {
    return `document has ${patternInstances} pattern instances (limit ${MAX_PATTERN_INSTANCES})`;
  }
  return null;
}

type RasterOutcome =
  | { png: Buffer }
  | { png: null; reason: "module-missing" | string };

/** Rasterize SVG to PNG via the optional resvg dependency. A missing
 *  module and a genuine rasterization failure are distinct outcomes so
 *  the fallback note never tells the agent to install a dependency that
 *  is present but failing. */
export async function rasterize(
  svg: string,
  widthPx: number,
  background = "white",
): Promise<RasterOutcome> {
  let ResvgCtor: typeof import("@resvg/resvg-js").Resvg;
  try {
    ({ Resvg: ResvgCtor } = await import("@resvg/resvg-js"));
  } catch (e) {
    const code = (e as NodeJS.ErrnoException)?.code;
    if (code === "ERR_MODULE_NOT_FOUND" || code === "MODULE_NOT_FOUND") {
      return { png: null, reason: "module-missing" };
    }
    return {
      png: null,
      reason: `resvg import failed: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
  try {
    const resvg = new ResvgCtor(svg, {
      fitTo: { mode: "width", value: widthPx },
      background,
    });
    return { png: resvg.render().asPng() };
  } catch (e) {
    return {
      png: null,
      reason: `rasterization failed: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

export async function renderView(
  args: Record<string, unknown>,
): Promise<RenderViewResult> {
  const { doc, documentId: resolvedId } = resolveDocInput(args);
  // Echoed back in result/error payloads; the empty string marks the inline path.
  const documentId = resolvedId ?? "";

  const widthRaw = Number(args.width_px ?? DEFAULT_WIDTH_PX);
  const widthPx = Math.min(
    2048,
    Math.max(64, Number.isFinite(widthRaw) ? Math.round(widthRaw) : DEFAULT_WIDTH_PX),
  );

  const tooComplex = complexityGuard(doc);
  if (tooComplex) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `render refused: ${tooComplex}`,
            document_id: documentId,
            hint: "Reduce pattern counts or node count, or inspect numerically via inspect_cad.",
          }),
        },
      ],
      isError: true,
    };
  }

  const viewRaw = String(args.view ?? "iso").toLowerCase();
  const view = (
    viewRaw === "isometric"
      ? "iso"
      : ["iso", "top", "front", "side"].includes(viewRaw)
        ? viewRaw
        : "iso"
  ) as "iso" | "top" | "front" | "side";
  // Canonical label for the result payload — the default view is reported as
  // "isometric" (stable contract); orthographic views report their own name.
  const viewLabel = view === "iso" ? "isometric" : view;

  const wasm = (await getKernelWasm()) as unknown as {
    render_svg: (vcadJson: string, scale: number) => string;
    render_svg_view?: (vcadJson: string, scale: number, view: string) => string;
  };
  if (typeof wasm.render_svg !== "function") {
    return {
      content: [
        {
          type: "text",
          text: "render_view unavailable: kernel WASM build predates render_svg — rebuild @vcad/kernel-wasm.",
        },
      ],
      isError: true,
    };
  }

  let svg: string;
  try {
    svg =
      view !== "iso" && typeof wasm.render_svg_view === "function"
        ? wasm.render_svg_view(JSON.stringify(doc), SVG_SCALE, view)
        : wasm.render_svg(JSON.stringify(doc), SVG_SCALE);
  } catch (e) {
    // A WebAssembly trap means a kernel panic that did NOT unwind —
    // wasm32 compiles panics to `unreachable`, so the kernel's own
    // catch_unwind never fires and the instance is left in an undefined
    // state. Recover by dropping it and re-instantiating in place, so this
    // one bad document fails without taking down every other session.
    if (e instanceof WebAssembly.RuntimeError) {
      resetKernelWasm(`render_svg trapped: ${e.message}`);
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify({
              error: `kernel trap during render: ${e.message}`,
              document_id: documentId,
              hint: "This document hit a kernel bug while rendering; the kernel was reset, so other documents are unaffected. Please report the document that triggered it.",
            }),
          },
        ],
        isError: true,
      };
    }
    // Loud, structured failure — never a silent blank image.
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `render failed: ${e instanceof Error ? e.message : String(e)}`,
            document_id: documentId,
            hint: "The document may be empty or evaluation may have failed — check inspect_cad output.",
          }),
        },
      ],
      isError: true,
    };
  }

  const raster = await rasterize(svg, widthPx);
  if (raster.png) {
    const asset = makePngRenderAsset(raster.png, {
      tool: "render_view",
      filename: `${documentId || "inline"}-${viewLabel}-${widthPx}.png`,
      width: widthPx,
      alt: `vcad ${viewLabel} render`,
    });
    return withRenderAssets<RenderViewResult>({
      content: [
        {
          type: "image",
          data: raster.png.toString("base64"),
          mimeType: "image/png",
        },
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            view: viewLabel,
            width_px: widthPx,
            format: "png",
            asset: renderAssetSummary(asset),
            suggested_final_markdown: asset.markdown,
          }),
        },
      ],
    }, [asset]);
  }

  // Rasterizer unavailable or failed — degrade to raw SVG text (size
  // capped) rather than failing, with an honest note about why.
  const note =
    raster.reason === "module-missing"
      ? "Install @resvg/resvg-js for PNG output; returning raw SVG."
      : `PNG ${raster.reason}; returning raw SVG.`;
  const svgBytes = Buffer.byteLength(svg, "utf8");
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: documentId,
          view: viewLabel,
          format: "svg",
          note,
          ...(svgBytes <= MAX_INLINE_SVG_BYTES
            ? { svg }
            : {
                svg_omitted: `SVG is ${svgBytes} bytes (inline cap ${MAX_INLINE_SVG_BYTES}) — use export_cad for geometry output.`,
              }),
        }),
      },
    ],
  };
}

// ───────────────────────────────────────────────────────────────────────────
// render_pcb — flat, top-down, per-layer 2D board view (agent eyes for PCBs)
// ───────────────────────────────────────────────────────────────────────────

export const renderPcbSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id of a board (from create_schematic / place_components).",
    },
    layers: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        'Layers to draw, front-to-back. Accepts KiCad ("F.Cu", "F.SilkS", "Edge_Cuts") or serde ("FCu", "FSilkS", "EdgeCuts") names. Default: ["F.Cu", "F.SilkS", "Edge_Cuts"].',
    },
    width_px: {
      type: "number" as const,
      description: "Target raster width in pixels (default 1200, clamped 64–2048).",
    },
    theme: {
      type: "string" as const,
      enum: ["dark", "light"],
      description:
        'Color theme. "dark" (default) is the high-contrast "Studio Graphite" editor look; "light" is the legacy white/green fabrication look.',
    },
    highlight: {
      type: "object" as const,
      properties: {
        nets: { type: "array" as const, items: { type: "string" as const } },
        refs: { type: "array" as const, items: { type: "string" as const } },
      },
      description:
        'Focus a subset: highlighted nets/refs recolor to brand pink with a glow and everything else dims. E.g. {"nets":["GND"]} to trace a net, {"refs":["U1"]} to spotlight a part.',
    },
    net_labels: {
      type: "boolean" as const,
      description:
        "Annotate routed copper with net names (off by default; useful as a verification overlay).",
    },
    values: {
      type: "boolean" as const,
      description: "Draw component value labels (on by default, zoom-gated).",
    },
    hero: {
      type: "boolean" as const,
      description:
        "Marketing/hero still: adds a copper bloom. Off by default — never use for verification renders (glow misrepresents copper extents).",
    },
  },
  required: ["document_id"],
};

/** Extract the PCB from a session document (PcbBoard node, or a bare `pcb`). */
function docPcb(doc: Document): Pcb | null {
  const ids = getPcbNodeIds(doc);
  if (ids.length > 0) return getNodePcb(doc, ids[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
}

export async function renderPcb(
  args: Record<string, unknown>,
): Promise<RenderViewResult> {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);
  const pcb = docPcb(doc);
  if (!pcb) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: "document has no PCB — run place_components first (or open a board)",
            document_id: documentId,
          }),
        },
      ],
      isError: true,
    };
  }

  const validity = validatePcb(pcb);
  if (!validity.valid) {
    return pcbValidationError("render_pcb", validity, documentId) as unknown as RenderViewResult;
  }

  const layers =
    Array.isArray(args.layers) && args.layers.length > 0
      ? (args.layers as unknown[]).map(String)
      : ["F.Cu", "F.SilkS", "Edge_Cuts"];

  const widthRaw = Number(args.width_px ?? 1200);
  const widthPx = Math.min(
    2048,
    Math.max(64, Number.isFinite(widthRaw) ? Math.round(widthRaw) : 1200),
  );

  // Assemble the "Studio Graphite" render options.
  const theme = args.theme === "light" ? "light" : "dark";
  const hl = (args.highlight ?? {}) as { nets?: unknown; refs?: unknown };
  const asStrings = (v: unknown): string[] =>
    Array.isArray(v) ? v.map(String) : [];
  const opts: Record<string, unknown> = { theme };
  if (typeof args.net_labels === "boolean") opts.netLabels = args.net_labels;
  if (typeof args.values === "boolean") opts.values = args.values;
  if (typeof args.hero === "boolean") opts.hero = args.hero;
  const hlNets = asStrings(hl.nets);
  const hlRefs = asStrings(hl.refs);
  if (hlNets.length > 0 || hlRefs.length > 0) {
    opts.highlight = { nets: hlNets, refs: hlRefs };
  }
  const optsJson = JSON.stringify(opts);

  const wasm = (await getKernelWasm()) as unknown as {
    render_pcb_svg?: (pcbJson: string, layersJson: string, scale: number) => string;
    render_pcb_svg_opts?: (
      pcbJson: string,
      layersJson: string,
      scale: number,
      optsJson: string,
    ) => string;
  };
  if (typeof wasm.render_pcb_svg !== "function") {
    return {
      content: [
        {
          type: "text",
          text: "render_pcb unavailable: kernel WASM build predates render_pcb_svg — rebuild vcad-kernel-wasm.",
        },
      ],
      isError: true,
    };
  }

  let svg: string;
  try {
    const pcbJson = JSON.stringify(pcb);
    const layersJson = JSON.stringify(layers);
    // Prefer the options-aware binding (theme/highlight/labels); fall back to
    // the 3-arg form on an older WASM build (which now also defaults to dark).
    svg =
      typeof wasm.render_pcb_svg_opts === "function"
        ? wasm.render_pcb_svg_opts(pcbJson, layersJson, SVG_SCALE, optsJson)
        : wasm.render_pcb_svg(pcbJson, layersJson, SVG_SCALE);
  } catch (e) {
    // A kernel trap here would otherwise leave the shared instance in an
    // undefined state and break every other session — recover it in place.
    if (e instanceof WebAssembly.RuntimeError) {
      resetKernelWasm(`render_pcb_svg trapped: ${e.message}`);
    }
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `PCB render failed: ${e instanceof Error ? e.message : String(e)}`,
            document_id: documentId,
            layers,
          }),
        },
      ],
      isError: true,
    };
  }

  const raster = await rasterize(svg, widthPx, theme === "light" ? "white" : "#0E1014");
  if (raster.png) {
    const asset = makePngRenderAsset(raster.png, {
      tool: "render_pcb",
      filename: `${documentId || "board"}-${layers.join("-")}-${widthPx}.png`,
      width: widthPx,
      alt: `PCB render of ${layers.join(", ")}`,
    });
    return withRenderAssets<RenderViewResult>({
      content: [
        { type: "image", data: raster.png.toString("base64"), mimeType: "image/png" },
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            layers,
            width_px: widthPx,
            theme,
            format: "png",
            asset: renderAssetSummary(asset),
            suggested_final_markdown: asset.markdown,
          }),
        },
      ],
    }, [asset]);
  }

  const note =
    raster.reason === "module-missing"
      ? "Install @resvg/resvg-js for PNG output; returning raw SVG."
      : `PNG ${raster.reason}; returning raw SVG.`;
  const svgBytes = Buffer.byteLength(svg, "utf8");
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: documentId,
          layers,
          format: "svg",
          note,
          ...(svgBytes <= MAX_INLINE_SVG_BYTES
            ? { svg }
            : { svg_omitted: `SVG is ${svgBytes} bytes (inline cap ${MAX_INLINE_SVG_BYTES}).` }),
        }),
      },
    ],
  };
}

// ───────────────────────────────────────────────────────────────────────────
// render_ratsnest — board render + airwire overlay (judge placement pre-route)
// ───────────────────────────────────────────────────────────────────────────

/** Margin (mm) the kernel PCB renderer pads the viewBox with — mirrored here so
 *  injected airwires share its pixel transform. Keep in sync with MARGIN_MM in
 *  crates/vcad-render/src/pcb.rs. */
const PCB_MARGIN_MM = 2.0;

/** Brand-pink airwire stroke. */
const RATSNEST_COLOR = "#F92672";

interface BoardBounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/** Replicates `board_bounds` in crates/vcad-render/src/pcb.rs: outline bbox,
 *  falling back to all geometry, then a fixed extent — so the TS-side airwire
 *  transform matches the kernel render exactly. */
function boardBounds(pcb: Pcb): BoardBounds {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  const add = (x: number, y: number) => {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  };
  const valid = () =>
    Number.isFinite(minX) && Number.isFinite(minY) && maxX >= minX && maxY >= minY;
  for (const v of pcb.outline.vertices) add(v.x, v.y);
  if (valid()) return { minX, minY, maxX, maxY };
  for (const t of pcb.traces) {
    add(t.start.x, t.start.y);
    add(t.end.x, t.end.y);
  }
  for (const v of pcb.vias) add(v.position.x, v.position.y);
  for (const fp of pcb.footprints) add(fp.position.x, fp.position.y);
  if (valid()) return { minX, minY, maxX, maxY };
  return { minX: 0, minY: 0, maxX: 100, maxY: 100 };
}

/** Build a pad-derived netlist for the kernel ratsnest (net → pin refs). */
function netlistFromPads(pcb: Pcb): NetlistResult {
  const m = new Map<string, Array<{ component_ref: string; pin_number: string }>>();
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      if (!pad.net) continue;
      const conns = m.get(pad.net) ?? [];
      conns.push({ component_ref: fp.ref, pin_number: pad.number });
      m.set(pad.net, conns);
    }
  }
  return { nets: [...m.entries()].map(([name, connections]) => ({ name, connections })) };
}

export const renderRatsnestSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id of a board (from create_schematic / place_components).",
    },
    layers: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        'Base layers drawn under the airwires. Default: ["F.Cu", "F.SilkS", "Edge_Cuts"].',
    },
    width_px: {
      type: "number" as const,
      description: "Target raster width in pixels (default 900, clamped 64–2048).",
    },
  },
  required: ["document_id"],
};

/**
 * Render the board with the unrouted-connection ratsnest (per-net MST airwires)
 * overlaid as dashed lines — so an agent can judge placement quality and
 * crossing density BEFORE committing to a route pass. Reuses the kernel ratsnest
 * and PCB renderer; airwires are injected in the renderer's own pixel transform.
 */
export async function renderRatsnest(
  args: Record<string, unknown>,
): Promise<RenderViewResult> {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);
  const pcb = docPcb(doc);
  if (!pcb) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: "document has no PCB — run place_components first (or open a board)",
            document_id: documentId,
          }),
        },
      ],
      isError: true,
    };
  }

  const validity = validatePcb(pcb);
  if (!validity.valid) {
    return pcbValidationError("render_ratsnest", validity, documentId) as unknown as RenderViewResult;
  }

  const layers =
    Array.isArray(args.layers) && args.layers.length > 0
      ? (args.layers as unknown[]).map(String)
      : ["F.Cu", "F.SilkS", "Edge_Cuts"];
  const widthRaw = Number(args.width_px ?? 900);
  const widthPx = Math.min(
    2048,
    Math.max(64, Number.isFinite(widthRaw) ? Math.round(widthRaw) : 900),
  );

  const wasm = (await getKernelWasm()) as unknown as {
    render_pcb_svg?: (pcbJson: string, layersJson: string, scale: number) => string;
  };
  if (typeof wasm.render_pcb_svg !== "function") {
    return {
      content: [
        {
          type: "text",
          text: "render_ratsnest unavailable: kernel WASM build predates render_pcb_svg — rebuild vcad-kernel-wasm.",
        },
      ],
      isError: true,
    };
  }

  let svg: string;
  try {
    svg = wasm.render_pcb_svg(JSON.stringify(pcb), JSON.stringify(layers), SVG_SCALE);
  } catch (e) {
    if (e instanceof WebAssembly.RuntimeError) {
      resetKernelWasm(`render_pcb_svg trapped: ${e.message}`);
    }
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `ratsnest render failed: ${e instanceof Error ? e.message : String(e)}`,
            document_id: documentId,
          }),
        },
      ],
      isError: true,
    };
  }

  // Compute the airwires and inject them in the kernel renderer's pixel space.
  const lines = await computeRatsnest(pcb, netlistFromPads(pcb));
  const b = boardBounds(pcb);
  const margin = PCB_MARGIN_MM * SVG_SCALE;
  const tx = (x: number) => (x - b.minX) * SVG_SCALE + margin;
  const ty = (y: number) => (b.maxY - y) * SVG_SCALE + margin;
  const dash = `${(SVG_SCALE * 1.5).toFixed(2)},${(SVG_SCALE * 1.0).toFixed(2)}`;
  let overlay = `<g stroke="${RATSNEST_COLOR}" stroke-width="${Math.max(0.6, SVG_SCALE * 0.35).toFixed(2)}" stroke-dasharray="${dash}" opacity="0.85" fill="none">`;
  for (const l of lines) {
    overlay += `<line x1="${tx(l.from.x).toFixed(2)}" y1="${ty(l.from.y).toFixed(2)}" x2="${tx(l.to.x).toFixed(2)}" y2="${ty(l.to.y).toFixed(2)}"/>`;
  }
  overlay += "</g>";
  svg = svg.replace("</svg>", `${overlay}</svg>`);

  const meta = {
    document_id: documentId,
    layers,
    width_px: widthPx,
    airwires: lines.length,
  };

  const raster = await rasterize(svg, widthPx);
  if (raster.png) {
    const asset = makePngRenderAsset(raster.png, {
      tool: "render_ratsnest",
      filename: `${documentId || "board"}-ratsnest-${widthPx}.png`,
      width: widthPx,
      alt: `PCB ratsnest render with ${lines.length} airwires`,
    });
    return withRenderAssets<RenderViewResult>({
      content: [
        { type: "image", data: raster.png.toString("base64"), mimeType: "image/png" },
        {
          type: "text",
          text: JSON.stringify({
            ...meta,
            format: "png",
            asset: renderAssetSummary(asset),
            suggested_final_markdown: asset.markdown,
          }),
        },
      ],
    }, [asset]);
  }
  const note =
    raster.reason === "module-missing"
      ? "Install @resvg/resvg-js for PNG output; returning raw SVG."
      : `PNG ${raster.reason}; returning raw SVG.`;
  const svgBytes = Buffer.byteLength(svg, "utf8");
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          ...meta,
          format: "svg",
          note,
          ...(svgBytes <= MAX_INLINE_SVG_BYTES
            ? { svg }
            : { svg_omitted: `SVG is ${svgBytes} bytes (inline cap ${MAX_INLINE_SVG_BYTES}).` }),
        }),
      },
    ],
  };
}

// ───────────────────────────────────────────────────────────────────────────
// render_stackup — one image per copper layer (read inner planes legibly)
// ───────────────────────────────────────────────────────────────────────────

const COPPER_LAYERS = [
  "FCu",
  "In1Cu",
  "In2Cu",
  "In3Cu",
  "In4Cu",
  "In5Cu",
  "In6Cu",
  "BCu",
] as const;

export const renderStackupSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id of a board (from create_schematic / place_components).",
    },
    width_px: {
      type: "number" as const,
      description: "Per-layer raster width in pixels (default 700, clamped 64–2048).",
    },
  },
  required: ["document_id"],
};

/**
 * Render each copper layer of a multilayer board to its own image (with the
 * board edge for framing) — so inner planes are legible instead of being
 * buried under an all-layers composite. Returns one image content block per
 * layer plus a text index mapping layer → image position.
 */
export async function renderStackup(
  args: Record<string, unknown>,
): Promise<RenderViewResult> {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);
  const pcb = docPcb(doc);
  if (!pcb) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: "document has no PCB — run place_components first (or open a board)",
            document_id: documentId,
          }),
        },
      ],
      isError: true,
    };
  }

  const validity = validatePcb(pcb);
  if (!validity.valid) {
    return pcbValidationError("render_stackup", validity, documentId) as unknown as RenderViewResult;
  }

  const widthRaw = Number(args.width_px ?? 700);
  const widthPx = Math.min(
    2048,
    Math.max(64, Number.isFinite(widthRaw) ? Math.round(widthRaw) : 700),
  );

  const wasm = (await getKernelWasm()) as unknown as {
    render_pcb_svg?: (pcbJson: string, layersJson: string, scale: number) => string;
  };
  if (typeof wasm.render_pcb_svg !== "function") {
    return {
      content: [
        {
          type: "text",
          text: "render_stackup unavailable: kernel WASM build predates render_pcb_svg — rebuild vcad-kernel-wasm.",
        },
      ],
      isError: true,
    };
  }

  // Copper layers actually present: from the stackup, falling back to the two
  // outer layers when the stackup is unspecified.
  const declared = new Set(pcb.stackup.layers.map((l) => String(l.layer)));
  const present = COPPER_LAYERS.filter((l) => declared.has(l));
  const copper: string[] = present.length > 0 ? [...present] : ["FCu", "BCu"];

  const pcbJson = JSON.stringify(pcb);
  const content: ContentBlock[] = [];
  const index: Array<{ layer: string; image_index: number }> = [];
  const assets: ReturnType<typeof makePngRenderAsset>[] = [];
  let resvgMissing = false;
  for (const layer of copper) {
    let svg: string;
    try {
      svg = wasm.render_pcb_svg(pcbJson, JSON.stringify([layer, "EdgeCuts"]), SVG_SCALE);
    } catch (e) {
      if (e instanceof WebAssembly.RuntimeError) {
        resetKernelWasm(`render_pcb_svg trapped: ${e.message}`);
      }
      continue;
    }
    const raster = await rasterize(svg, widthPx);
    if (raster.png) {
      index.push({ layer, image_index: content.length });
      content.push({
        type: "image",
        data: raster.png.toString("base64"),
        mimeType: "image/png",
      });
      assets.push(
        makePngRenderAsset(raster.png, {
          tool: "render_stackup",
          filename: `${documentId || "board"}-${layer}-${widthPx}.png`,
          width: widthPx,
          alt: `PCB ${layer} layer render`,
          role: "layer_render",
        }),
      );
    } else if (raster.reason === "module-missing") {
      resvgMissing = true;
      break;
    }
  }

  if (content.length === 0) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            format: "svg",
            note: resvgMissing
              ? "Install @resvg/resvg-js for per-layer PNG output."
              : "No copper layers rendered.",
            layers: copper,
          }),
        },
      ],
      isError: resvgMissing ? undefined : true,
    };
  }

  content.push({
    type: "text",
    text: JSON.stringify({
      document_id: documentId,
      width_px: widthPx,
      format: "png",
      layers: index,
      assets: assets.map(renderAssetSummary),
      suggested_final_markdown: assets.map((a) => a.markdown),
    }),
  });
  return withRenderAssets<RenderViewResult>({ content }, assets);
}

export const toolDefs: ToolDef[] = [
  {
    name: "render_view",
    pack: null,
    description:
      "Render an open session document to an isometric PNG image so you can SEE the current geometry — silhouettes, holes, creases — not just numbers. Drafting-style line art, Z-up, same renderer as the vcad CLI. Call after mutations to visually confirm the part matches intent before declaring done.",
    inputSchema: renderViewSchema,
    handler: async (a) => (await renderView(a)) as unknown as ToolResult,
    behavior: behavior({}),
  },
  {
    name: "render_pcb",
    pack: "ecad",
    description:
      "Render a flat, top-down, per-layer 2D image of a PCB (copper, silk, " +
      "drills, outline) — agent eyes for boards. Pick `layers` (e.g. " +
      "[\"F.Cu\", \"F.SilkS\", \"Edge_Cuts\"]); returns a PNG. Complements " +
      "the isometric render_view and numeric run_drc.",
    inputSchema: renderPcbSchema,
    handler: async (a) => (await renderPcb(a)) as unknown as ToolResult,
    behavior: behavior({}),
  },
  {
    name: "render_ratsnest",
    pack: "ecad",
    description:
      "Render the board with its unrouted-connection ratsnest (per-net MST " +
      "airwires) overlaid as dashed lines — judge placement quality and " +
      "crossing density BEFORE routing. Returns a PNG plus the airwire " +
      "(unconnected-pair) count.",
    inputSchema: renderRatsnestSchema,
    handler: async (a) => (await renderRatsnest(a)) as unknown as ToolResult,
    behavior: behavior({}),
  },
  {
    name: "render_stackup",
    pack: "ecad",
    description:
      "Render each copper layer of a multilayer board to its own image " +
      "(with the board edge for framing), so inner planes are legible " +
      "instead of buried under an all-layers composite. Returns one image " +
      "per layer plus a layer→image index.",
    inputSchema: renderStackupSchema,
    handler: async (a) => (await renderStackup(a)) as unknown as ToolResult,
    behavior: behavior({}),
  },
];
