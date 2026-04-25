import { useEffect, useMemo, useState } from "react";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Terminal } from "@phosphor-icons/react/dist/ssr/Terminal";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { GlobeSimple } from "@phosphor-icons/react/dist/ssr/GlobeSimple";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import * as Popover from "@radix-ui/react-popover";
import { useDocumentStore, useUiStore, useSketchStore, t, tFmt, type LogLevelName } from "@vcad/core";
import { useLocaleStore, supportedLocales, type SupportedLocale } from "@/stores/locale-store";
import { useLogStore, getFilteredEntries } from "@/stores/log-store";
import { FooterUsageMeter } from "@/components/FooterUsageMeter";
import { cn } from "@/lib/utils";
import { useCapabilities } from "@/lib/capabilities";

const LOCALE_LABELS: Record<string, string> = {
  en: "English",
  es: "Español",
  fr: "Français",
};

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
  if (diff < 2000) return t("status.ago.now");
  if (diff < 60_000) return tFmt("status.ago.seconds", { count: String(Math.floor(diff / 1000)) });
  if (diff < 3_600_000) return tFmt("status.ago.minutes", { count: String(Math.floor(diff / 60_000)) });
  return tFmt("status.ago.hours", { count: String(Math.floor(diff / 3_600_000)) });
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
  useLocaleStore((s) => s.locale);

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
  const { tauri, platform } = useCapabilities();
  const macOverlay = tauri && platform === "mac";

  return (
    <div
      data-tauri-drag-region={macOverlay ? "" : undefined}
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
        title={latest ? t("status.console_open") : t("status.console_empty_title")}
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
        title={t("status.cursor_pos")}
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
          {tFmt(parts.length === 1 ? "status.part" : "status.parts", {
            count: String(parts.length),
          })}
        </span>
        {selCount > 0 && (
          <span className="text-brand tabular-nums">
            {tFmt("status.sel", { count: String(selCount) })}
          </span>
        )}
      </div>

      <FooterUsageMeter />

      <LocalePicker />
    </div>
  );
}

function LocalePicker() {
  const locale = useLocaleStore((s) => s.locale);
  const setLoc = useLocaleStore((s) => s.setLocale);
  const locales = supportedLocales();

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className={cn(
            "flex items-center gap-1 px-2 border-l border-border/40",
            "text-text-muted hover:text-text hover:bg-hover transition-colors",
          )}
          title={t("status.language")}
        >
          <GlobeSimple size={11} className="shrink-0" />
          <span className="uppercase tracking-wide">{locale}</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={4}
          className={cn(
            "z-50 min-w-[140px] rounded-md border border-border bg-surface p-1 shadow-lg",
            "animate-in fade-in slide-in-from-bottom-2 duration-150",
            "text-[11px] font-mono",
          )}
        >
          {locales.map((loc) => (
            <button
              key={loc}
              type="button"
              onClick={() => setLoc(loc as SupportedLocale)}
              className={cn(
                "flex w-full items-center gap-2 rounded px-2 py-1",
                "hover:bg-hover transition-colors",
                loc === locale ? "text-text" : "text-text-muted",
              )}
            >
              <span className="w-3">
                {loc === locale && <Check size={10} weight="bold" className="text-brand" />}
              </span>
              <span className="uppercase tracking-wide w-5">{loc}</span>
              <span className="text-text-muted">{LOCALE_LABELS[loc] ?? loc}</span>
            </button>
          ))}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
