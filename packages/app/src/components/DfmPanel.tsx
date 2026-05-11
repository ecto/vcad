/**
 * DFM control + summary panel.
 *
 * Sits in the right sidebar; lets the user pick the target process,
 * toggle severity visibility, and click an issue in the list to focus
 * it in the viewport. The actual badges live in `DfmAnnotations`; this
 * panel is the editorial side of the same data.
 *
 * The panel is hidden until the user enables DFM (master toggle).
 * That keeps the sidebar uncluttered for users who don't care about
 * manufacturability.
 */

import { useEffect } from "react";
import { useDocumentStore } from "@vcad/core";
import { useDfmStore, severityCounts } from "@/stores/dfm-store";
import type { DfmProcess, DfmSeverity } from "@vcad/engine";
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

export function DfmPanel() {
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
  // useDocumentStore exposes the canonical IR as `document`.
  const document = useDocumentStore((s) => s.document);

  useEffect(() => {
    if (!enabled || !document) return;
    scheduleRun(document);
  }, [enabled, process, document, scheduleRun]);

  const counts = severityCounts(report);
  const total = counts.error + counts.warning + counts.info;

  return (
    <div className="flex h-full flex-col gap-2 p-3 text-text">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold">Manufacturability</h3>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          Live check
        </label>
      </div>

      {enabled && (
        <>
          <select
            value={process}
            onChange={(e) => setProcess(e.target.value as DfmProcess)}
            className="rounded border border-border bg-surface px-2 py-1 text-xs"
          >
            {PROCESSES.map((p) => (
              <option key={p.id} value={p.id}>
                {p.group} · {p.label}
              </option>
            ))}
          </select>

          <div className="flex items-center gap-2 text-xs">
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

          {error && (
            <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-[11px] text-red-300">
              {error}
            </div>
          )}

          {report && total === 0 && !running && (
            <div className="rounded bg-emerald-500/10 p-2 text-xs text-emerald-300">
              No issues — manufacturable as-is.
            </div>
          )}

          <div className="flex-1 overflow-y-auto -mx-1 px-1">
            <ul className="space-y-1">
              {report?.issues.map((issue) => {
                if (!visibleSeverities.has(issue.severity)) return null;
                const isSel = issue.id === selectedIssueId;
                return (
                  <li key={issue.id}>
                    <button
                      type="button"
                      onClick={() =>
                        selectIssue(isSel ? null : issue.id)
                      }
                      className={cn(
                        "w-full rounded border border-border/50 bg-surface px-2 py-1.5 text-left text-[11px] hover:border-border",
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
          </div>
        </>
      )}
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
