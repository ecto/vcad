import { create } from "zustand";
import type { Document } from "@vcad/ir";
import type { Engine, SensitivityReport, SensitivityRow } from "@vcad/engine";

/**
 * Parameter influence: which knob actually moves a quantity, and how far you
 * may act on the answer.
 *
 * Computed on demand rather than on every edit. A sensitivity sweep costs one
 * seam pass per parameter plus a bounded topology search, which is cheap next
 * to rebuilding the document N times but far too expensive to run on every
 * keystroke. The report is invalidated whenever the document changes, so a
 * stale gradient never sits next to a changed model.
 */
export interface SensitivityState {
  report: SensitivityReport | null;
  loading: boolean;
  error: string | null;
  /** Quantity the panel ranks by. */
  quantity: string;
  /** Document revision the current report describes. */
  computedFor: string | null;

  setQuantity: (quantity: string) => void;
  compute: (doc: Document, engine: Engine, revision: string) => Promise<void>;
  /** Drop the report — call when the document changes under it. */
  invalidate: () => void;
}

/** Quantities the panel offers. */
export const SENSITIVITY_QUANTITIES = [
  "mass",
  "volume",
  "bbox_x",
  "bbox_y",
  "bbox_z",
] as const;

export const useSensitivityStore = create<SensitivityState>((set, get) => ({
  report: null,
  loading: false,
  error: null,
  quantity: "mass",
  computedFor: null,

  setQuantity: (quantity) => set({ quantity, report: null, computedFor: null }),

  compute: async (doc, engine, revision) => {
    if (get().loading) return;
    set({ loading: true, error: null });
    // Yield a frame so the spinner paints before the synchronous WASM call
    // blocks the main thread.
    await new Promise((r) => setTimeout(r, 0));
    try {
      const report = engine.documentSensitivities(doc, {
        quantities: [get().quantity],
        findTrustRadius: true,
      });
      set({ report, loading: false, computedFor: revision });
    } catch (e) {
      set({
        report: null,
        loading: false,
        computedFor: null,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  invalidate: () => set({ report: null, computedFor: null, error: null }),
}));

/**
 * What counts as "the document changed" for the purposes of a stale gradient.
 *
 * Node count, parameter values, and bindings — everything a sensitivity
 * depends on. Defined here rather than inline in the panel so the component
 * and anything checking it agree by construction; two definitions of "changed"
 * is how a stale gradient ends up sitting next to an edited model.
 */
export function documentRevision(doc: Document): string {
  return JSON.stringify({
    n: Object.keys(doc.nodes ?? {}).length,
    p: doc.parameters,
    b: doc.bindings,
  });
}

/** Influence of a row: |dJ/dθ| × the width of its trust radius. */
export function influenceOf(row: SensitivityRow): number | null {
  if (!row.trust) return null;
  const span = row.trust.upper - row.trust.lower;
  if (!Number.isFinite(span) || span <= 0) return null;
  return Math.abs(row.value) * span;
}

/**
 * Rows for one objective, most influential first, plus the largest influence
 * so a caller can size bars against it. Rows without a trust radius have no
 * comparable influence and sort last.
 */
export function rankedRows(
  report: SensitivityReport | null,
  objective: string,
): { rows: SensitivityRow[]; max: number } {
  if (!report) return { rows: [], max: 0 };
  const rows = report.table.rows
    .filter((r) => r.objective === objective)
    .slice()
    .sort((a, b) => {
      const ia = influenceOf(a);
      const ib = influenceOf(b);
      if (ia == null && ib == null) return 0;
      if (ia == null) return 1;
      if (ib == null) return -1;
      return ib - ia;
    });
  const max = rows.reduce((m, r) => Math.max(m, influenceOf(r) ?? 0), 0);
  return { rows, max };
}

/** A short human label for why a trust radius ends. */
export function trustLabel(row: SensitivityRow): string | null {
  if (!row.trust) return null;
  const { lower, upper, limited_by } = row.trust;
  const why =
    limited_by === "topology_stable"
      ? "topology"
      : limited_by === "parameter_bounds"
        ? "bounds"
        : limited_by === "grid_resolution"
          ? "grid"
          : limited_by === "model_validity"
            ? "model"
            : "curvature";
  // One precision across the pair. Rounding each end independently gives
  // "0.0000–10.00", which reads as two different measurements rather than
  // as one interval.
  const decimals = decimalsFor(Math.max(Math.abs(lower), Math.abs(upper)));
  return `valid ${fixed(lower, decimals)}–${fixed(upper, decimals)} (${why})`;
}

function decimalsFor(magnitude: number): number {
  if (!Number.isFinite(magnitude)) return 2;
  return magnitude >= 100 ? 0 : magnitude >= 1 ? 2 : 4;
}

function fixed(n: number, decimals: number): string {
  return Number.isFinite(n) ? n.toFixed(decimals) : "?";
}

function round(n: number): string {
  return fixed(n, decimalsFor(Math.abs(n)));
}

/** Format a derivative for a dense panel row. */
export function formatDerivative(row: SensitivityRow): string {
  const v = row.value;
  if (!Number.isFinite(v)) return "—";
  const abs = Math.abs(v);
  const num =
    abs !== 0 && (abs < 1e-3 || abs >= 1e5) ? v.toExponential(2) : round(v);
  return `${num} ${row.unit}`;
}
