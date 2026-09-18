/**
 * The CAM panel's own surfaces, rendered.
 *
 * The store tests say the gate is shut; these say the operator can see *why*.
 * A refusal the panel computes correctly and then does not draw is a refusal
 * nobody acts on, and the failure mode the whole roadmap exists to stop is a
 * program leaving the app with no one having read the word "refused".
 */
import { describe, it, expect, beforeEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import type { CamOutlineSectioned } from "@vcad/engine";
import { VerificationPanel } from "@/components/cam/VerificationPanel";
import { OperationList } from "@/components/cam/OperationList";
import { ToolpathPreview } from "@/components/cam/ToolpathPreview";
import { deriveOperations, useCamJobStore, DEFAULT_VALUES } from "@/stores/cam-job-store";

function outline(): CamOutlineSectioned {
  return {
    torn: false,
    mesh_source: "raw_tessellation",
    z: 3,
    auto_z: true,
    z_range: [0, 6],
    suggested_stock_thickness: 6,
    plane_nudge: 0,
    healed: 0,
    discarded_slivers: 0,
    regions: [
      {
        outer: [
          [0, 0],
          [80, 0],
          [80, 50],
          [0, 50],
        ],
        holes: [],
        area: 4000,
      },
    ],
    circles: [],
    bounds: [0, 0, 80, 50],
    area: 4000,
    prismatic: { prismatic: true },
    outline: {},
  };
}

function check(name: string, pass: boolean) {
  return {
    name,
    pass,
    severity: "Error" as const,
    violation_count: pass ? 0 : 1,
    worst: 0,
    examples: [] as Array<Record<string, unknown>>,
    note: `${name} note`,
  };
}

const VERIFICATION = {
  pass: false,
  gouge: check("gouge", true),
  material_left: {
    check: check("material_left", true),
    unswept_area: 0,
    max_standoff: 0,
    reachable_band_area: 0,
    untouched_walls: 0,
    walls: 4,
    tab_area: 0,
  },
  rapids: check("rapids", true),
  depth: {
    check: {
      ...check("depth", false),
      worst: 3,
      examples: [
        {
          index: 67,
          xy: [12.9, 12] as [number, number],
          z: -6,
          what: "cuts 3.000 mm past the stock underside with no spoilboard declared",
        },
      ],
    },
    deepest_z: -6,
    floor_z: -3,
    remaining_under_part: -3,
    features: 1,
  },
  tabs: { check: check("tabs", true), observations: [], tab_count: 3, passes_below_tabs: 0 },
  envelope: {
    check: check("envelope", true),
    work_min: [0, 0, -6] as [number, number, number],
    work_max: [80, 50, 5] as [number, number, number],
    machine_min: null,
    machine_max: null,
    stock_margin: [8, 8, 8, 8] as [number, number, number, number],
  },
  loose: {
    check: check("loose_pieces", true),
    pieces: [],
    frame_pieces: [],
    skin_holds: true,
  },
  plunges: check("plunges", true),
  moves: 120,
};

beforeEach(() => {
  cleanup();
  useCamJobStore.getState().reset();
  const o = outline();
  useCamJobStore.setState({
    outline: o,
    operations: deriveOperations(o, {
      toolDiameter: 2,
      stockThickness: 6,
      values: DEFAULT_VALUES,
    }).operations,
  });
});

describe("VerificationPanel", () => {
  it("says what to do before anything has been built", () => {
    const { container } = render(<VerificationPanel />);
    expect(container.textContent).toContain("Build the job");
  });

  it("leads with the refusal and spells the blocker out in millimetres", () => {
    useCamJobStore.setState({
      result: {
        blocked: true,
        policy: { verified: true, blocked_by: ["depth"], warnings: [] },
        notes: [],
        verification: VERIFICATION,
      } as never,
      builtKey: "k",
    });
    const { container } = render(<VerificationPanel />);

    expect(container.textContent).toContain("Refused — this job will not run");
    expect(container.textContent).toContain("3.000 mm past the stock underside");
    // The check name never reaches the operator on its own.
    expect(container.textContent).toContain("Depth against the stock");
    expect(container.textContent).not.toContain("loose_pieces");
    // Every check shows a number, pass or fail: "0 mm" is a result, "clean"
    // is a mood.
    expect(container.textContent).toContain("deepest Z -6.000");
    expect(container.textContent).toContain("nothing breaks through");
  });

  it("offers a checkbox per warning, and nothing to tick on a blocker", () => {
    useCamJobStore.setState({
      result: {
        blocked: false,
        gcode: "G21\nM2\n",
        policy: { verified: true, blocked_by: [], warnings: ["loose_pieces"] },
        notes: [],
        verification: {
          ...VERIFICATION,
          pass: true,
          depth: { ...VERIFICATION.depth, check: check("depth", true) },
          loose: {
            check: { ...check("loose_pieces", false), severity: "Warning" as const, worst: 201 },
            pieces: [{ area: 201 }],
            frame_pieces: [],
            skin_holds: false,
          },
        },
      } as never,
      builtKey: "k",
    });
    const { container } = render(<VerificationPanel />);

    expect(container.textContent).toContain("Replayed against the part and clean");
    const boxes = container.querySelectorAll('input[type="checkbox"]');
    expect(boxes).toHaveLength(1);
    expect(boxes[0]!.getAttribute("aria-label")).toMatch(/^Acknowledge: /);
    expect(container.textContent).toContain("comes free");
  });
});

describe("OperationList", () => {
  it("asks for a contour before it will invent operations", () => {
    useCamJobStore.setState({ outline: null, operations: [] });
    const { container } = render(<OperationList />);
    expect(container.textContent).toContain("Take the contour from the part");
  });

  it("names the cut rather than the kernel's kind string", () => {
    const { container } = render(<OperationList />);
    expect(container.textContent).toContain("Outside profile");
    expect(container.textContent).not.toContain("contour_outside");
  });
});

describe("ToolpathPreview", () => {
  it("draws cut and rapid moves differently, and labels which is which", () => {
    useCamJobStore.setState({
      result: {
        blocked: false,
        gcode: "",
        policy: { verified: true, blocked_by: [], warnings: [] },
        notes: [],
        moves: [
          { to: [0, 0, 5], rapid: true },
          { to: [10, 0, 5], rapid: true },
          { to: [10, 0, -1], rapid: false, feed: 400 },
          { to: [10, 20, -1], rapid: false, feed: 400 },
        ],
        op_ranges: [
          { block: "operation", name: "Outside profile", op_index: 0, tool: 1, start: 0, end: 4, seconds: 3 },
        ],
      } as never,
      builtKey: "k",
    });
    const { container } = render(<ToolpathPreview />);

    const paths = [...container.querySelectorAll("path")];
    // The part outline plus at least one rapid run and one cutting run.
    expect(paths.length).toBeGreaterThanOrEqual(3);
    const dashed = paths.filter((p) => p.getAttribute("stroke-dasharray"));
    expect(dashed.length).toBeGreaterThan(0);
    const solid = paths.filter((p) => !p.getAttribute("stroke-dasharray"));
    expect(solid.length).toBeGreaterThan(0);
    expect(container.textContent).toContain("cut");
    expect(container.textContent).toContain("rapid");
  });
});
