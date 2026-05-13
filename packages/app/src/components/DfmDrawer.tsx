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

import { useEffect, useMemo, useState } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { useDocumentStore } from "@vcad/core";
import { useDfmStore, severityCounts } from "@/stores/dfm-store";
import type { DfmIssue, DfmProcess, DfmSeverity } from "@vcad/engine";
import type { CsgOp, Document, NodeId } from "@vcad/ir";
import { Tooltip } from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
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

  // node → human-readable part label. Resolved once per document mutation;
  // used to give each issue row a "where" instead of leaving it anonymous.
  const nodeLabels = useMemo(
    () => (document ? buildNodeLabelMap(document) : new Map<NodeId, string>()),
    [document],
  );

  // Group identical rule firings into one collapsible row. 90 "Draft 0.0°"
  // issues become one row "Draft <3° — 90 places" until expanded.
  const groups = useMemo(() => {
    if (!report) return [];
    const visible = report.issues.filter((i) =>
      visibleSeverities.has(i.severity),
    );
    return groupByRule(visible);
  }, [report, visibleSeverities]);

  const [expandedRules, setExpandedRules] = useState<Set<string>>(new Set());
  const toggleRule = (rule: string) =>
    setExpandedRules((prev) => {
      const next = new Set(prev);
      if (next.has(rule)) next.delete(rule);
      else next.add(rule);
      return next;
    });

  if (!drawerOpen) return null;

  const counts = severityCounts(report);
  const total = counts.error + counts.warning + counts.info;

  return (
    <div
      className={cn(
        "absolute inset-x-0 bottom-0 z-30",
        "border-t border-border/60 bg-surface/95 backdrop-blur-md",
        "flex h-72 flex-col text-text shadow-[0_-4px_24px_-12px_rgba(0,0,0,0.5)]",
        "animate-in slide-in-from-bottom-4 fade-in duration-150",
      )}
    >
      {/* Toolbar row: title, process, severity filters, live-check toggle, close */}
      <div className="flex items-center gap-3 border-b border-border/40 px-4 py-2">
        <h3 className="text-[13px] font-medium tracking-tight text-text">
          Manufacturability
        </h3>

        <Separator orientation="vertical" className="h-4 bg-border/60" />

        <select
          value={process}
          onChange={(e) => setProcess(e.target.value as DfmProcess)}
          className={cn(
            "h-7 rounded-md border border-border/60 bg-bg/40 px-2 text-xs text-text",
            "transition-colors hover:border-border hover:bg-bg/60",
            "focus:outline-none focus:ring-1 focus:ring-brand/40",
            "disabled:opacity-50",
          )}
          disabled={!enabled}
        >
          {PROCESSES.map((p) => (
            <option key={p.id} value={p.id}>
              {p.group} · {p.label}
            </option>
          ))}
        </select>

        <div className="flex items-center gap-1">
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
            <span className="ml-1 text-[11px] text-text-muted/70 italic">
              checking…
            </span>
          )}
        </div>

        <div className="flex-1" />

        <EnabledToggle enabled={enabled} onChange={setEnabled} />

        <Tooltip content="Close" side="top">
          <button
            type="button"
            onClick={() => setDrawerOpen(false)}
            className="rounded-md p-1.5 text-text-muted transition-colors hover:bg-hover hover:text-text"
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
            Manufacturability checks are disabled. Re-enable to scan the
            model against the selected process.
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
          <ul className="divide-y divide-border/30 rounded border border-border/40">
            {groups.map((g) => {
              const expanded = expandedRules.has(g.rule);
              return (
                <li key={g.rule}>
                  <button
                    type="button"
                    onClick={() => toggleRule(g.rule)}
                    className="flex w-full items-center gap-2 px-2 py-1.5 text-left text-[11px] hover:bg-hover"
                    aria-expanded={expanded}
                  >
                    <CaretRight
                      size={10}
                      weight="bold"
                      className={cn(
                        "shrink-0 text-text-muted transition-transform",
                        expanded && "rotate-90",
                      )}
                    />
                    <SeverityDot kind={g.severity} />
                    <span className="flex-1 truncate font-medium">
                      {g.headline}
                    </span>
                    <span className="shrink-0 text-[10px] text-text-muted">
                      {g.count} {g.count === 1 ? "place" : "places"}
                    </span>
                  </button>
                  {expanded && (
                    <ul className="divide-y divide-border/20 border-t border-border/20 bg-bg/40">
                      {g.issues.map((issue) => {
                        const isSel = issue.id === selectedIssueId;
                        const part = issue.origin_op != null
                          ? nodeLabels.get(issue.origin_op) ?? null
                          : null;
                        return (
                          <li key={issue.id}>
                            <button
                              type="button"
                              onClick={() =>
                                selectIssue(isSel ? null : issue.id)
                              }
                              className={cn(
                                "flex w-full items-center gap-2 pl-7 pr-2 py-1 text-left text-[11px] hover:bg-hover",
                                isSel && "bg-brand/10",
                              )}
                            >
                              <span className="flex-1 truncate">
                                {part ?? <span className="text-text-muted italic">unattributed</span>}
                              </span>
                              <span className="shrink-0 text-[10px] text-text-muted tabular-nums">
                                {formatMeasurement(issue)}
                              </span>
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  )}
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
  const dotCls =
    kind === "error"
      ? "bg-red-400"
      : kind === "warning"
        ? "bg-amber-400"
        : "bg-sky-400";
  const label = kind[0]!.toUpperCase() + kind.slice(1);
  return (
    <Tooltip content={`${count} ${label.toLowerCase()} · click to filter`} side="top">
      <button
        type="button"
        onClick={onClick}
        aria-pressed={active}
        className={cn(
          "flex h-6 items-center gap-1.5 rounded-md border px-2 text-[11px] tabular-nums transition-all",
          active
            ? "border-border/60 bg-bg/40 text-text"
            : "border-transparent bg-transparent text-text-muted/50 hover:text-text-muted",
        )}
      >
        <span
          className={cn(
            "h-1.5 w-1.5 rounded-full transition-opacity",
            dotCls,
            active ? "opacity-100" : "opacity-40",
          )}
        />
        {count}
      </button>
    </Tooltip>
  );
}

function EnabledToggle({
  enabled,
  onChange,
}: {
  enabled: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-xs text-text-muted hover:text-text">
      <span className="select-none">{enabled ? "On" : "Off"}</span>
      <button
        type="button"
        role="switch"
        aria-checked={enabled}
        onClick={() => onChange(!enabled)}
        className={cn(
          "relative inline-flex h-4 w-7 shrink-0 items-center rounded-full p-0.5 transition-colors",
          "focus:outline-none focus:ring-1 focus:ring-brand/40",
          enabled ? "bg-emerald-500" : "bg-border",
        )}
      >
        <span
          className={cn(
            "h-3 w-3 rounded-full bg-white shadow-sm transition-transform",
            enabled ? "translate-x-3" : "translate-x-0",
          )}
        />
      </button>
    </label>
  );
}

interface RuleGroup {
  rule: string;
  severity: DfmSeverity;
  headline: string;
  count: number;
  issues: DfmIssue[];
}

const SEVERITY_RANK: Record<DfmSeverity, number> = {
  error: 0,
  warning: 1,
  info: 2,
};

function groupByRule(issues: DfmIssue[]): RuleGroup[] {
  const buckets = new Map<string, DfmIssue[]>();
  for (const i of issues) {
    const arr = buckets.get(i.rule);
    if (arr) arr.push(i);
    else buckets.set(i.rule, [i]);
  }
  const groups: RuleGroup[] = [];
  for (const [rule, arr] of buckets) {
    const top = arr.reduce(
      (acc, x) =>
        SEVERITY_RANK[x.severity] < SEVERITY_RANK[acc.severity] ? x : acc,
      arr[0]!,
    );
    groups.push({
      rule,
      severity: top.severity,
      headline: top.message,
      count: arr.length,
      issues: arr,
    });
  }
  // Errors first, then by count desc (loudest rules surface to the top).
  groups.sort((a, b) => {
    const s = SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity];
    return s !== 0 ? s : b.count - a.count;
  });
  return groups;
}

/**
 * Build a `nodeId → "part name"` lookup so each issue can name *where* it
 * lives instead of leaving the user to guess. Walks every PartDef's root
 * subtree and tags each visited node with the best display name we have —
 * the first instance using that part def, or the part def's own name.
 */
function buildNodeLabelMap(doc: Document): Map<NodeId, string> {
  const labels = new Map<NodeId, string>();
  if (!doc.partDefs) return labels;

  // First instance per partDef wins — assemblies often have many instances
  // of one def, and any of their names is a useful "where this lives" hint.
  const instanceByPartDef = new Map<string, string>();
  for (const inst of doc.instances ?? []) {
    if (inst.name && !instanceByPartDef.has(inst.partDefId)) {
      instanceByPartDef.set(inst.partDefId, inst.name);
    }
  }

  for (const pdef of Object.values(doc.partDefs)) {
    const label =
      instanceByPartDef.get(pdef.id) ?? pdef.name ?? `part ${pdef.id}`;
    walkNodes(doc, pdef.root, (nodeId, nodeName) => {
      // Prefer a node-local name when present (sketches, named ops); fall
      // back to the part label otherwise.
      labels.set(nodeId, nodeName ?? label);
    });
  }
  return labels;
}

function walkNodes(
  doc: Document,
  rootId: NodeId,
  visit: (id: NodeId, name: string | null) => void,
): void {
  const stack: NodeId[] = [rootId];
  const seen = new Set<NodeId>();
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const node = doc.nodes[String(id)];
    if (!node) continue;
    visit(id, node.name);
    for (const child of opChildren(node.op)) stack.push(child);
  }
}

function opChildren(op: CsgOp): NodeId[] {
  switch (op.type) {
    case "Union":
    case "Difference":
    case "Intersection":
      return [op.left, op.right];
    case "Translate":
    case "Rotate":
    case "Scale":
    case "LinearPattern":
    case "CircularPattern":
    case "Shell":
    case "Fillet":
    case "Chamfer":
      return [op.child];
    case "Extrude":
    case "Revolve":
    case "Sweep":
      return [op.sketch];
    case "Loft":
      return op.sketches;
    default:
      return [];
  }
}

function formatMeasurement(issue: DfmIssue): string {
  const { measured, limit, units } = issue;
  if (!Number.isFinite(measured) || !Number.isFinite(limit)) return "";
  const u = units ? ` ${units}` : "";
  return `${formatNum(measured)}${u} measured · ${formatNum(limit)}${u} required`;
}

function formatNum(n: number): string {
  if (Math.abs(n) >= 100) return n.toFixed(0);
  if (Math.abs(n) >= 10) return n.toFixed(1);
  return n.toFixed(2);
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
