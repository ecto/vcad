/**
 * Guard against actions that would corrupt an in-flight recording.
 *
 * Centralized so every renderMode toggle gates the same way — a per-call-site
 * inline check is easy to miss, and the MediaRecorder will silently capture
 * a now-detached canvas if the user changes render mode mid-recording (the
 * stream is bound to one HTMLCanvasElement at start and can't be swapped).
 */

import { useRecordingStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";

/**
 * Returns true when the action can proceed. If a recording is in flight,
 * surfaces a toast and returns false.
 */
export function ensureNotRecording(
  action = "Switch render mode",
): boolean {
  const status = useRecordingStore.getState().status;
  if (status === "idle") return true;
  useNotificationStore
    .getState()
    .addToast(`${action} disabled while recording — stop recording first.`, "info");
  return false;
}
