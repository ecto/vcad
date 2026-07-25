/**
 * Analyze mode store (#592): one shell for all solver domains.
 *
 * Studies (structural FEA, tolerance stackup) are document data — they live
 * in `document.analysis_studies`, persisted through the CRDT
 * `analysis-studies` feature — while runs (transient solver results, field
 * overlays) live here.
 *
 * Receipts are mandatory and fail-closed: a run without claims renders no
 * result, and every rendered result carries a claim status:
 *   - unverifiable — the solver refused to certify (e.g. FEA convergence
 *     gate failed); reasons shown, nothing else.
 *   - violated     — a claim failed (safety factor < 1, requirement missed).
 *   - provisional  — predicted-basis claims pass; a real measurement would
 *     close them (the claim text says which).
 *   - holds        — this run reproduces the stored baseline.
 *   - stale        — geometry/spec drifted from the stored baseline (or the
 *     scene changed since the run); re-run / re-accept.
 */

import { create } from "zustand";
import { useDocumentStore, useEngineStore } from "@vcad/core";
import { getAnalyzeClient } from "@vcad/engine";
import type { FeaAnalysis, ToleranceAnalysis } from "@vcad/engine";
import type {
  AnalysisStudy,
  AnalysisBaseline,
  ReceiptClaim,
  StudyRegion,
} from "@vcad/ir";

/** Claim status shown on every result (fail-closed: never absent). */
export type StudyClaimStatus =
  | "provisional"
  | "holds"
  | "stale"
  | "violated"
  | "unverifiable"
  | "error";

/**
 * Which scalar field is painted on the mesh. `velocity` and `pressure`
 * are flow fields (vcad-kernel-flow simulate_flow / GPU preview
 * lattice); they ride the same per-vertex color path as the FEA
 * overlays.
 */
export type FieldKind = "displacement" | "vonMises" | "velocity" | "pressure";

export interface StudyRun {
  status: "idle" | "running" | "done" | "error";
  error?: string;
  fea?: FeaAnalysis;
  tolerance?: ToleranceAnalysis;
  claims?: ReceiptClaim[];
  claimStatus?: StudyClaimStatus;
  /** Reasons when unverifiable. */
  reasons?: string[];
  /** Geometry fingerprint of the studied part at run time — a different
   *  fingerprint later means the geometry changed and the run is Stale.
   *  (Object identity is useless here: writing the study baseline itself
   *  re-evaluates the document and mints a new scene object.) */
  meshKeyAtRun?: string | null;
}

export interface FieldOverlay {
  studyId: string;
  partId: string;
  field: FieldKind;
  /** Per-vertex RGB (0–1), aligned with the part mesh positions. */
  colors: Float32Array;
  min: number;
  max: number;
  unit: string;
}

/** Relative tolerance for Holds-vs-Stale baseline comparison. */
const BASELINE_REL_EPS = 1e-6;

interface AnalyzeState {
  panelOpen: boolean;
  runs: Record<string, StudyRun>;
  fieldOverlay: FieldOverlay | null;

  openPanel: () => void;
  closePanel: () => void;
  addStudy: (study: AnalysisStudy) => void;
  updateStudy: (study: AnalysisStudy) => void;
  removeStudy: (id: string) => void;
  runStudy: (id: string) => Promise<void>;
  acceptBaseline: (id: string) => void;
  setField: (studyId: string, field: FieldKind | null) => void;
}

function studies(): AnalysisStudy[] {
  return useDocumentStore.getState().document.analysis_studies ?? [];
}

function writeStudies(next: AnalysisStudy[]): void {
  useDocumentStore.getState().setAnalysisStudies(next);
}

/** Blue → cyan → green → yellow → red ramp (t in [0,1]). */
export function rampColor(t: number): [number, number, number] {
  const x = Math.min(1, Math.max(0, t));
  const stops: Array<[number, [number, number, number]]> = [
    [0.0, [0.05, 0.15, 0.9]],
    [0.25, [0.0, 0.75, 0.95]],
    [0.5, [0.1, 0.85, 0.3]],
    [0.75, [0.98, 0.85, 0.1]],
    [1.0, [0.95, 0.15, 0.1]],
  ];
  for (let i = 1; i < stops.length; i++) {
    if (x <= stops[i]![0]) {
      const [t0, c0] = stops[i - 1]!;
      const [t1, c1] = stops[i]!;
      const f = (x - t0) / (t1 - t0);
      return [
        c0[0] + f * (c1[0] - c0[0]),
        c0[1] + f * (c1[1] - c0[1]),
        c0[2] + f * (c1[2] - c0[2]),
      ];
    }
  }
  return stops[stops.length - 1]![1];
}

function buildColors(values: number[]): { colors: Float32Array; min: number; max: number } {
  let min = Infinity;
  let max = -Infinity;
  for (const v of values) {
    if (v < min) min = v;
    if (v > max) max = v;
  }
  if (!isFinite(min)) {
    min = 0;
    max = 0;
  }
  const span = max - min || 1;
  const colors = new Float32Array(values.length * 3);
  for (let i = 0; i < values.length; i++) {
    const [r, g, b] = rampColor((values[i]! - min) / span);
    colors[3 * i] = r;
    colors[3 * i + 1] = g;
    colors[3 * i + 2] = b;
  }
  return { colors, min, max };
}

/** Extract predicted quantities from claims for the baseline record. */
function claimQuantities(claims: ReceiptClaim[]): AnalysisBaseline["quantities"] {
  const out: AnalysisBaseline["quantities"] = [];
  for (const c of claims) {
    const q = c.predicted;
    if (q && typeof q.value === "number") {
      out.push({ id: c.id, value: q.value, unit: q.unit ?? "" });
    }
  }
  return out;
}

/** Classify a finished run against its study's stored baseline. */
function classify(
  claims: ReceiptClaim[],
  baseline: AnalysisBaseline | null | undefined,
  unverifiableReasons: string[] | null,
): { status: StudyClaimStatus; reasons?: string[] } {
  if (unverifiableReasons) {
    return { status: "unverifiable", reasons: unverifiableReasons };
  }
  // Fail-closed: no claims, no result.
  if (claims.length === 0) {
    return {
      status: "unverifiable",
      reasons: ["solver returned no claims — nothing to certify"],
    };
  }
  if (claims.some((c) => c.verdict === "fail")) return { status: "violated" };
  if (claims.some((c) => c.verdict === "unverifiable")) {
    return {
      status: "unverifiable",
      reasons: claims
        .filter((c) => c.verdict === "unverifiable")
        .map((c) => c.description),
    };
  }
  if (baseline) {
    const now = new Map(claimQuantities(claims).map((q) => [q.id, q.value]));
    let allMatch = true;
    for (const q of baseline.quantities) {
      const v = now.get(q.id);
      if (v === undefined) {
        allMatch = false;
        break;
      }
      const scale = Math.max(Math.abs(q.value), 1e-30);
      if (Math.abs(v - q.value) / scale > BASELINE_REL_EPS) {
        allMatch = false;
        break;
      }
    }
    return { status: allMatch ? "holds" : "stale" };
  }
  return { status: "provisional" };
}

/** Cheap geometry fingerprint: vertex/index counts + bbox, enough to
 *  detect any real shape or placement change. */
export function partMeshKey(partId: string): string | null {
  const m = partMesh(partId);
  if (!m) return null;
  const p = m.positions;
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (let i = 0; i < p.length; i += 3) {
    const x = p[i]!, y = p[i + 1]!, z = p[i + 2]!;
    if (x < minX) minX = x;
    if (y < minY) minY = y;
    if (z < minZ) minZ = z;
    if (x > maxX) maxX = x;
    if (y > maxY) maxY = y;
    if (z > maxZ) maxZ = z;
  }
  return `${p.length}:${m.indices.length}:${minX.toFixed(4)},${minY.toFixed(4)},${minZ.toFixed(4)}:${maxX.toFixed(4)},${maxY.toFixed(4)},${maxZ.toFixed(4)}`;
}

function partMesh(partId: string): { positions: Float32Array; indices: Uint32Array } | null {
  const scene = useEngineStore.getState().scene;
  const parts = useDocumentStore.getState().parts;
  if (!scene) return null;
  const idx = parts.findIndex((p) => p.id === partId);
  const mesh = idx >= 0 ? scene.parts[idx]?.mesh : null;
  if (!mesh || mesh.positions.length === 0) return null;
  return { positions: mesh.positions, indices: mesh.indices };
}

export const useAnalyzeStore = create<AnalyzeState>((set, get) => ({
  panelOpen: false,
  runs: {},
  fieldOverlay: null,

  openPanel: () => set({ panelOpen: true }),
  closePanel: () => set({ panelOpen: false, fieldOverlay: null }),

  addStudy: (study) => writeStudies([...studies(), study]),

  updateStudy: (study) =>
    writeStudies(studies().map((s) => (s.id === study.id ? study : s))),

  removeStudy: (id) => {
    writeStudies(studies().filter((s) => s.id !== id));
    set((s) => {
      const runs = { ...s.runs };
      delete runs[id];
      return {
        runs,
        fieldOverlay: s.fieldOverlay?.studyId === id ? null : s.fieldOverlay,
      };
    });
  },

  runStudy: async (id) => {
    const study = studies().find((s) => s.id === id);
    if (!study) return;
    const setRun = (run: StudyRun) =>
      set((s) => ({ runs: { ...s.runs, [id]: run } }));
    setRun({ status: "running" });

    try {
      const client = getAnalyzeClient();

      if (study.study.type === "structural") {
        const k = study.study;
        const mesh = partMesh(k.partId);
        if (!mesh) {
          throw new Error(
            "part mesh not found — the study's part may have been deleted",
          );
        }
        // Map IR (camelCase) → kernel FeaSpec (snake_case).
        const spec = {
          resolution: k.resolution,
          youngs_modulus_mpa: k.youngsModulusMpa,
          poisson: k.poisson,
          yield_strength_mpa: k.yieldStrengthMpa ?? null,
          loads: k.loads.map((l) => ({ region: l.region, force: l.force })),
          supports: k.supports.map((s) => ({ region: s.region, fix: s.fix })),
        };
        const fea = await client.runStructural(
          spec,
          { fields: true },
          mesh.positions,
          mesh.indices,
        );
        const unv =
          fea.study.verdict.verdict === "Unverifiable"
            ? fea.study.verdict.reasons
            : null;
        const claims = fea.receipt_claims ?? [];
        const { status, reasons } = classify(claims, study.baseline, unv);
        setRun({
          status: "done",
          fea,
          claims,
          claimStatus: status,
          reasons,
          meshKeyAtRun: partMeshKey(k.partId),
        });
        if (status === "provisional" && !study.baseline) {
          get().updateStudy({
            ...study,
            baseline: {
              recordedAtIso: new Date().toISOString(),
              quantities: claimQuantities(claims),
            },
          });
        }
      } else {
        const k = study.study;
        // Map IR (camelCase) → kernel StackupSpec (snake_case).
        const spec = {
          name: study.name,
          contributors: k.contributors.map((c) => ({
            name: c.name,
            coeff: c.coeff,
            nominal: c.nominal,
            tol_minus: c.tolMinus,
            tol_plus: c.tolPlus,
            // Kernel DistSpec is tagged; default to the industry-standard
            // ±tol = 3σ derivation (same default as the MCP tool).
            dist:
              c.dist === "uniform"
                ? { type: "uniform", lo: -c.tolMinus, hi: c.tolPlus }
                : { type: "normal_from_tol", convention: { type: "three_sigma" } },
          })),
          requirement: {
            name: k.requirement.name,
            ...(k.requirement.lowerMm != null ? { lower_mm: k.requirement.lowerMm } : {}),
            ...(k.requirement.upperMm != null ? { upper_mm: k.requirement.upperMm } : {}),
          },
        };
        const tolerance = await client.runTolerance(spec);
        const claims = tolerance.receipt_claims ?? [];
        const { status, reasons } = classify(claims, study.baseline, null);
        setRun({
          status: "done",
          tolerance,
          claims,
          claimStatus: status,
          reasons,
        });
        if (status === "provisional" && !study.baseline) {
          get().updateStudy({
            ...study,
            baseline: {
              recordedAtIso: new Date().toISOString(),
              quantities: claimQuantities(claims),
            },
          });
        }
      }
    } catch (err) {
      setRun({
        status: "error",
        error: String(err instanceof Error ? err.message : err),
        claimStatus: "error",
      });
    }
  },

  acceptBaseline: (id) => {
    const study = studies().find((s) => s.id === id);
    const run = get().runs[id];
    if (!study || !run?.claims) return;
    get().updateStudy({
      ...study,
      baseline: {
        recordedAtIso: new Date().toISOString(),
        quantities: claimQuantities(run.claims),
      },
    });
    set((s) => ({
      runs: { ...s.runs, [id]: { ...run, claimStatus: "provisional" } },
    }));
  },

  setField: (studyId, field) => {
    if (!field) {
      set({ fieldOverlay: null });
      return;
    }
    const study = studies().find((s) => s.id === studyId);
    const run = get().runs[studyId];
    if (!study || study.study.type !== "structural" || !run?.fea) return;
    const values =
      field === "displacement"
        ? run.fea.vertex_displacement_mm
        : run.fea.vertex_von_mises_mpa;
    if (!values || values.length === 0) return;
    const { colors, min, max } = buildColors(values);
    set({
      fieldOverlay: {
        studyId,
        partId: study.study.partId,
        field,
        colors,
        min,
        max,
        unit: field === "displacement" ? "mm" : "MPa",
      },
    });
  },
}));

/** Face-pick helper: world AABB of a StudyRegion source face, slightly
 *  inflated so the kernel's node selection (tolerance h/4) catches the
 *  surface nodes. */
export function inflateRegion(
  min: [number, number, number],
  max: [number, number, number],
  eps = 0.5,
): StudyRegion {
  return {
    min: [min[0] - eps, min[1] - eps, min[2] - eps],
    max: [max[0] + eps, max[1] + eps, max[2] + eps],
  };
}
