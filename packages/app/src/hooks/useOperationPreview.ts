import { useEffect, useRef } from "react";
import {
  useSketchStore,
  useEngineStore,
  getSketchPlaneDirections,
} from "@vcad/core";
import type { Vec3 } from "@vcad/ir";

/**
 * Continuously rebuilds the downstream operation's preview mesh as the user
 * sketches. Subscribes to:
 *   - sketch segments (drawing changes the profile)
 *   - sketch profiles (loft adds profiles)
 *   - pendingOperation params (depth/twist/scale/angle/path/etc.)
 *
 * Calls the matching `engine.evaluate*Preview` function and writes the result
 * to `useEngineStore.setPreviewMesh`. The viewport's existing PreviewMesh
 * component already renders that mesh — no viewport changes needed.
 *
 * Calls are coalesced into a single `requestAnimationFrame` so a flurry of
 * pointer-driven updates (drag-rectangle, scrub) doesn't queue dozens of
 * wasm calls. Preview clears on operation commit or sketch exit.
 */
export function useOperationPreview() {
  const active = useSketchStore((s) => s.active);
  const segments = useSketchStore((s) => s.segments);
  const profiles = useSketchStore((s) => s.profiles);
  const plane = useSketchStore((s) => s.plane);
  const origin = useSketchStore((s) => s.origin);
  const pendingOperation = useSketchStore((s) => s.pendingOperation);
  const engine = useEngineStore((s) => s.engine);
  const setPreviewMesh = useEngineStore((s) => s.setPreviewMesh);

  // We keep the latest values in refs so the rAF loop reads them lazily —
  // that way a scrub that fires 100x/second doesn't reschedule rAF 100x/sec.
  const latest = useRef({ segments, profiles, plane, origin, pendingOperation });
  latest.current = { segments, profiles, plane, origin, pendingOperation };

  useEffect(() => {
    if (!active || !engine || !pendingOperation) {
      setPreviewMesh(null);
      return;
    }

    let scheduled: number | null = null;
    let cancelled = false;

    function schedule() {
      if (scheduled !== null) return;
      scheduled = requestAnimationFrame(() => {
        scheduled = null;
        if (cancelled) return;
        const { segments, profiles, plane, origin, pendingOperation } = latest.current;
        if (!engine || !pendingOperation) {
          setPreviewMesh(null);
          return;
        }

        const { x_dir, y_dir, normal } = getSketchPlaneDirections(plane);

        try {
          if (pendingOperation.kind === "extrude") {
            if (segments.length === 0) {
              setPreviewMesh(null);
              return;
            }
            const depth = pendingOperation.flip ? -pendingOperation.depth : pendingOperation.depth;
            const direction: Vec3 = {
              x: normal.x * depth,
              y: normal.y * depth,
              z: normal.z * depth,
            };
            const mesh = engine.evaluateExtrudePreview(origin, x_dir, y_dir, segments, direction);
            setPreviewMesh(mesh);
          } else if (pendingOperation.kind === "revolve") {
            if (segments.length === 0) {
              setPreviewMesh(null);
              return;
            }
            const axisDir = pendingOperation.flip
              ? { x: -x_dir.x, y: -x_dir.y, z: -x_dir.z }
              : x_dir;
            const mesh = engine.evaluateRevolvePreview(
              origin,
              x_dir,
              y_dir,
              segments,
              origin,
              axisDir,
              pendingOperation.angleDeg,
            );
            setPreviewMesh(mesh);
          } else if (pendingOperation.kind === "sweep") {
            if (segments.length === 0) {
              setPreviewMesh(null);
              return;
            }
            const path =
              pendingOperation.pathType === "line"
                ? {
                    type: "Line" as const,
                    start: origin,
                    end: {
                      x: origin.x + normal.x * pendingOperation.height,
                      y: origin.y + normal.y * pendingOperation.height,
                      z: origin.z + normal.z * pendingOperation.height,
                    },
                  }
                : {
                    type: "Helix" as const,
                    radius: pendingOperation.radius,
                    pitch: pendingOperation.height / pendingOperation.turns,
                    height: pendingOperation.height,
                    turns: pendingOperation.turns,
                  };
            const mesh = engine.evaluateSweepPreview(origin, x_dir, y_dir, segments, path);
            setPreviewMesh(mesh);
          } else if (pendingOperation.kind === "loft") {
            // Combine saved profiles with the in-progress one.
            const allProfiles = [
              ...profiles.map((p) => {
                const dirs = getSketchPlaneDirections(p.plane);
                return {
                  plane: { x_dir: dirs.x_dir, y_dir: dirs.y_dir },
                  origin: p.origin,
                  segments: p.segments,
                };
              }),
            ];
            if (segments.length > 0) {
              allProfiles.push({
                plane: { x_dir, y_dir },
                origin,
                segments,
              });
            }
            if (allProfiles.length < 2) {
              setPreviewMesh(null);
              return;
            }
            const mesh = engine.evaluateLoftPreview(allProfiles, pendingOperation.closed);
            setPreviewMesh(mesh);
          }
        } catch (err) {
          // Wasm preview can throw mid-edit (incomplete profile, etc.).
          // Silent in dev console; the next valid edit will resolve.
          console.warn("[sketch] preview failed:", err);
        }
      });
    }

    // Initial paint + reschedule on dependency change.
    schedule();

    return () => {
      cancelled = true;
      if (scheduled !== null) cancelAnimationFrame(scheduled);
      setPreviewMesh(null);
    };
    // We intentionally read mutable bits via the ref above — re-running the
    // effect on every segment/origin change would tear down rAF; we want
    // schedule() to be called from the deps, not the effect cleanup.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    active,
    engine,
    setPreviewMesh,
    segments,
    profiles,
    plane,
    origin,
    pendingOperation,
  ]);
}
