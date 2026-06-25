/**
 * Recording store for capturing the viewport canvas to video.
 *
 * Drives the simulate-toolbar Record button and the `useCanvasRecorder` hook.
 * The hook owns the `MediaRecorder` instance and the captured Blob chunks —
 * this store only tracks lifecycle state so the UI can reflect it and so the
 * hook can react to start/stop intents from anywhere in the app.
 */

import { create } from "zustand";

/**
 * Recording lifecycle:
 *   idle     → no recording, no recorder.
 *   recording → toolbar requested start; hook spins up MediaRecorder.
 *   paused   → MediaRecorder.pause() called (sim is paused).
 *   saving   → toolbar (or sim-stop) requested finalize; hook flushes and
 *              calls reset() once the file is written.
 */
export type RecordingStatus = "idle" | "recording" | "paused" | "saving";

export interface RecordingState {
  status: RecordingStatus;
  /** ms since epoch when recording started; null when idle */
  startedAt: number | null;
  /** Negotiated MIME container (hook reports back after starting MediaRecorder) */
  mimeType: string | null;
  /** Bytes received from MediaRecorder dataavailable chunks so far */
  bytesRecorded: number;
  /** User-visible error message from the last attempt, if any */
  error: string | null;

  /** Toolbar intent: begin recording. Hook reacts and starts MediaRecorder. */
  start: () => void;
  /** Toolbar intent: finalize. Hook stops MediaRecorder and saves the file. */
  stop: () => void;
  /** Hook callback: MediaRecorder has paused. */
  pause: () => void;
  /** Hook callback: MediaRecorder has resumed. */
  resume: () => void;
  /** Hook callback: MediaRecorder started successfully. */
  setMimeType: (mime: string) => void;
  /** Hook callback: dataavailable produced a chunk. */
  addBytes: (n: number) => void;
  /** Reset to idle (hook calls this after the file is written). */
  reset: () => void;
  /** Hook reports failure; status returns to idle. */
  setError: (message: string) => void;
}

export const useRecordingStore = create<RecordingState>((set) => ({
  status: "idle",
  startedAt: null,
  mimeType: null,
  bytesRecorded: 0,
  error: null,

  start: () =>
    set((s) =>
      s.status === "idle"
        ? {
            status: "recording",
            startedAt: Date.now(),
            mimeType: null,
            bytesRecorded: 0,
            error: null,
          }
        : s,
    ),

  stop: () =>
    set((s) =>
      s.status === "recording" || s.status === "paused"
        ? { status: "saving" }
        : s,
    ),

  pause: () =>
    set((s) => (s.status === "recording" ? { status: "paused" } : s)),

  resume: () =>
    set((s) => (s.status === "paused" ? { status: "recording" } : s)),

  setMimeType: (mimeType) => set({ mimeType }),

  addBytes: (n) => set((s) => ({ bytesRecorded: s.bytesRecorded + n })),

  reset: () =>
    set({
      status: "idle",
      startedAt: null,
      mimeType: null,
      bytesRecorded: 0,
    }),

  setError: (message) =>
    set({
      status: "idle",
      startedAt: null,
      mimeType: null,
      error: message,
    }),
}));
