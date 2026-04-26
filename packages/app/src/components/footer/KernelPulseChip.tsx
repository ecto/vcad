import { useEngineStore, useDocumentStore } from "@vcad/core";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/shadcn/hover-card";
import { FooterChip } from "@/components/footer/FooterChip";
import { Sparkline } from "@/components/footer/Sparkline";
import { usePerfMonitor } from "@/components/footer/usePerfMonitor";
import { cn } from "@/lib/utils";

/**
 * Kernel pulse + frame-time sparkline.
 *
 * Surface (always visible):
 *   • breathing dot — color + rate map to frame health
 *   • current frame ms (numeric)
 *   • inline frame-ms sparkline (32×8)
 *
 * Hover surfaces fps / frame / heap streams as larger sparklines plus the
 * total triangle count, the most recent kernel evaluation time, JS heap
 * usage, long-task count, and session uptime. Numbers are real — pulled from
 * `performance.memory`, `PerformanceObserver` long-task entries, and the
 * EvaluatedScene the engine produces.
 */
export function KernelPulseChip({
  className,
  divider,
}: {
  className?: string;
  divider?: boolean;
}) {
  const perf = usePerfMonitor();
  const engineLoading = useEngineStore((s) => s.loading);
  const scene = useEngineStore((s) => s.scene);
  const evalMs = scene?.timing?.total_ms ?? null;
  const partCount = useDocumentStore((s) => s.parts.length);

  const fps = perf.fps;
  const ms = perf.frameMs;

  const triangles = countSceneTriangles(scene);

  // Pulse style — color + animation duration map to frame health.
  let dotColor = "bg-emerald-400";
  let pulseDur = 1.6;
  if (engineLoading) {
    dotColor = "bg-brand";
    pulseDur = 0.5;
  } else if (fps < 20) {
    dotColor = "bg-red-400";
    pulseDur = 0.5;
  } else if (fps < 45) {
    dotColor = "bg-amber-400";
    pulseDur = 0.9;
  }

  const recentMaxMs = perf.frameMsSamples.length
    ? Math.max(...perf.frameMsSamples.slice(-10))
    : ms;
  const sparkColor =
    engineLoading || recentMaxMs > 50
      ? "text-red-400"
      : recentMaxMs > 22
        ? "text-amber-400"
        : "text-emerald-400/70";

  return (
    <HoverCard openDelay={150} closeDelay={80}>
      <HoverCardTrigger asChild>
        <FooterChip
          divider={divider}
          className={cn(
            "opacity-70 hover:opacity-100 transition-opacity gap-2",
            className,
          )}
        >
          <span
            className={cn("inline-block w-1.5 h-1.5 rounded-full", dotColor)}
            style={{
              animation: `vcad-pulse ${pulseDur.toFixed(2)}s cubic-bezier(0.4, 0, 0.6, 1) infinite`,
            }}
            aria-hidden
          />
          <span className="hidden xl:inline tabular-nums text-text-muted/80">
            {ms.toFixed(1)}
            <span className="text-text-muted/50">ms</span>
          </span>
          <Sparkline
            samples={perf.frameMsSamples}
            width={32}
            height={8}
            min={8}
            max={33}
            className={sparkColor}
          />
        </FooterChip>
      </HoverCardTrigger>
      <HoverCardContent
        side="top"
        align="end"
        sideOffset={6}
        className="w-72 p-0 font-mono text-[10px] border-border bg-surface"
      >
        <div className="px-3 py-2 border-b border-border/50 flex items-center justify-between text-text-muted">
          <span className="uppercase tracking-[0.15em] text-text-muted/80">
            kernel
          </span>
          <span className="tabular-nums">
            {formatUptime(perf.uptimeSec)}
          </span>
        </div>

        <div className="px-3 py-2 space-y-1.5">
          <PerfRow label="fps" value={fps.toFixed(0)} samples={perf.fpsSamples} sparkColor="text-emerald-400/70" />
          <PerfRow
            label="frame"
            value={`${ms.toFixed(1)} ms`}
            samples={perf.frameMsSamples}
            sparkColor={sparkColor}
          />
          {perf.heapMb !== null && (
            <PerfRow
              label="heap"
              value={
                perf.heapLimitMb !== null
                  ? `${perf.heapMb.toFixed(0)}/${perf.heapLimitMb.toFixed(0)} MB`
                  : `${perf.heapMb.toFixed(0)} MB`
              }
              samples={perf.heapSamples}
              sparkColor="text-blue-400/70"
            />
          )}
        </div>

        <div className="px-3 py-2 border-t border-border/50 grid grid-cols-2 gap-x-3 gap-y-1 text-text-muted">
          <Stat label="parts" value={partCount.toString()} />
          <Stat label="tris" value={formatCount(triangles)} />
          <Stat
            label="eval"
            value={evalMs !== null ? `${evalMs.toFixed(1)} ms` : "—"}
          />
          <Stat label="jank" value={perf.longTasks.toString()} />
        </div>

        {engineLoading && (
          <div className="px-3 py-2 border-t border-border/50 text-brand uppercase tracking-[0.15em]">
            kernel evaluating…
          </div>
        )}
      </HoverCardContent>
    </HoverCard>
  );
}

function PerfRow({
  label,
  value,
  samples,
  sparkColor,
}: {
  label: string;
  value: string;
  samples: number[];
  sparkColor: string;
}) {
  return (
    <div className="grid grid-cols-[2.75rem_1fr_4.5rem] items-center gap-2">
      <span className="text-text-muted/70 uppercase tracking-[0.15em]">{label}</span>
      <Sparkline samples={samples} width={104} height={12} className={sparkColor} />
      <span className="tabular-nums text-text text-right">{value}</span>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-text-muted/70 uppercase tracking-[0.15em]">{label}</span>
      <span className="tabular-nums text-text">{value}</span>
    </div>
  );
}

function countSceneTriangles(
  scene: ReturnType<typeof useEngineStore.getState>["scene"],
): number {
  if (!scene) return 0;
  let total = 0;
  for (const p of scene.parts) total += p.mesh.indices.length / 3;
  if (scene.instances) {
    for (const i of scene.instances) total += i.mesh.indices.length / 3;
  }
  return Math.round(total);
}

function formatCount(n: number): string {
  if (n === 0) return "—";
  if (n < 1000) return n.toString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

function formatUptime(sec: number): string {
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = Math.floor(sec % 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}
