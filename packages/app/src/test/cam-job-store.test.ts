import { describe, it, expect, beforeEach } from "vitest";
import type { CamHost, CamOutlineSectioned } from "@vcad/engine";
import {
  buildJobRequest,
  canExport,
  deriveOperations,
  exportBlocker,
  findingsOf,
  jobKeyOf,
  pendingWarnings,
  useCamJobStore,
  DEFAULT_VALUES,
  type CamJobOperation,
} from "@/stores/cam-job-store";

/**
 * The panel's own two jobs: turning an outline into the cuts it implies, and
 * refusing to let G-code leave the app until the oracle and the operator have
 * both had their say.
 *
 * Nothing here checks "a toolpath came back" — that is how an inside contour
 * that cut outward shipped. These assert diameters against the cutter, which
 * operation each hole becomes, and that the export gate is *shut* in every
 * state where it should be.
 */

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** A circle as a closed polyline, so the region's hole loops are real geometry. */
function circleLoop(cx: number, cy: number, d: number, n = 48): Array<[number, number]> {
  const r = d / 2;
  return Array.from({ length: n }, (_, i) => {
    const t = (2 * Math.PI * i) / n;
    return [cx + r * Math.cos(t), cy + r * Math.sin(t)] as [number, number];
  });
}

/**
 * A 80×50 plate with the three diameters the derivation rules turn on, plus a
 * second hole of one of them so the grouping is exercised.
 */
function plateOutline(diameters: number[]): CamOutlineSectioned {
  const holes = diameters.map((d, i) => circleLoop(10 + i * 15, 25, d));
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
        holes,
        area: 4000,
      },
    ],
    circles: diameters.map((d, i) => ({
      region: 0,
      hole: i,
      center: [10 + i * 15, 25] as [number, number],
      diameter: d,
      rms_error: 1e-6,
      max_error: 1e-5,
    })),
    bounds: [0, 0, 80, 50],
    area: 4000,
    prismatic: { prismatic: true },
    outline: {},
  };
}

const SETUP = {
  toolDiameter: 2,
  stockThickness: 6,
  values: DEFAULT_VALUES,
};

// ---------------------------------------------------------------------------
// Derivation
// ---------------------------------------------------------------------------

describe("deriveOperations: which cut each hole becomes", () => {
  it("bores a Ø2.5 hole helically with a Ø2 cutter — wider than the tool, narrower than two", () => {
    const { operations, refusal } = deriveOperations(plateOutline([2.5]), SETUP);
    expect(refusal).toBeNull();

    const bore = operations.find((o) => o.source.type === "bore");
    expect(bore, "Ø2.5 with a Ø2 cutter must be a helical bore").toBeDefined();
    expect(bore!.kind).toBe("helical_bore");
    if (bore!.source.type !== "bore") throw new Error("narrowing is broken");
    expect(bore!.source.diameter).toBeCloseTo(2.5, 6);
    expect(bore!.source.centres).toHaveLength(1);
    // The bore runs before the profile that would free the part around it.
    expect(operations.indexOf(bore!)).toBeLessThan(
      operations.findIndex((o) => o.kind === "contour_outside"),
    );
  });

  it("cuts a Ø16 hole out as an inside contour — two cutter diameters is the line", () => {
    const { operations, refusal } = deriveOperations(plateOutline([16]), SETUP);
    expect(refusal).toBeNull();

    const opening = operations.find((o) => o.source.type === "opening");
    expect(opening).toBeDefined();
    expect(opening!.kind).toBe("contour_inside");
    // Not a pocket: the default is that the slug drops free, and keeping it is
    // the operator's choice.
    expect(opening!.kind).not.toBe("pocket");
    expect(operations.some((o) => o.source.type === "bore")).toBe(false);
  });

  it("refuses a Ø1.5 hole with a Ø2 cutter rather than quietly skipping it", () => {
    const { operations, unmachinable, refusal } = deriveOperations(
      plateOutline([1.5]),
      SETUP,
    );

    expect(unmachinable).toHaveLength(1);
    expect(unmachinable[0]!.diameter).toBeCloseTo(1.5, 6);
    expect(refusal).toMatch(/cannot be machined/);
    expect(refusal).toContain("1.5");
    expect(refusal).toContain("Ø2");
    // The fix is a smaller cutter, and the refusal says so.
    expect(refusal).toMatch(/smaller one|drill them separately/);

    // No operation is produced for it, and it is not silently turned into
    // something else: the only cut is the outside profile.
    expect(operations).toHaveLength(1);
    expect(operations[0]!.kind).toBe("contour_outside");
  });

  it("two cutter diameters is the line, and it is tested at the line", () => {
    // Ø16 against a Ø2 cutter proves nothing about *where* the threshold is —
    // it would pass with the rule at 2×, 4× or 7×. These two do: below two
    // diameters there is no room to spiral an offset pass, so the hole is
    // bored; above it, it is an opening the cutter can work inside.
    const under = deriveOperations(plateOutline([3.9]), SETUP).operations;
    expect(under.find((o) => o.source.type === "bore")?.kind).toBe("helical_bore");
    expect(under.some((o) => o.source.type === "opening")).toBe(false);

    const over = deriveOperations(plateOutline([4.1]), SETUP).operations;
    expect(over.some((o) => o.source.type === "bore")).toBe(false);
    expect(over.find((o) => o.source.type === "opening")?.kind).toBe("contour_inside");
  });

  it("a hole exactly the cutter's diameter is still unmachinable", () => {
    // The boundary matters: a helical bore has to be *larger* than the cutter,
    // and the kernel refuses an equal one by name. Deriving it here would only
    // move the refusal later.
    const { unmachinable, operations } = deriveOperations(plateOutline([2]), SETUP);
    expect(unmachinable).toHaveLength(1);
    expect(operations.some((o) => o.source.type === "bore")).toBe(false);
  });

  it("holes of one diameter are one pilot row, however many there are", () => {
    const { operations } = deriveOperations(plateOutline([3, 3, 3]), SETUP);
    const bores = operations.filter((o) => o.source.type === "bore");
    expect(bores).toHaveLength(1);
    if (bores[0]!.source.type !== "bore") throw new Error("narrowing is broken");
    expect((bores[0]!.source as { centres: unknown[] }).centres).toHaveLength(3);
    expect(bores[0]!.label).toBe("Pilot Ø3 × 3");
  });

  it("the outside profile is last and carries tabs that hold the part", () => {
    const { operations } = deriveOperations(plateOutline([2.5, 16]), SETUP);
    const last = operations[operations.length - 1]!;
    expect(last.kind).toBe("contour_outside");
    expect(last.tabs).toBe(3);
    expect(last.tab_width).toBe(4);
    // A tab taller than half the stock is most of the part, not a tab.
    expect(last.tab_height).toBeLessThanOrEqual(SETUP.stockThickness / 2);
  });

  it("clamps the stepover to the cutter's radius, whatever was typed", () => {
    const { operations } = deriveOperations(plateOutline([16]), {
      ...SETUP,
      values: { ...DEFAULT_VALUES, stepover: 99 },
    });
    // A pass cannot step over further than the cutter's radius without moving
    // through metal no earlier pass reached.
    for (const op of operations) {
      expect(op.stepover).toBeLessThanOrEqual(0.45 * SETUP.toolDiameter + 1e-9);
    }
  });
});

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

const TOOL = {
  number: 1,
  kind: "flat_end_mill" as const,
  diameter: 2,
  flutes: 2,
  flute_length: 8,
  centre_cutting: true,
};
const MACHINE = {
  name: "Anolex Ultra 2",
  spindle: "dial" as const,
  class: "hobby" as const,
  max_feed: 3000,
  max_accel: 300,
};

function requestFor(operations: CamJobOperation[], outline: CamOutlineSectioned) {
  return buildJobRequest({
    stock: { thickness: 6, margin: 8, spoilboard: null },
    machine: MACHINE,
    tool: TOOL,
    operations,
    outline,
    safeZ: 5,
  });
}

describe("buildJobRequest", () => {
  it("expands a pilot row into one kernel operation per centre", () => {
    const outline = plateOutline([3, 3]);
    const { operations } = deriveOperations(outline, SETUP);
    const request = requestFor(operations, outline);

    const bores = request.operations.filter((o) => o.kind === "helical_bore");
    expect(bores).toHaveLength(2);
    // Each names its own centre and the bore diameter, and the pitch is the
    // stepdown: a helix that descends further per turn than the pass depth
    // would be a different cut.
    expect(bores.map((b) => [b.x, b.y])).toEqual([
      [10, 25],
      [25, 25],
    ]);
    for (const b of bores) {
      expect(b.diameter).toBeCloseTo(3, 6);
      expect(b.pitch).toBe(b.stepdown);
      expect(b.diameter!).toBeGreaterThan(TOOL.diameter);
    }
    expect(bores[0]!.name).toMatch(/· 1$/);
    expect(bores[1]!.name).toMatch(/· 2$/);
  });

  it("puts the part the oracle checks against into the request, and the profile last", () => {
    const outline = plateOutline([16]);
    const { operations } = deriveOperations(outline, SETUP);
    const request = requestFor(operations, outline);

    expect(request.options?.part?.outer).toEqual(outline.regions[0]!.outer);
    expect(request.options?.part?.holes).toHaveLength(1);
    expect(request.options?.verify).toBe(true);

    const last = request.operations[request.operations.length - 1]!;
    expect(last.kind).toBe("contour_outside");
    expect(last.tabs).toBe(3);
    // `order` is what the kernel sorts by, and it follows the list.
    expect(request.operations.map((o) => o.order)).toEqual([0, 1]);
  });

  it("leaves a skipped operation out entirely", () => {
    const outline = plateOutline([16]);
    const { operations } = deriveOperations(outline, SETUP);
    const request = requestFor(
      operations.map((o) => (o.kind === "contour_inside" ? { ...o, enabled: false } : o)),
      outline,
    );
    expect(request.operations).toHaveLength(1);
    expect(request.operations[0]!.kind).toBe("contour_outside");
  });

  it("omits the spoilboard when there is none, rather than declaring a zero one", () => {
    const outline = plateOutline([16]);
    const { operations } = deriveOperations(outline, SETUP);
    expect("spoilboard" in requestFor(operations, outline).stock).toBe(false);

    const withBoard = buildJobRequest({
      stock: { thickness: 6, margin: 8, spoilboard: 12 },
      machine: MACHINE,
      tool: TOOL,
      operations,
      outline,
      safeZ: 5,
    });
    expect(withBoard.stock.spoilboard).toBe(12);
  });

  it("the job key changes with the setup, and ignores the job's name", () => {
    const outline = plateOutline([16]);
    const { operations } = deriveOperations(outline, SETUP);
    const a = requestFor(operations, outline);
    const b = requestFor(operations, outline);
    expect(jobKeyOf(a)).toBe(jobKeyOf(b));
    expect(jobKeyOf({ ...a, name: "something else" })).toBe(jobKeyOf(a));

    const faster = requestFor(
      operations.map((o) => ({ ...o, feed: o.feed + 1 })),
      outline,
    );
    expect(jobKeyOf(faster)).not.toBe(jobKeyOf(a));
  });
});

// ---------------------------------------------------------------------------
// The export gate
// ---------------------------------------------------------------------------

/** A host that answers one canned job document. */
function hostAnswering(answer: unknown): CamHost {
  const give = <T,>(): T => answer as T;
  return {
    camJob: give,
    camVerifyGcode: give,
    camFit: give,
    camMaterials: give,
    camRecommendFeeds: give,
    camCheckFeeds: give,
    camGear: give,
    camOutlineFromDocument: give,
  };
}

const PASSED_WITH_WARNING = {
  name: "plate",
  blocked: false,
  gcode: "G21\nG90\nG0 Z5\nM2\n",
  policy: {
    verified: true,
    replayed: "gcode",
    blocked_by: [] as string[],
    warnings: ["loose_pieces"],
  },
  notes: [],
  verification: {
    pass: true,
    gouge: check("gouge", true),
    material_left: { check: check("material_left", true), unswept_area: 0, max_standoff: 0, reachable_band_area: 0, untouched_walls: 0, walls: 4, tab_area: 0 },
    rapids: check("rapids", true),
    depth: { check: check("depth", true), deepest_z: -6, floor_z: -6, remaining_under_part: 0, features: 1 },
    tabs: { check: check("tabs", true), observations: [], tab_count: 3, passes_below_tabs: 0 },
    envelope: { check: check("envelope", true), work_min: [0, 0, -6], work_max: [80, 50, 5], machine_min: null, machine_max: null, stock_margin: [8, 8, 8, 8] },
    loose: {
      check: { ...check("loose_pieces", false), severity: "Warning" as const, violation_count: 1, worst: 201.06 },
      pieces: [{ area: 201.06 }],
      frame_pieces: [],
      skin_holds: false,
    },
    plunges: check("plunges", true),
    moves: 2130,
  },
};

const REFUSED = {
  name: "plate",
  blocked: true,
  policy: {
    verified: true,
    replayed: "gcode",
    blocked_by: ["depth"],
    warnings: [] as string[],
  },
  notes: [],
  verification: {
    ...PASSED_WITH_WARNING.verification,
    pass: false,
    depth: {
      check: {
        ...check("depth", false),
        violation_count: 1,
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
  },
};

function check(name: string, pass: boolean) {
  return {
    name,
    pass,
    severity: "Error" as const,
    violation_count: 0,
    worst: 0,
    examples: [] as Array<Record<string, unknown>>,
    note: `${name} note`,
  };
}

/** Put the store in a state that has a buildable job. */
function seedStore(): void {
  const outline = plateOutline([3, 16]);
  const { operations } = deriveOperations(outline, SETUP);
  useCamJobStore.setState({
    outline,
    operations,
    tool: { ...TOOL },
    stock: { thickness: 6, margin: 8, spoilboard: null },
    machine: MACHINE,
    values: DEFAULT_VALUES,
    safeZ: 5,
    result: null,
    builtKey: null,
    buildError: null,
    acknowledgedKey: null,
    acknowledgedIds: [],
  });
}

beforeEach(() => {
  useCamJobStore.getState().reset();
  seedStore();
});

describe("the export gate", () => {
  it("is shut before anything has been built", () => {
    expect(canExport(useCamJobStore.getState())).toBe(false);
    expect(exportBlocker(useCamJobStore.getState())).toMatch(/Build the job/);
  });

  it("stays shut on a refused job, and there is no G-code to leak", () => {
    useCamJobStore.getState().build(hostAnswering(REFUSED));
    const s = useCamJobStore.getState();

    expect(s.result?.blocked).toBe(true);
    expect("gcode" in (s.result ?? {})).toBe(false);
    expect(canExport(s)).toBe(false);

    // The blocker is a sentence, not a check name.
    const blocker = exportBlocker(s);
    expect(blocker).toMatch(/past the stock underside/);
    expect(blocker).not.toMatch(/^depth$/);

    const { blockers } = findingsOf(s.result!);
    expect(blockers.map((b) => b.id)).toContain("depth");
    expect(blockers[0]!.xy).toEqual([12.9, 12]);
  });

  it("holds a passing job shut until its warning is acknowledged", () => {
    useCamJobStore.getState().build(hostAnswering(PASSED_WITH_WARNING));
    let s = useCamJobStore.getState();

    expect(s.result?.blocked).toBe(false);
    expect(pendingWarnings(s).map((w) => w.id)).toEqual(["loose_pieces"]);
    expect(canExport(s)).toBe(false);
    expect(exportBlocker(s)).toMatch(/^Acknowledge: /);
    // The warning is the sentence, with the area — "loose_pieces" is not a
    // thing to click past.
    expect(exportBlocker(s)).toMatch(/comes free/);

    useCamJobStore.getState().acknowledge("loose_pieces", true);
    s = useCamJobStore.getState();
    expect(canExport(s)).toBe(true);
    expect(exportBlocker(s)).toBeNull();
  });

  it("forgets the acknowledgement when the job is edited", () => {
    useCamJobStore.getState().build(hostAnswering(PASSED_WITH_WARNING));
    useCamJobStore.getState().acknowledge("loose_pieces", true);
    expect(canExport(useCamJobStore.getState())).toBe(true);

    // A feed change is a different job. The warning was read about the old one.
    useCamJobStore.getState().setValues({ feed: 555 });
    let s = useCamJobStore.getState();
    expect(s.result).toBeNull();
    expect(canExport(s)).toBe(false);

    // Even once the same answer comes back, the acknowledgement is stale:
    // the key it was given under is not the key of the job now on the bench.
    useCamJobStore.getState().build(hostAnswering(PASSED_WITH_WARNING));
    s = useCamJobStore.getState();
    expect(s.result?.blocked).toBe(false);
    expect(pendingWarnings(s).map((w) => w.id)).toEqual(["loose_pieces"]);
    expect(canExport(s)).toBe(false);
  });

  it("un-acknowledging shuts it again", () => {
    useCamJobStore.getState().build(hostAnswering(PASSED_WITH_WARNING));
    useCamJobStore.getState().acknowledge("loose_pieces", true);
    expect(canExport(useCamJobStore.getState())).toBe(true);
    useCamJobStore.getState().acknowledge("loose_pieces", false);
    expect(canExport(useCamJobStore.getState())).toBe(false);
  });

  /**
   * Mutation check for the gate itself.
   *
   * The failure this guards against is a gate that reads "did the kernel
   * answer?" rather than "did it say yes?" — which is what the old panel did,
   * and why an unverified program could be exported. Both mutations below are
   * plausible one-line edits, and both are caught.
   */
  it("mutation: a gate that ignores `blocked`, or ignores warnings, is wrong", () => {
    useCamJobStore.getState().build(hostAnswering(REFUSED));
    const refused = useCamJobStore.getState();
    // Mutant A — `canExport = () => !!s.result`
    expect(!!refused.result).toBe(true);
    expect(canExport(refused)).toBe(false);

    // Mutant A' — dropping the `blocked` test and leaning on "there is no
    // G-code anyway". There is exactly one state where those differ: a refused
    // job that came back carrying a program. `camJob` refuses to hand that
    // over, so it is written in here by hand — the gate must not be the only
    // thing standing between a refusal and a spindle, and it must not be the
    // thing that fails first either.
    useCamJobStore.setState({
      result: {
        ...(REFUSED as unknown as Record<string, unknown>),
        gcode: "G21\nG90\nM2\n",
      } as never,
    });
    const smuggled = useCamJobStore.getState();
    expect(smuggled.result?.blocked).toBe(true);
    expect(canExport(smuggled)).toBe(false);
    expect(exportBlocker(smuggled)).toMatch(/past the stock underside/);

    useCamJobStore.getState().reset();
    seedStore();
    useCamJobStore.getState().build(hostAnswering(PASSED_WITH_WARNING));
    const warned = useCamJobStore.getState();
    // Mutant B — `canExport = (s) => !s.result?.blocked`
    expect(warned.result?.blocked).toBe(false);
    expect(canExport(warned)).toBe(false);
  });

  it("a request the kernel could not read is an error, not a silent pass", () => {
    useCamJobStore
      .getState()
      .build(hostAnswering({ error: "stock.thickness must be greater than zero, not 0." }));
    const s = useCamJobStore.getState();
    expect(s.result).toBeNull();
    expect(s.buildError).toMatch(/stock.thickness/);
    expect(canExport(s)).toBe(false);
  });
});
