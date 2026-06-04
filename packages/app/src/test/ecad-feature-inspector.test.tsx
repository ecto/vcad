/**
 * Task 3 (modeless PCB editing): the per-item ECAD inspector renders inside
 * the main contextual PropertyPanel. These tests pin the component's dispatch
 * — the selection type alone drives what shows — so the unified inspector
 * can't silently regress to the old floating-panel behaviour.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import { EcadFeatureInspector } from "@/components/electronics/EcadFeatureInspector";
import { useElectronicsStore } from "@/stores/electronics-store";

describe("EcadFeatureInspector", () => {
  beforeEach(() => {
    cleanup();
    useElectronicsStore.setState({ selection: { type: "none" }, netlist: null });
  });

  it("renders nothing for the empty selection (the board HUD owns that state)", () => {
    useElectronicsStore.setState({ selection: { type: "none" } });
    const { container } = render(<EcadFeatureInspector />);
    expect(container.textContent).toBe("");
  });

  it("renders a pad inspector from the selection alone (no board data needed)", () => {
    useElectronicsStore.setState({
      selection: { type: "pad", fpRef: "R1", padNum: "2", net: "GND" },
    });
    const { container } = render(<EcadFeatureInspector />);
    expect(container.textContent).toContain("R1.2");
    expect(container.textContent).toContain("GND");
  });

  it("renders a net inspector with its name as the heading", () => {
    useElectronicsStore.setState({
      selection: { type: "net", netId: "VCC" },
      netlist: { nets: [] } as never,
    });
    const { container } = render(<EcadFeatureInspector />);
    expect(container.textContent).toContain("VCC");
    // With no netlist connections, counts fall back to zero rather than crash.
    expect(container.textContent).toContain("Pads");
  });
});
