/**
 * Inline 3D Design-for-Manufacturing badges.
 *
 * Mounted inside the R3F scene transform group (next to SelectionOverlay
 * in ViewportContent). Each visible DFM issue gets a small floating
 * badge anchored at its world-space `anchor` point. Clicking the badge
 * opens a popover with the message + suggested fix, and selects the
 * issue in the dfm-store so the side panel can react.
 *
 * The component is intentionally thin — all heavy lifting (running the
 * checks, holding the report) lives in `dfm-store.ts`. It only reacts
 * to whatever the store currently holds.
 */

import { useMemo } from "react";
import { Html } from "@react-three/drei";
import { useDfmStore } from "@/stores/dfm-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import type { DfmIssue, DfmSeverity } from "@vcad/engine";
import { cn } from "@/lib/utils";

const SEVERITY_RING: Record<DfmSeverity, string> = {
  error: "bg-red-500/90 text-white ring-red-300",
  warning: "bg-amber-500/90 text-white ring-amber-200",
  info: "bg-sky-500/90 text-white ring-sky-200",
};

const SEVERITY_GLYPH: Record<DfmSeverity, string> = {
  error: "!",
  warning: "!",
  info: "i",
};

export function DfmAnnotations() {
  const enabled = useDfmStore((s) => s.enabled);
  const report = useDfmStore((s) => s.report);
  const visibleSeverities = useDfmStore((s) => s.visibleSeverities);
  const selectedId = useDfmStore((s) => s.selectedIssueId);
  const selectIssue = useDfmStore((s) => s.selectIssue);
  // DFM checks the mechanical model — its badges are meaningless (and visually
  // bleed through, via drei's portalled <Html>) while editing a circuit.
  const electronicsActive = useElectronicsStore((s) => s.active);

  // Filter + group issues at the same anchor so we don't stack a dozen
  // badges on one face. Quantize anchors to 0.5 mm. Filtering is done
  // here (not in a zustand selector) so the snapshot returned from the
  // store stays referentially stable — otherwise useSyncExternalStore
  // sees a new array every render and loops.
  const grouped = useMemo(() => {
    if (!report) return [];
    const issues = report.issues.filter((i) => visibleSeverities.has(i.severity));
    return groupByAnchor(issues);
  }, [report, visibleSeverities]);

  if (!enabled || grouped.length === 0 || electronicsActive) return null;

  return (
    <group>
      {grouped.map((group, idx) => {
        const lead = group[0];
        if (!lead) return null;
        // Account for vcad's Z-up → Three.js Y-up wrap (-90° X rotation
        // applied by the renderer wrapper).
        const position: [number, number, number] = [
          lead.anchor[0]!,
          lead.anchor[1]!,
          lead.anchor[2]!,
        ];
        const isSelected = group.some((i) => i.id === selectedId);
        return (
          <Html
            key={`dfm-${idx}`}
            position={position}
            center
            // Lift the selected group so its popover paints above sibling
            // badges that would otherwise overlap it.
            zIndexRange={isSelected ? [1000, 900] : [100, 0]}
            // Let wheel/drag events fall through to the canvas behind us so
            // the user can still orbit/zoom while pointing at a badge cluster.
            // Only the badge button opts back in to pointer-events.
            style={{ pointerEvents: "none" }}
          >
            <DfmBadge
              issues={group}
              selected={isSelected}
              onSelect={(id) => selectIssue(id === selectedId ? null : id)}
            />
          </Html>
        );
      })}
    </group>
  );
}

interface DfmBadgeProps {
  issues: DfmIssue[];
  selected: boolean;
  onSelect: (id: string) => void;
}

function DfmBadge({ issues, selected, onSelect }: DfmBadgeProps) {
  const top = highestSeverity(issues);
  const lead = issues.find((i) => i.severity === top) ?? issues[0];
  if (!lead) return null;
  const ringClass = SEVERITY_RING[top];
  const glyph = SEVERITY_GLYPH[top];
  const count = issues.length;

  return (
    <div className="flex items-start gap-2">
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onSelect(lead.id);
        }}
        className={cn(
          // pointer-events-auto reopens hits on the button itself; the parent
          // Html wrapper is pointer-events-none so wheel/drag fall through.
          "pointer-events-auto relative flex h-6 w-6 items-center justify-center rounded-full font-bold text-xs ring-2 shadow-md transition-transform",
          ringClass,
          selected ? "scale-125" : "hover:scale-110",
        )}
        title={lead.message}
      >
        {glyph}
        {count > 1 && (
          <span className="absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-surface px-1 text-[10px] font-semibold text-text ring-1 ring-border">
            {count}
          </span>
        )}
      </button>
      {selected && <DfmPopover issue={lead} extras={count - 1} />}
    </div>
  );
}

function DfmPopover({ issue, extras }: { issue: DfmIssue; extras: number }) {
  return (
    <div className="w-72 rounded-md border border-border bg-surface/95 backdrop-blur-sm p-3 shadow-lg text-text text-xs space-y-2">
      <div className="flex items-center justify-between gap-2">
        <p className="font-medium leading-snug">{issue.message}</p>
        <span
          className={cn(
            "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold capitalize",
            issue.severity === "error"
              ? "bg-red-500/20 text-red-300"
              : issue.severity === "warning"
                ? "bg-amber-500/20 text-amber-300"
                : "bg-sky-500/20 text-sky-300",
          )}
        >
          {issue.severity}
        </span>
      </div>
      {issue.explanation && (
        <p className="leading-relaxed text-text-muted">{issue.explanation}</p>
      )}
      <div className="flex gap-2 text-[11px] text-text-muted">
        <span>
          measured <strong className="text-text">{format(issue.measured)}</strong>{" "}
          {issue.units}
        </span>
        <span>
          limit <strong className="text-text">{format(issue.limit)}</strong>{" "}
          {issue.units}
        </span>
      </div>
      {issue.suggested_fix && (
        <div className="mt-1 rounded bg-surface-2 px-2 py-1 text-[11px]">
          <div className="font-semibold opacity-80">Suggested fix</div>
          <div className="opacity-90">{describeFix(issue.suggested_fix)}</div>
        </div>
      )}
      {extras > 0 && (
        <div className="text-[10px] text-text-muted italic">
          +{extras} more issue{extras > 1 ? "s" : ""} at this location
        </div>
      )}
    </div>
  );
}

function format(n: number): string {
  if (Math.abs(n) >= 100) return n.toFixed(0);
  if (Math.abs(n) >= 10) return n.toFixed(1);
  return n.toFixed(2);
}

function describeFix(fix: DfmIssue["suggested_fix"]): string {
  if (!fix) return "";
  switch (fix.type) {
    case "set_param":
      return `Set ${fix.path} = ${JSON.stringify(fix.value)} on node ${fix.node}`;
    case "wrap_op":
      return `Wrap node ${fix.node} with a new op`;
    case "replace_op":
      return `Replace node ${fix.node}`;
    case "manual":
      return fix.description;
  }
}

function highestSeverity(issues: DfmIssue[]): DfmSeverity {
  if (issues.some((i) => i.severity === "error")) return "error";
  if (issues.some((i) => i.severity === "warning")) return "warning";
  return "info";
}

function groupByAnchor(issues: DfmIssue[]): DfmIssue[][] {
  const buckets = new Map<string, DfmIssue[]>();
  for (const i of issues) {
    const key = `${q(i.anchor[0])}|${q(i.anchor[1])}|${q(i.anchor[2])}`;
    const arr = buckets.get(key);
    if (arr) arr.push(i);
    else buckets.set(key, [i]);
  }
  return Array.from(buckets.values());
}

function q(v: number): number {
  return Math.round(v * 2);
}
