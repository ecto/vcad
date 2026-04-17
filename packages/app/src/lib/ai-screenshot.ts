import {
  AI_PARTICIPANT_ID,
  defaultCameraGoal,
  frameBbox,
  isSnapView,
  useParticipantStore,
} from "@vcad/core";
import type { AnthropicTool, CameraGoal, SnapView } from "@vcad/core";
import { computeSceneBbox, setAiCamera } from "@/lib/ai-camera-tools";

/** Named camera presets the screenshot tool can target. Subset of SnapView. */
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
    "Capture a screenshot from YOUR camera (not the user's) so you can visually verify your work. The user's viewport is not disturbed — they see this angle as your wireframe frustum if they're watching. Pass an optional `view` (front/back/left/right/top/bottom/iso/hero) to reframe your camera first; omit it to shoot from wherever you last aimed your camera with set_view / frame_all / focus_part. Each screenshot costs tokens, so don't spam — one iso shot after a multi-step task is usually enough.",
  input_schema: {
    type: "object",
    properties: {
      view: {
        type: "string",
        enum: [...SCREENSHOT_VIEW_NAMES],
        description:
          "Optional named camera angle. If provided, your camera is reframed to this angle (fitted to the scene) before the shot. If omitted, your camera stays where set_view / frame_all / focus_part left it.",
      },
    },
  },
};

/** System-prompt addendum describing when to reach for the screenshot tool.
 * Appended to the main commandRegistry prompt client-side so the core package
 * doesn't need to know about an app-only capability. */
export const SCREENSHOT_SYSTEM_PROMPT_APPENDIX = `

## Visual verification

You have a screenshot_viewport tool that captures a JPEG from YOUR camera — not the user's. The user's view is untouched; they just see your camera's wireframe frustum overlay in their scene, so they know what angle you're looking at.

Typical flow: aim your camera first (set_view / frame_all / focus_part), then call screenshot_viewport with no args to shoot what you're aimed at. Or pass a view name to reframe + shoot in one step.

Use it to verify non-trivial work — after building a multi-part assembly, before reporting success on a complex model, or when you're unsure whether an operation produced the intended shape. You can call it multiple times with different angles if something looks off. Don't use it for trivial single-primitive requests. Each screenshot consumes tokens, so be judicious.`;

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

/** Delay (ms) after reframing the AI camera before capturing — the frustum
 * overlay lerps toward its goal; this lets the user see the move happen.
 * The offscreen capture itself doesn't need the lerp to finish (we pass the
 * goal directly to the renderer), but the user-facing frustum looks natural
 * if the shot happens just after it has started moving. */
const AIM_SETTLE_MS = 250;

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function nextFrame(): Promise<void> {
  return new Promise((r) => requestAnimationFrame(() => r()));
}

function findViewportCanvas(): HTMLCanvasElement | null {
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

/** Pick the camera goal the screenshot should render from.
 *
 * - If `view` is provided, compute a bbox-fitted goal for that snap view and
 *   also write it to the AI participant so the user sees the frustum move.
 * - Otherwise use the AI's current camera if it has one, else frame_all + iso.
 */
function resolveGoal(view: ScreenshotView | undefined): CameraGoal {
  if (view) {
    const snap: SnapView = isSnapView(view) ? view : "iso";
    const bbox = computeSceneBbox();
    const goal = bbox ? frameBbox(bbox, { view: snap }) : defaultCameraGoal();
    setAiCamera(goal);
    return goal;
  }
  const existing =
    useParticipantStore.getState().participants.get(AI_PARTICIPANT_ID)?.camera;
  if (existing) return existing;
  const bbox = computeSceneBbox();
  const goal = bbox ? frameBbox(bbox, { view: "iso" }) : defaultCameraGoal();
  setAiCamera(goal);
  return goal;
}

type CaptureFn = (goal: CameraGoal) => HTMLCanvasElement | null;

function getCaptureFn(): CaptureFn | null {
  const win = window as unknown as { __vcadCaptureAiCamera?: CaptureFn };
  return win.__vcadCaptureAiCamera ?? null;
}

export async function executeScreenshotViewport(
  args: Record<string, unknown>,
): Promise<ScreenshotExecutionResult> {
  const t0 = performance.now();
  const viewArg = args.view as string | undefined;
  if (viewArg && !SCREENSHOT_VIEW_NAMES.includes(viewArg as ScreenshotView)) {
    return {
      status: "error",
      result: `screenshot_viewport: view must be one of ${SCREENSHOT_VIEW_NAMES.join(", ")}`,
      toolResultContent: null,
      imageDataUrl: null,
      duration: performance.now() - t0,
    };
  }

  const capture = getCaptureFn();
  if (!capture) {
    return {
      status: "error",
      result: "screenshot_viewport: AI camera renderer not mounted",
      toolResultContent: null,
      imageDataUrl: null,
      duration: performance.now() - t0,
    };
  }

  try {
    const goal = resolveGoal(viewArg as ScreenshotView | undefined);

    // Small settle window so the user's frustum overlay starts moving before
    // the shot, and so any just-committed scene changes (material swaps, new
    // geometry) have been flushed into the R3F scene graph.
    await sleep(AIM_SETTLE_MS);
    await nextFrame();
    await nextFrame();

    const rendered = capture(goal);
    if (!rendered) {
      return {
        status: "error",
        result: "screenshot_viewport: capture failed",
        toolResultContent: null,
        imageDataUrl: null,
        duration: performance.now() - t0,
      };
    }

    const { base64, mediaType, dataUrl, width, height } =
      encodeCanvasAsJpeg(rendered);

    const viewLabel = viewArg ?? "current";
    return {
      status: "success",
      result: `Captured ${viewLabel} view (${width}×${height})`,
      toolResultContent: [
        {
          type: "image",
          source: { type: "base64", media_type: mediaType, data: base64 },
        },
        {
          type: "text",
          text: `Screenshot from your camera (${viewLabel}). This is YOUR view; the user's viewport is unchanged but they see this angle as your frustum overlay. Use this to verify the document state and then continue.`,
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
