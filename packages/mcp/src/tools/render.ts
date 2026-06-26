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

import { getKernelWasm, resetKernelWasm } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import type { Document, Pcb } from "@vcad/ir";
import { getSession } from "./session.js";

export const renderViewSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
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
  required: ["document_id"],
};

type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string };

interface RenderViewResult {
  content: ContentBlock[];
  isError?: boolean;
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
async function rasterize(
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
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);

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
    return {
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
          }),
        },
      ],
    };
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
    return {
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
          }),
        },
      ],
    };
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
