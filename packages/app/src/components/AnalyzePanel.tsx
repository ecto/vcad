/**
 * Unified Analyze panel (#592): one shell for all solver domains.
 *
 * V1 study types: structural FEA and tolerance stackup. Every rendered
 * result carries its receipt claim status (fail-closed) — see
 * analyze-store.ts for the status semantics.
 */

import { useMemo, useState } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import {
  useDocumentStore,
  useEngineStore,
  useUiStore,
} from "@vcad/core";
import type {
  AnalysisStudy,
  StudyLoad,
  StudySupport,
  StudyContributor,
  ReceiptClaim,
} from "@vcad/ir";
import {
  useAnalyzeStore,
  inflateRegion,
  partMeshKey,
  type StudyClaimStatus,
  type StudyRun,
  type FieldKind,
} from "@/stores/analyze-store";
import { useNotificationStore } from "@/stores/notification-store";
import { findCoplanarTriangles } from "@/lib/sub-feature-geometry";

// ---------------------------------------------------------------------------
// Claim status ribbon — the mandatory, fail-closed receipt surface
// ---------------------------------------------------------------------------

const STATUS_STYLE: Record<StudyClaimStatus, { label: string; cls: string }> = {
  provisional: { label: "Provisional", cls: "bg-amber-500/15 text-amber-500" },
  holds: { label: "Holds", cls: "bg-green-500/15 text-green-500" },
  stale: { label: "Stale", cls: "bg-orange-500/15 text-orange-400" },
  violated: { label: "Violated", cls: "bg-red-500/15 text-red-500" },
  unverifiable: { label: "Unverifiable", cls: "bg-red-500/15 text-red-400" },
  error: { label: "Error", cls: "bg-red-500/15 text-red-500" },
};

function ClaimBadge({ status }: { status: StudyClaimStatus }) {
  const s = STATUS_STYLE[status];
  return (
    <span className={`px-1.5 py-0.5 text-[10px] font-medium rounded ${s.cls}`}>
      {s.label}
    </span>
  );
}

/** What real-world measurement would close a predicted-basis claim. */
function closesWith(claims: ReceiptClaim[]): string | null {
  const predicted = claims.find((c) => c.basis === "predicted");
  if (!predicted) return null;
  return `Predicted by ${predicted.oracle.id}; a physical measurement of “${predicted.description}” closes this claim.`;
}

function ClaimRibbon({ run }: { run: StudyRun }) {
  if (!run.claimStatus) return null;
  return (
    <div className="mt-2 rounded border border-border p-2 space-y-1">
      <div className="flex items-center gap-2">
        <span className="text-[10px] uppercase tracking-wide text-text-muted">
          Receipt
        </span>
        <ClaimBadge status={run.claimStatus} />
      </div>
      {run.error && <div className="text-[11px] text-red-400">{run.error}</div>}
      {run.reasons?.map((r, i) => (
        <div key={i} className="text-[11px] text-red-400">
          {r}
        </div>
      ))}
      {run.claims?.map((c) => (
        <div key={c.id} className="text-[11px] text-text-muted flex items-start gap-1.5">
          <span
            className={
              c.verdict === "pass"
                ? "text-green-500"
                : c.verdict === "fail"
                  ? "text-red-500"
                  : "text-amber-500"
            }
          >
            {c.verdict === "pass" ? "✓" : c.verdict === "fail" ? "✕" : "?"}
          </span>
          <span>
            {c.description}
            {c.predicted && typeof c.predicted.value === "number" && (
              <>
                {" "}
                — {formatNum(c.predicted.value)}
                {c.predicted.unit ? ` ${c.predicted.unit}` : ""}
              </>
            )}
          </span>
        </div>
      ))}
      {run.claims && closesWith(run.claims) && (
        <div className="text-[10px] text-text-muted italic">{closesWith(run.claims)}</div>
      )}
    </div>
  );
}

function formatNum(v: number): string {
  if (v === 0) return "0";
  const a = Math.abs(v);
  if (a >= 1000 || a < 0.001) return v.toExponential(3);
  return v.toPrecision(4);
}

// ---------------------------------------------------------------------------
// Face pick → AABB region
// ---------------------------------------------------------------------------

/** World AABB of the currently selected face (null when no face selected). */
function useSelectedFaceRegion(): {
  region: { min: [number, number, number]; max: [number, number, number] } | null;
  partId: string | null;
} {
  const selection = useUiStore((s) => s.selection);
  const scene = useEngineStore((s) => s.scene);
  const parts = useDocumentStore((s) => s.parts);
  return useMemo(() => {
    const face = selection.find((i) => i.kind === "face");
    if (!face || face.kind !== "face" || !scene) return { region: null, partId: null };
    const idx = parts.findIndex((p) => p.id === face.partId);
    const mesh = idx >= 0 ? scene.parts[idx]?.mesh : null;
    if (!mesh) return { region: null, partId: null };
    const tris = findCoplanarTriangles(mesh, face.faceIndex);
    if (tris.length === 0) return { region: null, partId: null };
    const min: [number, number, number] = [Infinity, Infinity, Infinity];
    const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
    for (const t of tris) {
      for (let k = 0; k < 3; k++) {
        const vi = mesh.indices[3 * t + k]!;
        for (let a = 0; a < 3; a++) {
          const v = mesh.positions[3 * vi + a]!;
          if (v < min[a]!) min[a] = v;
          if (v > max[a]!) max[a] = v;
        }
      }
    }
    return { region: { min, max }, partId: face.partId };
  }, [selection, scene, parts]);
}

// ---------------------------------------------------------------------------
// Small form primitives (properties-panel style)
// ---------------------------------------------------------------------------

function NumField({
  label,
  value,
  onChange,
  step = 1,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
  step?: number;
}) {
  return (
    <label className="flex items-center justify-between gap-2 text-[11px]">
      <span className="text-text-muted">{label}</span>
      <input
        type="number"
        className="w-24 bg-background border border-border rounded px-1.5 py-0.5 text-right"
        value={value}
        step={step}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </label>
  );
}

// ---------------------------------------------------------------------------
// Structural study editor + results
// ---------------------------------------------------------------------------

function StructuralEditor({ study }: { study: AnalysisStudy }) {
  const updateStudy = useAnalyzeStore((s) => s.updateStudy);
  const addToast = useNotificationStore((s) => s.addToast);
  const { region, partId } = useSelectedFaceRegion();
  if (study.study.type !== "structural") return null;
  const k = study.study;

  const patch = (p: Partial<typeof k>) =>
    updateStudy({ ...study, study: { ...k, ...p } });

  const addLoad = () => {
    if (!region || partId !== k.partId) {
      addToast("Select a face on the study's part first", "error");
      return;
    }
    const load: StudyLoad = { region: inflateRegion(region.min, region.max), force: [0, 0, -100] };
    patch({ loads: [...k.loads, load] });
  };

  const addSupport = () => {
    if (!region || partId !== k.partId) {
      addToast("Select a face on the study's part first", "error");
      return;
    }
    const support: StudySupport = {
      region: inflateRegion(region.min, region.max),
      fix: [true, true, true],
    };
    patch({ supports: [...k.supports, support] });
  };

  return (
    <div className="space-y-2">
      <NumField
        label="Resolution"
        value={k.resolution}
        onChange={(v) => patch({ resolution: Math.max(4, Math.round(v)) })}
      />
      <NumField
        label="Young's modulus (MPa)"
        value={k.youngsModulusMpa}
        step={1000}
        onChange={(v) => patch({ youngsModulusMpa: v })}
      />
      <NumField
        label="Poisson"
        value={k.poisson}
        step={0.01}
        onChange={(v) => patch({ poisson: v })}
      />
      <NumField
        label="Yield strength (MPa)"
        value={k.yieldStrengthMpa ?? 0}
        onChange={(v) => patch({ yieldStrengthMpa: v > 0 ? v : undefined })}
      />

      <div className="pt-1">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-medium">Loads (N)</span>
          <button
            className="flex items-center gap-1 text-[10px] text-brand hover:underline"
            onClick={addLoad}
          >
            <Plus size={10} /> from selected face
          </button>
        </div>
        {k.loads.length === 0 && (
          <div className="text-[10px] text-text-muted">
            Pick a face in the viewport, then add it as a load region.
          </div>
        )}
        {k.loads.map((l, i) => (
          <div key={i} className="mt-1 flex items-center gap-1">
            {[0, 1, 2].map((a) => (
              <input
                key={a}
                type="number"
                className="w-16 bg-background border border-border rounded px-1 py-0.5 text-[11px] text-right"
                value={l.force[a]}
                onChange={(e) => {
                  const loads = k.loads.map((x, j) =>
                    j === i
                      ? {
                          ...x,
                          force: x.force.map((f, b) =>
                            b === a ? Number(e.target.value) : f,
                          ) as [number, number, number],
                        }
                      : x,
                  );
                  patch({ loads });
                }}
              />
            ))}
            <button
              className="text-text-muted hover:text-red-400"
              onClick={() => patch({ loads: k.loads.filter((_, j) => j !== i) })}
            >
              <Trash size={12} />
            </button>
          </div>
        ))}
      </div>

      <div className="pt-1">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-medium">Supports (fixed)</span>
          <button
            className="flex items-center gap-1 text-[10px] text-brand hover:underline"
            onClick={addSupport}
          >
            <Plus size={10} /> from selected face
          </button>
        </div>
        {k.supports.map((s, i) => (
          <div key={i} className="mt-1 flex items-center gap-2 text-[11px]">
            <span className="text-text-muted">
              [{s.region.min.map((v) => v.toFixed(0)).join(", ")}] →{" "}
              [{s.region.max.map((v) => v.toFixed(0)).join(", ")}]
            </span>
            <button
              className="ml-auto text-text-muted hover:text-red-400"
              onClick={() => patch({ supports: k.supports.filter((_, j) => j !== i) })}
            >
              <Trash size={12} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

function StructuralResults({ study, run }: { study: AnalysisStudy; run: StudyRun }) {
  const fieldOverlay = useAnalyzeStore((s) => s.fieldOverlay);
  const setField = useAnalyzeStore((s) => s.setField);
  if (!run.fea) return null;
  const fine = run.fea.study.levels[run.fea.study.levels.length - 1];
  if (!fine) return null;
  const converged = run.fea.study.verdict.verdict === "Converged";

  const FieldButton = ({ field, label }: { field: FieldKind; label: string }) => {
    const active = fieldOverlay?.studyId === study.id && fieldOverlay.field === field;
    return (
      <button
        className={`px-1.5 py-0.5 text-[10px] rounded transition-colors ${
          active ? "bg-brand text-white" : "bg-brand/15 text-brand hover:bg-brand/25"
        }`}
        onClick={() => setField(study.id, active ? null : field)}
      >
        {label}
      </button>
    );
  };

  return (
    <div className="mt-2 space-y-1 text-[11px]">
      <div className="grid grid-cols-2 gap-x-2 gap-y-0.5">
        <span className="text-text-muted">Max displacement</span>
        <span className="text-right">{formatNum(fine.max_displacement_mm)} mm</span>
        <span className="text-text-muted">Max von Mises</span>
        <span className="text-right">{formatNum(fine.max_von_mises_mpa)} MPa</span>
        {run.fea.study.safety_factor != null && (
          <>
            <span className="text-text-muted">Safety factor</span>
            <span
              className={`text-right ${run.fea.study.safety_factor < 1 ? "text-red-500" : ""}`}
            >
              {formatNum(run.fea.study.safety_factor)}
            </span>
          </>
        )}
        <span className="text-text-muted">Convergence</span>
        <span className={`text-right ${converged ? "text-green-500" : "text-red-400"}`}>
          {converged ? "Converged" : "Unverifiable"}
        </span>
      </div>
      <div className="flex items-center gap-1.5 pt-1">
        <span className="text-[10px] text-text-muted">Color by:</span>
        <FieldButton field="displacement" label="Displacement" />
        <FieldButton field="vonMises" label="von Mises" />
      </div>
      {fieldOverlay?.studyId === study.id && (
        <div className="flex items-center gap-1 text-[10px] text-text-muted">
          <span>{formatNum(fieldOverlay.min)}</span>
          <div
            className="h-2 flex-1 rounded"
            style={{
              background:
                "linear-gradient(to right, #0d26e6, #00bff2, #1ad94d, #fad91a, #f2261a)",
            }}
          />
          <span>
            {formatNum(fieldOverlay.max)} {fieldOverlay.unit}
          </span>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tolerance study editor + results
// ---------------------------------------------------------------------------

function ToleranceEditor({ study }: { study: AnalysisStudy }) {
  const updateStudy = useAnalyzeStore((s) => s.updateStudy);
  if (study.study.type !== "tolerance") return null;
  const k = study.study;
  const patch = (p: Partial<typeof k>) =>
    updateStudy({ ...study, study: { ...k, ...p } });

  const setContrib = (i: number, c: StudyContributor) =>
    patch({ contributors: k.contributors.map((x, j) => (j === i ? c : x)) });

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-medium">Contributors (mm)</span>
        <button
          className="flex items-center gap-1 text-[10px] text-brand hover:underline"
          onClick={() =>
            patch({
              contributors: [
                ...k.contributors,
                { name: `dim ${k.contributors.length + 1}`, coeff: 1, nominal: 10, tolMinus: 0.1, tolPlus: 0.1 },
              ],
            })
          }
        >
          <Plus size={10} /> add
        </button>
      </div>
      {k.contributors.map((c, i) => (
        <div key={i} className="flex items-center gap-1 text-[11px]">
          <input
            className="w-20 bg-background border border-border rounded px-1 py-0.5"
            value={c.name}
            onChange={(e) => setContrib(i, { ...c, name: e.target.value })}
          />
          {(["coeff", "nominal", "tolMinus", "tolPlus"] as const).map((f) => (
            <input
              key={f}
              type="number"
              title={f}
              className="w-14 bg-background border border-border rounded px-1 py-0.5 text-right"
              value={c[f]}
              onChange={(e) => setContrib(i, { ...c, [f]: Number(e.target.value) })}
            />
          ))}
          <button
            className="text-text-muted hover:text-red-400"
            onClick={() => patch({ contributors: k.contributors.filter((_, j) => j !== i) })}
          >
            <Trash size={12} />
          </button>
        </div>
      ))}
      <NumField
        label={`Requirement “${k.requirement.name}” lower (mm)`}
        value={k.requirement.lowerMm ?? 0}
        step={0.01}
        onChange={(v) => patch({ requirement: { ...k.requirement, lowerMm: v } })}
      />
      <NumField
        label="Requirement upper (mm)"
        value={k.requirement.upperMm ?? 0}
        step={0.01}
        onChange={(v) => patch({ requirement: { ...k.requirement, upperMm: v } })}
      />
    </div>
  );
}

function ToleranceResults({ run }: { run: StudyRun }) {
  if (!run.tolerance) return null;
  const rows: Array<[string, unknown]> = [
    ["Worst case", run.tolerance.worst_case],
    ["RSS", run.tolerance.rss],
    ["Monte Carlo", run.tolerance.monte_carlo],
  ];
  return (
    <div className="mt-2 space-y-1 text-[11px]">
      {rows.map(([label, dist]) => (
        <div key={label} className="flex justify-between gap-2">
          <span className="text-text-muted">{label}</span>
          <span className="text-right truncate">{summarizeDist(dist)}</span>
        </div>
      ))}
    </div>
  );
}

function summarizeDist(dist: unknown): string {
  if (!dist || typeof dist !== "object") return "—";
  const d = dist as Record<string, unknown>;
  const num = (k: string) => (typeof d[k] === "number" ? (d[k] as number) : null);
  const parts: string[] = [];
  // Worst case: {min_gap, max_gap, passes}
  const lo = num("min_gap");
  const hi = num("max_gap");
  if (lo != null && hi != null) {
    parts.push(`${formatNum(lo)} … ${formatNum(hi)} mm`);
    if (typeof d["passes"] === "boolean") parts.push(d["passes"] ? "passes" : "FAILS");
    return parts.join(", ");
  }
  // RSS: {mean_gap, sigma_gap, yield_estimate}; MC: {mean_gap, sigma_gap, fit_probability}
  const mean = num("mean_gap");
  const sigma = num("sigma_gap");
  const y = num("yield_estimate") ?? num("fit_probability");
  if (mean != null) parts.push(`μ ${formatNum(mean)}${sigma != null ? ` σ ${formatNum(sigma)}` : ""} mm`);
  if (y != null) parts.push(`yield ${(y * 100).toFixed(2)}%`);
  return parts.join(", ") || "—";
}

// ---------------------------------------------------------------------------
// Study card + panel shell
// ---------------------------------------------------------------------------

function StudyCard({ study }: { study: AnalysisStudy }) {
  const run = useAnalyzeStore((s) => s.runs[study.id]) ?? { status: "idle" as const };
  const runStudy = useAnalyzeStore((s) => s.runStudy);
  const removeStudy = useAnalyzeStore((s) => s.removeStudy);
  const acceptBaseline = useAnalyzeStore((s) => s.acceptBaseline);
  const scene = useEngineStore((s) => s.scene);
  // Geometry changed since this run → whatever the run said, it's Stale now.
  const currentMeshKey = useMemo(
    () =>
      study.study.type === "structural" ? partMeshKey(study.study.partId) : null,
    // The scene is the reactive input; the key derives from it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [scene, study],
  );
  const sceneStale =
    run.status === "done" &&
    run.meshKeyAtRun != null &&
    run.meshKeyAtRun !== currentMeshKey;
  const effectiveRun: StudyRun =
    sceneStale && run.claimStatus && run.claimStatus !== "violated"
      ? { ...run, claimStatus: "stale" }
      : run;

  return (
    <div className="rounded border border-border p-2">
      <div className="flex items-center gap-2">
        <span className="text-[12px] font-medium truncate">{study.name}</span>
        <span className="text-[10px] text-text-muted uppercase">{study.study.type}</span>
        <div className="ml-auto flex items-center gap-1">
          <button
            className="p-1 rounded bg-brand/15 text-brand hover:bg-brand/25 disabled:opacity-50"
            title="Run study"
            disabled={run.status === "running"}
            onClick={() => void runStudy(study.id)}
          >
            {run.status === "running" ? (
              <Spinner size={12} className="animate-spin" />
            ) : (
              <Play size={12} />
            )}
          </button>
          <button
            className="p-1 rounded text-text-muted hover:text-red-400"
            title="Delete study"
            onClick={() => removeStudy(study.id)}
          >
            <Trash size={12} />
          </button>
        </div>
      </div>

      {study.study.type === "structural" ? (
        <StructuralEditor study={study} />
      ) : (
        <ToleranceEditor study={study} />
      )}

      {/* Results render only alongside their receipt ribbon (fail-closed):
          even an Unverifiable run shows its field picture as diagnostics,
          but never without the claim status right below. */}
      {run.status === "done" && (
        study.study.type === "structural" ? (
          <StructuralResults study={study} run={run} />
        ) : (
          <ToleranceResults run={run} />
        )
      )}
      {(run.status === "done" || run.status === "error") && (
        <ClaimRibbon run={effectiveRun} />
      )}
      {effectiveRun.claimStatus === "stale" && run.status === "done" && (
        <button
          className="mt-1 text-[10px] text-brand hover:underline"
          onClick={() => (sceneStale ? void runStudy(study.id) : acceptBaseline(study.id))}
        >
          {sceneStale ? "Re-run against current geometry" : "Accept as new baseline"}
        </button>
      )}
    </div>
  );
}

let _nextStudyNum = 1;

export function AnalyzePanel() {
  const closePanel = useAnalyzeStore((s) => s.closePanel);
  const addStudy = useAnalyzeStore((s) => s.addStudy);
  const studies = useDocumentStore((s) => s.document.analysis_studies) ?? [];
  const selection = useUiStore((s) => s.selection);
  const parts = useDocumentStore((s) => s.parts);
  const addToast = useNotificationStore((s) => s.addToast);
  const [creating, setCreating] = useState(false);

  const newStructural = () => {
    const sel = selection.find((i) => i.kind === "part" || i.kind === "face");
    const partId =
      sel?.kind === "face" ? sel.partId : sel?.kind === "part" ? sel.id : parts[0]?.id;
    if (!partId) {
      addToast("Select a part first", "error");
      return;
    }
    addStudy({
      id: `study-${Date.now()}-${_nextStudyNum++}`,
      name: `Structural ${studies.length + 1}`,
      study: {
        type: "structural",
        partId,
        resolution: 24,
        youngsModulusMpa: 69000,
        poisson: 0.33,
        yieldStrengthMpa: 276,
        loads: [],
        supports: [],
      },
    });
    setCreating(false);
  };

  const newTolerance = () => {
    addStudy({
      id: `study-${Date.now()}-${_nextStudyNum++}`,
      name: `Stackup ${studies.length + 1}`,
      study: {
        type: "tolerance",
        contributors: [
          { name: "dim 1", coeff: 1, nominal: 10, tolMinus: 0.1, tolPlus: 0.1 },
        ],
        requirement: { name: "gap", lowerMm: 0 },
      },
    });
    setCreating(false);
  };

  return (
    <div className="fixed right-0 top-0 bottom-0 w-80 bg-surface border-l border-border z-50 flex flex-col">
      <div className="flex items-center justify-between p-3 border-b border-border">
        <h2 className="font-medium">Analyze</h2>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted"
          onClick={closePanel}
          title="Close"
        >
          <X size={16} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-3 space-y-2">
        {studies.length === 0 && (
          <div className="text-[11px] text-text-muted">
            No studies yet. A study persists on the document and re-verifies
            when the geometry changes.
          </div>
        )}
        {studies.map((s) => (
          <StudyCard key={s.id} study={s} />
        ))}

        {creating ? (
          <div className="flex gap-2">
            <button
              className="flex-1 px-2 py-1.5 text-[11px] rounded bg-brand/15 text-brand hover:bg-brand/25"
              onClick={newStructural}
            >
              Structural FEA
            </button>
            <button
              className="flex-1 px-2 py-1.5 text-[11px] rounded bg-brand/15 text-brand hover:bg-brand/25"
              onClick={newTolerance}
            >
              Tolerance stackup
            </button>
          </div>
        ) : (
          <button
            className="w-full flex items-center justify-center gap-1 px-2 py-1.5 text-[11px] rounded border border-dashed border-border text-text-muted hover:text-text hover:border-text-muted"
            onClick={() => setCreating(true)}
          >
            <Plus size={12} /> New study
          </button>
        )}
      </div>
    </div>
  );
}
