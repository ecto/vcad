/**
 * Circuit analysis panel — Bode plot, solver health, and fail-closed blockers.
 *
 * Docked over the schematic view while the Analyze flow is open. Shows the
 * small-signal AC response (magnitude + phase) of a user-picked output net
 * with log-sweep controls, the Tellegen power-balance residual of the DC
 * solve as a health indicator, and — when the schematic can't be mapped — the
 * per-component blocker list from the fail-closed netlist seam. Also hosts
 * the tune-to-target dialog (adjoint optimizer, one free component).
 */

import { useMemo, useState } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useDocumentStore } from "@vcad/core";
import {
  circuitSignature,
  formatSiValue,
  outputNets,
  runCircuitAnalysis,
  runCircuitTune,
  type TuneTarget,
} from "@/lib/circuit-analysis";

const PANEL =
  "absolute top-3 right-3 z-30 w-[340px] max-h-[calc(100%-24px)] overflow-y-auto rounded-lg border border-border bg-background/95 backdrop-blur px-3 py-2 text-xs shadow-lg";

function fmtHz(f: number): string {
  if (f >= 1e6) return `${parseFloat((f / 1e6).toFixed(1))}MHz`;
  if (f >= 1e3) return `${parseFloat((f / 1e3).toFixed(1))}kHz`;
  return `${parseFloat(f.toFixed(1))}Hz`;
}

function fmtSi(v: number, unit: string): string {
  const a = Math.abs(v);
  if (a === 0) return `0 ${unit}`;
  const scales: Array<[number, string]> = [
    [1, ""],
    [1e-3, "m"],
    [1e-6, "µ"],
    [1e-9, "n"],
    [1e-12, "p"],
    [1e-15, "f"],
  ];
  for (const [s, p] of scales) {
    if (a >= s) return `${(v / s).toPrecision(3)} ${p}${unit}`;
  }
  return `${v.toExponential(1)} ${unit}`;
}

/** Bode plot: |H| in dB and phase in degrees over the swept frequencies. */
function BodePlot({
  points,
  node,
}: {
  points: Array<{ omega: number; nodeVoltagesRe: number[]; nodeVoltagesIm: number[] }>;
  node: number;
}) {
  const W = 312;
  const H = 120;
  const PH = 70;
  const PAD = { l: 34, r: 8, t: 8, b: 16 };

  const data = useMemo(() => {
    return points.map((p) => {
      const re = p.nodeVoltagesRe[node] ?? 0;
      const im = p.nodeVoltagesIm[node] ?? 0;
      const mag = Math.hypot(re, im);
      return {
        f: p.omega / (2 * Math.PI),
        db: 20 * Math.log10(Math.max(mag, 1e-12)),
        deg: (Math.atan2(im, re) * 180) / Math.PI,
      };
    });
  }, [points, node]);

  if (data.length < 2) return null;

  const fMin = Math.log10(data[0]!.f);
  const fMax = Math.log10(data[data.length - 1]!.f);
  const dbVals = data.map((d) => d.db);
  const dbMin = Math.floor(Math.min(...dbVals) / 10) * 10;
  const dbMax = Math.ceil(Math.max(...dbVals) / 10) * 10;
  const x = (f: number) =>
    PAD.l + ((Math.log10(f) - fMin) / Math.max(fMax - fMin, 1e-9)) * (W - PAD.l - PAD.r);
  const yDb = (db: number) =>
    PAD.t + (1 - (db - dbMin) / Math.max(dbMax - dbMin, 1e-9)) * (H - PAD.t - PAD.b);
  const yPh = (deg: number) => PAD.t + (1 - (deg + 180) / 360) * (PH - PAD.t - 12);

  const magPath = data.map((d, i) => `${i ? "L" : "M"}${x(d.f).toFixed(1)},${yDb(d.db).toFixed(1)}`).join(" ");
  const phPath = data.map((d, i) => `${i ? "L" : "M"}${x(d.f).toFixed(1)},${yPh(d.deg).toFixed(1)}`).join(" ");

  const decades: number[] = [];
  for (let e = Math.ceil(fMin); e <= Math.floor(fMax); e++) decades.push(Math.pow(10, e));

  const axis = "var(--border, #444)";
  const muted = "var(--muted-foreground, #888)";

  return (
    <div>
      <svg width={W} height={H} className="block">
        {decades.map((f) => (
          <line key={f} x1={x(f)} y1={PAD.t} x2={x(f)} y2={H - PAD.b} stroke={axis} strokeWidth={0.5} opacity={0.5} />
        ))}
        {[dbMin, (dbMin + dbMax) / 2, dbMax].map((db) => (
          <g key={db}>
            <line x1={PAD.l} y1={yDb(db)} x2={W - PAD.r} y2={yDb(db)} stroke={axis} strokeWidth={0.5} opacity={0.5} />
            <text x={PAD.l - 3} y={yDb(db) + 3} fontSize={8} fill={muted} textAnchor="end">
              {Math.round(db)}
            </text>
          </g>
        ))}
        <path d={magPath} fill="none" stroke="#4ade80" strokeWidth={1.5} />
        {decades.map((f) => (
          <text key={`t-${f}`} x={x(f)} y={H - 4} fontSize={8} fill={muted} textAnchor="middle">
            {fmtHz(f)}
          </text>
        ))}
        <text x={PAD.l} y={PAD.t + 2} fontSize={8} fill="#4ade80">
          |H| dB
        </text>
      </svg>
      <svg width={W} height={PH} className="block">
        {decades.map((f) => (
          <line key={f} x1={x(f)} y1={PAD.t} x2={x(f)} y2={PH - 10} stroke={axis} strokeWidth={0.5} opacity={0.5} />
        ))}
        {[-180, 0, 180].map((deg) => (
          <g key={deg}>
            <line x1={PAD.l} y1={yPh(deg)} x2={W - PAD.r} y2={yPh(deg)} stroke={axis} strokeWidth={0.5} opacity={0.5} />
            <text x={PAD.l - 3} y={yPh(deg) + 3} fontSize={8} fill={muted} textAnchor="end">
              {deg}°
            </text>
          </g>
        ))}
        <path d={phPath} fill="none" stroke="#60a5fa" strokeWidth={1.5} />
        <text x={PAD.l} y={PAD.t + 2} fontSize={8} fill="#60a5fa">
          phase
        </text>
      </svg>
    </div>
  );
}

/** Power-balance residual as a solver-health badge (it is solver error and
 *  nothing else — green means the numbers can be trusted). */
function HealthBadge({ residualW }: { residualW: number }) {
  const a = Math.abs(residualW);
  const ok = a < 1e-9;
  const warn = !ok && a < 1e-6;
  const color = ok ? "#4ade80" : warn ? "#facc15" : "#f87171";
  return (
    <span
      className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 font-mono"
      style={{ color, background: `${color}1a` }}
      title="Tellegen power-balance residual Σv·i of the DC solve — nonzero only through solver error"
    >
      <span className="h-1.5 w-1.5 rounded-full" style={{ background: color }} />
      ΣP = {fmtSi(residualW, "W")}
    </span>
  );
}

/** Tune-to-target dialog for one component (opened from the context menu). */
function TuneDialog() {
  const tuningRef = useElectronicsStore((s) => s.analysis.tuningRef);
  const tuneBusy = useElectronicsStore((s) => s.analysis.tuneBusy);
  const tuneResult = useElectronicsStore((s) => s.analysis.tuneResult);
  const outNet = useElectronicsStore((s) => s.analysis.outNet);
  const setAnalysis = useElectronicsStore((s) => s.setAnalysis);
  const [mode, setMode] = useState<"cutoff" | "dcVoltage">("cutoff");
  const [cutoffHz, setCutoffHz] = useState("1000");
  const [qFactor, setQFactor] = useState("0.707");
  const [volts, setVolts] = useState("2.5");
  const [error, setError] = useState<string | null>(null);

  if (!tuningRef) return null;

  const close = () => setAnalysis({ tuningRef: null, tuneResult: null });
  const run = async () => {
    setError(null);
    setAnalysis({ tuneBusy: true, tuneResult: null });
    try {
      const target: TuneTarget =
        mode === "cutoff"
          ? { type: "cutoff", cutoffHz: parseFloat(cutoffHz), qFactor: parseFloat(qFactor) }
          : { type: "dcVoltage", volts: parseFloat(volts) };
      const result = await runCircuitTune(tuningRef, target);
      useElectronicsStore.getState().setAnalysis({ tuneResult: result });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      useElectronicsStore.getState().setAnalysis({ tuneBusy: false });
    }
  };

  const inputCls =
    "w-full rounded border border-border bg-transparent px-1.5 py-0.5 font-mono text-xs";

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-black/30">
      <div className="w-[300px] rounded-lg border border-border bg-background p-3 text-xs shadow-xl">
        <div className="mb-2 flex items-center justify-between">
          <span className="font-semibold">
            Tune {tuningRef} to target
          </span>
          <button onClick={close} aria-label="Close" className="opacity-60 hover:opacity-100">
            <X size={14} />
          </button>
        </div>
        <div className="mb-2 flex gap-1">
          <button
            className={`flex-1 rounded border px-1.5 py-1 ${mode === "cutoff" ? "border-primary text-primary" : "border-border opacity-70"}`}
            onClick={() => setMode("cutoff")}
          >
            Cutoff / Q
          </button>
          <button
            className={`flex-1 rounded border px-1.5 py-1 ${mode === "dcVoltage" ? "border-primary text-primary" : "border-border opacity-70"}`}
            onClick={() => setMode("dcVoltage")}
          >
            DC voltage
          </button>
        </div>
        {mode === "cutoff" ? (
          <div className="mb-2 grid grid-cols-2 gap-2">
            <label className="block">
              <span className="opacity-70">Cutoff (Hz)</span>
              <input className={inputCls} value={cutoffHz} onChange={(e) => setCutoffHz(e.target.value)} />
            </label>
            <label className="block">
              <span className="opacity-70">Q</span>
              <input className={inputCls} value={qFactor} onChange={(e) => setQFactor(e.target.value)} />
            </label>
          </div>
        ) : (
          <label className="mb-2 block">
            <span className="opacity-70">Target voltage on {outNet ?? "output"} (V)</span>
            <input className={inputCls} value={volts} onChange={(e) => setVolts(e.target.value)} />
          </label>
        )}
        <div className="mb-1 opacity-60">
          Measured at {outNet ?? "—"}; only {tuningRef} moves (adjoint descent).
        </div>
        {error && <div className="mb-1 text-red-400">{error}</div>}
        {tuneResult && (
          <div className="mb-1 rounded bg-primary/10 p-1.5 font-mono">
            {tuneResult.tunedValues.map((t) => (
              <div key={t.device}>
                {formatSiValue(t.before)} → {formatSiValue(t.after)}
              </div>
            ))}
            {tuneResult.achievedCutoffHz != null && (
              <div>achieved fc = {fmtHz(tuneResult.achievedCutoffHz)}</div>
            )}
            {tuneResult.achievedQFactor != null && (
              <div>achieved Q = {tuneResult.achievedQFactor.toPrecision(3)}</div>
            )}
            {tuneResult.achievedDcVoltage != null && (
              <div>achieved V = {tuneResult.achievedDcVoltage.toPrecision(4)} V</div>
            )}
            <div className="opacity-60">{tuneResult.iterations} iterations</div>
          </div>
        )}
        <button
          className="mt-1 w-full rounded bg-primary px-2 py-1 font-semibold text-primary-foreground disabled:opacity-50"
          disabled={tuneBusy}
          onClick={run}
        >
          {tuneBusy ? "Tuning…" : "Tune"}
        </button>
      </div>
    </div>
  );
}

export function CircuitAnalysisPanel() {
  const analysis = useElectronicsStore((s) => s.analysis);
  const setAnalysis = useElectronicsStore((s) => s.setAnalysis);
  const netlist = useElectronicsStore((s) => s.netlist);
  const components = useDocumentStore((s) => s.document.schematic?.components);

  const stale = useMemo(() => {
    if (!analysis.signature || !components || !netlist) return false;
    return circuitSignature(components, netlist) !== analysis.signature;
  }, [analysis.signature, components, netlist]);

  if (!analysis.showPanel) return <TuneDialog />;

  const nets = analysis.mapping ? outputNets(analysis.mapping) : [];
  const outNode =
    analysis.mapping && analysis.outNet != null
      ? analysis.mapping.nodeOfNet[analysis.outNet]
      : undefined;

  const numCls =
    "w-full rounded border border-border bg-transparent px-1.5 py-0.5 font-mono text-xs";

  return (
    <>
      <div className={PANEL}>
        <div className="mb-2 flex items-center justify-between">
          <span className="text-sm font-semibold">Circuit analysis</span>
          <div className="flex items-center gap-2">
            {analysis.dc && <HealthBadge residualW={analysis.dc.powerBalanceW} />}
            <button
              onClick={() => setAnalysis({ showPanel: false })}
              aria-label="Close analysis panel"
              className="opacity-60 hover:opacity-100"
            >
              <X size={14} />
            </button>
          </div>
        </div>

        {stale && (
          <div className="mb-2 flex items-center gap-1 rounded bg-yellow-500/10 px-1.5 py-1 text-yellow-500">
            <Warning size={12} /> Schematic changed since this run — re-analyze.
          </div>
        )}

        <div className="mb-2 flex items-end gap-2">
          <label className="block flex-1">
            <span className="opacity-70">Start</span>
            <input
              className={numCls}
              value={analysis.sweep.startHz}
              onChange={(e) =>
                setAnalysis({ sweep: { ...analysis.sweep, startHz: parseFloat(e.target.value) || 1 } })
              }
            />
          </label>
          <label className="block flex-1">
            <span className="opacity-70">Stop (Hz)</span>
            <input
              className={numCls}
              value={analysis.sweep.stopHz}
              onChange={(e) =>
                setAnalysis({ sweep: { ...analysis.sweep, stopHz: parseFloat(e.target.value) || 10 } })
              }
            />
          </label>
          <label className="block w-14">
            <span className="opacity-70">Pts</span>
            <input
              className={numCls}
              value={analysis.sweep.points}
              onChange={(e) =>
                setAnalysis({ sweep: { ...analysis.sweep, points: parseInt(e.target.value) || 2 } })
              }
            />
          </label>
          <button
            className="flex h-6 items-center gap-1 rounded bg-primary px-2 font-semibold text-primary-foreground disabled:opacity-50"
            disabled={analysis.status === "running"}
            onClick={() => void runCircuitAnalysis()}
            title="Run DC operating point + AC sweep"
          >
            <Play size={12} weight="fill" />
            {analysis.status === "running" ? "…" : "Run"}
          </button>
        </div>

        {analysis.status === "error" && (
          <div className="mb-2 rounded bg-red-500/10 px-1.5 py-1 text-red-400">{analysis.error}</div>
        )}

        {analysis.status === "blocked" && (
          <div className="mb-2">
            <div className="mb-1 flex items-center gap-1 font-semibold text-red-400">
              <Warning size={12} /> {analysis.blockers.length} component
              {analysis.blockers.length === 1 ? "" : "s"} blocked simulation
            </div>
            <ul className="space-y-1">
              {analysis.blockers.map((b) => (
                <li
                  key={b.reference}
                  className="cursor-pointer rounded bg-red-500/10 px-1.5 py-1 hover:bg-red-500/20"
                  onClick={() =>
                    useElectronicsStore.getState().select({ type: "component", ref: b.reference })
                  }
                >
                  <span className="font-mono font-semibold">{b.reference}</span>{" "}
                  <span className="opacity-80">{b.message}</span>
                </li>
              ))}
            </ul>
            <div className="mt-1 opacity-60">
              Nothing simulates until every blocker is fixed — nothing is silently skipped.
            </div>
          </div>
        )}

        {analysis.status === "ok" && analysis.mapping && (
          <>
            <label className="mb-2 block">
              <span className="opacity-70">Output net (Bode)</span>
              <select
                className={numCls}
                value={analysis.outNet ?? ""}
                onChange={(e) => {
                  setAnalysis({ outNet: e.target.value });
                  void runCircuitAnalysis();
                }}
              >
                {nets.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </label>
            {analysis.ac && outNode != null && outNode !== 0 ? (
              <BodePlot points={analysis.ac.points} node={outNode} />
            ) : (
              <div className="mb-2 opacity-60">No AC source found — add a supply or V source.</div>
            )}
            {analysis.mapping.unconnectedSupplies.length > 0 && (
              <div className="mt-1 text-yellow-500">
                Unconnected supplies: {analysis.mapping.unconnectedSupplies.join(", ")}
              </div>
            )}
            {analysis.mapping.stubbed.length > 0 && (
              <div className="mt-1 opacity-60">
                Stubbed as open: {analysis.mapping.stubbed.join(", ")}
              </div>
            )}
            <label className="mt-2 flex items-center gap-1.5">
              <input
                type="checkbox"
                checked={analysis.showDcAnnotations}
                onChange={(e) => setAnalysis({ showDcAnnotations: e.target.checked })}
              />
              <span>Show DC voltages / currents on schematic</span>
            </label>
            <div className="mt-1 opacity-60">
              Right-click a component to tune it to a target.
            </div>
          </>
        )}
      </div>
      <TuneDialog />
    </>
  );
}
