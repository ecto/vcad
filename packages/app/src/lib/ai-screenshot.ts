import type { AnthropicTool } from "@vcad/core";

/** Named camera presets the screenshot tool can target. Must match the
 * handler in ViewportContent.tsx (`vcad:snap-view` listener). */
export const SCREENSHOT_VIEW_NAMES = [
  "front",
  "back",
  "left",
  "right",
  "top",
  "bottom",
  "iso",
  "hero",
] as const;
export type ScreenshotView = (typeof SCREENSHOT_VIEW_NAMES)[number];

export const SCREENSHOT_VIEWPORT_TOOL: AnthropicTool = {
  name: "screenshot_viewport",
  description:
    "Capture a screenshot of the 3D viewport from a named camera angle so you can visually verify your work. The camera animates smoothly to the angle before the shot is taken. Views: front, back, left, right, top, bottom, iso (isometric 3/4 view), hero (dramatic presentation angle). Each screenshot costs tokens, so don't spam — one iso shot after a multi-step task is usually enough.",
  input_schema: {
    type: "object",
    properties: {
      view: {
        type: "string",
        enum: [...SCREENSHOT_VIEW_NAMES],
        description: "Named camera angle to capture from.",
      },
    },
    required: ["view"],
  },
};

/** System-prompt addendum describing when to reach for the screenshot tool.
 * Appended to the main commandRegistry prompt client-side so the core package
 * doesn't need to know about an app-only capability. */
export const SCREENSHOT_SYSTEM_PROMPT_APPENDIX = `

## Visual verification

You have a screenshot_viewport tool that captures the live 3D viewport from a named angle (front, back, left, right, top, bottom, iso, hero). Use it to visually verify non-trivial work — after building a multi-part assembly, before reporting success on a complex model, or when you're unsure whether an operation produced the intended shape. You can call it multiple times with different angles if something looks off and you need another view. Don't use it for trivial single-primitive requests. Each screenshot consumes tokens, so be judicious.`;

export interface ScreenshotExecutionResult {
  status: "success" | "error";
  /** Short human-readable summary shown on the UI chip. */
  result: string;
  /** Anthropic tool_result content blocks (image + text), or null on error. */
  toolResultContent: object[] | null;
  /** Full `data:image/jpeg;base64,...` URL for UI preview, or null on error. */
  imageDataUrl: string | null;
  /** Duration in ms. */
  duration: number;
}

/** Max dimension (px) of the delivered image on its longest side. Keeps
 * token usage low — Anthropic bills image input roughly per pixel. */
const MAX_IMAGE_DIM = 1024;

/** Camera animation + render settle window in ms. The viewport uses a lerp
 * that converges in a few dozen frames; 700 ms is a safe upper bound. */
const SETTLE_MS = 700;

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function nextFrame(): Promise<void> {
  return new Promise((r) => requestAnimationFrame(() => r()));
}

function findViewportCanvas(): HTMLCanvasElement | null {
  // The Viewport div is tagged with data-viewport-root; R3F renders its
  // canvas as a descendant. Target that specifically so we don't pick up
  // the schematic overlay or the 2D drawing canvas.
  const root = document.querySelector<HTMLElement>("[data-viewport-root]");
  if (!root) return null;
  return root.querySelector<HTMLCanvasElement>("canvas");
}

interface EncodedCanvas {
  base64: string;
  mediaType: string;
  dataUrl: string;
  width: number;
  height: number;
}

function downscaleToCanvas(canvas: HTMLCanvasElement): HTMLCanvasElement {
  const scale = Math.min(
    1,
    MAX_IMAGE_DIM / Math.max(canvas.width, canvas.height),
  );
  const outW = Math.max(1, Math.round(canvas.width * scale));
  const outH = Math.max(1, Math.round(canvas.height * scale));

  const tmp = document.createElement("canvas");
  tmp.width = outW;
  tmp.height = outH;
  const ctx = tmp.getContext("2d");
  if (!ctx) throw new Error("2D context unavailable");
  // Fill with a neutral backdrop first — the viewport normally paints every
  // pixel, but if anything ever leaves transparent regions we don't want
  // JPEG encoding them as pure black.
  ctx.fillStyle = "#1a1a1a";
  ctx.fillRect(0, 0, outW, outH);
  ctx.drawImage(canvas, 0, 0, outW, outH);
  return tmp;
}

function encodeCanvasAsJpeg(canvas: HTMLCanvasElement): EncodedCanvas {
  const tmp = downscaleToCanvas(canvas);
  const dataUrl = tmp.toDataURL("image/jpeg", 0.78);
  const comma = dataUrl.indexOf(",");
  if (comma < 0) throw new Error("Invalid data URL");
  return {
    base64: dataUrl.slice(comma + 1),
    mediaType: "image/jpeg",
    dataUrl,
    width: tmp.width,
    height: tmp.height,
  };
}

/** Capture the current viewport as a JPEG File, without touching the camera.
 * Used by the chat-input attach-viewport button so the user can aim the camera
 * themselves before grabbing the shot. Returns null if the viewport canvas
 * isn't mounted or encoding fails. */
export async function captureViewportAsFile(): Promise<File | null> {
  const canvas = findViewportCanvas();
  if (!canvas) return null;
  try {
    const tmp = downscaleToCanvas(canvas);
    const blob = await new Promise<Blob | null>((resolve) => {
      tmp.toBlob((b) => resolve(b), "image/jpeg", 0.78);
    });
    if (!blob) return null;
    const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
    return new File([blob], `viewport-${stamp}.jpg`, {
      type: "image/jpeg",
      lastModified: Date.now(),
    });
  } catch {
    return null;
  }
}

export async function executeScreenshotViewport(
  args: Record<string, unknown>,
): Promise<ScreenshotExecutionResult> {
  const t0 = performance.now();
  const view = args.view as string | undefined;
  if (!view || !SCREENSHOT_VIEW_NAMES.includes(view as ScreenshotView)) {
    return {
      status: "error",
      result: `screenshot_viewport: view must be one of ${SCREENSHOT_VIEW_NAMES.join(", ")}`,
      toolResultContent: null,
      imageDataUrl: null,
      duration: performance.now() - t0,
    };
  }

  try {
    // Dispatch the existing snap-view event. ViewportContent listens on
    // window and animates the camera via the same code path used by the
    // desktop view menu.
    window.dispatchEvent(
      new CustomEvent("vcad:snap-view", { detail: view }),
    );

    // Wait for the lerp animation to settle, then two rAFs so the final
    // frame is definitely in the drawing buffer (preserveDrawingBuffer is
    // enabled on the R3F Canvas so the last rendered frame survives read).
    await sleep(SETTLE_MS);
    await nextFrame();
    await nextFrame();

    const canvas = findViewportCanvas();
    if (!canvas) {
      return {
        status: "error",
        result: "screenshot_viewport: viewport canvas not found",
        toolResultContent: null,
        imageDataUrl: null,
        duration: performance.now() - t0,
      };
    }

    const { base64, mediaType, dataUrl, width, height } =
      encodeCanvasAsJpeg(canvas);

    return {
      status: "success",
      result: `Captured ${view} view (${width}×${height})`,
      toolResultContent: [
        {
          type: "image",
          source: { type: "base64", media_type: mediaType, data: base64 },
        },
        {
          type: "text",
          text: `Screenshot from ${view} view. Use this to verify the current document state and then continue.`,
        },
      ],
      imageDataUrl: dataUrl,
      duration: performance.now() - t0,
    };
  } catch (err) {
    return {
      status: "error",
      result: `screenshot_viewport: ${err instanceof Error ? err.message : "capture failed"}`,
      toolResultContent: null,
      imageDataUrl: null,
      duration: performance.now() - t0,
    };
  }
}
