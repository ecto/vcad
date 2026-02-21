import { useEffect, useState, useCallback, useRef } from "react";

/**
 * Samples the canvas luminance behind a given screen region.
 * Returns "light" or "dark" based on average brightness.
 * Uses WebGL readPixels for efficient pixel sampling (no full-canvas copy).
 */
export function useBackgroundLuminance(
  canvasSelector = "canvas",
  region?: { x: number; y: number; width: number; height: number },
  idleDelayMs = 300
): "light" | "dark" {
  const [luminance, setLuminance] = useState<"light" | "dark">("dark");
  const idleTimerRef = useRef<number | null>(null);
  const lastSampleRef = useRef<number>(0);

  const sample = useCallback(() => {
    const canvas = document.querySelector(canvasSelector) as HTMLCanvasElement;
    if (!canvas) return;

    // Default to top-left corner region if not specified
    const r = region ?? { x: 10, y: 70, width: 180, height: 300 };

    // Sample points relative to the region
    const samplePoints = [
      { x: r.x + 20, y: r.y + 30 },
      { x: r.x + 90, y: r.y + 80 },
      { x: r.x + 40, y: r.y + 150 },
      { x: r.x + 120, y: r.y + 200 },
      { x: r.x + 60, y: r.y + 250 },
    ];

    try {
      // Try WebGL readPixels first (much faster than toDataURL)
      const gl =
        canvas.getContext("webgl2", { preserveDrawingBuffer: false }) ??
        canvas.getContext("webgl", { preserveDrawingBuffer: false });

      if (gl) {
        const pixel = new Uint8Array(4);
        let totalLuminance = 0;
        let validSamples = 0;
        const dpr = window.devicePixelRatio || 1;

        for (const point of samplePoints) {
          if (point.x >= canvas.width / dpr || point.y >= canvas.height / dpr) continue;

          // WebGL y-axis is flipped (0 = bottom)
          const glX = Math.round(point.x * dpr);
          const glY = Math.round(canvas.height - point.y * dpr);
          gl.readPixels(glX, glY, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);

          const lum = (0.2126 * pixel[0]! + 0.7152 * pixel[1]! + 0.0722 * pixel[2]!) / 255;
          totalLuminance += lum;
          validSamples++;
        }

        if (validSamples > 0) {
          const avgLuminance = totalLuminance / validSamples;
          setLuminance(avgLuminance > 0.5 ? "light" : "dark");
        }
        return;
      }

      // Fallback: 2D canvas (shouldn't happen with Three.js)
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      let totalLuminance = 0;
      let validSamples = 0;

      for (const point of samplePoints) {
        if (point.x >= canvas.width || point.y >= canvas.height) continue;
        const pixel = ctx.getImageData(point.x, point.y, 1, 1).data;
        const lum = (0.2126 * pixel[0]! + 0.7152 * pixel[1]! + 0.0722 * pixel[2]!) / 255;
        totalLuminance += lum;
        validSamples++;
      }

      if (validSamples > 0) {
        const avgLuminance = totalLuminance / validSamples;
        setLuminance(avgLuminance > 0.5 ? "light" : "dark");
      }
    } catch {
      // Canvas may not be ready yet
    }
  }, [canvasSelector, region]);

  // Schedule a sample after idle delay
  const scheduleSample = useCallback(() => {
    if (idleTimerRef.current) {
      clearTimeout(idleTimerRef.current);
    }
    idleTimerRef.current = window.setTimeout(() => {
      // Throttle: don't sample more than once per second
      const now = Date.now();
      if (now - lastSampleRef.current > 1000) {
        lastSampleRef.current = now;
        sample();
      }
    }, idleDelayMs);
  }, [sample, idleDelayMs]);

  useEffect(() => {
    // Sample once on mount (after a short delay for canvas to render)
    const initialTimer = setTimeout(sample, 500);

    // Sample when camera stops moving
    const handleCameraEnd = () => scheduleSample();
    window.addEventListener("vcad:camera-end", handleCameraEnd);

    // Also sample on pointer up (end of interaction)
    const handlePointerUp = () => scheduleSample();
    window.addEventListener("pointerup", handlePointerUp);

    return () => {
      clearTimeout(initialTimer);
      if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
      window.removeEventListener("vcad:camera-end", handleCameraEnd);
      window.removeEventListener("pointerup", handlePointerUp);
    };
  }, [sample, scheduleSample]);

  return luminance;
}
