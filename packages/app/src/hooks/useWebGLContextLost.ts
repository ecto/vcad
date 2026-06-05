import { useEffect, useState } from "react";
import { useThree } from "@react-three/fiber";

/**
 * Tracks whether the viewport's WebGL context is currently lost.
 *
 * Browsers can drop a WebGL context at any time — Safari/WebKit is especially
 * aggressive about it under GPU memory pressure, when too many live contexts
 * exist, or when a tab is backgrounded and restored. When that happens,
 * `gl.getContext().getContextAttributes()` returns `null`, and anything that
 * reads off it (notably the `postprocessing` EffectComposer, which does
 * `renderer.getContext().getContextAttributes().alpha`) throws
 * `null is not an object`.
 *
 * Calling `preventDefault()` on the `webglcontextlost` event tells the browser
 * we intend to recover, which is what triggers the matching
 * `webglcontextrestored` event. Three.js's WebGLRenderer re-initializes its own
 * GL state on restore; consumers can use this flag to unmount fragile
 * GPU-dependent subtrees (e.g. post-processing) while the context is gone and
 * remount them cleanly once it returns.
 *
 * Must be called from within an R3F `<Canvas>` (it relies on `useThree`).
 */
export function useWebGLContextLost(): boolean {
  const gl = useThree((state) => state.gl);
  const invalidate = useThree((state) => state.invalidate);
  const [lost, setLost] = useState(false);

  useEffect(() => {
    const canvas = gl.domElement;
    if (!canvas) return;

    // If the browser doesn't restore within this window after a loss, the
    // context is most likely gone for good (GPU resource exhaustion / too many
    // live contexts). Rather than leave a dead, white viewport, ask the app to
    // remount the <Canvas> for a fresh context (Viewport listens for this).
    let stuckTimer: ReturnType<typeof setTimeout> | undefined;
    const scheduleRecovery = () => {
      clearTimeout(stuckTimer);
      stuckTimer = setTimeout(() => {
        window.dispatchEvent(new CustomEvent("vcad:gl-stuck-lost"));
      }, 1500);
    };

    // Seed the initial state in case the context was already lost before we
    // attached listeners (e.g. created on a backgrounded tab). We deliberately
    // do NOT schedule a remount here: a freshly-created context can read as
    // momentarily lost during a heavy load while the GPU is under pressure, and
    // remounting on that transient would churn the canvas. Only a real runtime
    // `webglcontextlost` that fails to restore triggers recovery (below).
    try {
      const ctx = gl.getContext();
      if (ctx && typeof ctx.isContextLost === "function" && ctx.isContextLost()) {
        setLost(true);
      }
    } catch {
      // getContext can itself throw on a half-torn-down renderer; treat as lost.
      setLost(true);
    }

    const handleLost = (event: Event) => {
      // Signal intent to recover so the browser fires webglcontextrestored.
      event.preventDefault();
      setLost(true);
      scheduleRecovery();
    };
    const handleRestored = () => {
      clearTimeout(stuckTimer);
      setLost(false);
    };

    canvas.addEventListener("webglcontextlost", handleLost as EventListener, false);
    canvas.addEventListener("webglcontextrestored", handleRestored, false);

    return () => {
      clearTimeout(stuckTimer);
      canvas.removeEventListener("webglcontextlost", handleLost as EventListener, false);
      canvas.removeEventListener("webglcontextrestored", handleRestored, false);
    };
  }, [gl]);

  // Repaint once the context is back. The viewport runs `frameloop="demand"`
  // with a transparent canvas, so a restored context that nobody invalidates
  // leaves the canvas unpainted — the page background shows through as a
  // "white screen" until the next user interaction. Scheduling a frame here
  // (now and on the next rAF, after fragile GPU subtrees have remounted) makes
  // the scene reappear on its own. Also covers the case where the context was
  // already lost at mount and later restored.
  useEffect(() => {
    if (lost) return;
    invalidate();
    const raf = requestAnimationFrame(() => invalidate());
    return () => cancelAnimationFrame(raf);
  }, [lost, invalidate]);

  return lost;
}
