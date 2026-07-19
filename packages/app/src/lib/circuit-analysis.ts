/**
 * One-shot circuit analysis over the schematic — the "Analyze" flow.
 *
 * Maps the active schematic through the fail-closed netlist seam
 * (`circuitFromSchematic`, kernel-side `vcad-ecad-sim::circuit::netlist`),
 * then runs the DC operating point and a log-swept small-signal AC response
 * via the stateless WASM analyses. Ground and supply rails are resolved by
 * power *symbol* first (like the live-sim builder), then by net name, and the
 * power symbols themselves are stubbed as opens so the seam never sees their
 * unmappable refdes prefixes. Blockers come back as data pinned to component
 * refs — nothing is silently skipped.
 */

import { useDocumentStore } from "@vcad/core";
import {
  circuitAcResponse,
  circuitDcOperatingPoint,
  circuitFromSchematic,
  circuitTune,
  type CircuitMapOptions,
  type CircuitMapResult,
  type CircuitTuneResult,
  type NetlistResult,
} from "@vcad/engine";
import type { SchematicComponent } from "@vcad/ir";
import { parseSiValue } from "@/lib/circuit-build";
import { useElectronicsStore } from "@/stores/electronics-store";

/** Default log sweep, matching the MCP `simulate_circuit` defaults. */
export const DEFAULT_SWEEP = { startHz: 10, stopHz: 1e6, points: 60 };

const isGroundName = (name: string) => /^(gnd|agnd|dgnd|0|0v|vss)$/i.test(name);
const isSupplyName = (name: string) =>
  /^(vcc|vdd|v\+)$/i.test(name) || /^\+?\d+(\.\d+)?(v\d*|v)$/i.test(name);

/** Symbol ids that define rails rather than devices. */
const POWER_SYMBOLS = new Set(["gnd", "vcc", "vdd"]);

/**
 * Resolve mapping options from the schematic's power symbols + net names:
 * which nets are ground, which are supplies (and at what voltage), and which
 * components (the power symbols) to stub as opens.
 */
export function computeMapOptions(
  components: SchematicComponent[],
  netlist: NetlistResult,
): CircuitMapOptions {
  const symById = new Map(
    components.map((c) => [c.ref, (c.properties?.symbolId ?? "").toLowerCase()]),
  );
  const groundNets: string[] = [];
  const supplies: Array<{ net: string; volts: number }> = [];
  for (const net of netlist.nets) {
    let role: "gnd" | "vcc" | null = null;
    let volts = 5;
    for (const conn of net.connections) {
      const sym = symById.get(conn.component_ref);
      if (sym === "gnd") role = "gnd";
      if (sym === "vcc" || sym === "vdd") {
        role = "vcc";
        const comp = components.find((c) => c.ref === conn.component_ref);
        volts = parseSiValue(comp?.value, 5);
      }
      if (role) break;
    }
    if (!role) {
      if (isGroundName(net.name)) role = "gnd";
      else if (isSupplyName(net.name)) {
        role = "vcc";
        const m = net.name.match(/^\+?(\d+(\.\d+)?)v/i);
        if (m) volts = parseFloat(m[1]!);
      }
    }
    if (role === "gnd") groundNets.push(net.name);
    else if (role === "vcc") supplies.push({ net: net.name, volts });
  }
  const stubAsOpen = components
    .filter((c) => POWER_SYMBOLS.has((c.properties?.symbolId ?? "").toLowerCase()))
    .map((c) => c.ref);
  return { groundNets, supplies, stubAsOpen };
}

/**
 * Structural signature of the circuit (same idea as the live-sim rebuild key):
 * results are stale when this no longer matches the schematic.
 */
export function circuitSignature(
  components: SchematicComponent[],
  netlist: NetlistResult,
): string {
  return JSON.stringify({
    c: components.map((c) => [c.ref, c.value, c.properties?.symbolId]),
    n: netlist.nets.map((n) => [n.name, n.connections.length]),
  });
}

/** Log-spaced frequency grid (Hz). */
export function logSweep(startHz: number, stopHz: number, points: number): number[] {
  const n = Math.min(Math.max(Math.round(points), 2), 500);
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    out.push(startHz * Math.pow(stopHz / startHz, i / (n - 1)));
  }
  return out;
}

/**
 * The AC driving source: an explicit V-refdes source if one exists (lowest
 * refdes wins for determinism), otherwise the first injected supply rail.
 */
export function pickAcSource(mapping: CircuitMapResult): number | null {
  const vRefs = Object.entries(mapping.deviceOfRef)
    .filter(([ref]) => /^v/i.test(ref))
    .sort(([a], [b]) => a.localeCompare(b, undefined, { numeric: true }));
  if (vRefs.length > 0) return vRefs[0]![1];
  const supplies = Object.values(mapping.supplySourceOfNet);
  return supplies.length > 0 ? supplies[0]! : null;
}

/** Non-ground nets, the pickable Bode outputs. */
export function outputNets(mapping: CircuitMapResult): string[] {
  return Object.entries(mapping.nodeOfNet)
    .filter(([, node]) => node !== 0)
    .map(([net]) => net)
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
}

/**
 * Run the full analysis (map → DC → AC) and publish results to the
 * electronics store. Reads the current schematic + netlist from the stores.
 */
export async function runCircuitAnalysis(): Promise<void> {
  const el = useElectronicsStore.getState();
  const schematic = useDocumentStore.getState().document.schematic;
  const netlist = el.netlist;
  if (!schematic || !netlist) {
    el.setAnalysis({ status: "error", error: "No schematic to analyze." });
    return;
  }
  el.setAnalysis({ status: "running", error: null });
  try {
    const options = computeMapOptions(schematic.components, netlist);
    const mapping = await circuitFromSchematic(schematic, options);
    if (!mapping) {
      el.setAnalysis({
        status: "error",
        error: "Circuit analysis needs a newer kernel WASM build.",
      });
      return;
    }
    if (!mapping.ok) {
      el.setAnalysis({
        status: "blocked",
        blockers: mapping.blockers ?? [],
        mapping: null,
        dc: null,
        ac: null,
        signature: circuitSignature(schematic.components, netlist),
      });
      return;
    }
    const devices = mapping.devices ?? [];
    if (devices.length === 0) {
      el.setAnalysis({ status: "error", error: "Nothing to simulate yet — place some components." });
      return;
    }
    const spec = { devices };
    const dc = await circuitDcOperatingPoint(spec);

    // AC sweep: keep the prior output net if it still exists, else default to
    // the last non-ground net (usually the interesting output).
    const nets = outputNets(mapping);
    const prevOut = useElectronicsStore.getState().analysis.outNet;
    const outNet = prevOut && nets.includes(prevOut) ? prevOut : (nets[nets.length - 1] ?? null);
    const sourceId = pickAcSource(mapping);
    const sweep = useElectronicsStore.getState().analysis.sweep;
    const ac =
      sourceId != null
        ? await circuitAcResponse(
            spec,
            sourceId,
            logSweep(sweep.startHz, sweep.stopHz, sweep.points).map((f) => 2 * Math.PI * f),
          )
        : null;

    el.setAnalysis({
      status: "ok",
      error: null,
      blockers: [],
      mapping,
      spec,
      dc,
      ac,
      outNet,
      sourceId,
      signature: circuitSignature(schematic.components, netlist),
    });
  } catch (e) {
    el.setAnalysis({
      status: "error",
      error: e instanceof Error ? e.message : String(e),
    });
  }
}

/** A tune request from the UI. */
export type TuneTarget =
  | { type: "cutoff"; cutoffHz: number; qFactor: number }
  | { type: "dcVoltage"; volts: number };

/**
 * Tune one component toward a target with the adjoint optimizer, then animate
 * the value change into the document and re-run the analysis.
 */
export async function runCircuitTune(
  ref: string,
  target: TuneTarget,
): Promise<CircuitTuneResult | null> {
  const el = useElectronicsStore.getState();
  const { mapping, spec, outNet, sourceId } = el.analysis;
  if (!mapping || !spec) throw new Error("Run an analysis first.");
  const device = mapping.deviceOfRef[ref];
  if (device === undefined) throw new Error(`${ref} is not a simulated device.`);
  const outNode = outNet != null ? mapping.nodeOfNet[outNet] : undefined;
  if (outNode === undefined || outNode === 0) {
    throw new Error("Pick a non-ground output net first.");
  }

  let tuneSpec;
  if (target.type === "cutoff") {
    if (sourceId == null) throw new Error("No driving source for a filter tune.");
    tuneSpec = {
      filter: {
        cutoffHz: target.cutoffHz,
        qFactor: target.qFactor,
        sourceId,
        outNode,
      },
      freeDevices: [{ device }],
    };
  } else {
    tuneSpec = {
      dc: { node: outNode, dcVoltage: target.volts },
      freeDevices: [{ device }],
    };
  }
  const result = await circuitTune(spec, tuneSpec);
  if (!result) throw new Error("Circuit tuning needs a newer kernel WASM build.");

  const tuned = result.tunedValues.find((t) => t.device === device);
  if (tuned && isFinite(tuned.after) && tuned.after > 0) {
    const kind = spec.devices[device]?.kind;
    await animateComponentValue(ref, tuned.before, tuned.after, kind);
  }
  await runCircuitAnalysis();
  return result;
}

/** Engineering-notation formatter for schematic value strings. */
export function formatSiValue(value: number, kind?: string): string {
  const unit = kind === "capacitor" ? "F" : kind === "inductor" ? "H" : kind === "vsource" ? "V" : "";
  const scales: Array<[number, string]> = [
    [1e9, "G"],
    [1e6, "M"],
    [1e3, "k"],
    [1, ""],
    [1e-3, "m"],
    [1e-6, "u"],
    [1e-9, "n"],
    [1e-12, "p"],
  ];
  for (const [scale, prefix] of scales) {
    if (Math.abs(value) >= scale) {
      const mant = value / scale;
      const digits = mant >= 100 ? 0 : mant >= 10 ? 1 : 2;
      return `${parseFloat(mant.toFixed(digits))}${prefix}${unit}`;
    }
  }
  return `${value}${unit}`;
}

/**
 * Animate a component's value from `before` to `after` (log-space ease over
 * ~600 ms) by writing intermediate value strings into the document — the
 * optimization-as-direct-manipulation beat. The final frame writes the exact
 * tuned value.
 */
async function animateComponentValue(
  ref: string,
  before: number,
  after: number,
  kind?: string,
): Promise<void> {
  const doc = useDocumentStore.getState();
  const idx = doc.document.schematic?.components.findIndex((c) => c.ref === ref) ?? -1;
  if (idx < 0) return;
  const setValue = (v: number) =>
    useDocumentStore.getState().updateSchematicComponent(idx, { value: formatSiValue(v, kind) });

  const duration = 600;
  const frames = 12;
  const lnB = Math.log(before);
  const lnA = Math.log(after);
  for (let i = 1; i <= frames; i++) {
    const t = i / frames;
    const eased = t * t * (3 - 2 * t); // smoothstep
    setValue(i === frames ? after : Math.exp(lnB + (lnA - lnB) * eased));
    if (i < frames) await new Promise((r) => setTimeout(r, duration / frames));
  }
}
