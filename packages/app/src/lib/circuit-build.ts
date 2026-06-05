/**
 * Translate a schematic + its netlist into a circuit-sim spec.
 *
 * Nets become numbered nodes (ground → 0), components become devices, and
 * power symbols define the rails: a `GND` net is the reference, a `VCC`-like
 * net gets an independent voltage source. The returned maps let the renderer
 * look the live state back up: a net's node voltage colours its wires, a
 * component's device current drives its glow.
 */

import type { SchematicComponent } from "@vcad/ir";
import type { NetlistResult } from "@vcad/engine";

/** One device in a {@link CircuitSpec}; `p`/`n` are node ids (0 = ground). */
export interface CircuitDeviceSpec {
  kind: "resistor" | "capacitor" | "inductor" | "vsource" | "isource" | "diode" | "led";
  p: number;
  n: number;
  value?: number;
}

/** JSON spec passed to the WASM `CircuitSim`. */
export interface CircuitSpec {
  dt: number;
  devices: CircuitDeviceSpec[];
}

/** A built circuit plus the maps needed to project results back onto the schematic. */
export interface BuiltCircuit {
  spec: CircuitSpec;
  /** Net name → node id. */
  netToNode: Map<string, number>;
  /** Component ref → device index (into `spec.devices`). */
  refToDevice: Map<string, number>;
}

const SI_PREFIX: Record<string, number> = {
  p: 1e-12,
  n: 1e-9,
  u: 1e-6,
  µ: 1e-6,
  m: 1e-3,
  k: 1e3,
  K: 1e3,
  M: 1e6,
  G: 1e9,
};

/**
 * Parse an engineering value like `"330"`, `"10k"`, `"100nF"`, `"4.7uF"`,
 * `"10mH"`, `"2.2M"` into a base-unit number. Unit letters after the prefix
 * (F, H, Ω, …) are ignored. Returns `fallback` if it can't parse.
 */
export function parseSiValue(s: string | undefined, fallback: number): number {
  if (!s) return fallback;
  const m = s.trim().match(/^([0-9]*\.?[0-9]+)\s*([pnuµmkKMG]?)/);
  if (!m) return fallback;
  const num = parseFloat(m[1]!);
  if (!isFinite(num)) return fallback;
  const prefix = m[2]!;
  const mult = prefix ? (SI_PREFIX[prefix] ?? 1) : 1;
  return num * mult;
}

const isGround = (name: string) => /^(gnd|0|vss)$/i.test(name);
const isSupply = (name: string) =>
  /^(vcc|vdd|v\+)$/i.test(name) || /^\+?\d+(v\d*|v)$/i.test(name);

/**
 * Build a circuit-sim spec from a schematic + netlist.
 *
 * @param vcc supply voltage applied to each VCC-like net (default 5 V).
 * @param dt  simulation timestep (default 10 µs).
 */
export function buildCircuitSpec(
  components: SchematicComponent[],
  netlist: NetlistResult,
  opts?: { dt?: number; vcc?: number },
): BuiltCircuit {
  const dt = opts?.dt ?? 1e-5;
  const vcc = opts?.vcc ?? 5.0;

  // (ref, pinNumber) → net name
  const pinNet = new Map<string, string>();
  for (const net of netlist.nets) {
    for (const conn of net.connections) {
      pinNet.set(`${conn.component_ref}.${conn.pin_number}`, net.name);
    }
  }

  // A net's electrical role is decided first by the power symbols connected to
  // it (robust to whatever the netlister names the net), then by net name.
  const symById = new Map(
    components.map((c) => [c.ref, (c.properties?.symbolId ?? "").toLowerCase()]),
  );
  const netRole = (net: NetlistResult["nets"][number]): "gnd" | "vcc" | null => {
    for (const conn of net.connections) {
      const sym = symById.get(conn.component_ref);
      if (sym === "gnd") return "gnd";
      if (sym === "vcc" || sym === "vdd") return "vcc";
    }
    if (isGround(net.name)) return "gnd";
    if (isSupply(net.name)) return "vcc";
    return null;
  };
  const roleOf = new Map(netlist.nets.map((n) => [n.name, netRole(n)]));

  // Assign node ids: every ground net → 0, the rest → 1, 2, …
  const netToNode = new Map<string, number>();
  for (const net of netlist.nets) {
    if (roleOf.get(net.name) === "gnd") netToNode.set(net.name, 0);
  }
  let nextNode = 1;
  for (const net of netlist.nets) {
    if (!netToNode.has(net.name)) netToNode.set(net.name, nextNode++);
  }

  const nodeOf = (ref: string, pin: string): number | null => {
    const net = pinNet.get(`${ref}.${pin}`);
    if (net === undefined) return null;
    return netToNode.get(net) ?? null;
  };

  const devices: CircuitDeviceSpec[] = [];
  const refToDevice = new Map<string, number>();

  // One voltage source per supply net (referenced to ground).
  for (const net of netlist.nets) {
    if (roleOf.get(net.name) === "vcc") {
      const node = netToNode.get(net.name)!;
      devices.push({ kind: "vsource", p: node, n: 0, value: vcc });
    }
  }

  // Components → two-terminal devices (pin "1" = p, pin "2" = n).
  for (const comp of components) {
    const sym = (comp.properties?.symbolId ?? "").toLowerCase();
    const p = nodeOf(comp.ref, "1");
    const n = nodeOf(comp.ref, "2");
    const add = (kind: CircuitDeviceSpec["kind"], value?: number) => {
      if (p === null || n === null) return; // unconnected pin → skip
      refToDevice.set(comp.ref, devices.length);
      devices.push({ kind, p, n, value });
    };
    switch (sym) {
      case "resistor":
        add("resistor", parseSiValue(comp.value, 1_000));
        break;
      case "capacitor":
        add("capacitor", parseSiValue(comp.value, 1e-7));
        break;
      case "inductor":
        add("inductor", parseSiValue(comp.value, 1e-3));
        break;
      case "led":
        add("led");
        break;
      case "diode":
        add("diode");
        break;
      default:
        break; // vcc/gnd symbols define nets, not devices
    }
  }

  return { spec: { dt, devices }, netToNode, refToDevice };
}
