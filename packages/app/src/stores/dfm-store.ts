/**
 * Zustand store for the live DFM check. Subscribes to document changes
 * and re-runs `runDfm` in a debounced loop so badges + counts update
 * as the user scrubs parameters.
 *
 * The store deliberately doesn't own the rule pack TOML — that lives in
 * `cad-lib/dfm/<process>.toml` and is sourced via the kernel's
 * `getDefaultDfmPack`. A future "advanced" panel can let the user
 * override the TOML per session.
 */

import { create } from "zustand";
import {
  runDfm,
  type DfmReport,
  type DfmProcess,
  type DfmSeverity,
} from "@vcad/engine";
import type { Document } from "@vcad/ir";

const RUN_DEBOUNCE_MS = 250;

interface DfmStoreState {
  /** Most recent report; null until the first run completes. */
  report: DfmReport | null;
  /** Process the live check is targeting. */
  process: DfmProcess;
  /** True while a run is in flight. */
  running: boolean;
  /** Last run error (cleared on success). */
  error: string | null;
  /** Issue id the user clicked on, or null. */
  selectedIssueId: string | null;
  /** Severities currently visible — toggled from the panel. */
  visibleSeverities: Set<DfmSeverity>;
  /** Master toggle; when off the annotations component renders nothing. */
  enabled: boolean;
  /** Whether the bottom drawer is expanded. The footer chip is the
   *  primary entry point — clicking it toggles this. The chip itself
   *  stays visible regardless. */
  drawerOpen: boolean;

  setProcess: (p: DfmProcess) => void;
  setEnabled: (v: boolean) => void;
  setDrawerOpen: (v: boolean) => void;
  toggleDrawer: () => void;
  toggleSeverity: (s: DfmSeverity) => void;
  selectIssue: (id: string | null) => void;
  /** Trigger an immediate run. Debounced internally — safe to call on
   *  every doc change. */
  scheduleRun: (doc: Document) => void;
}

let runTimer: ReturnType<typeof setTimeout> | null = null;
let pendingDoc: Document | null = null;

export const useDfmStore = create<DfmStoreState>((set, get) => ({
  report: null,
  process: "fdm",
  running: false,
  error: null,
  selectedIssueId: null,
  visibleSeverities: new Set<DfmSeverity>(["error", "warning", "info"]),
  enabled: true,
  drawerOpen: false,

  setProcess: (p) => {
    set({ process: p });
    if (pendingDoc) get().scheduleRun(pendingDoc);
  },
  setEnabled: (v) => set({ enabled: v }),
  setDrawerOpen: (v) => set({ drawerOpen: v }),
  toggleDrawer: () => set({ drawerOpen: !get().drawerOpen }),
  toggleSeverity: (s) => {
    const next = new Set(get().visibleSeverities);
    next.has(s) ? next.delete(s) : next.add(s);
    set({ visibleSeverities: next });
  },
  selectIssue: (id) => set({ selectedIssueId: id }),

  scheduleRun: (doc) => {
    if (!get().enabled) return;
    pendingDoc = doc;
    if (runTimer) clearTimeout(runTimer);
    runTimer = setTimeout(async () => {
      runTimer = null;
      const docToRun = pendingDoc;
      if (!docToRun) return;
      set({ running: true, error: null });
      try {
        const report = await runDfm(docToRun, { process: get().process });
        set({ report, running: false });
      } catch (e) {
        set({
          running: false,
          error: e instanceof Error ? e.message : String(e),
        });
      }
    }, RUN_DEBOUNCE_MS);
  },
}));

/** Per-severity issue counts for the badge display. */
export function severityCounts(report: DfmReport | null): {
  error: number;
  warning: number;
  info: number;
} {
  const out = { error: 0, warning: 0, info: 0 };
  if (!report) return out;
  for (const i of report.issues) {
    out[i.severity] += 1;
  }
  return out;
}
