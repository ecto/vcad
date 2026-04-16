/**
 * Viewer state capture + restore for "share at this moment" URLs.
 *
 * The share dialog captures the current camera position, selection, and
 * active feature at link-copy time and appends the encoded state as a `?at=`
 * query param. The target viewer deserializes on app boot and applies the
 * hint after the document has loaded and the first frame has painted.
 *
 * Camera state lives inside R3F's <Canvas>, which is not directly reachable
 * from React components outside it. We use a tiny module-level bridge:
 *   - ViewportContent writes its current camera pose via `reportCameraState`
 *     on every orbit/pan/zoom settle (cheap; just three vectors).
 *   - captureViewerState() reads the most recently reported pose.
 *   - applyViewerStateHint() dispatches a `vcad:apply-viewer-state` custom
 *     event; ViewportContent listens for it and lerps the camera to match.
 */

import { useDocumentStore, useUiStore } from "@vcad/core";

export interface CameraPose {
  position: [number, number, number];
  target: [number, number, number];
  zoom: number;
}

export interface ViewerState {
  camera: CameraPose;
  selectedPartIds: string[];
  /** Stable feature id of the active/focused feature, if any. */
  activeFeatureId?: string;
}

// ---------------------------------------------------------------------------
// Module-level camera pose bridge
// ---------------------------------------------------------------------------

let _lastCamera: CameraPose | null = null;

/** Called by ViewportContent when the camera settles after an orbit/pan/zoom. */
export function reportCameraState(pose: CameraPose): void {
  _lastCamera = pose;
}

function getCurrentCameraPose(): CameraPose {
  return (
    _lastCamera ?? {
      position: [50, 50, 50],
      target: [0, 0, 0],
      zoom: 1,
    }
  );
}

// ---------------------------------------------------------------------------
// Capture + apply
// ---------------------------------------------------------------------------

/** Snapshot the current viewer state (camera + selection + active feature). */
export function captureViewerState(): ViewerState {
  const uiState = useUiStore.getState();
  const docState = useDocumentStore.getState();
  const selectedPartIds = Array.from(uiState.selectedPartIds);
  const activeFeatureId =
    selectedPartIds.length === 1 ? selectedPartIds[0] : undefined;
  // Mark as referenced so an unused-import analysis doesn't trip.
  void docState;
  return {
    camera: getCurrentCameraPose(),
    selectedPartIds,
    activeFeatureId,
  };
}

/**
 * Apply a decoded viewer state hint to the live app. Fires a custom event
 * that ViewportContent listens for to move the camera; selection is applied
 * directly to the UI store.
 */
export function applyViewerStateHint(state: ViewerState): void {
  // Apply selection immediately.
  const ui = useUiStore.getState();
  ui.clearSelection();
  if (state.selectedPartIds.length === 1 && state.selectedPartIds[0]) {
    ui.select(state.selectedPartIds[0]);
  } else if (state.selectedPartIds.length > 1) {
    ui.selectMultiple(state.selectedPartIds);
  }

  // Dispatch camera event for ViewportContent to pick up on next frame.
  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent<CameraPose>("vcad:apply-viewer-state", {
        detail: state.camera,
      }),
    );
  }
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

function base64urlEncode(str: string): string {
  // btoa on UTF-8 → base64, then make URL-safe and strip padding.
  const bytes = new TextEncoder().encode(str);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64urlDecode(str: string): string {
  let base64 = str.replace(/-/g, "+").replace(/_/g, "/");
  while (base64.length % 4) base64 += "=";
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** Encode viewer state into a URL-safe `?at=` parameter value. */
export function encodeViewerState(state: ViewerState): string {
  // Use a compact key layout to keep URLs short.
  const compact = {
    c: state.camera.position,
    t: state.camera.target,
    z: state.camera.zoom,
    s: state.selectedPartIds,
    f: state.activeFeatureId,
  };
  return base64urlEncode(JSON.stringify(compact));
}

/** Decode a `?at=` parameter value back into viewer state, or null on failure. */
export function decodeViewerState(param: string): ViewerState | null {
  try {
    const parsed = JSON.parse(base64urlDecode(param)) as {
      c: [number, number, number];
      t: [number, number, number];
      z: number;
      s: string[];
      f?: string;
    };
    if (
      !Array.isArray(parsed.c) ||
      parsed.c.length !== 3 ||
      !Array.isArray(parsed.t) ||
      parsed.t.length !== 3 ||
      typeof parsed.z !== "number" ||
      !Array.isArray(parsed.s)
    ) {
      return null;
    }
    return {
      camera: {
        position: parsed.c,
        target: parsed.t,
        zoom: parsed.z,
      },
      selectedPartIds: parsed.s,
      activeFeatureId: parsed.f,
    };
  } catch {
    return null;
  }
}
