import { useEffect, useMemo, useState } from "react";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Terminal } from "@phosphor-icons/react/dist/ssr/Terminal";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { GlobeSimple } from "@phosphor-icons/react/dist/ssr/GlobeSimple";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { CrosshairSimple } from "@phosphor-icons/react/dist/ssr/CrosshairSimple";
import * as Popover from "@radix-ui/react-popover";
import { useDocumentStore, useUiStore, useSketchStore, t, tFmt, type LogLevelName, SELECTION_FILTER_OPTIONS } from "@vcad/core";
import { useLocaleStore, supportedLocales, type SupportedLocale } from "@/stores/locale-store";
import { useDrawingStore } from "@/stores/drawing-store";
import { useLogStore, getFilteredEntries } from "@/stores/log-store";
import { FooterUsageMeter } from "@/components/FooterUsageMeter";
import { FooterChip, FooterChipButton } from "@/components/footer/FooterChip";
import { CursorCoordChip } from "@/components/footer/CursorCoordChip";
import { KernelPulseChip } from "@/components/footer/KernelPulseChip";
import { JobsChip } from "@/components/footer/JobsChip";
import { KoanChip } from "@/components/footer/KoanChip";
import { RaytraceChip } from "@/components/footer/RaytraceChip";
import { DfmChip } from "@/components/footer/DfmChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useCapabilities } from "@/lib/capabilities";

const LOCALE_LABELS: Record<string, string> = {
  cs: "Čeština",
  de: "Deutsch",
  en: "English",
  es: "Español",
  fr: "Français",
  it: "Italiano",
  ja: "日本語",
  ko: "한국어",
  nl: "Nederlands",
  pl: "Polski",
  pt: "Português",
  ru: "Русский",
  tr: "Türkçe",
  zh: "中文",
  "zh-tw": "繁體中文",
};

const LEVEL_COLOR: Record<LogLevelName, string> = {
  DEBUG: "text-text-muted",
  INFO: "text-blue-400",
  WARN: "text-yellow-400",
  ERROR: "text-red-400",
};

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
 * Composed from FooterChip primitives. Left → right:
 * - Koan slot: selection summary, doc stats, or rotating idle koan.
 * - Console ticker (expands to fill): latest log entry, click opens panel.
 * - Sketch ribbon (when active): plane, cursor, snap, counts, solver state.
 * - Cursor world coords (Z-up, with click-to-cycle unit and click-to-pan).
 * - Active job (when running) → usage meter → locale picker.
 * - Kernel pulse + frame-ms sparkline (rightmost, system telemetry).
 */
export function StatusBar() {
  const parts = useDocumentStore((s) => s.parts);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const selection = useUiStore((s) => s.selection);
  useLocaleStore((s) => s.locale);

  const sketchActive = useSketchStore((s) => s.active);
  const sketchPlane = useSketchStore((s) => s.plane);
  const drafting2d = useDrawingStore((s) => s.viewMode === "2d");
  const toolbarTab = useUiStore((s) => s.toolbarTab);
  const sketchCursor = useSketchStore((s) => s.cursorSketchPos);
  const sketchSnap = useSketchStore((s) => s.snapTarget);
  const sketchSegmentCount = useSketchStore((s) => s.segments.length);
  const sketchConstraintCount = useSketchStore((s) => s.constraints.length);
  const sketchStatus = useSketchStore((s) => s.constraintStatus);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);

  const entries = useLogStore((s) => s.entries);
  const minLevel = useLogStore((s) => s.minLevel);
  const enabledSources = useLogStore((s) => s.enabledSources);
  const togglePanel = useLogStore((s) => s.togglePanel);

  const latest = useMemo(() => {
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

  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const fresh = latest && now - latest.timestamp < 3000;
  const selCount = selectedPartIds.size;

  /**
   * One-line summary of what's currently selected. Prefers sub-feature
   * detail (face / edge / vertex) over a raw part count when the user
   * has picked something more specific. Shown in the right-edge cluster
   * so the existing parts-stat copy keeps its place.
   */
  const subFeatureLabel = (() => {
    const subItems = selection.filter(
      (it) => it.kind === "face" || it.kind === "edge" || it.kind === "vertex",
    );
    if (subItems.length === 0) return null;
    if (subItems.length === 1) {
      const it = subItems[0]!;
      const partName =
        parts.find((p) => p.id === (it as { partId: string }).partId)?.name ??
        "part";
      if (it.kind === "face") return `face on ${partName}`;
      if (it.kind === "edge") return `edge on ${partName}`;
      if (it.kind === "vertex") return `vertex on ${partName}`;
    }
    // Multi-select — name the kinds.
    const counts = { face: 0, edge: 0, vertex: 0 };
    for (const it of subItems) {
      counts[it.kind as "face" | "edge" | "vertex"]++;
    }
    const parts2 = (Object.entries(counts) as [keyof typeof counts, number][])
      .filter(([, n]) => n > 0)
      .map(([k, n]) => `${n} ${k}${n === 1 ? "" : "s"}`);
    return parts2.join(", ");
  })();
  const levelColor = latest ? LEVEL_COLOR[latest.level] : "";
  const { tauri, platform } = useCapabilities();
  const macOverlay = tauri && platform === "mac";

  return (
    <div
      data-tauri-drag-region={macOverlay ? "" : undefined}
      className={cn(
        "vcad-footer flex h-6 items-stretch bg-surface select-none",
        "border-t border-border/40",
      )}
    >
      {/* Leftmost slot: sketch ribbon when sketching, koan / mode badge
          otherwise. Both share the no-leading-divider position so the
          chrome stays consistent regardless of mode. */}
      {sketchActive ? (
        <Tooltip
          side="top"
          content="Active sketch — plane, cursor, snap, entity / constraint counts, solver state"
        >
          <FooterChip
            divider={false}
            severity="warn"
            className="tabular-nums"
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
          </FooterChip>
        </Tooltip>
      ) : (
        <KoanChip
          divider={false}
          mode={
            drafting2d
              ? "drafting"
              : toolbarTab === "assembly"
                ? "assembly"
                : null
          }
        />
      )}

      <Tooltip
        side="top"
        content={
          latest
            ? "Latest console message — click to open the panel"
            : "Console is empty — click to open the panel"
        }
      >
      <FooterChipButton flex onClick={togglePanel}>
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
            <span className="hidden md:inline shrink-0 text-text-muted/70">
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
      </FooterChipButton>
      </Tooltip>

      <CursorCoordChip className={cn(sketchActive && "hidden lg:flex")} />

      <JobsChip />

      <RaytraceChip />

      <DfmChip />

      <FooterChip className="gap-3">
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
        {subFeatureLabel && (
          <span className="text-brand">{subFeatureLabel}</span>
        )}
      </FooterChip>

      <SelectionFilterChips />

      <FooterUsageMeter />

      <LocalePicker />

      <KernelPulseChip className="hidden md:flex" />
    </div>
  );
}

/**
 * Selection filter chip — single popover that shows the current pick mode
 * (auto / body / face / edge / vertex) and lets you switch without taking
 * over a digit hotkey. The toolbar tab strip owns 1–7, so we don't fight
 * for them here. Options come from `SELECTION_FILTER_OPTIONS` in core so the
 * right-click context menu and any future surface stay in sync.
 */
function SelectionFilterChips() {
  const filter = useUiStore((s) => s.selectionFilter);
  const setFilter = useUiStore((s) => s.setSelectionFilter);
  const current = SELECTION_FILTER_OPTIONS.find((o) => o.value === filter) ?? SELECTION_FILTER_OPTIONS[0]!;
  return (
    <Popover.Root>
      <Tooltip side="top" content="Pick mode — what hover and click target">
        <Popover.Trigger asChild>
          <FooterChipButton className="gap-1 px-2">
            <CrosshairSimple size={11} className="shrink-0" />
            <span className="uppercase tracking-wide">{current.label}</span>
          </FooterChipButton>
        </Popover.Trigger>
      </Tooltip>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={6}
          collisionPadding={8}
          className={cn(
            "z-50 w-[260px]",
            "rounded-md border border-border/60 bg-surface/95 backdrop-blur-md",
            "p-1 shadow-xl",
            "animate-in fade-in slide-in-from-bottom-2 duration-150",
            "text-[11px]",
          )}
        >
          <div className="px-2 pt-1 pb-1.5 text-text-muted/60 uppercase tracking-[0.15em] text-[9px]">
            Pick mode
          </div>
          {SELECTION_FILTER_OPTIONS.map(({ value, label, hint, hotkey }) => {
            const active = filter === value;
            return (
              <button
                key={value}
                type="button"
                onClick={() => setFilter(value)}
                className={cn(
                  "flex w-full items-start gap-2 rounded-sm px-2 py-1.5 text-left",
                  "transition-colors",
                  active
                    ? "bg-brand/15 text-text"
                    : "text-text-muted hover:bg-hover hover:text-text",
                )}
              >
                <span className="flex-1">
                  <span
                    className={cn(
                      "block uppercase tracking-wide text-[10px]",
                      active ? "text-brand" : "text-text",
                    )}
                  >
                    {label}
                  </span>
                  <span className="block text-[10px] text-text-muted/70">
                    {hint}
                  </span>
                </span>
                {hotkey && (
                  <kbd
                    className={cn(
                      "mt-0.5 shrink-0 rounded-sm border border-border/60 bg-surface px-1 py-px",
                      "font-mono text-[9px] uppercase tracking-wide",
                      active ? "text-brand" : "text-text-muted/70",
                    )}
                  >
                    {hotkey}
                  </kbd>
                )}
                {active && (
                  <Check size={11} weight="bold" className="mt-0.5 shrink-0 text-brand" />
                )}
              </button>
            );
          })}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

function LocalePicker() {
  const locale = useLocaleStore((s) => s.locale);
  const setLoc = useLocaleStore((s) => s.setLocale);
  const locales = supportedLocales();

  return (
    <Popover.Root>
      <Tooltip side="top" content="Display language — click to switch">
        <Popover.Trigger asChild>
          <FooterChipButton className="gap-1 px-2">
            <GlobeSimple size={11} className="shrink-0" />
            <span className="uppercase tracking-wide">{locale}</span>
          </FooterChipButton>
        </Popover.Trigger>
      </Tooltip>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={6}
          collisionPadding={8}
          className={cn(
            "z-50 w-[200px] max-h-[60vh] overflow-y-auto",
            "rounded-md border border-border/60 bg-surface/95 backdrop-blur-md",
            "p-1 shadow-xl",
            "animate-in fade-in slide-in-from-bottom-2 duration-150",
            "text-[11px] font-mono",
          )}
        >
          <div className="px-2 pt-1 pb-1.5 text-text-muted/60 uppercase tracking-[0.15em] text-[9px]">
            {t("status.language")}
          </div>
          {locales.map((loc) => {
            const isActive = loc === locale;
            return (
              <button
                key={loc}
                type="button"
                onClick={() => setLoc(loc as SupportedLocale)}
                className={cn(
                  "flex w-full items-center gap-2 rounded-sm px-2 py-1.5",
                  "transition-colors",
                  isActive
                    ? "bg-brand/15 text-text"
                    : "text-text-muted hover:bg-hover hover:text-text",
                )}
              >
                <span
                  className={cn(
                    "shrink-0 w-7 text-[10px] uppercase tracking-wide tabular-nums",
                    isActive ? "text-brand" : "text-text-muted/70",
                  )}
                >
                  {loc}
                </span>
                <span className="flex-1 text-left whitespace-nowrap">
                  {LOCALE_LABELS[loc] ?? loc}
                </span>
                {isActive && (
                  <Check size={11} weight="bold" className="shrink-0 text-brand" />
                )}
              </button>
            );
          })}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
