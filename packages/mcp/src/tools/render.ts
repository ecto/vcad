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

import { getKernelWasm } from "@vcad/engine";
import { getSession } from "./session.js";

export const renderViewSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
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

/** Rasterize SVG to PNG via the optional resvg dependency.
 *  Returns null when the native module isn't installed/loadable. */
async function rasterize(svg: string, widthPx: number): Promise<Buffer | null> {
  try {
    const { Resvg } = await import("@resvg/resvg-js");
    const resvg = new Resvg(svg, {
      fitTo: { mode: "width", value: widthPx },
      background: "white",
    });
    return resvg.render().asPng();
  } catch {
    return null;
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

  const wasm = (await getKernelWasm()) as unknown as {
    render_svg: (vcadJson: string, scale: number) => string;
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
    svg = wasm.render_svg(JSON.stringify(doc), SVG_SCALE);
  } catch (e) {
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

  const png = await rasterize(svg, widthPx);
  if (png) {
    return {
      content: [
        { type: "image", data: png.toString("base64"), mimeType: "image/png" },
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            view: "isometric",
            width_px: widthPx,
            format: "png",
          }),
        },
      ],
    };
  }

  // Rasterizer missing — degrade to raw SVG text rather than failing.
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: documentId,
          view: "isometric",
          format: "svg",
          note: "Install @resvg/resvg-js for PNG output; returning raw SVG.",
          svg,
        }),
      },
    ],
  };
}
