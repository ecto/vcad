import { describe, it, expect } from "vitest";
import type { SchematicComponent } from "@vcad/ir";
import type { NetlistResult } from "@vcad/engine";
import { parseSiValue, buildCircuitSpec } from "@/lib/circuit-build";

describe("parseSiValue", () => {
  it("parses plain, prefixed, and unit-suffixed values", () => {
    expect(parseSiValue("330", 0)).toBe(330);
    expect(parseSiValue("10k", 0)).toBe(10_000);
    expect(parseSiValue("4.7k", 0)).toBe(4_700);
    expect(parseSiValue("100nF", 0)).toBeCloseTo(1e-7);
    expect(parseSiValue("1uF", 0)).toBeCloseTo(1e-6);
    expect(parseSiValue("10mH", 0)).toBeCloseTo(1e-2);
    expect(parseSiValue("2.2M", 0)).toBeCloseTo(2.2e6);
  });
  it("distinguishes milli from mega by case", () => {
    expect(parseSiValue("1m", 0)).toBeCloseTo(1e-3);
    expect(parseSiValue("1M", 0)).toBeCloseTo(1e6);
  });
  it("falls back when unparseable", () => {
    expect(parseSiValue("", 42)).toBe(42);
    expect(parseSiValue("abc", 42)).toBe(42);
    expect(parseSiValue(undefined, 42)).toBe(42);
  });
});

const comp = (ref: string, symbolId: string, value = ""): SchematicComponent => ({
  ref,
  value,
  footprintId: "",
  position: { x: 0, y: 0 },
  pins: [],
  properties: { symbolId },
});

describe("buildCircuitSpec", () => {
  it("builds an LED + resistor circuit with a VCC source and ground", () => {
    // VCC → R1 → (N1) → LED → GND
    const netlist: NetlistResult = {
      nets: [
        { name: "VCC", connections: [{ component_ref: "PWR1", pin_number: "1" }, { component_ref: "R1", pin_number: "1" }] },
        { name: "N1", connections: [{ component_ref: "R1", pin_number: "2" }, { component_ref: "D1", pin_number: "1" }] },
        { name: "GND", connections: [{ component_ref: "PWR2", pin_number: "1" }, { component_ref: "D1", pin_number: "2" }] },
      ],
    };
    const components = [comp("R1", "resistor", "330"), comp("D1", "led"), comp("PWR1", "vcc"), comp("PWR2", "gnd")];

    const { spec, netToNode, refToDevice } = buildCircuitSpec(components, netlist);

    expect(netToNode.get("GND")).toBe(0);
    expect(netToNode.get("VCC")).toBeGreaterThan(0);

    // vsource(VCC,0,5) + resistor(330) + led
    expect(spec.devices).toHaveLength(3);
    const vsrc = spec.devices.find((d) => d.kind === "vsource")!;
    expect(vsrc).toMatchObject({ n: 0, value: 5 });
    expect(vsrc.p).toBe(netToNode.get("VCC"));

    const r = spec.devices[refToDevice.get("R1")!]!;
    expect(r).toMatchObject({ kind: "resistor", value: 330 });
    expect(r.p).toBe(netToNode.get("VCC"));
    expect(r.n).toBe(netToNode.get("N1"));

    const led = spec.devices[refToDevice.get("D1")!]!;
    expect(led).toMatchObject({ kind: "led", n: 0 });
    expect(led.p).toBe(netToNode.get("N1"));

    // power symbols are nets, not devices
    expect(refToDevice.has("PWR1")).toBe(false);
  });

  it("skips components with an unconnected pin", () => {
    const netlist: NetlistResult = {
      nets: [{ name: "GND", connections: [{ component_ref: "R1", pin_number: "2" }] }],
    };
    const { spec } = buildCircuitSpec([comp("R1", "resistor", "1k")], netlist);
    expect(spec.devices).toHaveLength(0); // R1 pin 1 floating
  });
});
