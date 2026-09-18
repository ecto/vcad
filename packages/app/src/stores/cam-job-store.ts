/**
 * The web app's CAM job: outline in, verified job out, nothing in between.
 *
 * This store is the browser's half of the same pipeline the native app and the
 * MCP `cam` tools run. It holds no machining logic of its own — every number
 * that decides whether a cut is safe comes from `vcad-cam-api` through
 * `@vcad/engine`'s typed CAM client. What lives here is the *setup*: which
 * outline, which stock, which tool, which operations the outline implies, and
 * the two pieces of bookkeeping a verified pipeline needs — the blockers and
 * warnings the oracle reported, and which warnings the operator has read.
 *
 * ## Why the old panel had to go
 *
 * It called the granular WASM bindings one operation at a time — `camGenerateFace`,
 * `camGeneratePocket`, `camGenerateContour` — posted each toolpath separately,
 * and exported whatever came back. Nothing replayed the program against the
 * part, so nothing could say no; and until this week those browser contour and
 * pocket offsets were offsets of the *bounding box*, not the contour. The web
 * app must not be the one surface that can still export an unverified job.
 *
 * ## The two rules the UI is built on
 *
 * 1. **A blocked job has no G-code.** Not "an export button that is disabled":
 *    the key is absent from the answer, and `CamJobResult` types it as `never`
 *    on the blocked arm, so there is nothing to export.
 * 2. **A warning is read one at a time, and editing forgets it.** The
 *    acknowledgements are keyed to the job they were given for
 *    ({@link jobKeyOf}), so changing a feed, a tool or the stock drops them —
 *    no reset call to forget to make.
 */

import { create } from "zustand";
import type { Document } from "@vcad/ir";
import {
  CamError,
  camChecks,
  camJob,
  camOutlineFromDocument,
  camRecommend,
  type CamCheckReport,
  type CamHost,
  type CamJobRequest,
  type CamJobResult,
  type CamOperationKind,
  type CamOperationRequest,
  type CamOutlineResult,
  type CamOutlineSectioned,
  type CamRecommendResult,
  type CamToolRequest,
} from "@vcad/engine";

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/** The blank the part comes out of. Units are mm, Z up, stock top at Z0. */
export interface CamJobStock {
  thickness: number;
  /** How far the blank stands proud of the part. */
  margin: number;
  /** Sacrificial board under the stock. `null` means there is none, and a cut
   *  that would break through is refused rather than run into the bed. */
  spoilboard: number | null;
}

/** The one cutter a web job runs. */
export interface CamJobTool {
  number: number;
  kind: NonNullable<CamToolRequest["kind"]>;
  diameter: number;
  flutes: number;
  flute_length: number;
  stickout?: number;
  centre_cutting: boolean;
}

/** The machine, as far as the checks are concerned. */
export interface CamJobMachine {
  name: string;
  /** `"dial"` is a trim router whose `S` word does nothing. */
  spindle: "dial" | "controlled";
  class: "hobby" | "benchtop" | "vmc";
  max_feed: number;
  max_accel: number;
}

/** Feeds and step sizes shared by every derived operation. */
export interface CamJobCuttingValues {
  stepdown: number;
  stepover: number;
  feed: number;
  plunge: number;
  rpm: number;
  /** Onion skin. Negative breaks through into a declared spoilboard. */
  bottom_allowance: number;
}

/** Where an operation came from, which is also what it is cutting. */
export type CamOpSource =
  | {
      type: "bore";
      /** Diameter rounded to 1/100 mm — the grouping key. */
      key: number;
      diameter: number;
      centres: Array<[number, number]>;
    }
  | { type: "opening"; hole: number; contour: Array<[number, number]> }
  | { type: "outer"; contour: Array<[number, number]> };

/** One row of the operations list. */
export interface CamJobOperation extends CamJobCuttingValues {
  id: string;
  label: string;
  kind: CamOperationKind;
  source: CamOpSource;
  enabled: boolean;
  depth: number;
  tabs?: number;
  tab_width?: number;
  tab_height?: number;
}

/** A hole no cutter this size can reach into. */
export interface CamUnmachinableHole {
  diameter: number;
  center: [number, number];
}

// ---------------------------------------------------------------------------
// Operation derivation — the same rules the native app applies
// ---------------------------------------------------------------------------

/** Floating-point slack for the diameter comparisons, in mm. */
const EPS = 1e-9;

/**
 * Round a diameter to 1/100 mm. Two holes drawn at Ø4 and Ø4.0000001 are one
 * pilot group, and a machinist would call them one drill.
 */
export function boreGroupKey(diameter: number): number {
  return Math.round(diameter * 100);
}

/** `Ø4`, `Ø4.25` — trailing zeros dropped, because "Ø4.00" reads as a tolerance. */
function diameterLabel(d: number): string {
  return `Ø${Number(d.toFixed(2))}`;
}

/** The label an opening gets when it is not round. */
function openingLabel(points: Array<[number, number]>, circleDiameter?: number): string {
  if (circleDiameter !== undefined) return `Bore ${diameterLabel(circleDiameter)}`;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const [x, y] of points) {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  }
  return `Opening ${(maxX - minX).toFixed(1)} × ${(maxY - minY).toFixed(1)}`;
}

/** What {@link deriveOperations} answers. */
export interface CamDerivation {
  operations: CamJobOperation[];
  /** Holes the cutter is too fat to enter. These are a refusal, not an op. */
  unmachinable: CamUnmachinableHole[];
  /** The refusal to show when there are any, in machinist language. */
  refusal: string | null;
}

/**
 * The cuts an outline implies, in the order they have to run.
 *
 * Three rules, and they are the native app's, because an operator who moves
 * between the two must not get a different plan:
 *
 * - **A round hole no wider than the cutter cannot be machined at all.** It is
 *   not quietly skipped and it is not drilled: it is a refusal that names the
 *   diameters, because the fix is a smaller cutter, not a different toolpath.
 * - **A round hole wider than the cutter but narrower than two of it is bored
 *   helically.** Below 2× there is no room to spiral an offset pass, and
 *   contouring it would leave the cutter cutting on both sides at once. Holes
 *   of the same diameter share one row.
 * - **Everything else inside the part is an opening**, cut out with an inside
 *   contour so the waste drops free. A pocket — which keeps the slug — is the
 *   operator's choice, not the default.
 *
 * The outside profile comes last and carries tabs, so the part is still held
 * when the cut that frees it finishes.
 */
export function deriveOperations(
  outline: CamOutlineSectioned,
  setup: {
    toolDiameter: number;
    stockThickness: number;
    values: CamJobCuttingValues;
  },
): CamDerivation {
  const { toolDiameter, stockThickness, values } = setup;
  const region = outline.regions[0];
  if (!region) {
    return {
      operations: [],
      unmachinable: [],
      refusal:
        "this section has no closed area, so there is nothing to machine. Section the part at a Z where it has material.",
    };
  }

  const base: CamJobCuttingValues = {
    ...values,
    // A pass cannot step over further than the cutter's radius without moving
    // through metal no earlier pass reached.
    stepover: Math.min(values.stepover, 0.45 * toolDiameter),
  };
  const depth = stockThickness;

  const circles = outline.circles.filter((c) => c.region === 0);
  const circleByHole = new Map(circles.map((c) => [c.hole, c]));

  const unmachinable: CamUnmachinableHole[] = circles
    .filter((c) => c.diameter <= toolDiameter + EPS)
    .map((c) => ({ diameter: c.diameter, center: c.center }));

  const bored = new Set<number>();
  const groups = new Map<number, typeof circles>();
  for (const c of circles) {
    if (c.diameter <= toolDiameter + EPS) continue;
    if (c.diameter >= 2 * toolDiameter - EPS) continue;
    const key = boreGroupKey(c.diameter);
    const group = groups.get(key) ?? [];
    group.push(c);
    groups.set(key, group);
    bored.add(c.hole);
  }

  const operations: CamJobOperation[] = [];

  // Pilots first: a bore taken after the opening beside it has been cut is a
  // bore into a wall that is no longer there.
  for (const [key, group] of [...groups.entries()].sort((a, b) => a[0] - b[0])) {
    const diameter = group.reduce((s, c) => s + c.diameter, 0) / group.length;
    operations.push({
      ...base,
      id: `bore-${key}`,
      label:
        group.length > 1
          ? `Pilot ${diameterLabel(diameter)} × ${group.length}`
          : `Pilot ${diameterLabel(diameter)}`,
      kind: "helical_bore",
      source: {
        type: "bore",
        key,
        diameter,
        centres: group.map((c) => c.center),
      },
      enabled: true,
      depth,
    });
  }

  for (let hole = 0; hole < region.holes.length; hole++) {
    if (bored.has(hole)) continue;
    const circle = circleByHole.get(hole);
    if (circle && circle.diameter <= toolDiameter + EPS) continue;
    const contour = region.holes[hole] as Array<[number, number]>;
    operations.push({
      ...base,
      id: `opening-${hole}`,
      label: openingLabel(contour, circle?.diameter),
      kind: "contour_inside",
      source: { type: "opening", hole, contour },
      enabled: true,
      depth,
    });
  }

  operations.push({
    ...base,
    id: "outer",
    label: "Outside profile",
    kind: "contour_outside",
    source: { type: "outer", contour: region.outer as Array<[number, number]> },
    enabled: true,
    depth,
    tabs: 3,
    tab_width: 4,
    // A tab taller than half the stock is most of the part, not a tab.
    tab_height: Math.min(1, stockThickness / 2),
  });

  const refusal = unmachinable.length
    ? `${unmachinable.length} hole(s) (${unmachinable
        .map((h) => diameterLabel(h.diameter).slice(1))
        .join(", ")} mm) cannot be machined with the Ø${toolDiameter} mm cutter. Fit a smaller one, or drill them separately.`
    : null;

  return { operations, unmachinable, refusal };
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

/**
 * The job request these operations mean.
 *
 * A helical-bore row is one row in the UI and one kernel operation *per
 * centre* — the kernel bores one hole at a time, and the row exists because a
 * machinist thinks of "the Ø4 pilots" as one thing.
 */
export function buildJobRequest(state: {
  name?: string;
  stock: CamJobStock;
  machine: CamJobMachine;
  tool: CamJobTool;
  operations: CamJobOperation[];
  outline: CamOutlineSectioned | null;
  safeZ: number;
}): CamJobRequest {
  const { stock, machine, tool, safeZ } = state;
  const enabled = state.operations.filter((op) => op.enabled);
  const operations: CamOperationRequest[] = [];

  enabled.forEach((op, position) => {
    const common = {
      tool: tool.number,
      depth: op.depth,
      stepdown: op.stepdown,
      feed: op.feed,
      plunge: op.plunge,
      rpm: op.rpm,
      bottom_allowance: op.bottom_allowance,
      order: position,
    };
    if (op.source.type === "bore") {
      const many = op.source.centres.length > 1;
      op.source.centres.forEach(([x, y], i) => {
        operations.push({
          ...common,
          name: many ? `${op.label} · ${i + 1}` : op.label,
          kind: "helical_bore",
          x,
          y,
          diameter: op.source.type === "bore" ? op.source.diameter : 0,
          pitch: op.stepdown,
          // `through: false` is not the same as leaving it out: the kernel's
          // default is "stop at the floor", and saying so explicitly would
          // fight the bottom allowance.
          ...(op.bottom_allowance < 0 ? { through: true } : {}),
        });
      });
      return;
    }
    if (op.kind === "pocket") {
      operations.push({
        ...common,
        name: op.label,
        kind: "pocket",
        stepover: op.stepover,
        contour: op.source.contour,
      });
      return;
    }
    operations.push({
      ...common,
      name: op.label,
      kind: op.kind,
      contour: op.source.contour,
      direction: "climb",
      entry: "ramp",
      ramp_angle: 3,
      lead_in: true,
      thin_slot: { strategy: "refuse" },
      ...(op.tabs
        ? { tabs: op.tabs, tab_width: op.tab_width, tab_height: op.tab_height }
        : {}),
    });
  });

  const region = state.outline?.regions[0];
  return {
    name: state.name ?? "vcad job",
    stock: {
      thickness: stock.thickness,
      margin: stock.margin,
      ...(stock.spoilboard !== null ? { spoilboard: stock.spoilboard } : {}),
    },
    machine: {
      name: machine.name,
      spindle: machine.spindle,
      class: machine.class,
      max_feed: machine.max_feed,
      max_accel: machine.max_accel,
    },
    tools: [
      {
        number: tool.number,
        kind: tool.kind,
        diameter: tool.diameter,
        flutes: tool.flutes,
        flute_length: tool.flute_length,
        ...(tool.stickout !== undefined ? { stickout: tool.stickout } : {}),
        centre_cutting: tool.centre_cutting,
      },
    ],
    operations,
    options: {
      arc_fit: { tolerance: 0.005 },
      tool_change: { type: "manual_pause_reprobe" },
      spin_up_seconds: 3,
      park_z: safeZ,
      safe_z: safeZ,
      wcs: "G54",
      // Nothing to replay the job against is not a reason to skip the replay
      // silently — but it is a reason the kernel has to be told, or it derives
      // the part from the operations and checks the job against itself.
      verify: true,
      ...(region
        ? {
            part: {
              outer: region.outer as Array<[number, number]>,
              holes: region.holes as Array<Array<[number, number]>>,
            },
          }
        : {}),
    },
  };
}

/**
 * The identity of a job: two requests with the same key are the same job.
 *
 * This is how "the acknowledgements are stale" is answered without any
 * bookkeeping — an edit anywhere in the setup changes the key, and
 * {@link useCamJobStore}'s `acknowledgements` getter stops recognising the
 * ones given for the old one.
 */
export function jobKeyOf(request: CamJobRequest): string {
  const { name: _name, ...identity } = request;
  return JSON.stringify(identity);
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/** One thing wrong with the job, in a sentence an operator can act on. */
export interface CamFinding {
  /** Stable across rebuilds of the same job: the acknowledgement key. */
  id: string;
  text: string;
  /** Which operation row to flag, when the fault can be placed. */
  opIndex?: number;
  /** Where in the stock frame, when the oracle reported a place. */
  xy?: [number, number];
}

/** The inspector title of a check. `policy.blocked_by` names checks, and
 *  nobody standing at a machine wants to read `loose_pieces`. */
export const CHECK_TITLES: Record<string, string> = {
  gouge: "Cuts into the part",
  material_left: "Wall left standing",
  rapids: "Rapids below the stock top",
  depth: "Depth against the stock",
  tabs: "Holding tabs",
  envelope: "Travel and sweep",
  loose_pieces: "Pieces that come free",
  plunges: "Plunges into uncut metal",
};

/** The title of a check, falling back to its own name made readable. */
export function checkTitle(name: string): string {
  return CHECK_TITLES[name] ?? name.replace(/_/g, " ");
}

const mm = (v: number | undefined, places = 2): string =>
  v === undefined || !Number.isFinite(v) ? "—" : v.toFixed(places);

const upperFirst = (s: string): string =>
  s.length ? s.charAt(0).toUpperCase() + s.slice(1) : s;

/**
 * One failed check as one sentence: what happened, how far, and where.
 *
 * Built from the check's own worst violation, which the kernel puts first in
 * `examples`.
 */
export function explainCheck(
  check: CamCheckReport,
  result: CamJobResult,
): { text: string; xy?: [number, number] } {
  const worst = check.examples?.[0];
  const xy = worst && worst.xy?.length >= 2 ? worst.xy : undefined;
  const here = xy ? ` at X ${mm(xy[0], 1)}, Y ${mm(xy[1], 1)}` : "";
  const v = result.verification;
  switch (check.name) {
    case "gouge":
      return { text: `This job cuts ${mm(check.worst)} mm into the part${here}.`, xy };
    case "material_left": {
      const m = v?.material_left;
      return {
        text: `${mm(m?.unswept_area, 1)} mm² of wall is left standing, up to ${mm(
          m?.max_standoff,
        )} mm proud${here}.`,
        xy,
      };
    }
    case "depth": {
      const under = v?.depth?.remaining_under_part;
      if (under !== undefined && under < -1e-6) {
        return {
          text: `The cut goes ${mm(-under, 3)} mm past the stock underside${here}. Declare a thicker spoilboard, or take less off the bottom.`,
          xy,
        };
      }
      return {
        text: `The cut does not reach the floor the stock and allowance imply (Z ${mm(
          v?.depth?.floor_z,
          3,
        )})${here}.`,
        xy,
      };
    }
    case "loose_pieces":
      return {
        text: `${mm(check.worst, 1)} mm² comes free${here} — no tab and no skin holds it.`,
        xy,
      };
    case "plunges":
      return { text: `This job plunges straight down into uncut metal${here}.`, xy };
    default:
      return {
        text: upperFirst(`${String(worst?.what ?? check.note)}${here}.`),
        xy,
      };
  }
}

/**
 * The blockers and warnings of a job answer.
 *
 * Note what decides which list a check lands in: membership in
 * `policy.blocked_by` / `policy.warnings`, **not** `check.severity`. The
 * policy is the oracle's severities *after* the caller's overrides, so
 * reading severity here would quietly ignore `verify_policy`.
 */
export function findingsOf(result: CamJobResult): {
  blockers: CamFinding[];
  warnings: CamFinding[];
} {
  const blockers: CamFinding[] = [];
  const warnings: CamFinding[] = [];

  // A refusal that never reached the oracle — a bad number, a tool that cannot
  // do the cut — arrives as a sentence with no check behind it.
  if (result.blocked && typeof result.error === "string") {
    blockers.push({ id: "request", text: result.error });
  }
  for (const c of result.tool_checks ?? []) {
    const finding: CamFinding = {
      id: `tool-${c.op_index}-${c.kind}`,
      text: c.op ? `${c.op}: ${c.message}` : upperFirst(c.message),
      opIndex: c.op_index,
    };
    (c.severity === "error" ? blockers : warnings).push(finding);
  }

  const byName = new Map(camChecks(result.verification).map((c) => [c.name, c]));
  for (const name of result.policy?.blocked_by ?? []) {
    const check = byName.get(name);
    if (!check) continue;
    const { text, xy } = explainCheck(check, result);
    blockers.push({ id: name, text, xy });
  }
  for (const name of result.policy?.warnings ?? []) {
    const check = byName.get(name);
    if (!check) continue;
    const { text, xy } = explainCheck(check, result);
    warnings.push({ id: name, text, xy });
  }
  if (result.policy && !result.policy.verified) {
    warnings.push({
      id: "unverified",
      text: "Nothing replayed this job against the part it is meant to make. It has not been checked.",
    });
  }
  return { blockers, warnings };
}

/** The headline over the checks, and how loud it is. */
export function verdictOf(result: CamJobResult | null): {
  tone: "refused" | "clean" | "unverified" | "none";
  text: string;
} {
  if (!result) {
    return { tone: "none", text: "Build the job to have it checked against the part." };
  }
  if (result.blocked) {
    return { tone: "refused", text: "Refused — this job will not run" };
  }
  if (result.policy?.verified) {
    return { tone: "clean", text: "Replayed against the part and clean" };
  }
  return { tone: "unverified", text: "Not verified" };
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/** Defaults that match the machine the first real vcad job was cut on. */
export const DEFAULT_MACHINE: CamJobMachine = {
  name: "Anolex Ultra 2",
  spindle: "dial",
  class: "hobby",
  max_feed: 3000,
  max_accel: 300,
};

/** A Ø3.175 two-flute flat end mill: the cutter most hobby routers ship with. */
export const DEFAULT_TOOL: CamJobTool = {
  number: 1,
  kind: "flat_end_mill",
  diameter: 3.175,
  flutes: 2,
  flute_length: 12,
  stickout: 20,
  centre_cutting: true,
};

export const DEFAULT_VALUES: CamJobCuttingValues = {
  stepdown: 0.5,
  stepover: 1.5,
  feed: 400,
  plunge: 100,
  rpm: 10000,
  bottom_allowance: 0,
};

export const DEFAULT_STOCK: CamJobStock = {
  thickness: 6,
  margin: 8,
  spoilboard: null,
};

interface CamJobState {
  /** Which scene part was sectioned, in document order. */
  partIndex: number;
  outline: CamOutlineSectioned | null;
  /** A section that did not close, with the gaps. Shown, never healed. */
  torn: Extract<CamOutlineResult, { torn: true }> | null;
  outlineError: string | null;
  sectioning: boolean;

  stock: CamJobStock;
  machine: CamJobMachine;
  tool: CamJobTool;
  values: CamJobCuttingValues;
  material: string;
  safeZ: number;

  operations: CamJobOperation[];
  unmachinable: CamUnmachinableHole[];
  derivationRefusal: string | null;

  recommendation: CamRecommendResult | null;
  recommendError: string | null;

  building: boolean;
  result: CamJobResult | null;
  buildError: string | null;
  /** The job the current `result` was built for. */
  builtKey: string | null;

  acknowledgedKey: string | null;
  acknowledgedIds: string[];

  /** Which operation row the viewport preview should highlight. */
  highlightedOpIndex: number | null;

  setPartIndex: (index: number) => void;
  setStock: (patch: Partial<CamJobStock>) => void;
  setMachine: (patch: Partial<CamJobMachine>) => void;
  setTool: (patch: Partial<CamJobTool>) => void;
  setValues: (patch: Partial<CamJobCuttingValues>) => void;
  setMaterial: (id: string) => void;
  updateOperation: (id: string, patch: Partial<CamJobOperation>) => void;
  moveOperation: (id: string, direction: "up" | "down") => void;
  setHighlightedOp: (index: number | null) => void;

  sectionPart: (host: CamHost, doc: Document, partIndex?: number) => void;
  recommendFeeds: (host: CamHost) => void;
  build: (host: CamHost) => void;

  acknowledge: (id: string, on: boolean) => void;
  reset: () => void;
}

/** Anything that changes the job invalidates the result that was built for it. */
const STALE = {
  result: null,
  builtKey: null,
  buildError: null,
  highlightedOpIndex: null,
} as const;

export const useCamJobStore = create<CamJobState>((set, get) => ({
  partIndex: 0,
  outline: null,
  torn: null,
  outlineError: null,
  sectioning: false,

  stock: DEFAULT_STOCK,
  machine: DEFAULT_MACHINE,
  tool: DEFAULT_TOOL,
  values: DEFAULT_VALUES,
  material: "aluminium-6061-t6",
  safeZ: 5,

  operations: [],
  unmachinable: [],
  derivationRefusal: null,

  recommendation: null,
  recommendError: null,

  building: false,
  result: null,
  buildError: null,
  builtKey: null,

  acknowledgedKey: null,
  acknowledgedIds: [],
  highlightedOpIndex: null,

  setPartIndex: (partIndex) => set({ partIndex, ...STALE }),
  setStock: (patch) =>
    set((s) => {
      const stock = { ...s.stock, ...patch };
      return { stock, ...rederive(s, { stock }), ...STALE };
    }),
  setMachine: (patch) => set((s) => ({ machine: { ...s.machine, ...patch }, ...STALE })),
  setTool: (patch) =>
    set((s) => {
      const tool = { ...s.tool, ...patch };
      return { tool, ...rederive(s, { tool }), ...STALE };
    }),
  setValues: (patch) =>
    set((s) => {
      const values = { ...s.values, ...patch };
      return { values, ...rederive(s, { values }), ...STALE };
    }),
  setMaterial: (material) => set({ material }),

  updateOperation: (id, patch) =>
    set((s) => ({
      operations: s.operations.map((op) => (op.id === id ? { ...op, ...patch } : op)),
      ...STALE,
    })),

  moveOperation: (id, direction) =>
    set((s) => {
      const i = s.operations.findIndex((op) => op.id === id);
      if (i === -1) return s;
      const j = direction === "up" ? i - 1 : i + 1;
      if (j < 0 || j >= s.operations.length) return s;
      const operations = [...s.operations];
      const a = operations[i]!;
      const b = operations[j]!;
      operations[i] = b;
      operations[j] = a;
      return { operations, ...STALE };
    }),

  setHighlightedOp: (highlightedOpIndex) => set({ highlightedOpIndex }),

  sectionPart: (host, doc, partIndex) => {
    const index = partIndex ?? get().partIndex;
    set({ sectioning: true, outlineError: null, torn: null, ...STALE });
    try {
      // Auto Z: the mid-height of the part's own bounds, which is the section
      // a constant-section part wants and the one an operator would pick.
      const outline = camOutlineFromDocument(host, doc, index, "auto");
      if (outline.torn) {
        set({
          sectioning: false,
          partIndex: index,
          outline: null,
          torn: outline,
          operations: [],
          unmachinable: [],
          derivationRefusal: null,
        });
        return;
      }
      const s = get();
      const thickness = outline.suggested_stock_thickness;
      // The part's own height is what the stock has to be, unless the operator
      // has already said otherwise by typing a thicker blank.
      const stock =
        Number.isFinite(thickness) && thickness > 0 && thickness > s.stock.thickness
          ? { ...s.stock, thickness }
          : s.stock;
      const derived = deriveOperations(outline, {
        toolDiameter: s.tool.diameter,
        stockThickness: stock.thickness,
        values: s.values,
      });
      set({
        sectioning: false,
        partIndex: index,
        outline,
        stock,
        operations: derived.operations,
        unmachinable: derived.unmachinable,
        derivationRefusal: derived.refusal,
      });
    } catch (e) {
      set({
        sectioning: false,
        outline: null,
        operations: [],
        unmachinable: [],
        derivationRefusal: null,
        outlineError: e instanceof CamError ? e.message : String(e),
      });
    }
  },

  recommendFeeds: (host) => {
    const s = get();
    try {
      const recommendation = camRecommend(host, {
        material: s.material,
        // A profile that cuts right through sheet stock has material on both
        // sides of the cutter the whole way round: that is a slot, whatever
        // the operation is called.
        op: "slot",
        tool: {
          diameter: s.tool.diameter,
          flutes: s.tool.flutes,
          kind: s.tool.kind,
          flute_length: s.tool.flute_length,
        },
        machine: {
          class: s.machine.class,
          spindle: s.machine.spindle,
          max_feed: s.machine.max_feed,
        },
      });
      const r = recommendation.recommendation;
      const values: CamJobCuttingValues = {
        ...s.values,
        feed: r.feed_mm_min,
        plunge: r.plunge_mm_min,
        rpm: r.rpm,
        stepdown: r.stepdown_mm,
        stepover: r.stepover_mm > 0 ? r.stepover_mm : s.values.stepover,
      };
      set({
        recommendation,
        recommendError: null,
        values,
        ...rederive(s, { values }),
        ...STALE,
      });
    } catch (e) {
      set({
        recommendation: null,
        recommendError: e instanceof CamError ? e.message : String(e),
      });
    }
  },

  build: (host) => {
    const s = get();
    if (!s.operations.some((op) => op.enabled)) {
      set({ buildError: "Add an operation before building the job.", result: null });
      return;
    }
    const request = buildJobRequest({
      name: "vcad job",
      stock: s.stock,
      machine: s.machine,
      tool: s.tool,
      operations: s.operations,
      outline: s.outline,
      safeZ: s.safeZ,
    });
    set({ building: true, buildError: null });
    try {
      const result = camJob(host, request);
      set({ building: false, result, builtKey: jobKeyOf(request) });
    } catch (e) {
      set({
        building: false,
        result: null,
        builtKey: null,
        buildError: e instanceof CamError ? e.message : String(e),
      });
    }
  },

  acknowledge: (id, on) =>
    set((s) => {
      const fresh = s.acknowledgedKey === s.builtKey;
      const ids = new Set(fresh ? s.acknowledgedIds : []);
      if (on) ids.add(id);
      else ids.delete(id);
      return { acknowledgedKey: s.builtKey, acknowledgedIds: [...ids] };
    }),

  reset: () =>
    set({
      outline: null,
      torn: null,
      outlineError: null,
      operations: [],
      unmachinable: [],
      derivationRefusal: null,
      recommendation: null,
      recommendError: null,
      result: null,
      builtKey: null,
      buildError: null,
      acknowledgedKey: null,
      acknowledgedIds: [],
      highlightedOpIndex: null,
    }),
}));

/** Re-derive the operations after a change that alters what they should be. */
function rederive(
  s: Pick<CamJobState, "outline" | "tool" | "stock" | "values">,
  patch: Partial<Pick<CamJobState, "tool" | "stock" | "values">>,
): Partial<CamJobState> {
  const outline = s.outline;
  if (!outline) return {};
  const tool = patch.tool ?? s.tool;
  const stock = patch.stock ?? s.stock;
  const values = patch.values ?? s.values;
  const derived = deriveOperations(outline, {
    toolDiameter: tool.diameter,
    stockThickness: stock.thickness,
    values,
  });
  return {
    operations: derived.operations,
    unmachinable: derived.unmachinable,
    derivationRefusal: derived.refusal,
  };
}

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

/**
 * The acknowledgements that are still good.
 *
 * An edit rebuilds the job under a new key, and the ones given for the old job
 * stop counting — which is the point: a warning about a slug that drops free
 * was read about *that* job.
 *
 * Takes the three fields it reads rather than the whole state, because a
 * component has to be able to select those three and memoise on them: used
 * directly as a zustand selector this builds a fresh `Set` every render, which
 * never compares equal and re-renders until React gives up.
 */
export function liveAcknowledgements(
  s: Pick<CamJobState, "acknowledgedKey" | "acknowledgedIds" | "builtKey">,
): Set<string> {
  return s.acknowledgedKey === s.builtKey && s.builtKey !== null
    ? new Set(s.acknowledgedIds)
    : new Set();
}

/** Warnings the operator has not read yet. */
export function pendingWarnings(s: CamJobState): CamFinding[] {
  if (!s.result) return [];
  const live = liveAcknowledgements(s);
  return findingsOf(s.result).warnings.filter((w) => !live.has(w.id));
}

/**
 * Whether the G-code may leave the app.
 *
 * Three conditions, and all three are real: there is a program, the oracle did
 * not refuse it, and every warning has been read. The first is structural —
 * a refused job has no `gcode` key — and the other two are this panel's.
 */
export function canExport(s: CamJobState): boolean {
  const result = s.result;
  if (!result) return false;
  if (result.blocked) return false;
  if (typeof result.gcode !== "string" || result.gcode.length === 0) return false;
  return pendingWarnings(s).length === 0;
}

/** Why the export button is off, in a sentence, or `null` when it is on. */
export function exportBlocker(s: CamJobState): string | null {
  if (s.building) return "Wait for the job to be built and checked.";
  const result = s.result;
  if (!result) return "Build the job before exporting it.";
  if (result.blocked) {
    const { blockers } = findingsOf(result);
    const first = blockers[0];
    if (!first) return "This job was refused by its own verification.";
    return blockers.length > 1
      ? `${first.text} (${blockers.length - 1} more)`
      : first.text;
  }
  if (typeof result.gcode !== "string" || result.gcode.length === 0) {
    return "This job produced no G-code.";
  }
  const pending = pendingWarnings(s);
  const first = pending[0];
  if (!first) return null;
  return pending.length === 1
    ? `Acknowledge: ${first.text}`
    : `Acknowledge ${pending.length} warnings, starting with: ${first.text}`;
}
