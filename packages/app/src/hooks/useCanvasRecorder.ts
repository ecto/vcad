/**
 * Records the viewport to a video file via MediaRecorder.
 *
 * Capture target depends on render mode:
 *   - standard mode  → R3F's WebGL canvas (via `useThree().gl.domElement`)
 *   - raytrace mode  → the raytrace overlay canvas (sits on top of WebGL)
 *
 * In raytrace mode the recorder also drives a per-animation-frame raytrace
 * render via `triggerRaytraceRender()`. The normal scheduler in
 * `RayTracedViewport` only re-renders on camera change, so animation would
 * otherwise produce a frozen file.
 *
 * Lifecycle is driven by `useRecordingStore` — the toolbar calls `start()`
 * and `stop()`, this hook spins MediaRecorder up and down accordingly. Sim
 * mode is observed so pause/resume mirror the sim state and the sim Stop
 * button auto-finalizes the recording.
 */

import { useEffect, useRef } from "react";
import { useThree } from "@react-three/fiber";
import {
  useDocumentStore,
  useRecordingStore,
  useSimulationStore,
  useUiStore,
  logger,
} from "@vcad/core";
import {
  getRayTracedOverlayCanvas,
  triggerRaytraceRender,
} from "@/components/RayTracedViewport";
import { downloadBlob } from "@/lib/download";

const MIME_PREFERENCES = [
  "video/mp4;codecs=avc1.640028",
  "video/mp4;codecs=avc1.42E01E",
  "video/mp4",
  "video/webm;codecs=vp9",
  "video/webm;codecs=vp8",
  "video/webm",
];

const VIDEO_BITRATE = 12_000_000;
const CAPTURE_FPS = 60;
const CHUNK_MS = 250;

function pickMimeType(): string | null {
  if (typeof MediaRecorder === "undefined") return null;
  for (const mime of MIME_PREFERENCES) {
    if (MediaRecorder.isTypeSupported(mime)) return mime;
  }
  return null;
}

function extensionFor(mimeType: string): string {
  if (mimeType.startsWith("video/mp4")) return "mp4";
  if (mimeType.startsWith("video/webm")) return "webm";
  return "bin";
}

function slugify(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 60) || "untitled"
  );
}

function timestampForFilename(): string {
  // 2026-06-25T18-04-12 — colon-free so all OSes accept it.
  return new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
}

interface RecorderRefs {
  recorder: MediaRecorder | null;
  chunks: Blob[];
  mimeType: string | null;
  rafId: number | null;
  raytraceDriving: boolean;
}

/**
 * Hook to manage video recording lifecycle. Must be called inside the R3F
 * Canvas tree (uses `useThree` to get the WebGL canvas).
 */
export function useCanvasRecorder() {
  const { gl } = useThree();
  const status = useRecordingStore((s) => s.status);
  const renderMode = useUiStore((s) => s.renderMode);
  const simMode = useSimulationStore((s) => s.mode);

  const refs = useRef<RecorderRefs>({
    recorder: null,
    chunks: [],
    mimeType: null,
    rafId: null,
    raytraceDriving: false,
  });

  // React to recording status intents from the store.
  useEffect(() => {
    const store = useRecordingStore.getState();
    const cur = refs.current;

    // Intent: start. Store has flipped to "recording" but we haven't built
    // a MediaRecorder yet — do that now.
    if (status === "recording" && !cur.recorder) {
      const mimeType = pickMimeType();
      if (!mimeType) {
        store.setError("MediaRecorder is not supported in this browser.");
        return;
      }

      const overlay = getRayTracedOverlayCanvas();
      const useOverlay = renderMode === "raytrace" && overlay !== null;
      if (renderMode === "raytrace" && !useOverlay) {
        logger.warn(
          "app",
          "Raytrace overlay not mounted at record start; falling back to WebGL canvas.",
        );
      }
      const canvas = useOverlay ? overlay! : gl.domElement;

      const captureSource = canvas as HTMLCanvasElement & {
        captureStream?: (fps?: number) => MediaStream;
      };
      if (typeof captureSource.captureStream !== "function") {
        store.setError("Canvas.captureStream is not supported in this browser.");
        return;
      }
      const stream = captureSource.captureStream(CAPTURE_FPS);

      let recorder: MediaRecorder;
      try {
        recorder = new MediaRecorder(stream, {
          mimeType,
          videoBitsPerSecond: VIDEO_BITRATE,
        });
      } catch (err) {
        store.setError(
          err instanceof Error
            ? err.message
            : "Failed to create MediaRecorder.",
        );
        return;
      }

      cur.chunks = [];
      cur.recorder = recorder;
      cur.mimeType = mimeType;
      cur.raytraceDriving = useOverlay;

      recorder.ondataavailable = (ev) => {
        if (ev.data && ev.data.size > 0) {
          cur.chunks.push(ev.data);
          useRecordingStore.getState().addBytes(ev.data.size);
        }
      };

      recorder.onerror = (ev) => {
        const error = (ev as { error?: { message?: string } }).error;
        const msg = error?.message ?? "MediaRecorder error";
        logger.warn("app", `Recorder error: ${msg}`);
        useRecordingStore.getState().setError(msg);
      };

      recorder.onstop = () => {
        const chunks = cur.chunks;
        const mime = cur.mimeType ?? "video/webm";
        cur.chunks = [];
        cur.recorder = null;
        cur.mimeType = null;
        cur.raytraceDriving = false;
        if (cur.rafId !== null) {
          cancelAnimationFrame(cur.rafId);
          cur.rafId = null;
        }

        if (chunks.length === 0) {
          useRecordingStore.getState().reset();
          return;
        }

        const blob = new Blob(chunks, { type: mime });
        const docName =
          useDocumentStore.getState().documentName ?? "untitled";
        const filename = `vcad-sim-${slugify(docName)}-${timestampForFilename()}.${extensionFor(mime)}`;
        downloadBlob(blob, filename);
        useRecordingStore.getState().reset();
      };

      try {
        recorder.start(CHUNK_MS);
        useRecordingStore.getState().setMimeType(mimeType);
      } catch (err) {
        cur.recorder = null;
        cur.mimeType = null;
        cur.raytraceDriving = false;
        store.setError(
          err instanceof Error ? err.message : "Failed to start recording.",
        );
        return;
      }

      if (cur.raytraceDriving) {
        const tick = () => {
          if (!refs.current.raytraceDriving) return;
          triggerRaytraceRender();
          refs.current.rafId = requestAnimationFrame(tick);
        };
        refs.current.rafId = requestAnimationFrame(tick);
      }
      return;
    }

    // Intent: finalize. Store has flipped to "saving" — stop the recorder so
    // onstop fires, the file gets written, and the store moves back to idle.
    if (status === "saving" && cur.recorder) {
      try {
        if (cur.recorder.state !== "inactive") cur.recorder.stop();
      } catch (err) {
        logger.warn("app", `Recorder stop failed: ${err}`);
        useRecordingStore.getState().reset();
      }
      return;
    }

    // Intent: store reset to idle while a recorder was live — discard.
    if (status === "idle" && cur.recorder) {
      cur.chunks = [];
      try {
        cur.recorder.ondataavailable = null;
        cur.recorder.onstop = null;
        cur.recorder.onerror = null;
        if (cur.recorder.state !== "inactive") cur.recorder.stop();
      } catch {
        // best-effort
      }
      cur.recorder = null;
      cur.mimeType = null;
      cur.raytraceDriving = false;
      if (cur.rafId !== null) {
        cancelAnimationFrame(cur.rafId);
        cur.rafId = null;
      }
    }
  }, [status, renderMode, gl]);

  // Mirror sim play/pause into recorder pause/resume so the captured timeline
  // matches what the user sees. Sim Stop auto-finalizes the recording.
  useEffect(() => {
    const cur = refs.current;
    const rec = cur.recorder;
    if (!rec) return;

    if (simMode === "paused" && rec.state === "recording") {
      try {
        rec.pause();
        useRecordingStore.getState().pause();
        if (cur.rafId !== null) {
          cancelAnimationFrame(cur.rafId);
          cur.rafId = null;
        }
      } catch (err) {
        logger.warn("app", `Recorder pause failed: ${err}`);
      }
    } else if (simMode === "running" && rec.state === "paused") {
      try {
        rec.resume();
        useRecordingStore.getState().resume();
        if (cur.raytraceDriving && cur.rafId === null) {
          const tick = () => {
            if (!refs.current.raytraceDriving) return;
            triggerRaytraceRender();
            refs.current.rafId = requestAnimationFrame(tick);
          };
          cur.rafId = requestAnimationFrame(tick);
        }
      } catch (err) {
        logger.warn("app", `Recorder resume failed: ${err}`);
      }
    } else if (simMode === "off" && rec.state !== "inactive") {
      // Sim stopped while recording: finalize via the store so the same
      // save path runs.
      useRecordingStore.getState().stop();
    }
  }, [simMode]);

  // Clean up on unmount.
  useEffect(() => {
    return () => {
      const cur = refs.current;
      if (cur.rafId !== null) {
        cancelAnimationFrame(cur.rafId);
        cur.rafId = null;
      }
      const rec = cur.recorder;
      if (rec && rec.state !== "inactive") {
        try {
          rec.ondataavailable = null;
          rec.onstop = null;
          rec.onerror = null;
          rec.stop();
        } catch {
          // best-effort
        }
      }
      cur.recorder = null;
      cur.chunks = [];
      cur.mimeType = null;
      cur.raytraceDriving = false;
    };
  }, []);
}
