/**
 * Unified view of the app's current mode.
 *
 * Mirrors `vcad_app::mode::AppMode`. The scattered mode flags — sketch
 * active, electronics active, physics mode, drawing view mode — are
 * folded into one value by priority:
 *
 *   electronics  → Electronics   (whole-UI takeover, owns Esc)
 *   sketch       → Sketch        (modal overlay, sketch-escape on Esc)
 *   physics run  → Physics
 *   drawing 2D   → Drawing
 *   otherwise    → Normal
 *
 * CAM and slicer panels are *not* modes — they're overlays that open
 * alongside the normal viewport and don't claim keyboard input.
 *
 * The keybinding dispatcher reads this for mode-scope filtering. The
 * prefs panel uses it to scope conflict checks to the user's current
 * mode. Any other code that wants a single "what mode is this" selector
 * can share the same hook.
 */

import {
  useSketchStore,
  useSimulationStore,
  type KeybindingMode as AppMode,
} from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useDrawingStore } from "@/stores/drawing-store";

/** React hook: subscribes to the underlying stores so components re-render
 * when the mode changes. */
export function useAppMode(): AppMode {
  const electronicsActive = useElectronicsStore((s) => s.active);
  const sketchActive = useSketchStore((s) => s.active);
  const simMode = useSimulationStore((s) => s.mode);
  const drawingView = useDrawingStore((s) => s.viewMode);
  return computeAppMode({
    electronicsActive,
    sketchActive,
    simMode,
    drawingView,
  });
}

/** Non-reactive variant for use inside keydown / event handlers that read
 * stores via `getState()`. Same priority as [`useAppMode`]. */
export function readAppMode(): AppMode {
  return computeAppMode({
    electronicsActive: useElectronicsStore.getState().active,
    sketchActive: useSketchStore.getState().active,
    simMode: useSimulationStore.getState().mode,
    drawingView: useDrawingStore.getState().viewMode,
  });
}

interface ModeInputs {
  electronicsActive: boolean;
  sketchActive: boolean;
  simMode: ReturnType<typeof useSimulationStore.getState>["mode"];
  drawingView: "2d" | "3d";
}

function computeAppMode(inputs: ModeInputs): AppMode {
  if (inputs.electronicsActive) return "Electronics";
  if (inputs.sketchActive) return "Sketch";
  if (inputs.simMode === "running") return "Physics";
  if (inputs.drawingView === "2d") return "Drawing";
  return "Normal";
}
