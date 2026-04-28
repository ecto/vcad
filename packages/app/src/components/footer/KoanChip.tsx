import { useEffect, useMemo, useState } from "react";
import { useDocumentStore, useUiStore, useEngineStore, tFmt } from "@vcad/core";
import { FooterChip } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const KOANS = [
  "kernel idle",
  "shapes settling",
  "blank page loaded",
  "ready when you are",
  "pencil sharpened",
  "waiting for geometry",
  "the void beckons",
  "edges quiet, faces still",
  "no parts in flight",
  "begin anywhere",
  "every shape starts as a thought",
  "the kernel waits patiently",
  "geometry begins with a point",
  "BRep at rest",
  "topology unbothered",
  "z-up, ready",
  "an empty canvas is potential",
  "0 triangles, infinite possibilities",
  "the manifold sleeps",
  "draw something",
];

const ROTATE_MS = 18_000;

export type FooterModeBadge = "drafting" | "assembly";

const MODE_META: Record<
  FooterModeBadge,
  { label: string; dot: string; text: string }
> = {
  drafting: {
    label: "drafting",
    dot: "bg-stone-300",
    text: "text-stone-300",
  },
  assembly: {
    label: "assembly",
    dot: "bg-cyan-400",
    text: "text-cyan-400",
  },
};

/**
 * Idle-zone chip.
 *
 * Priority of what occupies the slot:
 *   1. selection > 0 — selection summary (count + name when single).
 *   2. mode badge — colored dot + label for the active app mode
 *      (drafting / assembly). Sketch is intentionally skipped
 *      because the sketch ribbon already covers it. Raytrace lives in
 *      its own right-side chip (`RaytraceChip`) since it's a render
 *      flag rather than a workspace mode.
 *   3. parts > 0 — gentle stat about the document (parts · k tris).
 *   4. empty doc — rotating koan, cycling every 18s.
 */
export function KoanChip({
  className,
  divider,
  mode,
}: {
  className?: string;
  divider?: boolean;
  mode?: FooterModeBadge | null;
}) {
  const parts = useDocumentStore((s) => s.parts);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const scene = useEngineStore((s) => s.scene);

  const [koanIdx, setKoanIdx] = useState(() => Math.floor(Math.random() * KOANS.length));
  useEffect(() => {
    const id = window.setInterval(() => {
      setKoanIdx((i) => (i + 1) % KOANS.length);
    }, ROTATE_MS);
    return () => window.clearInterval(id);
  }, []);

  const selCount = selectedPartIds.size;

  const triangles = useMemo(() => {
    if (!scene) return 0;
    let total = 0;
    for (const p of scene.parts) total += p.mesh.indices.length / 3;
    if (scene.instances) {
      for (const i of scene.instances) total += i.mesh.indices.length / 3;
    }
    return Math.round(total);
  }, [scene]);

  let content: React.ReactNode;
  let title: string;
  if (selCount > 0) {
    if (selCount === 1) {
      const part = parts.find((p) => p.id === Array.from(selectedPartIds)[0]);
      title = `Selected: ${part?.name ?? "1 part"}`;
    } else {
      title = `${selCount} parts selected`;
    }
  } else if (mode) {
    const meta = MODE_META[mode];
    title = `Mode: ${meta.label}`;
  } else if (parts.length > 0) {
    title = `${parts.length} ${parts.length === 1 ? "part" : "parts"}${
      triangles > 0 ? ` · ${formatTris(triangles)} triangles` : ""
    }`;
  } else {
    title = "Empty document";
  }

  if (selCount > 0) {
    if (selCount === 1) {
      const part = parts.find((p) => p.id === Array.from(selectedPartIds)[0]);
      content = (
        <span className="text-brand truncate max-w-[14rem]">
          {part?.name ?? tFmt("status.sel", { count: "1" })}
        </span>
      );
    } else {
      content = (
        <span className="text-brand tabular-nums">
          {tFmt("status.sel", { count: String(selCount) })}
        </span>
      );
    }
  } else if (mode) {
    const meta = MODE_META[mode];
    content = (
      <span
        key={mode}
        className={cn(
          "inline-flex items-center gap-1.5 uppercase tracking-[0.15em] font-medium",
          meta.text,
          "animate-in fade-in duration-300",
        )}
      >
        <span
          className={cn("inline-block w-1.5 h-1.5 rounded-full", meta.dot)}
          aria-hidden
        />
        {meta.label}
      </span>
    );
  } else if (parts.length > 0) {
    content = (
      <span className="text-text-muted">
        <span className="tabular-nums">{parts.length}</span>{" "}
        {parts.length === 1 ? "part" : "parts"}
        {triangles > 0 && (
          <>
            <span className="text-text-muted/40 mx-1.5">·</span>
            <span className="tabular-nums">{formatTris(triangles)}</span> tris
          </>
        )}
      </span>
    );
  } else {
    content = (
      <span
        key={koanIdx}
        className={cn(
          "text-text-muted/60 italic",
          "animate-in fade-in duration-500",
        )}
      >
        {KOANS[koanIdx]}
      </span>
    );
  }

  return (
    <Tooltip side="top" content={title}>
      <FooterChip
        divider={divider}
        className={cn("hidden lg:flex", className)}
      >
        {content}
      </FooterChip>
    </Tooltip>
  );
}

function formatTris(n: number): string {
  if (n < 1000) return n.toString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}
