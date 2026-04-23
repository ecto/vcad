import { useEffect, useRef } from "react";
import {
  useSketchStore,
  getPlaneBasis,
  computeSketchBounds,
} from "@vcad/core";

/**
 * Auto-fit the viewport camera to the active sketch the first time the
 * sketch has segments to show. Fires `vcad:fit-sketch` — see the handler
 * in ViewportContent for the camera math. Skips when a share URL supplied
 * a `?at=` viewer-state hint, since that hint owns the camera.
 *
 * Only fires once per sketch-active session: entering an empty sketch and
 * then drawing into it should not yank the camera, but opening a doc that
 * already has a populated sketch should frame it.
 */
export function useSketchAutoFit(): void {
  const active = useSketchStore((s) => s.active);
  const firedRef = useRef(false);

  // Reset the one-shot guard when the user leaves sketch mode so the next
  // session gets its own auto-fit.
  useEffect(() => {
    if (!active) firedRef.current = false;
  }, [active]);

  useEffect(() => {
    if (!active || firedRef.current) return;

    if (typeof window !== "undefined") {
      const params = new URLSearchParams(window.location.search);
      if (params.has("at")) return;
    }

    const { segments, plane, origin } = useSketchStore.getState();
    if (segments.length === 0) return;

    const bounds = computeSketchBounds(segments);
    if (!bounds) return;

    const basis = getPlaneBasis(plane);
    // Sketch-plane origin is stored on the sketch store (it's optional on
    // axis-aligned `"XY" | "XZ" | "YZ"` planes); prefer the store value so
    // face-derived sketches with a non-zero origin still fit correctly.
    const planeWithOrigin =
      typeof plane === "string"
        ? { ...basis, origin }
        : basis;

    const centerU = (bounds.minU + bounds.maxU) / 2;
    const centerV = (bounds.minV + bounds.maxV) / 2;
    const width = bounds.maxU - bounds.minU;
    const height = bounds.maxV - bounds.minV;

    // Project the 2D center back through the plane basis to get a kernel-
    // space world point. Done inline because `sketchToWorld` from
    // sketch-math.ts ignores the dynamic origin carried on the store for
    // axis-aligned planes.
    const planeCenter = {
      x:
        planeWithOrigin.origin.x +
        centerU * planeWithOrigin.xDir.x +
        centerV * planeWithOrigin.yDir.x,
      y:
        planeWithOrigin.origin.y +
        centerU * planeWithOrigin.xDir.y +
        centerV * planeWithOrigin.yDir.y,
      z:
        planeWithOrigin.origin.z +
        centerU * planeWithOrigin.xDir.z +
        centerV * planeWithOrigin.yDir.z,
    };

    firedRef.current = true;
    window.dispatchEvent(
      new CustomEvent("vcad:fit-sketch", {
        detail: {
          planeNormal: planeWithOrigin.normal,
          planeCenter,
          width,
          height,
        },
      }),
    );
  }, [active]);
}
