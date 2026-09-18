/**
 * Whether the CAM panel is on screen, and nothing else.
 *
 * This store used to carry a whole parallel CAM model — a tool library, hand-typed
 * rectangular operations, a settings block, a toolpath and a G-code string — because
 * the panel drove the kernel's granular WASM bindings one operation at a time and had
 * to assemble a job itself. Nothing replayed the result against the part, so nothing
 * could refuse it; and the browser's `Contour2D::offset_contour` was offsetting the
 * *bounding box*, so an "inside contour" could cut outward.
 *
 * All of that now lives in `cam-job-store.ts`, over `vcad-cam-api` through
 * `@vcad/engine`'s typed client — the same request schema the native app posts
 * through its C ABI and an agent posts through MCP. What is left here is the panel
 * toggle the menu and the command palette reach for.
 */

import { create } from "zustand";

interface CamStore {
  camPanelOpen: boolean;
  openCamPanel: () => void;
  closeCamPanel: () => void;
}

export const useCamStore = create<CamStore>((set) => ({
  camPanelOpen: false,
  openCamPanel: () => set({ camPanelOpen: true }),
  closeCamPanel: () => set({ camPanelOpen: false }),
}));

/** Format a duration in seconds the way a machinist reads a cycle time. */
export function formatMachiningTime(seconds: number): string {
  if (!Number.isFinite(seconds)) return "—";
  if (seconds < 60) {
    return `${Math.round(seconds)}s`;
  }
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.round(seconds % 60);
  if (minutes < 60) {
    return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return `${hours}h ${remainingMinutes}m`;
}
