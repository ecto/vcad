/**
 * AI camera tool executors — `focus_part`, `frame_all`, `set_view`.
 *
 * The AI participant is modelled as a separate user in the document. These
 * tools move the AI's camera (not the human user's) and record what the AI
 * is "looking at" in the AI participant's selection. The viewport renders
 * the AI camera as a frustum and tints the AI's selections in the AI's
 * color. Users can opt into Follow or Lock mode to make the view match.
 */

import {
  useDocumentStore,
  useEngineStore,
  useParticipantStore,
  ensureAiParticipant,
  AI_PARTICIPANT_ID,
  frameBbox,
  defaultCameraGoal,
  expandBboxFromPositions,
  isSnapView,
} from "@vcad/core";
import type {
  Bbox,
  CameraGoal,
  ExecutionResult,
  SnapView,
} from "@vcad/core";
import type { ToolCall } from "@/lib/chat-api";

/**
 * Union every part's mesh into a single bbox in kernel Z-up. Returns null
 * when the scene is empty or not yet evaluated.
 */
function computeSceneBbox(): Bbox | null {
  const scene = useEngineStore.getState().scene;
  if (!scene) return null;
  let box: Bbox | null = null;
  for (const part of scene.parts) {
    box = expandBboxFromPositions(box, part.mesh.positions);
  }
  if (scene.instances) {
    for (const inst of scene.instances) {
      box = expandBboxFromPositions(box, inst.mesh.positions);
    }
  }
  return box;
}

/** Union a single part's mesh into a bbox in kernel Z-up, or null if absent. */
function computePartBbox(partId: string): Bbox | null {
  const docStore = useDocumentStore.getState();
  const scene = useEngineStore.getState().scene;
  if (!scene) return null;

  // Legacy parts path: index into scene.parts by the matching docStore.parts index.
  const partIndex = docStore.parts.findIndex((p) => p.id === partId);
  if (partIndex >= 0) {
    const evalPart = scene.parts[partIndex];
    if (evalPart) return expandBboxFromPositions(null, evalPart.mesh.positions);
  }

  // Assembly-mode instances path: match by instance id.
  if (scene.instances) {
    for (const inst of scene.instances) {
      const id =
        (inst as { id?: string; instanceId?: string; partDefId?: string }).id ??
        (inst as { id?: string; instanceId?: string; partDefId?: string }).instanceId ??
        (inst as { id?: string; instanceId?: string; partDefId?: string }).partDefId;
      if (id === partId) {
        return expandBboxFromPositions(null, inst.mesh.positions);
      }
    }
  }

  return null;
}

/** Write the AI participant's camera + focus selection, creating it if needed. */
function setAiCamera(camera: CameraGoal, selection: string[] = []): void {
  ensureAiParticipant();
  const store = useParticipantStore.getState();
  store.setCamera(AI_PARTICIPANT_ID, camera);
  store.setSelection(AI_PARTICIPANT_ID, selection);
}

function exec(tool: ToolCall): ExecutionResult {
  switch (tool.name) {
    case "focus_part": {
      const partId = String(tool.args.part_id ?? "");
      if (!partId) {
        return { status: "error", result: "focus_part requires part_id." };
      }
      const docStore = useDocumentStore.getState();
      const partInfo = docStore.partIndex.get(partId);
      if (!partInfo) {
        const available = docStore.parts.map((p) => p.id).slice(0, 10).join(", ");
        return {
          status: "error",
          result: `Part "${partId}" not found. Available parts: [${available}]${
            docStore.parts.length > 10 ? ` (+${docStore.parts.length - 10} more)` : ""
          }`,
        };
      }
      const bbox = computePartBbox(partId) ?? computeSceneBbox();
      const camera = bbox ? frameBbox(bbox, { view: "iso" }) : defaultCameraGoal();
      setAiCamera(camera, [partId]);
      return {
        status: "success",
        result: `Focused on ${partInfo.name ?? partId}.`,
        display: {
          summary: [
            { type: "text", text: "Looking at " },
            { type: "partLink", partId, name: partInfo.name ?? partId.slice(-4) },
          ],
          affectedPartIds: [partId],
        },
      };
    }

    case "frame_all": {
      const bbox = computeSceneBbox();
      const camera = bbox ? frameBbox(bbox, { view: "iso" }) : defaultCameraGoal();
      setAiCamera(camera, []);
      return {
        status: "success",
        result: bbox ? "Framed the scene." : "Scene empty; framed the origin.",
        display: {
          summary: [{ type: "text", text: "Framed the scene." }],
        },
      };
    }

    case "set_view": {
      const name = String(tool.args.name ?? "");
      if (!isSnapView(name)) {
        return {
          status: "error",
          result: `Unknown view "${name}". Use one of: iso, hero, top, bottom, front, back, left, right.`,
        };
      }
      const view: SnapView = name;
      const bbox = computeSceneBbox();
      const camera = bbox
        ? frameBbox(bbox, { view })
        : frameBbox({ min: [-20, -20, -20], max: [20, 20, 20] }, { view });
      setAiCamera(camera, []);
      return {
        status: "success",
        result: `Set view to ${view}.`,
        display: {
          summary: [{ type: "text", text: `Set view to ${view}.` }],
        },
      };
    }

    default:
      return { status: "error", result: `Unknown camera tool "${tool.name}".` };
  }
}

/** Name set for the chat handler's dispatcher. */
export const AI_CAMERA_TOOL_NAMES = new Set([
  "focus_part",
  "frame_all",
  "set_view",
]);

/** Execute a camera tool call, measuring duration. */
export function executeAiCamera(tool: ToolCall): ExecutionResult {
  const t0 = performance.now();
  const result = exec(tool);
  result.duration = performance.now() - t0;
  return result;
}
