/**
 * record_simulation — server-side physics-to-video for AI agents.
 *
 * Pairs an open session document with an active PhysicsEnv (from
 * create_robot_env), steps the env N times, mutates the cached document's
 * joint state values to match each observation, then re-renders the document
 * through the same kernel SVG path that powers `render_view`. Frames are
 * rasterized to PNG via the optional `@resvg/resvg-js` rasterizer and muxed
 * into an animated GIF via the optional `gifenc` encoder.
 *
 * Returns inline `image/gif` so the agent can SEE the run (same content
 * shape as render_view's PNG). When either optional dep is missing, the
 * tool degrades to a JSON summary listing the per-frame joint trajectory —
 * never a silent failure.
 */

import { getKernelWasm, resetKernelWasm } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { getSession } from "./session.js";
import { getSimulation } from "./gym.js";
import type { PhysicsActionType } from "@vcad/engine";

export const recordSimulationSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment id from create_robot_env.",
    },
    document_id: {
      type: "string" as const,
      description:
        "Session document id from open_document. Must describe the same assembly the env was created from (same joint order).",
    },
    steps: {
      type: "number" as const,
      description: "Number of simulation steps to record (1–600).",
    },
    action_type: {
      type: "string" as const,
      enum: ["torque", "position", "velocity"],
      description:
        "Action mode used for every recorded step. Defaults to 'torque' with all-zeros (passive playback under gravity).",
    },
    action: {
      type: "array" as const,
      items: { type: "number" as const },
      description:
        "Constant action vector applied every step (length = action_dim from create_robot_env). Mutually exclusive with `actions`.",
    },
    actions: {
      type: "array" as const,
      items: { type: "array" as const, items: { type: "number" as const } },
      description:
        "Per-step action vectors (length = steps; each inner array length = action_dim). Mutually exclusive with `action`.",
    },
    fps: {
      type: "number" as const,
      description: "Playback frame rate of the encoded GIF (1–60, default 30).",
    },
    view: {
      type: "string" as const,
      enum: ["iso", "isometric", "top", "front", "side"],
      description: "Camera view; defaults to 'iso'.",
    },
    width_px: {
      type: "number" as const,
      description: "Raster width per frame (64–1024, default 480).",
    },
  },
  required: ["env_id", "document_id", "steps"],
};

type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string };

interface RecordResult {
  content: ContentBlock[];
  isError?: boolean;
}

/** px-per-mm passed to the kernel renderer; the raster step handles
 *  final sizing, so this only controls SVG coordinate precision. */
const SVG_SCALE = 2.0;

/** Per-frame hard cap. Each frame holds the WASM instance synchronously and
 *  buffers a PNG in Node heap, so the budget is multiplicative. */
const MAX_STEPS = 600;
const MIN_STEPS = 1;
const DEFAULT_WIDTH_PX = 480;
const MIN_WIDTH_PX = 64;
const MAX_WIDTH_PX = 1024;
const DEFAULT_FPS = 30;
const MIN_FPS = 1;
const MAX_FPS = 60;

type RasterOutcome =
  | { rgba: Uint8Array; width: number; height: number }
  | { rgba: null; reason: "module-missing" | string };

/** Rasterize one SVG frame to RGBA pixels via the optional resvg dep. */
async function rasterize(
  svg: string,
  widthPx: number,
): Promise<RasterOutcome> {
  let ResvgCtor: typeof import("@resvg/resvg-js").Resvg;
  try {
    ({ Resvg: ResvgCtor } = await import("@resvg/resvg-js"));
  } catch (e) {
    const code = (e as NodeJS.ErrnoException)?.code;
    if (code === "ERR_MODULE_NOT_FOUND" || code === "MODULE_NOT_FOUND") {
      return { rgba: null, reason: "module-missing" };
    }
    return {
      rgba: null,
      reason: `resvg import failed: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
  try {
    const resvg = new ResvgCtor(svg, {
      fitTo: { mode: "width", value: widthPx },
      background: "white",
    });
    const rendered = resvg.render();
    const pixels = rendered.pixels;
    return {
      rgba: new Uint8Array(
        pixels.buffer,
        pixels.byteOffset,
        pixels.byteLength,
      ),
      width: rendered.width,
      height: rendered.height,
    };
  } catch (e) {
    return {
      rgba: null,
      reason: `rasterization failed: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

type GifencModule = typeof import("gifenc");

/** Lazy-load the optional `gifenc` encoder. Returns null with a reason
 *  when the module is missing or import-time fails. */
async function loadGifenc(): Promise<
  { mod: GifencModule } | { mod: null; reason: string }
> {
  try {
    const mod = (await import("gifenc")) as unknown as GifencModule;
    return { mod };
  } catch (e) {
    const code = (e as NodeJS.ErrnoException)?.code;
    if (code === "ERR_MODULE_NOT_FOUND" || code === "MODULE_NOT_FOUND") {
      return { mod: null, reason: "module-missing" };
    }
    return {
      mod: null,
      reason: `gifenc import failed: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

export async function recordSimulation(
  args: Record<string, unknown>,
): Promise<RecordResult> {
  const envId = String(args.env_id ?? "");
  const documentId = String(args.document_id ?? "");

  const stepsRaw = Number(args.steps ?? 0);
  if (!Number.isFinite(stepsRaw)) {
    return errorResult(
      `record_simulation refused: 'steps' must be a finite number`,
    );
  }
  const steps = Math.min(
    MAX_STEPS,
    Math.max(MIN_STEPS, Math.round(stepsRaw)),
  );

  const widthRaw = Number(args.width_px ?? DEFAULT_WIDTH_PX);
  const widthPx = Math.min(
    MAX_WIDTH_PX,
    Math.max(
      MIN_WIDTH_PX,
      Number.isFinite(widthRaw) ? Math.round(widthRaw) : DEFAULT_WIDTH_PX,
    ),
  );

  const fpsRaw = Number(args.fps ?? DEFAULT_FPS);
  const fps = Math.min(
    MAX_FPS,
    Math.max(MIN_FPS, Number.isFinite(fpsRaw) ? Math.round(fpsRaw) : DEFAULT_FPS),
  );

  const viewRaw = String(args.view ?? "iso").toLowerCase();
  const view = (
    viewRaw === "isometric"
      ? "iso"
      : ["iso", "top", "front", "side"].includes(viewRaw)
        ? viewRaw
        : "iso"
  ) as "iso" | "top" | "front" | "side";
  const viewLabel = view === "iso" ? "isometric" : view;

  const actionType = (args.action_type ?? "torque") as PhysicsActionType;
  if (!["torque", "position", "velocity"].includes(actionType)) {
    return errorResult(
      `record_simulation refused: action_type '${actionType}' is not one of torque|position|velocity`,
    );
  }

  // Look up the env and document. Both must already exist; this tool does
  // not create either, by design — the agent's expected flow is
  //   open_document → create_robot_env → record_simulation
  // so failures here mean a typo, an expired session, or a closed env.
  const env = getSimulation(envId);
  if (!env) {
    return errorResult(`Unknown env_id: ${envId}`);
  }

  let doc: Document;
  try {
    doc = getSession(documentId);
  } catch (e) {
    return errorResult(
      `Unknown document_id: ${documentId} (${e instanceof Error ? e.message : String(e)})`,
    );
  }

  if (!doc.joints || doc.joints.length === 0) {
    return errorResult(
      `document_id ${documentId} has no joints — nothing to animate.`,
    );
  }
  if (doc.joints.length !== env.numJoints) {
    return errorResult(
      `joint-count mismatch: env_id has ${env.numJoints} joints, document_id has ${doc.joints.length}. The env must have been created from a different assembly.`,
    );
  }

  // Resolve the action vector(s). `actions` (per-step) wins over `action`
  // (constant); falling back to all-zeros means "passive playback".
  const perStepActions = resolveActions(args, env.actionDim, steps);
  if ("error" in perStepActions) {
    return errorResult(perStepActions.error);
  }

  const wasm = (await getKernelWasm()) as unknown as {
    render_svg: (vcadJson: string, scale: number) => string;
    render_svg_view?: (vcadJson: string, scale: number, view: string) => string;
  };
  if (typeof wasm.render_svg !== "function") {
    return errorResult(
      "record_simulation unavailable: kernel WASM build predates render_svg — rebuild @vcad/kernel-wasm.",
    );
  }

  // Pre-flight: load the GIF encoder so we can degrade early if it's
  // missing, instead of stepping the env first and discarding the work.
  const gifLoad = await loadGifenc();

  // Deep-copy the document for stepping. The session doc is shared mutable
  // state — if a second tool call lands while we're iterating, our restore
  // would clobber their writes. Operating on a clone keeps the session doc
  // untouched throughout, and the agent can always inspect the env via
  // gym_observe afterward to see the final pose.
  const docClone: Document = JSON.parse(JSON.stringify(doc));

  const jointTrajectory: number[][] = [];
  const delayMs = Math.max(1, Math.round(1000 / fps));

  let rasterDegraded: string | null = null;
  let firstFrameSize: { width: number; height: number } | null = null;
  let encoder: ReturnType<GifencModule["GIFEncoder"]> | null = null;
  if (gifLoad.mod) encoder = gifLoad.mod.GIFEncoder();

  let framesEncoded = 0;
  for (let s = 0; s < steps; s++) {
    const result = env.step(actionType, perStepActions.values[s]!);
    const obs = result.observation;

    for (let j = 0; j < docClone.joints!.length; j++) {
      const pos = obs.joint_positions[j];
      if (typeof pos === "number") docClone.joints![j]!.state = pos;
    }
    jointTrajectory.push([...obs.joint_positions]);

    let svg: string;
    try {
      svg =
        view !== "iso" && typeof wasm.render_svg_view === "function"
          ? wasm.render_svg_view(JSON.stringify(docClone), SVG_SCALE, view)
          : wasm.render_svg(JSON.stringify(docClone), SVG_SCALE);
    } catch (e) {
      if (e instanceof WebAssembly.RuntimeError) {
        resetKernelWasm(`render_svg trapped during record_simulation: ${e.message}`);
        return errorResult(
          `kernel trap during frame ${s + 1}/${steps}: ${e.message}`,
        );
      }
      return errorResult(
        `render failed at frame ${s + 1}/${steps}: ${e instanceof Error ? e.message : String(e)}`,
      );
    }

    const raster = await rasterize(svg, widthPx);
    if (!raster.rgba) {
      rasterDegraded =
        raster.reason === "module-missing"
          ? "Install @resvg/resvg-js for rasterization."
          : `Rasterization failed: ${raster.reason}.`;
      break;
    }
    if (!firstFrameSize) {
      firstFrameSize = { width: raster.width, height: raster.height };
    }

    // Stream the frame into the encoder so we never hold more than one
    // decoded RGBA buffer + the indexed bytes at once. Without this the
    // peak memory at MAX_STEPS×MAX_WIDTH_PX would be in the gigabytes.
    if (encoder && gifLoad.mod) {
      const palette = gifLoad.mod.quantize(raster.rgba, 256);
      const indexed = gifLoad.mod.applyPalette(raster.rgba, palette);
      encoder.writeFrame(indexed, raster.width, raster.height, {
        palette,
        delay: delayMs,
      });
      framesEncoded++;
    }
  }

  if (rasterDegraded || !encoder) {
    const note =
      rasterDegraded ??
      ("reason" in gifLoad && gifLoad.reason === "module-missing"
        ? "Install `gifenc` to receive an animated GIF; returning joint trajectory only."
        : "reason" in gifLoad
          ? `GIF ${gifLoad.reason}; returning joint trajectory only.`
          : "No frames captured.");
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            {
              env_id: envId,
              document_id: documentId,
              steps: jointTrajectory.length,
              fps,
              view: viewLabel,
              format: "json",
              note,
              joint_trajectory: jointTrajectory,
            },
            null,
            2,
          ),
        },
      ],
      isError: rasterDegraded !== null,
    };
  }

  encoder.finish();
  const gif = encoder.bytes();

  return {
    content: [
      {
        type: "image",
        data: Buffer.from(gif).toString("base64"),
        mimeType: "image/gif",
      },
      {
        type: "text",
        text: JSON.stringify(
          {
            env_id: envId,
            document_id: documentId,
            steps: framesEncoded,
            fps,
            view: viewLabel,
            width_px: firstFrameSize?.width ?? widthPx,
            height_px: firstFrameSize?.height ?? null,
            format: "gif",
            duration_s: Math.round((framesEncoded / fps) * 100) / 100,
          },
          null,
          2,
        ),
      },
    ],
  };
}

function resolveActions(
  args: Record<string, unknown>,
  actionDim: number,
  steps: number,
): { values: number[][] } | { error: string } {
  const perStep = args.actions;
  const constant = args.action;

  if (perStep !== undefined && constant !== undefined) {
    return {
      error: "Pass either `action` or `actions`, not both.",
    };
  }

  if (Array.isArray(perStep)) {
    if (perStep.length !== steps) {
      return {
        error: `'actions' length ${perStep.length} does not match steps=${steps}.`,
      };
    }
    const out: number[][] = [];
    for (let i = 0; i < perStep.length; i++) {
      const row = perStep[i];
      if (!Array.isArray(row) || row.length !== actionDim) {
        return {
          error: `'actions[${i}]' must be an array of length ${actionDim} (got ${Array.isArray(row) ? `length ${row.length}` : typeof row}).`,
        };
      }
      const checked = coerceFiniteVector(row, `actions[${i}]`);
      if ("error" in checked) return checked;
      out.push(checked.values);
    }
    return { values: out };
  }

  let vec: number[];
  if (Array.isArray(constant)) {
    if (constant.length !== actionDim) {
      return {
        error: `'action' length ${constant.length} does not match action_dim=${actionDim}.`,
      };
    }
    const checked = coerceFiniteVector(constant, "action");
    if ("error" in checked) return checked;
    vec = checked.values;
  } else {
    vec = new Array(actionDim).fill(0);
  }
  const values: number[][] = new Array(steps);
  for (let i = 0; i < steps; i++) values[i] = vec;
  return { values };
}

/** Validate that every element coerces to a finite number. NaN and ±Infinity
 *  silently propagate through the phyz solver and surface as corrupted joint
 *  states downstream, so we reject them at the boundary. */
function coerceFiniteVector(
  row: unknown[],
  label: string,
): { values: number[] } | { error: string } {
  const out = new Array<number>(row.length);
  for (let j = 0; j < row.length; j++) {
    const n = Number(row[j]);
    if (!Number.isFinite(n)) {
      return {
        error: `'${label}[${j}]' must be a finite number (got ${JSON.stringify(row[j])}).`,
      };
    }
    out[j] = n;
  }
  return { values: out };
}

function errorResult(text: string): RecordResult {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ error: text }),
      },
    ],
    isError: true,
  };
}
