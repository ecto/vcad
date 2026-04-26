import { useMemo } from "react";
import { cn } from "@/lib/utils";

interface SparklineProps {
  samples: number[];
  width?: number;
  height?: number;
  /** Force a min/max for the y-axis instead of auto-scaling. */
  min?: number;
  max?: number;
  className?: string;
  strokeWidth?: number;
}

/**
 * Inline SVG sparkline. Renders a polyline through `samples`, scaled to fill
 * the box. Uses `currentColor` so the parent text color drives the stroke,
 * which makes severity transitions free.
 */
export function Sparkline({
  samples,
  width = 28,
  height = 8,
  min,
  max,
  className,
  strokeWidth = 1,
}: SparklineProps) {
  const points = useMemo(() => {
    if (samples.length < 2) return null;
    let lo = min ?? Infinity;
    let hi = max ?? -Infinity;
    if (min === undefined || max === undefined) {
      for (const v of samples) {
        if (min === undefined && v < lo) lo = v;
        if (max === undefined && v > hi) hi = v;
      }
    }
    const range = hi - lo || 1;
    const stepX = width / (samples.length - 1);
    const parts: string[] = [];
    for (let i = 0; i < samples.length; i++) {
      const x = i * stepX;
      const v = samples[i] ?? 0;
      const y = height - ((v - lo) / range) * height;
      parts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
    }
    return parts.join(" ");
  }, [samples, width, height, min, max]);

  return (
    <svg
      width={width}
      height={height}
      className={cn("shrink-0 overflow-visible", className)}
      aria-hidden
    >
      {points && (
        <polyline
          points={points}
          fill="none"
          stroke="currentColor"
          strokeWidth={strokeWidth}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      )}
    </svg>
  );
}
