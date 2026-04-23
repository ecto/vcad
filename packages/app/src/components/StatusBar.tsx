import { useEffect, useMemo, useState } from "react";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Terminal } from "@phosphor-icons/react/dist/ssr/Terminal";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { useDocumentStore, useUiStore, useSketchStore, type LogLevelName } from "@vcad/core";
import { useLogStore, getFilteredEntries } from "@/stores/log-store";
import { cn } from "@/lib/utils";

const LEVEL_COLOR: Record<LogLevelName, string> = {
  DEBUG: "text-text-muted",
  INFO: "text-blue-400",
  WARN: "text-yellow-400",
  ERROR: "text-red-400",
};

function formatCoord(n: number): string {
  return n.toFixed(1).padStart(8, " ");
}

function formatAgo(ts: number, now: number): string {
  const diff = Math.max(0, now - ts);
  if (diff < 2000) return "now";
  if (diff < 60_000) return `${Math.floor(diff / 1000)}s`;
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m`;
  return `${Math.floor(diff / 3_600_000)}h`;
}

/**
 * Ambient status bar.
 *
 * Left: live ticker of the most recent console log entry matching the current
 * filters. Slides in on change, click opens the full Console panel. Middle:
 * live cursor world position from the viewport raycast. Right: compact doc
 * metrics — dirty dot, part count, selection count.
 */
export function StatusBar() {
  const parts = useDocumentStore((s) => s.parts);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const cursorWorld = useUiStore((s) => s.cursorWorld);

  // Sketch state — only rendered while sketch is active. Each subscription is
  // cheap (primitives or shallow length), so the subscriber count overhead is
  // a non-issue.
  const sketchActive = useSketchStore((s) => s.active);
  const sketchPlane = useSketchStore((s) => s.plane);
  const sketchCursor = useSketchStore((s) => s.cursorSketchPos);
  const sketchSnap = useSketchStore((s) => s.snapTarget);
  const sketchSegmentCount = useSketchStore((s) => s.segments.length);
  const sketchConstraintCount = useSketchStore((s) => s.constraints.length);
  const sketchStatus = useSketchStore((s) => s.constraintStatus);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);

  // Subscribe to the pieces of the log store the ticker actually needs so we
  // re-render only when filtered output changes.
  const entries = useLogStore((s) => s.entries);
  const minLevel = useLogStore((s) => s.minLevel);
  const enabledSources = useLogStore((s) => s.enabledSources);
  const togglePanel = useLogStore((s) => s.togglePanel);

  const latest = useMemo(() => {
    // getFilteredEntries wants the whole state shape but only reads these
    // four fields; the rest are never touched by the filter.
    const filtered = getFilteredEntries({
      entries,
      minLevel,
      enabledSources,
      panelOpen: false,
      togglePanel: () => {},
      openPanel: () => {},
      closePanel: () => {},
      setMinLevel: () => {},
      toggleSource: () => {},
      clearLogs: () => {},
    });
    return filtered.length > 0 ? filtered[filtered.length - 1] : null;
  }, [entries, minLevel, enabledSources]);

  // Tick for "Ns ago" readout and to re-key the slide-in animation.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const selCount = selectedPartIds.size;
  const fresh = latest && now - latest.timestamp < 3000;
  const levelColor = latest ? LEVEL_COLOR[latest.level] : "";

  return (
    <div
      className={cn(
        "flex h-6 items-stretch bg-surface text-[10px] font-mono select-none",
        "border-t border-border/40",
      )}
    >
      {/* Left: console ticker — click to open the full console panel */}
      <button
        type="button"
        onClick={togglePanel}
        className={cn(
          "flex items-center gap-2 px-3 min-w-0 flex-1",
          "text-text-muted hover:bg-hover hover:text-text",
          "focus:outline-none focus-visible:bg-hover",
          "transition-colors",
        )}
        title={latest ? "Open console (~)" : "Console (empty)"}
      >
        <Terminal size={11} className="shrink-0 opacity-60" />
        {latest ? (
          <>
            <span
              className={cn(
                "shrink-0 font-semibold uppercase tracking-wide",
                levelColor,
              )}
            >
              {latest.level}
            </span>
            <span className="shrink-0 text-text-muted/70">
              {latest.source}
            </span>
            <span
              key={latest.id}
              className={cn(
                "truncate text-left text-text",
                "animate-in fade-in slide-in-from-left-2 duration-300",
              )}
            >
              {latest.message}
            </span>
            {fresh && (
              <Circle
                size={6}
                weight="fill"
                className={cn("shrink-0 animate-pulse", levelColor)}
              />
            )}
            <span className="ml-auto shrink-0 tabular-nums text-text-muted/70">
              {formatAgo(latest.timestamp, now)}
            </span>
          </>
        ) : (
          <span className="text-text-muted/60">console empty</span>
        )}
      </button>

      {/* Sketch ribbon — only when active. Surfaces live cursor, snap state,
          entity/constraint counts, and constraint solver status. */}
      {sketchActive && (
        <div
          className={cn(
            "flex items-center gap-2 px-3 border-l border-border/40",
            "text-amber-400 tabular-nums whitespace-pre",
          )}
        >
          <PencilSimple size={11} className="shrink-0" />
          <span className="font-medium">SKETCH</span>
          <span className="text-text-muted">
            {typeof sketchPlane === "string" ? sketchPlane : "face"}
          </span>
          {sketchCursor && (
            <span className="hidden md:inline text-text-muted">
              ({sketchCursor.x.toFixed(1)}, {sketchCursor.y.toFixed(1)})
            </span>
          )}
          <span className="hidden lg:inline text-text-muted">
            snap: {sketchSnap ? "POINT" : gridSnap ? "GRID" : pointSnap ? "PT" : "OFF"}
          </span>
          <span className="text-text-muted">
            {sketchSegmentCount} ent · {sketchConstraintCount} con
          </span>
          <span
            className={cn(
              "uppercase",
              sketchStatus === "solved" && "text-emerald-400",
              sketchStatus === "error" && "text-red-400",
              sketchStatus === "over" && "text-orange-400",
              sketchStatus === "under" && "text-yellow-400",
            )}
          >
            [{sketchStatus}]
          </span>
        </div>
      )}

      {/* Middle: live cursor world coords (Z-up, mm) */}
      <div
        className={cn(
          "hidden sm:flex items-center gap-2 px-3 border-l border-border/40",
          "text-text-muted tabular-nums whitespace-pre",
          sketchActive && "hidden lg:flex",
        )}
        title="Cursor position on ground plane (mm)"
      >
        {cursorWorld ? (
          <>
            <span>
              <span className="text-brand">x</span>
              {formatCoord(cursorWorld.x)}
            </span>
            <span>
              <span className="text-brand">y</span>
              {formatCoord(cursorWorld.y)}
            </span>
            <span>
              <span className="text-brand">z</span>
              {formatCoord(cursorWorld.z)}
            </span>
          </>
        ) : (
          <span className="opacity-40">
            x       — y       — z       —
          </span>
        )}
      </div>

      {/* Right: doc metrics — save state now lives in the titlebar's DocTitle */}
      <div
        className={cn(
          "flex items-center gap-3 px-3 border-l border-border/40",
          "text-text-muted",
        )}
      >
        <span className="tabular-nums">
          {parts.length} {parts.length === 1 ? "part" : "parts"}
        </span>
        {selCount > 0 && (
          <span className="text-brand tabular-nums">{selCount} sel</span>
        )}
      </div>
    </div>
  );
}
