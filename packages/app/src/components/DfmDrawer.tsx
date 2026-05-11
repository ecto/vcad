/**
 * Bottom-anchored Manufacturability drawer.
 *
 * The DFM panel used to live in the right sidebar where it competed
 * with the parts tree for vertical space. Moved to a bottom drawer that
 * the footer `DfmChip` opens and closes — same shape as an IDE
 * "Problems" pane: process picker on the left, severity filters in the
 * middle, scrollable issue list across the bottom.
 *
 * Live check is auto-enabled on first load (see `dfm-store`); the
 * drawer starts closed. Click the footer chip or use Tools →
 * Manufacturability to toggle.
 */

import { useEffect } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { useDocumentStore } from "@vcad/core";
import { useDfmStore, severityCounts } from "@/stores/dfm-store";
import type { DfmProcess, DfmSeverity } from "@vcad/engine";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const PROCESSES: { id: DfmProcess; label: string; group: string }[] = [
  { id: "cnc_3axis", label: "CNC 3-axis", group: "Subtractive" },
  { id: "fdm", label: "FDM", group: "Additive" },
  { id: "sla", label: "SLA", group: "Additive" },
  { id: "injection", label: "Injection", group: "Molding" },
  { id: "sheet_metal", label: "Sheet metal", group: "Forming" },
  { id: "casting_sand", label: "Sand casting", group: "Casting" },
  { id: "casting_investment", label: "Investment casting", group: "Casting" },
];

export function DfmDrawer() {
  const drawerOpen = useDfmStore((s) => s.drawerOpen);
  const setDrawerOpen = useDfmStore((s) => s.setDrawerOpen);
  const enabled = useDfmStore((s) => s.enabled);
  const setEnabled = useDfmStore((s) => s.setEnabled);
  const process = useDfmStore((s) => s.process);
  const setProcess = useDfmStore((s) => s.setProcess);
  const report = useDfmStore((s) => s.report);
  const running = useDfmStore((s) => s.running);
  const error = useDfmStore((s) => s.error);
  const visibleSeverities = useDfmStore((s) => s.visibleSeverities);
  const toggleSeverity = useDfmStore((s) => s.toggleSeverity);
  const selectedIssueId = useDfmStore((s) => s.selectedIssueId);
  const selectIssue = useDfmStore((s) => s.selectIssue);
  const scheduleRun = useDfmStore((s) => s.scheduleRun);

  // Subscribe to document changes — every IR mutation re-runs DFM.
  const document = useDocumentStore((s) => s.document);

  useEffect(() => {
    if (!enabled || !document) return;
    scheduleRun(document);
  }, [enabled, process, document, scheduleRun]);

  if (!drawerOpen) return null;

  const counts = severityCounts(report);
  const total = counts.error + counts.warning + counts.info;

  return (
    <div
      className={cn(
        "absolute inset-x-0 bottom-0 z-20",
        "border-t border-border/60 bg-surface/95 backdrop-blur-md",
        "flex h-72 flex-col text-text shadow-[0_-4px_24px_-12px_rgba(0,0,0,0.5)]",
        "animate-in slide-in-from-bottom-4 fade-in duration-150",
      )}
    >
      {/* Toolbar row: title, process, severity filters, live-check toggle, close */}
      <div className="flex items-center gap-3 border-b border-border/40 px-3 py-1.5">
        <h3 className="text-xs font-semibold uppercase tracking-wide">
          Manufacturability
        </h3>

        <select
          value={process}
          onChange={(e) => setProcess(e.target.value as DfmProcess)}
          className="rounded border border-border bg-surface px-2 py-0.5 text-xs"
          disabled={!enabled}
        >
          {PROCESSES.map((p) => (
            <option key={p.id} value={p.id}>
              {p.group} · {p.label}
            </option>
          ))}
        </select>

        <div className="flex items-center gap-1.5">
          <SeverityChip
            kind="error"
            count={counts.error}
            active={visibleSeverities.has("error")}
            onClick={() => toggleSeverity("error")}
          />
          <SeverityChip
            kind="warning"
            count={counts.warning}
            active={visibleSeverities.has("warning")}
            onClick={() => toggleSeverity("warning")}
          />
          <SeverityChip
            kind="info"
            count={counts.info}
            active={visibleSeverities.has("info")}
            onClick={() => toggleSeverity("info")}
          />
          {running && (
            <span className="text-text-muted text-[10px]">checking…</span>
          )}
        </div>

        <div className="flex-1" />

        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          Live check
        </label>

        <Tooltip content="Close" side="top">
          <button
            type="button"
            onClick={() => setDrawerOpen(false)}
            className="rounded p-1 text-text-muted hover:bg-hover hover:text-text"
            aria-label="Close manufacturability drawer"
          >
            <X size={12} weight="bold" />
          </button>
        </Tooltip>
      </div>

      {/* Body */}
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {!enabled && (
          <div className="rounded border border-border/40 bg-surface p-3 text-xs text-text-muted">
            Live check is off. Enable it to scan the model for
            manufacturability issues against the selected process.
          </div>
        )}

        {enabled && error && (
          <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-[11px] text-red-300">
            {error}
          </div>
        )}

        {enabled && report && total === 0 && !running && !error && (
          <div className="rounded bg-emerald-500/10 p-3 text-xs text-emerald-300">
            No issues — manufacturable as-is.
          </div>
        )}

        {enabled && report && total > 0 && (
          <ul className="grid grid-cols-1 gap-1 md:grid-cols-2 xl:grid-cols-3">
            {report.issues.map((issue) => {
              if (!visibleSeverities.has(issue.severity)) return null;
              const isSel = issue.id === selectedIssueId;
              return (
                <li key={issue.id}>
                  <button
                    type="button"
                    onClick={() => selectIssue(isSel ? null : issue.id)}
                    className={cn(
                      "w-full rounded border border-border/40 bg-surface px-2 py-1.5 text-left text-[11px] hover:border-border",
                      isSel && "border-brand bg-brand/10",
                    )}
                  >
                    <div className="flex items-center gap-1.5">
                      <SeverityDot kind={issue.severity} />
                      <span className="flex-1 truncate font-medium">
                        {issue.message}
                      </span>
                    </div>
                    <div className="mt-0.5 text-[10px] text-text-muted">
                      {issue.rule}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}

function SeverityChip({
  kind,
  count,
  active,
  onClick,
}: {
  kind: DfmSeverity;
  count: number;
  active: boolean;
  onClick: () => void;
}) {
  const cls =
    kind === "error"
      ? "bg-red-500/20 text-red-300 border-red-500/40"
      : kind === "warning"
        ? "bg-amber-500/20 text-amber-300 border-amber-500/40"
        : "bg-sky-500/20 text-sky-300 border-sky-500/40";
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "rounded-full border px-2 py-0.5 text-[10px] font-medium transition-opacity",
        cls,
        active ? "opacity-100" : "opacity-40",
      )}
    >
      {count} {kind}
    </button>
  );
}

function SeverityDot({ kind }: { kind: DfmSeverity }) {
  const cls =
    kind === "error"
      ? "bg-red-500"
      : kind === "warning"
        ? "bg-amber-500"
        : "bg-sky-500";
  return <span className={cn("inline-block h-2 w-2 rounded-full", cls)} />;
}
