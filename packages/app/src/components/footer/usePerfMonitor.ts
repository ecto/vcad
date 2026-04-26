import { useEffect, useState } from "react";

const SAMPLE_COUNT = 30;
const UPDATE_HZ = 10;

export interface PerfSnapshot {
  /** Smoothed frames per second. */
  fps: number;
  /** Smoothed frame time in milliseconds (1000 / fps). */
  frameMs: number;
  /** JS heap usage in MB (Chrome only — null elsewhere). */
  heapMb: number | null;
  /** JS heap limit in MB (Chrome only). */
  heapLimitMb: number | null;
  /** Long-task count seen since hook mount (PerformanceObserver). */
  longTasks: number;
  /** Wall-clock seconds since the hook mounted (display: session uptime). */
  uptimeSec: number;
  fpsSamples: number[];
  frameMsSamples: number[];
  heapSamples: number[];
}

const EMPTY: PerfSnapshot = {
  fps: 0,
  frameMs: 0,
  heapMb: null,
  heapLimitMb: null,
  longTasks: 0,
  uptimeSec: 0,
  fpsSamples: [],
  frameMsSamples: [],
  heapSamples: [],
};

interface ChromePerformance extends Performance {
  memory?: { usedJSHeapSize: number; totalJSHeapSize: number; jsHeapSizeLimit: number };
}

/**
 * Samples requestAnimationFrame deltas for FPS / frame-time, plus JS heap when
 * available. Maintains a 30-sample ring buffer for sparkline rendering and
 * exposes a smoothed current value.
 *
 * Update is throttled to UPDATE_HZ (10 Hz) so the consuming chip re-renders
 * at most ten times per second regardless of frame rate.
 */
export function usePerfMonitor(): PerfSnapshot {
  const [snap, setSnap] = useState<PerfSnapshot>(EMPTY);

  useEffect(() => {
    let raf = 0;
    let cancelled = false;
    const t0 = performance.now();
    let lastFrame = t0;
    let lastEmit = t0;
    let smoothedFps = 60;
    let smoothedMs = 16.67;
    let longTasks = 0;
    const fpsSamples: number[] = [];
    const frameMsSamples: number[] = [];
    const heapSamples: number[] = [];

    // Long-task observer (Chrome / Edge). Each entry > 50ms on the main
    // thread bumps the counter; surfaces "we paused this many times" without
    // having to interpret the FPS dip.
    let observer: PerformanceObserver | null = null;
    if (typeof PerformanceObserver !== "undefined") {
      try {
        observer = new PerformanceObserver((list) => {
          longTasks += list.getEntries().length;
        });
        observer.observe({ type: "longtask", buffered: false });
      } catch {
        observer = null;
      }
    }

    const step = (now: number) => {
      if (cancelled) return;
      const dt = Math.max(0.001, now - lastFrame);
      lastFrame = now;
      const instFps = 1000 / dt;
      // Exponential smoothing — long enough to reject single janky frames
      // but quick enough to react to a sustained load.
      smoothedFps = smoothedFps * 0.9 + instFps * 0.1;
      smoothedMs = smoothedMs * 0.9 + dt * 0.1;

      if (now - lastEmit >= 1000 / UPDATE_HZ) {
        lastEmit = now;
        fpsSamples.push(smoothedFps);
        if (fpsSamples.length > SAMPLE_COUNT) fpsSamples.shift();
        frameMsSamples.push(smoothedMs);
        if (frameMsSamples.length > SAMPLE_COUNT) frameMsSamples.shift();

        const mem = (performance as ChromePerformance).memory;
        const heapMb = mem ? mem.usedJSHeapSize / (1024 * 1024) : null;
        const heapLimitMb = mem ? mem.jsHeapSizeLimit / (1024 * 1024) : null;
        if (heapMb !== null) {
          heapSamples.push(heapMb);
          if (heapSamples.length > SAMPLE_COUNT) heapSamples.shift();
        }

        setSnap({
          fps: smoothedFps,
          frameMs: smoothedMs,
          heapMb,
          heapLimitMb,
          longTasks,
          uptimeSec: (now - t0) / 1000,
          fpsSamples: [...fpsSamples],
          frameMsSamples: [...frameMsSamples],
          heapSamples: [...heapSamples],
        });
      }
      raf = requestAnimationFrame(step);
    };

    raf = requestAnimationFrame(step);
    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
      observer?.disconnect();
    };
  }, []);

  return snap;
}
