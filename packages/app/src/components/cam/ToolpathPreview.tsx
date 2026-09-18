/**
 * The moves, looking down at the stock.
 *
 * The job answers a preview polyline and the ranges that partition it, so the
 * panel draws exactly what the post emitted rather than re-deriving anything:
 * a second opinion about where the cutter goes is a second answer.
 *
 * Two colours, and the distinction is the one that matters on a machine —
 * **cut** moves are the ones that remove metal, **rapid** moves are the ones
 * that must not. A rapid drawn the same as a cut is how a plunge through the
 * clamp stops being visible. Selecting an operation row draws that range alone
 * and greys the rest, so "which of these is the bore" is answerable.
 */
import { useMemo } from "react";
import { useCamJobStore } from "@/stores/cam-job-store";

const WIDTH = 288;
const HEIGHT = 200;
const PAD = 8;

export function ToolpathPreview() {
  const result = useCamJobStore((s) => s.result);
  const outline = useCamJobStore((s) => s.outline);
  const highlighted = useCamJobStore((s) => s.highlightedOpIndex);
  const operations = useCamJobStore((s) => s.operations);

  const paths = useMemo(() => {
    const moves = result?.moves ?? [];
    const ranges = (result?.op_ranges ?? []).filter((r) => r.block === "operation");
    if (moves.length === 0) return null;

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const m of moves) {
      minX = Math.min(minX, m.to[0]);
      minY = Math.min(minY, m.to[1]);
      maxX = Math.max(maxX, m.to[0]);
      maxY = Math.max(maxY, m.to[1]);
    }
    if (!Number.isFinite(minX) || maxX <= minX || maxY <= minY) return null;
    const scale = Math.min(
      (WIDTH - 2 * PAD) / (maxX - minX),
      (HEIGHT - 2 * PAD) / (maxY - minY),
    );
    // Y is up in the stock frame and down in SVG, so the projection flips it.
    const px = (x: number) => PAD + (x - minX) * scale;
    const py = (y: number) => HEIGHT - PAD - (y - minY) * scale;

    // Which kernel operation indices belong to the highlighted UI row: a
    // helical-bore row is one row and several operations.
    let live: Set<number> | null = null;
    if (highlighted !== null) {
      let index = 0;
      const owned = new Set<number>();
      for (let i = 0; i < operations.length; i++) {
        const op = operations[i];
        if (!op?.enabled) continue;
        const span = op.source.type === "bore" ? op.source.centres.length : 1;
        if (i === highlighted) for (let k = 0; k < span; k++) owned.add(index + k);
        index += span;
      }
      live = owned;
    }

    const segments: Array<{ d: string; rapid: boolean; dim: boolean }> = [];
    let d = "";
    let currentRapid: boolean | null = null;
    let currentDim = false;
    const flush = () => {
      if (d && currentRapid !== null) {
        segments.push({ d, rapid: currentRapid, dim: currentDim });
      }
      d = "";
      currentRapid = null;
    };
    const rangeAt = (i: number) => ranges.find((r) => i >= r.start && i < r.end);

    for (let i = 1; i < moves.length; i++) {
      const from = moves[i - 1];
      const to = moves[i];
      if (!from || !to) continue;
      const range = rangeAt(i);
      const dim =
        live !== null && !(range?.op_index !== undefined && live.has(range.op_index));
      if (to.rapid !== currentRapid || dim !== currentDim) {
        flush();
        currentRapid = to.rapid;
        currentDim = dim;
        d = `M ${px(from.to[0]).toFixed(1)} ${py(from.to[1]).toFixed(1)}`;
      }
      d += ` L ${px(to.to[0]).toFixed(1)} ${py(to.to[1]).toFixed(1)}`;
    }
    flush();

    const outerLoop = outline?.regions[0]?.outer ?? [];
    const part =
      outerLoop.length >= 3
        ? `M ${outerLoop
            .map(([x, y]) => `${px(x).toFixed(1)} ${py(y).toFixed(1)}`)
            .join(" L ")} Z`
        : null;

    return { segments, part };
  }, [result, outline, highlighted, operations]);

  if (!paths) {
    return (
      <p className="text-xs text-text-muted">
        Build the job to see the moves the post emitted.
      </p>
    );
  }

  return (
    <figure className="space-y-1">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        className="w-full rounded border border-border bg-surface-secondary"
        role="img"
        aria-label="Toolpath, looking down at the stock"
      >
        {paths.part && (
          <path
            d={paths.part}
            fill="none"
            stroke="currentColor"
            className="text-text-muted"
            strokeWidth={0.6}
            strokeDasharray="3 3"
          />
        )}
        {paths.segments.map((s, i) => (
          <path
            key={i}
            d={s.d}
            fill="none"
            stroke="currentColor"
            className={
              s.dim
                ? "text-text-muted opacity-20"
                : s.rapid
                  ? "text-warning opacity-60"
                  : "text-brand"
            }
            strokeWidth={s.rapid ? 0.5 : 1}
            strokeDasharray={s.rapid ? "2 2" : undefined}
          />
        ))}
      </svg>
      <figcaption className="flex items-center gap-3 text-xs text-text-muted">
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 h-0.5 bg-brand" /> cut
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 h-0.5 bg-warning" /> rapid
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 border-t border-dashed border-current" /> part
        </span>
      </figcaption>
    </figure>
  );
}
