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
  const [lost, setLost] = useState(false);

  useEffect(() => {
    const canvas = gl.domElement;
    if (!canvas) return;

    // Seed the initial state in case the context was already lost before we
    // attached listeners (e.g. created on a backgrounded tab).
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
    };
    const handleRestored = () => {
      setLost(false);
    };

    canvas.addEventListener("webglcontextlost", handleLost as EventListener, false);
    canvas.addEventListener("webglcontextrestored", handleRestored, false);

    return () => {
      canvas.removeEventListener("webglcontextlost", handleLost as EventListener, false);
      canvas.removeEventListener("webglcontextrestored", handleRestored, false);
    };
  }, [gl]);

  return lost;
}
