/**
 * The live Receipt ledger (#280): a floating panel showing one attributed entry
 * per board mutation that changed DRC — the per-rule before/after delta, the
 * credit/blame attribution (footprint vs routing), and a verdict badge. Entries
 * are produced by useReceiptRecorder from @vcad/core's engine.
 */

import type { ReactNode } from "react";
import { useElectronicsStore } from "@/stores/electronics-store";
import type { ReceiptEntry, ViolationGroup, Verdict, Cause } from "@vcad/core";

const VERDICT: Record<Verdict, { label: string; cls: string }> = {
  regression: { label: "Regression", cls: "bg-red-500/20 text-red-300" },
  "improved-with-regressions": { label: "Improved · regressions", cls: "bg-amber-500/20 text-amber-300" },
  improved: { label: "Improved", cls: "bg-emerald-500/20 text-emerald-300" },
  clean: { label: "Clean", cls: "bg-emerald-500/20 text-emerald-300" },
  "no-op": { label: "No change", cls: "bg-white/10 text-text-muted" },
};

const CAUSE_LABEL: Record<Cause, string> = {
  footprint: "footprint",
  placement: "placement",
  routing: "routing",
  via: "via",
  connectivity: "unrouted",
  unknown: "other",
};

const signed = (n: number) => (n > 0 ? `+${n}` : `${n}`);

/** Collapse violation groups to one entry per cause, summed. */
function byCause(groups: ViolationGroup[]): Array<{ cause: Cause; count: number }> {
  const m = new Map<Cause, number>();
  for (const g of groups) m.set(g.cause, (m.get(g.cause) ?? 0) + g.count);
  return [...m.entries()].map(([cause, count]) => ({ cause, count })).sort((a, b) => b.count - a.count);
}

function ruleRows(e: ReceiptEntry) {
  const rules = new Set([...Object.keys(e.before.byRule), ...Object.keys(e.after.byRule)]);
  return [...rules]
    .map((rule) => ({
      rule,
      b: e.before.byRule[rule] ?? 0,
      a: e.after.byRule[rule] ?? 0,
      d: (e.after.byRule[rule] ?? 0) - (e.before.byRule[rule] ?? 0),
    }))
    .filter((r) => r.d !== 0)
    .sort((x, y) => Math.abs(y.d) - Math.abs(x.d));
}

function Chip({ children, cls }: { children: ReactNode; cls: string }) {
  return <span className={`px-1.5 py-0.5 rounded border text-[9.5px] whitespace-nowrap ${cls}`}>{children}</span>;
}

function Entry({ e }: { e: ReceiptEntry }) {
  const v = VERDICT[e.verdict] ?? VERDICT["no-op"];
  const rows = ruleRows(e);
  const fixed = byCause(e.fixed);
  const introduced = byCause(e.introduced);
  const preExisting = byCause(e.persisted.filter((g) => g.blame === "pre-existing"));
  return (
    <div className="rounded-md border border-border/60 bg-black/10 p-2">
      <div className="flex justify-between items-center mb-1.5">
        <span className="font-medium text-text font-mono">{e.tool}</span>
        <span className={`px-1.5 py-0.5 rounded text-[9.5px] font-medium ${v.cls}`}>{v.label}</span>
      </div>
      <div className="text-text-muted mb-1.5">
        DRC {e.before.errors} → {e.after.errors}{" "}
        <span className="opacity-60">({signed(e.deltaTotal)})</span>
      </div>
      {e.tally.shortsIntroduced > 0 && (
        <div className="mb-1.5 px-1.5 py-1 rounded bg-red-500/15 text-red-300 text-[10px]">
          ⚠ {e.tally.shortsIntroduced} hard short{e.tally.shortsIntroduced > 1 ? "s" : ""} introduced — board electrically broken
        </div>
      )}
      {rows.length > 0 && (
        <div className="space-y-0.5 mb-1.5">
          {rows.map((r) => (
            <div key={r.rule} className="flex justify-between">
              <span className="text-text-muted">{r.rule}</span>
              <span className="tabular-nums">
                <span className="opacity-60">{r.b}→{r.a}</span>{" "}
                <span className={r.d < 0 ? "text-emerald-400" : "text-red-400"}>{signed(r.d)}</span>
              </span>
            </div>
          ))}
        </div>
      )}
      <div className="flex flex-wrap gap-1">
        {fixed.map((g) => (
          <Chip key={`f-${g.cause}`} cls="text-emerald-300 border-emerald-500/30">
            ✓ {g.count}× {CAUSE_LABEL[g.cause]}
          </Chip>
        ))}
        {introduced.map((g) => (
          <Chip key={`i-${g.cause}`} cls="text-red-300 border-red-500/30">
            ✗ {g.count}× {CAUSE_LABEL[g.cause]}
          </Chip>
        ))}
        {preExisting.map((g) => (
          <Chip key={`p-${g.cause}`} cls="text-text-muted border-border">
            · {g.count}× {CAUSE_LABEL[g.cause]}
          </Chip>
        ))}
      </div>
    </div>
  );
}

export function ReceiptPanel() {
  const show = useElectronicsStore((s) => s.showReceiptPanel);
  const entries = useElectronicsStore((s) => s.receiptEntries);
  const clear = useElectronicsStore((s) => s.clearReceipt);
  const toggle = useElectronicsStore((s) => s.toggleReceiptPanel);

  if (!show) return null;
  const ordered = [...entries].reverse(); // newest first

  return (
    <div className="absolute bottom-3 left-3 w-80 max-h-[70vh] flex flex-col rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg text-[11px] pointer-events-auto">
      <div className="flex justify-between items-center px-3 py-2 border-b border-border">
        <span className="font-medium text-text">Receipt ledger</span>
        <div className="flex gap-2.5 items-center">
          <span className="text-text-muted tabular-nums">{entries.length}</span>
          <button onClick={clear} className="text-[10px] text-text-muted hover:text-text transition-colors">
            Clear
          </button>
          <button
            onClick={toggle}
            aria-label="Close ledger"
            className="text-[10px] text-text-muted hover:text-text transition-colors"
          >
            ✕
          </button>
        </div>
      </div>
      <div className="overflow-y-auto p-2 space-y-2">
        {ordered.length === 0 ? (
          <div className="text-text-muted px-1 py-3 leading-relaxed">
            No edits yet. Route, move, or place something and each change shows up here — what it fixed, what it
            introduced, and who's to blame.
          </div>
        ) : (
          ordered.map((e) => <Entry key={e.index} e={e} />)
        )}
      </div>
    </div>
  );
}
