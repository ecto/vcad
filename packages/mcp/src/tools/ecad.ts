/**
 * ECAD (Electronics CAD) MCP tools for PCB design.
 *
 * Tools for creating schematics, placing components, routing nets,
 * running DRC/ERC, exporting Gerber files, and calculating impedance.
 */

import type {
  Document,
  SchematicSheet,
  SchematicComponent,
  SchematicWire,
  SchematicLabel,
  SchematicPin,
  Pcb,
  BoardOutline,
  LayerStackup,
  StackupLayer,
  Net,
  NetTie,
  DesignRules,
  NetClassRules,
  Footprint,
  Pad,
  PadType,
  PcbLayer,
  Trace,
  TraceArc,
  Via,
  Zone,
  Vec2,
  Vec3,
  CsgOp,
  SketchSegment2D,
  Receipt,
  DesignReceipt,
  ReceiptStatus,
} from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { RECEIPT_SCHEMA, summarize, unifiedFromPcbReceipt } from "../receipt-unified.js";
import {
  clearanceReceiptClaims,
  hasClearanceClaims,
  verifyClearanceClaims,
} from "./clearance.js";
import {
  constraintReceiptClaims,
  constraintStaleWarning,
  hasConstraintClaims,
  pruneOutlineConstraints,
  verifyConstraintClaims,
} from "./constraint-claims.js";
import { getNodePcb, getPcbNodeIds, buildEntry, agentView, diffViolations } from "@vcad/core";
import {
  computeRatsnest,
  componentMeshes,
  exportFabFiles,
  pcbPreviewMeshes,
  exportKicadPcb,
  exportKicadProject,
  exportKicadSch,
  tryExportFabFiles,
  tryRunDrc,
  runFabPrep as kernelRunFabPrep,
  tryPcbPreviewMeshes,
  resolveFootprint,
  generateNetlist,
  isEcadAvailable,
  routeAll,
  routeDiffPair as kernelRouteDiffPair,
  matchTraceLengths as kernelMatchTraceLengths,
  critiqueRoute as kernelCritiqueRoute,
  runDrc as kernelRunDrc,
  runDrcInRegion as kernelRunDrcInRegion,
  runErc as kernelRunErc,
  checkErc as kernelCheckErc,
  evaluateMotor,
  airgapFluxDensity,
  airgapSolve,
  type AirGapSolutionResult,
  resolvePart as kernelResolvePart,
  resolvePartDef as kernelResolvePartDef,
  searchEcadParts as kernelSearchPartsEcad,
  findAlternatives as kernelFindAlternatives,
  verifySubstitution as kernelVerifySubstitution,
  buildReceipt as kernelBuildReceipt,
  verifyReceipt as kernelVerifyReceipt,
  netContinuity as kernelNetContinuity,
} from "@vcad/engine";
import type { Engine, NetlistResult, TriangleMesh, NetContinuity, FabPrepReport } from "@vcad/engine";
import {
  registerSession,
  getSession,
  undoLastSnapshot,
  historyDepth,
  documents,
  hydrateSession,
  persistSession,
  recordHistorySnapshot,
  resolveDocInput,
  type DocInputCtx,
} from "./session.js";
import { buildKernelEventPayload } from "./kernel-event.js";
import { emClaim } from "./em-claims.js";
import type { NextAction } from "./next-actions.js";
import { computeEnclosureFitForBoard } from "./enclosure.js";
import { validatePcb, pcbValidationError } from "./pcb-validate.js";
import { PCB_LAYERS } from "./pcb-layers.js";
import { sizePdnExact, ecadDiffEngineAvailable } from "../wasm/ecad-diff.js";
import { bundleBytes, storeArtifact } from "./artifact-store.js";
import { maxInlineArtifactBytes, maxInlineExportBytes } from "./remote.js";
import type { FabFile } from "@vcad/engine";
import { behavior, type ToolContext, type ToolDef } from "./tool-def.js";
import type { ToolResult } from "./tool-result.js";

/** Get PCB data from a document — checks PcbBoard nodes first, falls back to legacy doc.pcb */
function getDocPcb(doc: Document): Pcb | null {
  const nodeIds = getPcbNodeIds(doc);
  if (nodeIds.length > 0) return getNodePcb(doc, nodeIds[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
}

/**
 * Design-rule args supplied by set_design_rules *before* the board existed.
 * Stashed on the in-memory document and replayed when place_components builds
 * the board, so an agent can set rules in any order. An untyped extension (like
 * the legacy `pcb?` field above) — it round-trips through JSON persistence but
 * isn't part of the IR schema.
 */
type DocWithPendingRules = Document & {
  __pendingDesignRules?: Record<string, unknown>;
};

/** The board's starting design rules — JLCPCB-ish 2-layer defaults. */
function defaultDesignRules(): DesignRules {
  return {
    defaultRules: {
      name: "Default",
      traceWidth: 0.25,
      clearance: 0.2,
      viaDiameter: 0.8,
      viaDrill: 0.4,
    },
    edgeClearance: 0.5,
    holeToHole: 0.5,
    minAnnularRing: 0.15,
    minDrill: 0.2,
  };
}

/** Strip the document handle from buffered args so we stash only the rules. */
function stripDocArgs(args: Record<string, unknown>): Record<string, unknown> {
  const rest: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    if (k === "document" || k === "document_id") continue;
    rest[k] = v;
  }
  return rest;
}

/**
 * Apply set_design_rules-style args to a DesignRules object in place. Shared by
 * set_design_rules (which writes pcb.rules directly) and place_components (which
 * replays rules buffered before the board existed). Returns what changed plus
 * any warnings; an `error` means the args were malformed (e.g. a class with no
 * nets) and nothing meaningful should be persisted.
 */
function applyDesignRuleArgs(
  rules: DesignRules,
  knownNets: Set<string>,
  args: Record<string, unknown>,
  opts: { checkNets?: boolean } = {},
): { touched: boolean; warnings: string[]; classNames?: string[]; error?: string } {
  // When validating a call made before the board exists (the buffered probe),
  // there's no netlist yet — skip the "net not on the board" warning so it
  // doesn't fire spuriously on every buffered class.
  const checkNets = opts.checkNets ?? true;
  const dr = rules.defaultRules;
  const warnings: string[] = [];
  let touched = false;

  const num = (k: string) => (typeof args[k] === "number" ? (args[k] as number) : undefined);
  const requirePos = (k: string, v: number | undefined): boolean => {
    if (v === undefined) return false;
    if (!(v > 0)) {
      warnings.push(`${k} must be > 0 — ignored`);
      return false;
    }
    return true;
  };

  const clearance = num("clearance");
  if (requirePos("clearance", clearance)) { dr.clearance = clearance!; touched = true; }
  const trackWidth = num("track_width");
  if (requirePos("track_width", trackWidth)) { dr.traceWidth = trackWidth!; touched = true; }
  const viaDiameter = num("via_diameter");
  if (requirePos("via_diameter", viaDiameter)) { dr.viaDiameter = viaDiameter!; touched = true; }
  const viaDrill = num("via_drill");
  if (requirePos("via_drill", viaDrill)) { dr.viaDrill = viaDrill!; touched = true; }
  const dpg = num("diff_pair_gap");
  if (requirePos("diff_pair_gap", dpg)) { dr.diffPairGap = dpg!; touched = true; }
  const dpw = num("diff_pair_width");
  if (requirePos("diff_pair_width", dpw)) { dr.diffPairWidth = dpw!; touched = true; }

  const edgeClr = num("edge_clearance");
  if (requirePos("edge_clearance", edgeClr)) { rules.edgeClearance = edgeClr!; touched = true; }
  const h2h = num("hole_to_hole");
  if (requirePos("hole_to_hole", h2h)) { rules.holeToHole = h2h!; touched = true; }
  const minAnnular = num("min_annular_ring");
  if (requirePos("min_annular_ring", minAnnular)) { rules.minAnnularRing = minAnnular!; touched = true; }
  const minDrill = num("min_drill");
  if (requirePos("min_drill", minDrill)) { rules.minDrill = minDrill!; touched = true; }

  let classNames: string[] | undefined;
  if (Array.isArray(args.classes)) {
    const classesIn = args.classes as Array<Record<string, unknown>>;
    const classRules: NetClassRules[] = [];
    const assignments: Record<string, string[]> = {};
    for (const c of classesIn) {
      const name = String(c.name ?? "");
      const nets = Array.isArray(c.nets) ? (c.nets as unknown[]).map(String) : [];
      if (!name) return { touched, warnings, error: "each class needs a `name`" };
      if (nets.length === 0)
        return { touched, warnings, error: `class "${name}" needs a non-empty nets array` };
      const unknown = checkNets ? nets.filter((id) => !knownNets.has(id)) : [];
      if (unknown.length > 0) {
        warnings.push(`class "${name}" references nets not on the board: ${unknown.join(", ")}`);
      }
      classRules.push({
        name,
        traceWidth: typeof c.track_width === "number" ? (c.track_width as number) : dr.traceWidth,
        clearance: typeof c.clearance === "number" ? (c.clearance as number) : dr.clearance,
        viaDiameter: typeof c.via_diameter === "number" ? (c.via_diameter as number) : dr.viaDiameter,
        viaDrill: typeof c.via_drill === "number" ? (c.via_drill as number) : dr.viaDrill,
        ...(typeof c.diff_pair_gap === "number" ? { diffPairGap: c.diff_pair_gap as number } : {}),
        ...(typeof c.diff_pair_width === "number" ? { diffPairWidth: c.diff_pair_width as number } : {}),
      });
      assignments[name] = nets;
    }
    rules.classRules = classRules;
    rules.netClassAssignments = assignments;
    classNames = classRules.map((c) => c.name);
    touched = true;
  }

  return { touched, warnings, classNames };
}

/** Standard `{ content, isError }` failure result for ECAD tools. */
function ecadError(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

/**
 * Tool result for "the kernel could not verify this input" — a state that is
 * distinct from a clean pass. The board/schematic was never actually checked
 * (e.g. a malformed layer name like `In1.Cu` that should be `In1Cu`), so it must
 * NOT read as success: an agent that sees "0 violations" here could ship a board
 * that was never verified. Reads as `verifiable: false`, carries the kernel
 * `reason` and the `offending_field` when known, and a `next_actions` hint.
 */
function ecadUnverifiable(
  check: string,
  outcome: { reason: string; offending_field?: string },
) {
  const field = outcome.offending_field;
  const next_actions = field
    ? [
        `The kernel does not recognize '${field}'. Fix it and re-run ${check}. ` +
          `PCB layer names are un-dotted — e.g. 'In1Cu', not 'In1.Cu'.`,
      ]
    : [
        `The kernel could not process the input — it may be malformed, or the ` +
          `verification engine may be unavailable. Inspect 'reason', fix it, and re-run ${check}.`,
      ];
  const payload = {
    status: "errored" as const,
    verifiable: false as const,
    check,
    reason: outcome.reason,
    ...(field ? { offending_field: field } : {}),
    next_actions,
  };
  return {
    content: [
      {
        type: "text" as const,
        text:
          `${check} could not run — this board is UNVERIFIABLE (NOT clean): ${outcome.reason}` +
          (field ? ` (offending field: '${field}')` : ""),
      },
    ],
    structuredContent: payload,
    isError: true as const,
  };
}

// ============================================================================
// PCB layer name validation — the single gate guarding every write boundary
// ============================================================================

// The legal PCB layer names live in ./pcb-layers.ts (imported as PCB_LAYERS) —
// the single runtime copy, shared with pcb-validate.ts, mirroring the Rust
// `PcbLayer` enum (crates/vcad-ir/src/ecad.rs). The old scattered `/Cu$/` regex
// checks let malformed dotted KiCad names (`In1.Cu`) slip through, which then
// corrupted documents and broke render_pcb / export_gerber.

/** Fast membership test for any legal layer name. */
const PCB_LAYER_SET: ReadonlySet<string> = new Set(PCB_LAYERS);

/** Copper layers, in board order — the subset traces/vias/zones/coils sit on. */
const COPPER_LAYERS: readonly PcbLayer[] = [
  "FCu", "BCu", "In1Cu", "In2Cu", "In3Cu", "In4Cu", "In5Cu", "In6Cu",
];

/** Fast membership test for copper layers. */
const COPPER_LAYER_SET: ReadonlySet<string> = new Set(COPPER_LAYERS);

/** True when `layer` is a copper layer (FCu, BCu, In1Cu …). */
function isCopperLayer(layer: string): boolean {
  return COPPER_LAYER_SET.has(layer);
}

/**
 * Suggest the canonical layer for a malformed name: dotted KiCad form
 * (`In1.Cu`, `F.Cu`, `Edge.Cuts`) → its serde variant, plus a case-insensitive
 * fallback. Returns undefined when nothing close matches.
 */
function suggestLayer(name: string): PcbLayer | undefined {
  const dedotted = name.replace(/\./g, "");
  if (PCB_LAYER_SET.has(dedotted)) return dedotted as PcbLayer;
  const lower = name.toLowerCase();
  const lowerDedot = dedotted.toLowerCase();
  for (const l of PCB_LAYERS) {
    const ll = l.toLowerCase();
    if (ll === lower || ll === lowerDedot) return l;
  }
  return undefined;
}

/** Successful layer validation carries the canonical `PcbLayer`. */
interface LayerOk {
  layer: PcbLayer;
}
/** Failed layer validation carries a ready-to-return error message. */
interface LayerErr {
  error: string;
}

/**
 * The single gate for a layer name at a write boundary. Accepts only the exact
 * serde variant (`In1Cu`); a dotted KiCad name (`In1.Cu`) or any unknown string
 * is rejected with a helpful message that names the closest legal value and
 * lists them all. This is the durable fix for the malformed names that used to
 * slip past the `/Cu$/` checks and corrupt documents (a `serde(alias=…)` on the
 * Rust enum gives round-trip tolerance, but rejecting at the boundary is what
 * keeps new corruption out).
 */
function validateLayer(raw: unknown): LayerOk | LayerErr {
  const name = String(raw ?? "").trim();
  if (!name) return { error: `layer is required; legal: ${PCB_LAYERS.join(", ")}` };
  if (PCB_LAYER_SET.has(name)) return { layer: name as PcbLayer };
  const suggestion = suggestLayer(name);
  const hint = suggestion ? ` did you mean '${suggestion}'?` : "";
  return { error: `layer '${name}' is not valid;${hint} Legal: ${PCB_LAYERS.join(", ")}` };
}

/**
 * Like {@link validateLayer} but also requires a copper layer — for traces,
 * vias, coils, and pours, which can only sit on copper. Rejects valid
 * non-copper layers (e.g. `EdgeCuts`) with a copper-specific message.
 */
function validateCopperLayer(raw: unknown): LayerOk | LayerErr {
  const res = validateLayer(raw);
  if ("error" in res) return res;
  if (!isCopperLayer(res.layer)) {
    return {
      error: `layer '${res.layer}' is not a copper layer; copper layers: ${COPPER_LAYERS.join(", ")}`,
    };
  }
  return res;
}

// ============================================================================
// Document resolution — session-based (document_id) with inline fallback
// ============================================================================
//
// `resolveDocInput` now lives in session.ts and is shared with the core
// read-only tools (inspect_cad / render_view / export_cad / dfm_check). ECAD
// tools echo the full mutated document back for the inline path, so they keep
// their own result-payload helper below.

/**
 * The document part of a mutating tool's response. Session docs are mutated
 * server-side, so only the id is echoed; inline docs get the full mutated
 * document back (the caller has no other way to retrieve it).
 */
function docResultPayload(ctx: DocInputCtx): Record<string, unknown> {
  return ctx.documentId
    ? { document_id: ctx.documentId }
    : { document: ctx.doc };
}

// ============================================================================
// Netlist derivation — wires/labels (kernel) merged with explicit nets
// ============================================================================

/** Position coincidence tolerance, mirrors the kernel netlist extractor. */
const POSITION_TOLERANCE = 0.01;

/** Pin position on the sheet (component rotation applied). */
function pinWorldPosition(comp: SchematicComponent, pin: SchematicPin): Vec2 {
  const rot = (((comp.rotation as number) || 0) * Math.PI) / 180;
  const cos = Math.cos(rot);
  const sin = Math.sin(rot);
  return {
    x: comp.position.x + pin.position.x * cos - pin.position.y * sin,
    y: comp.position.y + pin.position.x * sin + pin.position.y * cos,
  };
}

/** Separator for pin map keys — NUL cannot appear in refs or pin numbers,
 *  so keys never collide (a space could: "R1 2" + "3" vs "R1" + "2 3"). */
const PIN_SEP = String.fromCharCode(0);
const pinKey = (ref: string, pinNumber: string) => `${ref}${PIN_SEP}${pinNumber}`;

/**
 * Validate an explicit netlist (`net name → ["R1.2", ...]`) against the
 * sheet's components. Returns normalized entries or throws with every
 * unknown ref/pin listed.
 */
function validateExplicitNets(
  sheet: SchematicSheet,
  nets: Record<string, string[]>,
): Array<{ name: string; pins: Array<{ ref: string; pin: string }> }> {
  const byRef = new Map(sheet.components.map((c) => [c.ref, c]));
  const problems: string[] = [];
  const out: Array<{ name: string; pins: Array<{ ref: string; pin: string }> }> = [];
  for (const [name, pinRefs] of Object.entries(nets)) {
    const pins: Array<{ ref: string; pin: string }> = [];
    for (const pinRef of pinRefs) {
      const dot = pinRef.indexOf(".");
      if (dot <= 0 || dot === pinRef.length - 1) {
        problems.push(`net "${name}": "${pinRef}" is not of the form "REF.PIN" (e.g. "R1.2")`);
        continue;
      }
      const ref = pinRef.slice(0, dot);
      const pin = pinRef.slice(dot + 1);
      const comp = byRef.get(ref);
      if (!comp) {
        problems.push(
          `net "${name}": unknown component "${ref}" (have: ${[...byRef.keys()].join(", ")})`,
        );
        continue;
      }
      if (!comp.pins.some((p) => p.number === pin)) {
        problems.push(
          `net "${name}": component "${ref}" has no pin "${pin}" (pins: ${comp.pins.map((p) => p.number).join(", ")})`,
        );
        continue;
      }
      pins.push({ ref, pin });
    }
    out.push({ name, pins });
  }
  if (problems.length > 0) {
    throw new Error(`Invalid nets:\n- ${problems.join("\n- ")}`);
  }
  return out;
}

/** Result of merging wire/label connectivity with the explicit netlist. */
interface DerivedNets {
  /** pinKey(ref, pin) → net name */
  netByPin: Map<string, string>;
  /** net name → pin refs ("R1.2"), for reporting back to the agent. */
  nets: Map<string, string[]>;
  warnings: string[];
}

/** One source group of electrically-connected pins. */
interface NetGroup {
  name?: string;
  /** True when the name itself denotes a single net wherever it appears
   *  (explicit nets, non-Local labels) — such groups merge by name. Local
   *  labels and auto NET-xxx names are scoped to their own group. */
  mergeByName: boolean;
  explicit: boolean;
  pins: Array<{ ref: string; pin: string }>;
}

/**
 * Derive per-pin net assignments for a sheet. Three sources, merged with
 * union-find:
 *  1. kernel netlist from wires/junctions/labels (coordinate coincidence),
 *     with same-named non-Local labels bridged first so a label names a net
 *     wherever it appears (KiCad global-label semantics);
 *  2. the sheet's explicit `nets` map (net name → pin refs);
 *  3. name precedence: explicit > label-derived > auto NET-xxx. Disjoint
 *     nets that would collide on a non-mergeable name are renamed apart.
 */
async function deriveNets(sheet: SchematicSheet): Promise<DerivedNets> {
  const warnings: string[] = [];
  const groups: NetGroup[] = [];

  // Names that mean "one net wherever this name appears".
  const globalNames = new Set<string>();
  for (const label of sheet.labels) {
    if (label.scope !== "Local") globalNames.add(label.name);
  }
  for (const name of Object.keys(sheet.nets ?? {})) globalNames.add(name);

  if (sheet.wires.length > 0 || sheet.labels.length > 0) {
    // Bridge same-named global labels with synthetic wires so the kernel's
    // position-based union-find merges them into one net.
    const bridged: SchematicSheet = { ...sheet, wires: [...sheet.wires] };
    const labelPositions = new Map<string, Vec2[]>();
    for (const label of sheet.labels) {
      if (label.scope === "Local") continue;
      const arr = labelPositions.get(label.name) ?? [];
      arr.push(label.position);
      labelPositions.set(label.name, arr);
    }
    for (const positions of labelPositions.values()) {
      for (let i = 0; i + 1 < positions.length; i++) {
        bridged.wires.push({ start: positions[i], end: positions[i + 1] });
      }
    }
    const netlist = await generateNetlist(bridged);
    for (const net of netlist.nets) {
      if (net.connections.length === 0) continue;
      // Anonymous single-pin groups are just unconnected pins — skip. A
      // label-named single-pin group survives (the name was intentional).
      if (net.connections.length < 2 && /^NET-\d+$/.test(net.name)) continue;
      groups.push({
        name: net.name,
        mergeByName: globalNames.has(net.name),
        explicit: false,
        pins: net.connections.map((c) => ({ ref: c.component_ref, pin: c.pin_number })),
      });
    }
  }

  if (sheet.nets && Object.keys(sheet.nets).length > 0) {
    for (const entry of validateExplicitNets(sheet, sheet.nets)) {
      groups.push({
        name: entry.name,
        mergeByName: true,
        explicit: true,
        pins: entry.pins,
      });
    }
  }

  // Union-find over pins: groups sharing a pin merge into one net.
  const parent = new Map<string, string>();
  const pinByKey = new Map<string, { ref: string; pin: string }>();
  const find = (k: string): string => {
    let root = k;
    while (parent.get(root) !== undefined && parent.get(root) !== root) {
      root = parent.get(root)!;
    }
    // Path compression
    let cur = k;
    while (cur !== root) {
      const next = parent.get(cur)!;
      parent.set(cur, root);
      cur = next;
    }
    return root;
  };
  const union = (a: string, b: string) => {
    const ra = find(a);
    const rb = find(b);
    if (ra !== rb) parent.set(ra, rb);
  };
  for (const g of groups) {
    for (const p of g.pins) {
      const k = pinKey(p.ref, p.pin);
      pinByKey.set(k, p);
      if (!parent.has(k)) parent.set(k, k);
    }
    const first = g.pins[0];
    for (let i = 1; i < g.pins.length; i++) {
      union(pinKey(first.ref, first.pin), pinKey(g.pins[i].ref, g.pins[i].pin));
    }
  }
  // Groups carrying a "global" name (explicit net or non-Local label) are
  // one net wherever the name appears — union them by name too.
  const firstPinForName = new Map<string, string>();
  for (const g of groups) {
    if (!g.name || !g.mergeByName || g.pins.length === 0) continue;
    const k = pinKey(g.pins[0].ref, g.pins[0].pin);
    const prior = firstPinForName.get(g.name);
    if (prior) union(prior, k);
    else firstPinForName.set(g.name, k);
  }

  // Collect merged components and resolve names.
  const byRoot = new Map<string, { pins: Set<string>; explicitNames: Set<string>; otherNames: Set<string> }>();
  for (const g of groups) {
    if (g.pins.length === 0) continue;
    const root = find(pinKey(g.pins[0].ref, g.pins[0].pin));
    let entry = byRoot.get(root);
    if (!entry) {
      entry = { pins: new Set(), explicitNames: new Set(), otherNames: new Set() };
      byRoot.set(root, entry);
    }
    for (const p of g.pins) entry.pins.add(pinKey(p.ref, p.pin));
    if (g.name) {
      (g.explicit ? entry.explicitNames : entry.otherNames).add(g.name);
    }
  }

  const netByPin = new Map<string, string>();
  const nets = new Map<string, string[]>();
  const usedNames = new Map<string, number>();
  for (const entry of byRoot.values()) {
    const explicitNames = [...entry.explicitNames].sort();
    const labelNames = [...entry.otherNames].filter((n) => !/^NET-\d+$/.test(n)).sort();
    const autoNames = [...entry.otherNames].filter((n) => /^NET-\d+$/.test(n)).sort();
    let name = explicitNames[0] ?? labelNames[0] ?? autoNames[0] ?? "NET-???";
    const distinct = [...new Set([...explicitNames, ...labelNames])];
    if (distinct.length > 1) {
      warnings.push(
        `Nets ${distinct.map((n) => `"${n}"`).join(" and ")} are connected together (merged by shared pins/wires) — using "${name}". If that's not intended, check the netlist.`,
      );
    }
    // Two electrically disjoint nets must never share a pad net id — that
    // would short them at routing time. Local-label collisions land here.
    const taken = usedNames.get(name);
    if (taken !== undefined) {
      const renamed = `${name}_${taken + 1}`;
      usedNames.set(name, taken + 1);
      warnings.push(
        `Two disjoint nets both resolve to "${name}" — the second was renamed "${renamed}". If they should be one net, use a Global label or declare it in \`nets\`.`,
      );
      name = renamed;
    }
    usedNames.set(name, usedNames.get(name) ?? 1);
    const pinRefs: string[] = [];
    for (const key of entry.pins) {
      netByPin.set(key, name);
      const p = pinByKey.get(key)!;
      pinRefs.push(`${p.ref}.${p.pin}`);
    }
    nets.set(name, pinRefs.sort());
  }

  return { netByPin, nets, warnings };
}

// ============================================================================
// Schemas
// ============================================================================

/** JSON Schema for create_schematic tool. */
export const createSchematicSchema = {
  type: "object" as const,
  properties: {
    title: {
      type: "string" as const,
      description: "Schematic sheet title",
    },
    components: {
      type: "array" as const,
      description: "Components to place on the schematic",
      items: {
        type: "object" as const,
        properties: {
          ref: { type: "string" as const, description: 'Reference designator (e.g. "R1", "U3")' },
          part: {
            type: "string" as const,
            description:
              'Optional part name to auto-resolve pins from the parts database, ' +
              'e.g. "NE555", "LM358", "ATmega328P" (aliases like "LM555" work too). ' +
              "When set and `pins` is omitted, the part's universal pin definitions " +
              "(number, name, electrical type) are populated automatically.",
          },
          value: {
            type: "string" as const,
            description:
              'Component value (e.g. "10k", "100nF"). Optional when `part` is given ' +
              "(defaults to the part name). Also tried as a part name when `part` is " +
              "absent and `pins` is omitted.",
          },
          footprint: {
            type: "string" as const,
            description: 'Footprint ID (e.g. "Resistor_SMD:R_0805", "SOIC-8", "DIP-8")',
          },
          x: { type: "number" as const, description: "X position on sheet" },
          y: { type: "number" as const, description: "Y position on sheet" },
          rotation: { type: "number" as const, description: "Rotation in degrees (default 0)" },
          pins: {
            type: "array" as const,
            description:
              "Component pins. Optional when `part` (or `value`) names a part in " +
              "the database — pins are auto-resolved. Provide explicitly to override " +
              "the database or to define a part it doesn't cover.",
            items: {
              type: "object" as const,
              properties: {
                number: { type: "string" as const },
                name: { type: "string" as const },
                type: { type: "string" as const, description: "Pin type: Input, Output, Passive, PowerInput, etc." },
                x: { type: "number" as const },
                y: { type: "number" as const },
              },
              required: ["number", "name", "type"],
            },
          },
          pads: {
            type: "array" as const,
            description:
              "Optional explicit pad geometry (footprint-local mm), an escape " +
              "hatch overriding the parametric footprint engine for parts it " +
              "doesn't cover. Each pad's `number` should match a pin number; " +
              "net/layers are assigned automatically.",
            items: {
              type: "object" as const,
              properties: {
                number: { type: "string" as const, description: "Matches a pin number" },
                padType: { type: "string" as const, description: '"SMD" | "THT" | "NPTH" (default SMD)' },
                shape: {
                  type: "object" as const,
                  description:
                    'Pad shape, e.g. {"type":"Rect","width":1,"height":1.2} or ' +
                    '{"type":"Circle","diameter":1.6}',
                },
                position: {
                  type: "object" as const,
                  description: "Footprint-local position {x, y} in mm",
                  properties: { x: { type: "number" as const }, y: { type: "number" as const } },
                },
                rotation: { type: "number" as const },
                drill: {
                  type: "object" as const,
                  description: 'Drill spec for THT pads, e.g. {"diameter":0.8}',
                },
              },
              required: ["number", "shape", "position"],
            },
          },
        },
        required: ["ref", "footprint", "x", "y"],
      },
    },
    wires: {
      type: "array" as const,
      description: "Wire connections between pins",
      items: {
        type: "object" as const,
        properties: {
          x1: { type: "number" as const },
          y1: { type: "number" as const },
          x2: { type: "number" as const },
          y2: { type: "number" as const },
        },
        required: ["x1", "y1", "x2", "y2"],
      },
    },
    labels: {
      type: "array" as const,
      description: "Net labels",
      items: {
        type: "object" as const,
        properties: {
          name: { type: "string" as const, description: "Net name" },
          x: { type: "number" as const },
          y: { type: "number" as const },
          scope: { type: "string" as const, description: "Label scope: Local, Global, Hierarchical" },
        },
        required: ["name", "x", "y"],
      },
    },
    nets: {
      type: "object" as const,
      description:
        'Explicit netlist: net name → array of pin refs ("REF.PIN"), e.g. ' +
        '{"PHA": ["L1.1", "J1.1"], "GND": ["C1.2", "U1.4"]}. The most reliable ' +
        "way to declare connectivity — no wire/label coordinates needed. Merged " +
        "with any wire-derived connectivity; explicit names win.",
      additionalProperties: {
        type: "array" as const,
        items: { type: "string" as const },
      },
    },
  },
  required: ["components"],
};

/** Shared document input properties for session-based ECAD tools. */
const docInputProperties = {
  document_id: {
    type: "string" as const,
    description:
      "Session id from create_schematic or open_document. Preferred — the " +
      "tool mutates the server-side session, so the full document never " +
      "crosses the wire.",
  },
  document: {
    type: "object" as const,
    description:
      "Inline vcad IR Document (legacy stateless flow). Prefer document_id.",
  },
};

/** JSON Schema for place_components tool. */
export const placeComponentsSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    board_width: {
      type: "number" as const,
      description: "Board width in mm (rectangular outline)",
    },
    board_height: {
      type: "number" as const,
      description: "Board height in mm (rectangular outline)",
    },
    board_shape: {
      type: "object" as const,
      description:
        "Non-rectangular outline shorthand. Circle: {type: 'circle', " +
        "outer_diameter, inner_diameter?} — inner_diameter > 0 adds a " +
        "center bore cutout (e.g. a motor stator). Centered at " +
        "(outer_diameter/2, outer_diameter/2) unless `center` is given.",
      properties: {
        type: { type: "string" as const, description: "'circle'" },
        outer_diameter: { type: "number" as const },
        inner_diameter: { type: "number" as const, description: "Center bore diameter (0 = none)" },
        center: {
          type: "object" as const,
          properties: { x: { type: "number" as const }, y: { type: "number" as const } },
        },
        segments: { type: "number" as const, description: "Polygon segments per circle (default 64)" },
      },
    },
    outline: {
      type: "object" as const,
      description:
        "Explicit board outline polygon: {vertices: [{x,y}, ...], cutouts?: " +
        "[[{x,y}, ...], ...]}. Use the output of board_from_solid to match an " +
        "existing enclosure or solid part.",
      properties: {
        vertices: {
          type: "array" as const,
          items: {
            type: "object" as const,
            properties: { x: { type: "number" as const }, y: { type: "number" as const } },
            required: ["x", "y"],
          },
        },
        cutouts: {
          type: "array" as const,
          items: {
            type: "array" as const,
            items: {
              type: "object" as const,
              properties: { x: { type: "number" as const }, y: { type: "number" as const } },
              required: ["x", "y"],
            },
          },
        },
      },
      required: ["vertices"],
    },
    board_thickness: {
      type: "number" as const,
      description: "Board thickness in mm (default 1.6)",
    },
    strategy: {
      type: "string" as const,
      description:
        "Placement strategy: grid, force_directed, radial (default: grid). " +
        "force_directed pulls net-sharing parts together and pushes overlapping " +
        "courtyards apart (size-aware), then legalizes any different-net pad " +
        "overlap to clearance so it never bakes in a short — prefer it for " +
        "dense, multi-cluster boards (power stage + MCU + connectors) where " +
        "routability matters. If the board is too small to separate every " +
        "cross-net pad it reports `placement_conflicts` and success:false. " +
        "radial places components evenly on a ring — natural for circular " +
        "boards like motor stators.",
    },
    radial_radius: {
      type: "number" as const,
      description:
        "Ring radius for strategy=radial (default: midway between bore and " +
        "rim for circular boards).",
    },
    radial_start_angle_deg: {
      type: "number" as const,
      description: "Angle of the first component for strategy=radial (default 0 = +X).",
    },
    edge_margin: {
      type: "number" as const,
      description:
        "Edge clearance in mm used when computing the advisory " +
        "`utilization.suggested_outline` (default: max of the design-rule edge " +
        "clearance and 2mm). Does not affect placement — only the suggested " +
        "right-sized outline reported back.",
    },
  },
  required: [],
};

/** JSON Schema for route_nets tool. */
export const routeNetsSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Net IDs to route (empty = route all). Re-running is safe and " +
        "self-cleaning: routing rips up the prior *autorouted* copper on each " +
        "net first and lays a complete fresh route, so trace counts don't grow " +
        "across iterations. Hand-placed copper (add_trace / add_via / coil " +
        "tools) is never ripped: a net carrying a manual trace is preserved " +
        "wholesale and reported in `manual_nets_preserved`. After a " +
        "set_placement move, nets whose pads no longer sit under their copper " +
        "are detected as stale and re-routed too (even if not listed here); " +
        "the result reports `traces_removed`/`vias_removed` and " +
        "`stale_nets_cleared` so the cleanup is visible.",
    },
    locked_nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Nets whose existing copper is preserved — never ripped up or " +
        "re-routed by this call. Copper on these nets survives across " +
        "route_nets passes; the kernel still routes every other net. Nets " +
        "carrying hand-placed traces (add_trace / coil tools) get this " +
        "protection automatically via copper provenance, so locking is only " +
        "needed for copper the tools didn't tag (e.g. imported or injected " +
        "boards).",
    },
    strategy: {
      type: "string" as const,
      description:
        "Net ordering: 'auto' (default — one negotiated whole-board pass, best " +
        "quality), 'power_first' (route power/plane nets before signals), " +
        "'fanout_desc' (high pin-count nets first, to claim channels), or " +
        "'fanout_asc'. Non-auto strategies route in priority tiers, each tier " +
        "seeing the previous tiers' copper as obstacles.",
    },
    trace_width: {
      type: "number" as const,
      description:
        "Fallback trace width in mm for nets with NO net class. Per-net-class " +
        "widths AND clearances are applied automatically from the design rules " +
        "(set_design_rules classes) — e.g. a VBAT/phase net in a 'power' class " +
        "routes at the class width even if you pass a thin trace_width here. So " +
        "you do NOT need one route_nets call per width; set classes once and " +
        "route all nets together. Defaults to the default-class width.",
    },
    receipt: {
      type: "boolean" as const,
      description:
        "When true, wrap the route in a before/after DRC and return a `receipt` verdict (what it fixed, what it introduced incl. shorts, with each violation attributed to footprint vs routing) — instead of just a document_id. Re-routing rips up the prior autorouted copper first (idempotent by construction); the receipt proves it, surfacing any re-route that would short the board.",
    },
    effort: {
      type: "number" as const,
      description:
        "Effort multiplier ≥ 0.1 (default 1): scales the router's iteration " +
        "budgets (negotiation and rip-up rounds). Raise to 2–10 on a congested " +
        "board that leaves nets unrouted; lower below 1 for a fast draft. " +
        "Legality is unaffected — effort buys more attempts, not looser rules.",
    },
  },
  required: [],
};

/** JSON Schema for run_drc tool. */
export const runDrcSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    detail: {
      type: "string" as const,
      description:
        "'summary' (default) returns counts by rule + net-pair, the worst " +
        "clearance, and a capped representative sample — small, even with " +
        "tens of thousands of violations. 'full' additionally attaches the " +
        "complete `details` array.",
    },
    sample_size: {
      type: "number" as const,
      description:
        "Max violations in the representative `sample` (default 20). The " +
        "sample is drawn round-robin across distinct (rule, net-pair) buckets.",
    },
  },
  required: [],
};

/** JSON Schema for run_erc tool. */
export const runErcSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
  },
  required: [],
};

/** JSON Schema for critique_route tool. */
export const critiqueRouteSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    net: {
      type: "string" as const,
      description: "Net to audit (read-only — mutates nothing).",
    },
  },
  required: ["net"],
};

/** JSON Schema for route_diff_pair tool. */
export const routeDiffPairSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    net_p: { type: "string" as const, description: "Positive-polarity net of the pair." },
    net_n: { type: "string" as const, description: "Negative-polarity net of the pair." },
  },
  required: ["net_p", "net_n"],
};

/** JSON Schema for length_match_traces tool. */
export const lengthMatchTracesSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "The match group: routed nets whose copper lengths must agree (a DDR " +
        "byte lane, a clock pair, a SPI bus). Shorter nets grow meanders; the " +
        "longest net (or target_length) sets the target.",
    },
    target_length: {
      type: "number" as const,
      description:
        "Explicit target routed length in mm. Omit to match everything to the " +
        "longest net in the group.",
    },
    tolerance: {
      type: "number" as const,
      description: "A net counts as matched within this of the target, mm (default 0.1).",
    },
    max_amplitude: {
      type: "number" as const,
      description: "Maximum meander amplitude, mm (default 2.0).",
    },
    spacing: {
      type: "number" as const,
      description: "Meander period spacing along the trace, mm (default 1.0).",
    },
    style: {
      type: "string" as const,
      description: "Meander pattern: 'trombone' (U-bends, default) or 'sawtooth' (zigzag).",
    },
    check_only: {
      type: "boolean" as const,
      description:
        "Measure and verdict only — report each net's routed length and " +
        "deviation from the target without generating meanders or touching copper.",
    },
  },
  required: ["nets"],
};

/** JSON Schema for export_gerber tool. */
export const exportGerberSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    output_dir: {
      type: "string" as const,
      description:
        "Directory to write the fabrication files to (created if missing). " +
        "Resolved on the MCP server's filesystem — on hosted/sandboxed servers " +
        "the write may fail. When omitted (or the write fails), a small bundle " +
        "is returned inline, but a bundle over the inline byte cap (~168 KB " +
        "Gerber sets exceed it) is written to the artifact store and the result " +
        "carries { artifact_url, manifest, artifact_id } instead of the files — " +
        "pass that artifact_id to quote_manufacturing / place_order so the fab " +
        "files never transit model context.",
    },
    require_clean_drc: {
      type: "boolean" as const,
      description:
        "Gate the export on a clean DRC (default TRUE). When set, the board is " +
        "DRC-checked first and the export is BLOCKED — returning `blocked:true` " +
        "plus the DRC summary — if it has any errors (shorts, clearance, " +
        "unconnected nets, fab-rule breaks) or the check is unverifiable " +
        "(fail-closed: a board that won't even parse never counts as clean). " +
        "Set false only to force a Gerber bundle from a board you know is dirty; " +
        "the resulting files would otherwise fabricate an invalid board. Use " +
        "validate_for_fab first for the full readiness verdict.",
    },
  },
  required: [],
};

/** JSON Schema for validate_for_fab tool. */
export const validateForFabSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
  },
  required: [],
};

/** JSON Schema for fab_prep tool. */
export const fabPrepSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    calibrate_rules: {
      type: "boolean" as const,
      description:
        "Derive and apply design-rule calibration from the board's OWN declared " +
        "via classes and pre-existing footprint holes (default FALSE). Imported " +
        "boards routinely carry global minima that forbid the via class they " +
        "themselves declare — e.g. a 0.21/0.12mm class under a 0.2mm minDrill, " +
        "which flags every via on the board. Calibration only ever relaxes a rule " +
        "to the point where the board's own GIVEN geometry stops being illegal, " +
        "is floored at laser-microvia limits, and records every change with its " +
        "derivation in the receipt. Off by default because silently relaxing DRC " +
        "rules to make a board pass is how an unbuildable board ships.",
    },
    route_remaining: {
      type: "boolean" as const,
      description:
        "Route or certify the connections the board arrived without, before the " +
        "fix loop (default TRUE). Each unrouted connection ends as Routed, " +
        "ProvedInfeasible (with a bottleneck-cut certificate), or an honest " +
        "unknown — never silently dropped.",
    },
    max_rounds: {
      type: "number" as const,
      description:
        "Maximum strip-and-re-route rounds (default 8). Each round censuses the " +
        "violations the ROUTING is answerable for, strips those nets, and hands " +
        "them back to the session-probed router.",
    },
    budget: {
      type: "number" as const,
      description:
        "Per-cluster search budget in node expansions for the complete router " +
        "(default 5,000,000). Lower is faster and yields more honest unknowns.",
    },
    max_cluster: {
      type: "number" as const,
      description: "Maximum connections coalesced into one joint search window (default 6).",
    },
    prune_dangling: {
      type: "boolean" as const,
      description:
        "Remove copper that reaches no pad or pour of its net before the final " +
        "DRC (default TRUE).",
    },
    accept_rules: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "DRC rule names whose route-attributable violations are ACCEPTED rather " +
        'than fixed (e.g. ["MinTraceWidth"]). Real fab packages ship with real, ' +
        "named exceptions; the difference between that and a footgun is whether " +
        "the exception is written down. Waived violations are still counted, " +
        "still listed, and the waiver is named in the receipt — it stops blocking " +
        "the verdict, it does not hide anything. An unrecognised rule name " +
        "refuses the run rather than silently accepting nothing.",
    },
    dry_run: {
      type: "boolean" as const,
      description:
        "Compute the receipt without writing the fixed board back to the session.",
    },
  },
  required: [],
};

/** JSON Schema for export_kicad tool. */
export const exportKicadSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    filename: {
      type: "string" as const,
      description:
        "Output filename ending in .kicad_pcb (board), .kicad_sch (schematic), or " +
        ".kicad_pro (linked project bundle). The extension selects what is exported: " +
        ".kicad_pcb writes the session's board (footprints, pads, nets, traces, vias, " +
        "zones, layers, outline) as a native, editable KiCad 9 file a human can open " +
        "and finish routing; .kicad_sch writes the session's schematic; .kicad_pro " +
        "(or a bare name with no extension) writes all three files with board " +
        "footprints linked to their schematic symbols so KiCad can cross-probe " +
        "(click a symbol → highlight its footprint). Defaults to board.kicad_pcb.",
    },
    output_dir: {
      type: "string" as const,
      description:
        "Directory to write the file to (created if missing). Resolved on the MCP " +
        "server's filesystem — on hosted/sandboxed servers the write may fail or be " +
        "invisible, in which case the file content is returned inline instead. When " +
        "omitted, the content is returned inline (subject to a size cap).",
    },
  },
  required: [],
};

/** JSON Schema for add_coil tool. */
export const addCoilSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    center: {
      type: "object" as const,
      description: "Spiral center on the board, mm",
      properties: { x: { type: "number" as const }, y: { type: "number" as const } },
      required: ["x", "y"],
    },
    turns: { type: "number" as const, description: "Number of turns (fractional allowed)" },
    inner_radius: { type: "number" as const, description: "Innermost turn radius, mm" },
    outer_radius: { type: "number" as const, description: "Outermost turn radius, mm" },
    trace_width: { type: "number" as const, description: "Copper trace width, mm" },
    clearance: {
      type: "number" as const,
      description:
        "Minimum gap between adjacent turns, mm (default: design-rule clearance). " +
        "The radial pitch (outer-inner)/turns must be >= trace_width + clearance.",
    },
    net: { type: "string" as const, description: "Net name for the coil copper (e.g. 'PHA')" },
    layer: { type: "string" as const, description: "Copper layer (default 'FCu')" },
    direction: {
      type: "string" as const,
      description: "'ccw' (default) or 'cw', looking at the front of the board",
    },
    start_angle_deg: {
      type: "number" as const,
      description: "Angle of the inner endpoint (default 0 = +X from center)",
    },
    segments_per_turn: {
      type: "number" as const,
      description: "Polyline resolution (default 48)",
    },
    inner_via: {
      type: "boolean" as const,
      description:
        "Place a via at the inner endpoint to escape on another layer " +
        "(default false). A spiral's inner end is otherwise trapped.",
    },
    via_to_layer: {
      type: "string" as const,
      description: "Layer the inner via connects to (default 'BCu')",
    },
    inner_lead_out: {
      type: "number" as const,
      description:
        "If > 0, prepend a tangential lead-out terminal of this length (mm) at " +
        "the inner end so the inner via no longer lands on the same radial spoke " +
        "as the outer endpoint (a real same-net bypass-short hazard). When " +
        "inner_via is set, the via is placed at the new lead-out terminal.",
    },
    layers: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Build a STACKED multilayer coil: same spiral geometry on each copper " +
        "layer (fields add), stitched together with vias at alternating inner/ " +
        "outer terminals. Length 2 means front+back. When given, `layer` and the " +
        "single-via escape (inner_via/via_to_layer) are ignored — stitching " +
        "handles inter-layer connections.",
    },
  },
  required: ["center", "turns", "inner_radius", "outer_radius", "trace_width", "net"],
};

/** JSON Schema for board_from_solid tool. */
export const boardFromSolidSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "CAD session id (open_document / create_cad_loon) holding the solid",
    },
    part_id: {
      type: "string" as const,
      description:
        "Root id of the part to trace (from `read`). Defaults to the only " +
        "solid part; errors with a list if the document has several.",
    },
    thickness: { type: "number" as const, description: "Board thickness in mm (default 1.6)" },
    resolution: {
      type: "number" as const,
      description:
        "Projection grid cell size in mm (default: auto, ~1/400 of the part " +
        "extent). Smaller = more outline detail.",
    },
    simplify_tolerance: {
      type: "number" as const,
      description: "Polygon simplification tolerance in mm (default: 1.5 × cell size)",
    },
  },
  required: ["document_id"],
};

/** JSON Schema for add_coil_array tool. */
export const addCoilArraySchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    count: {
      type: "number" as const,
      description: "Number of coils to place evenly around the ring (>= 1)",
    },
    center: {
      type: "object" as const,
      description: "Ring center on the board, mm",
      properties: { x: { type: "number" as const }, y: { type: "number" as const } },
      required: ["x", "y"],
    },
    pitch_radius: {
      type: "number" as const,
      description: "Radius of the circle the coil centers sit on, mm (>= 0)",
    },
    start_angle_deg: {
      type: "number" as const,
      description: "Angle of the first coil center (default 0 = +X), degrees CCW",
    },
    turns: { type: "number" as const, description: "Turns per coil (fractional allowed)" },
    inner_radius: { type: "number" as const, description: "Innermost turn radius per coil, mm" },
    outer_radius: { type: "number" as const, description: "Outermost turn radius per coil, mm" },
    trace_width: { type: "number" as const, description: "Copper trace width, mm" },
    layer: { type: "string" as const, description: "Copper layer for every coil (default 'FCu')" },
    clearance: {
      type: "number" as const,
      description: "Min turn-to-turn gap, mm (default: design-rule clearance)",
    },
    net_sequence: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Per-coil net names, cycled when shorter than count (e.g. " +
        "['PHA','PHB','PHC'] for a 3-phase ring). Overrides `net`.",
    },
    net: {
      type: "string" as const,
      description: "Single net for every coil when net_sequence is omitted",
    },
    chirality: {
      type: "string" as const,
      description:
        "Spiral winding sense: 'uniform' (all ccw, default), 'alternating' " +
        "(ccw, cw, ccw, …), or a fixed 'ccw'/'cw'. GEOMETRY ONLY — it carries " +
        "no phase/polarity meaning; derive correct per-coil polarity with " +
        "winding_layout.",
    },
    segments_per_turn: { type: "number" as const, description: "Polyline resolution (default 48)" },
    inner_via: {
      type: "boolean" as const,
      description: "Drop a via at each coil's inner endpoint to escape on another layer",
    },
    via_to_layer: {
      type: "string" as const,
      description: "Layer the inner vias connect to (default 'BCu')",
    },
  },
  required: ["count", "center", "pitch_radius", "turns", "inner_radius", "outer_radius", "trace_width"],
};

/** JSON Schema for winding_layout tool. */
export const windingLayoutSchema = {
  type: "object" as const,
  properties: {
    slots: { type: "number" as const, description: "Stator slot/tooth count Z (>= 1)" },
    poles: { type: "number" as const, description: "Rotor pole count 2p (even, >= 2)" },
    phases: { type: "number" as const, description: "Phase count m (default 3)" },
    turns_per_coil: {
      type: "number" as const,
      description: "Turns per coil (default 1, fractional allowed)",
    },
    connection: {
      type: "string" as const,
      description: "'wye' (default) or 'delta' — phase termination topology",
    },
    layer: {
      type: "string" as const,
      description:
        "'double' (default — one coil per tooth, the general FSCW case) or " +
        "'single'. Odd slot counts force double.",
    },
    phase_nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Net name per phase, in order (default ['PHA','PHB','PHC',…])",
    },
    neutral_net: {
      type: "string" as const,
      description: "Wye neutral net name (default 'WIND_N'); ignored for delta",
    },
  },
  required: ["slots", "poles"],
};

/** JSON Schema for calc_impedance tool. */
export const calcImpedanceSchema = {
  type: "object" as const,
  properties: {
    trace_width: {
      type: "number" as const,
      description: "Trace width in mm",
    },
    copper_thickness: {
      type: "number" as const,
      description: "Copper thickness in mm (default 0.035)",
    },
    dielectric_height: {
      type: "number" as const,
      description: "Dielectric height in mm",
    },
    dielectric_er: {
      type: "number" as const,
      description: "Relative permittivity (default 4.5 for FR4)",
    },
    trace_type: {
      type: "string" as const,
      description: "Trace type: microstrip, stripline, diff_microstrip, diff_stripline",
    },
    spacing: {
      type: "number" as const,
      description: "Spacing between traces in mm (for differential pairs)",
    },
    document_id: {
      type: "string" as const,
      description:
        "Optional: session id of a PCB to verify against. With `net`, the " +
        "impedance is only certified when that net's trace is actually realized " +
        "as continuous copper — a split/unrouted trace returns a blocked result.",
    },
    net: {
      type: "string" as const,
      description:
        "Optional: the board net this trace carries. Requires `document_id`; " +
        "gates the impedance number on the trace being galvanically realized.",
    },
  },
  required: ["trace_width", "dielectric_height"],
};

/** JSON Schema for size_impedance tool. */
export const sizeImpedanceSchema = {
  type: "object" as const,
  properties: {
    trace_type: {
      type: "string" as const,
      description:
        "microstrip (default), stripline, diff_microstrip, or diff_stripline. " +
        "diff_* solves trace width AND spacing for a target differential impedance.",
    },
    target_z0: {
      type: "number" as const,
      description:
        "Target single-ended characteristic impedance in Ω (default 50). For " +
        "diff pairs this is the per-line target (default target_diff_z0/2).",
    },
    target_diff_z0: {
      type: "number" as const,
      description: "Target differential impedance in Ω for diff_* types (default 100)",
    },
    dielectric_height: { type: "number" as const, description: "Dielectric height h in mm (required)" },
    dielectric_er: { type: "number" as const, description: "Relative permittivity (default 4.5, FR4)" },
    copper_thickness: { type: "number" as const, description: "Copper thickness t in mm (default 0.035)" },
    min_width: { type: "number" as const, description: "DFM minimum trace width in mm (default 0.1)" },
    max_width: { type: "number" as const, description: "Maximum trace width to consider in mm (default 5)" },
    min_spacing: { type: "number" as const, description: "DFM minimum edge-to-edge spacing in mm, diff only (default = min_width)" },
    max_spacing: { type: "number" as const, description: "Maximum spacing to consider in mm, diff only (default 5)" },
    fab_grid_mm: {
      type: "number" as const,
      description: "Snap the solved geometry to this manufacturing grid in mm (default 0.0254 = 1 mil)",
    },
    tolerance_pct: {
      type: "number" as const,
      description: "Pass band: |measured − target| ≤ this %% of target (default 5)",
    },
  },
  required: ["dielectric_height"],
};

/** JSON Schema for size_pdn tool. */
export const sizePdnSchema = {
  type: "object" as const,
  properties: {
    nodes: {
      type: "number" as const,
      description: "Number of PDN nodes; node 0 is the VRM / 0 V reference",
    },
    edges: {
      type: "array" as const,
      description: "Copper segments (a resistor mesh); each segment's width is solved for",
      items: {
        type: "object" as const,
        properties: {
          a: { type: "number" as const, description: "First node index" },
          b: { type: "number" as const, description: "Second node index" },
          length: { type: "number" as const, description: "Segment length in mm" },
        },
        required: ["a", "b", "length"],
      },
    },
    loads: {
      type: "array" as const,
      description: "Current drawn at a node, A",
      items: {
        type: "object" as const,
        properties: {
          node: { type: "number" as const },
          current: { type: "number" as const },
        },
        required: ["node", "current"],
      },
    },
    targets: {
      type: "array" as const,
      description:
        "Per-node IR-drop budget in V. Widths are sized so each node's drop meets " +
        "its budget with minimal copper (the solver drives drop → budget).",
      items: {
        type: "object" as const,
        properties: {
          node: { type: "number" as const },
          max_drop: { type: "number" as const },
        },
        required: ["node", "max_drop"],
      },
    },
    copper_thickness: { type: "number" as const, description: "Copper thickness in mm (default 0.035)" },
    resistivity: { type: "number" as const, description: "Copper resistivity Ω·mm (default 1.68e-5)" },
    min_width: { type: "number" as const, description: "DFM minimum segment width in mm (default 0.1)" },
    max_width: { type: "number" as const, description: "Maximum segment width to consider in mm (default 5)" },
    fab_grid_mm: { type: "number" as const, description: "Snap widths to this grid in mm (default 0.0254)" },
    tolerance_pct: { type: "number" as const, description: "Budget is met if drop ≤ target·(1 + this%) (default 5)" },
    engine: {
      type: "string" as const,
      description:
        "'ts' (default) uses the JS solver; 'exact' routes into the Rust " +
        "kernel engine (implicit-function adjoint) via WASM when available, " +
        "falling back to 'ts' if the artifact is absent.",
    },
    document_id: {
      type: "string" as const,
      description:
        "Optional: session id of a PCB this PDN mesh represents. With `net`, the " +
        "sizing PASS is gated on that power plane being galvanically continuous " +
        "— a plane split into islands returns a blocked result with coverage stats.",
    },
    net: {
      type: "string" as const,
      description:
        "Optional: the power net this PDN mesh models (e.g. '+3V3'). Requires " +
        "`document_id`; refuses to certify a PASS on a disconnected plane.",
    },
  },
  required: ["nodes", "edges", "loads", "targets"],
};

/** JSON Schema for calc_coil tool. */
export const calcCoilSchema = {
  type: "object" as const,
  properties: {
    inner_radius: { type: "number" as const, description: "Innermost turn radius in mm" },
    outer_radius: { type: "number" as const, description: "Outermost turn radius in mm" },
    turns: { type: "number" as const, description: "Number of turns" },
    trace_width: { type: "number" as const, description: "Copper trace width in mm" },
    copper_thickness: { type: "number" as const, description: "Copper thickness in mm (default 0.035)" },
    resistivity: { type: "number" as const, description: "Copper resistivity Ω·mm (default 1.68e-5)" },
    geometry: {
      type: "string" as const,
      description: "Spiral shape: circular (default), square, hexagonal, octagonal",
    },
  },
  required: ["inner_radius", "outer_radius", "turns", "trace_width"],
};

/** JSON Schema for size_coil tool. */
export const sizeCoilSchema = {
  type: "object" as const,
  properties: {
    target_inductance_nh: { type: "number" as const, description: "Target inductance in nH" },
    inner_radius: { type: "number" as const, description: "Innermost turn radius in mm" },
    outer_radius: { type: "number" as const, description: "Outermost turn radius in mm" },
    trace_width: { type: "number" as const, description: "Copper trace width in mm" },
    clearance: { type: "number" as const, description: "Turn-to-turn gap in mm (default = trace_width)" },
    copper_thickness: { type: "number" as const, description: "Copper thickness in mm (default 0.035)" },
    resistivity: { type: "number" as const, description: "Copper resistivity Ω·mm (default 1.68e-5)" },
    geometry: {
      type: "string" as const,
      description: "Spiral shape: circular (default), square, hexagonal, octagonal",
    },
    tolerance_pct: { type: "number" as const, description: "Pass band on inductance (default 5)" },
  },
  required: ["target_inductance_nh", "inner_radius", "outer_radius", "trace_width"],
};

/** JSON Schema for calc_rf tool. */
export const calcRfSchema = {
  type: "object" as const,
  properties: {
    topology: {
      type: "string" as const,
      description: "'series_rlc' (default) or 'parallel_rlc'",
    },
    r_ohm: { type: "number" as const, description: "Resistance in Ω" },
    l_henry: { type: "number" as const, description: "Inductance in H (e.g. 1e-9 for 1 nH)" },
    c_farad: { type: "number" as const, description: "Capacitance in F (e.g. 1e-12 for 1 pF)" },
    z0_ohm: { type: "number" as const, description: "Reference impedance for S11 (default 50)" },
    f_start_hz: { type: "number" as const, description: "Sweep start frequency (default 0.1·f0)" },
    f_stop_hz: { type: "number" as const, description: "Sweep stop frequency (default 10·f0)" },
    points: { type: "number" as const, description: "Log-spaced sweep points returned (default 21, max 256)" },
  },
  required: ["r_ohm", "l_henry", "c_farad"],
};

// ============================================================================
// Planar-magnetics leaves — modified-Wheeler spiral inductance (Mohan 1999)
// ============================================================================

/** Modified-Wheeler shape coefficients (K1, K2) per spiral geometry. */
const COIL_GEOMETRY: Record<string, { k1: number; k2: number }> = {
  circular: { k1: 2.25, k2: 3.55 },
  square: { k1: 2.34, k2: 2.75 },
  hexagonal: { k1: 2.33, k2: 3.82 },
  octagonal: { k1: 2.25, k2: 3.55 },
};
const MU0 = 4 * Math.PI * 1e-7; // H/m

/** Modified-Wheeler planar-spiral inductance (nH). Inputs in mm. */
function coilInductanceNh(turns: number, innerR: number, outerR: number, geometry: string): number {
  const { k1, k2 } = COIL_GEOMETRY[geometry] ?? COIL_GEOMETRY.circular!;
  const dAvgM = (innerR + outerR) * 1e-3; // (d_in + d_out)/2 in metres
  const fill = (outerR - innerR) / (outerR + innerR); // fill ratio ρ
  const lH = (k1 * MU0 * turns * turns * dAvgM) / (1 + k2 * fill);
  return lH * 1e9;
}

/** Spiral copper length (mm): turns × mean circumference. */
function coilWireLengthMm(turns: number, innerR: number, outerR: number): number {
  return turns * Math.PI * (innerR + outerR);
}

// ============================================================================
// Tool implementations
// ============================================================================

/** A pin with no net, enriched with handling guidance from the parts database. */
export interface UnconnectedPin {
  /** `${ref}.${number}` — the pin reference (e.g. "U1.5"). */
  ref: string;
  /** Pin name from the resolved part (e.g. "CTRL"); "~" when unnamed. */
  pin_name: string;
  /** Electrical pin type (PinType variant, e.g. "Input", "PowerInput"). */
  pin_type: string;
  /** How much leaving this pin open should worry the caller. */
  severity: "info" | "warning";
  /** Datasheet application notes that reference this specific pin, if any. */
  app_notes?: string[];
}

/**
 * Severity of leaving a pin of the given type unconnected. A floating signal
 * input or an unconnected power input is usually a real mistake (a chip with no
 * supply, or a logic input left to drift), so those are warnings; spare
 * outputs, passives, and open-collector pins are informational.
 */
export function unconnectedPinSeverity(pinType: string): "info" | "warning" {
  return pinType === "PowerInput" || pinType === "Input" ? "warning" : "info";
}

/**
 * Power/ground rail names that appear incidentally in application-note prose
 * ("bypass to GND", "decouple VCC") and would otherwise over-match. An
 * unconnected rail pin is already surfaced by its `warning` severity, so prose
 * matching doesn't need to flag it too.
 */
const RAIL_PIN_NAMES = new Set([
  "GND",
  "VCC",
  "VDD",
  "VSS",
  "VEE",
  "AVCC",
  "AVDD",
  "AGND",
  "DGND",
  "VDDIO",
]);

/** Escape a string for literal use inside a RegExp. */
function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Select the application notes that refer to a specific pin. A note matches when
 * it cites the pin by number ("pin 5", "pad 4") or names one of the pin's name
 * tokens — compound names like "PB5/~RESET" are split and overline markers
 * (`~`, `!`, `#`) dropped, so a note mentioning just "PB5" or "RESET" still
 * matches. Common power/ground rails are excluded from name matching.
 */
export function appNotesForPin(
  notes: string[],
  pin: { number: string; name: string },
): string[] {
  const numRe = new RegExp(
    `\\b(?:pin|pins|pad|pads)\\s*${escapeRegExp(pin.number)}\\b`,
    "i",
  );
  const nameRes = (pin.name || "")
    .split(/[\s/,]+/)
    .map((t) => t.replace(/[~!#]/g, "").trim())
    .filter((t) => t.length >= 2 && !RAIL_PIN_NAMES.has(t.toUpperCase()))
    .map((t) => new RegExp(`\\b${escapeRegExp(t)}\\b`, "i"));
  return notes.filter((n) => numRe.test(n) || nameRes.some((re) => re.test(n)));
}

/** Create a schematic from component, wire, and netlist definitions. */
// ============================================================================
// Pin-type validation
// ============================================================================

/**
 * Valid pin electrical types — mirrors the `PinType` enum in
 * `crates/vcad-ir/src/ecad.rs`. The compile-time assertions below fail the
 * build if this list drifts from the IR union (a missing or misspelled
 * variant), keeping Rust the single source of truth in spirit.
 */
const PIN_TYPES = [
  "Input",
  "Output",
  "Bidirectional",
  "TriState",
  "Passive",
  "PowerInput",
  "PowerOutput",
  "OpenCollector",
  "OpenEmitter",
  "NotConnected",
  "Free",
] as const satisfies readonly SchematicPin["pin_type"][];
// Drift guard: if the IR adds a PinType variant not listed above, the union
// `_MissingPinType` stops being `never` and this assignment fails to compile.
type _MissingPinType = Exclude<SchematicPin["pin_type"], (typeof PIN_TYPES)[number]>;
const _pinTypesAreExhaustive: _MissingPinType extends never ? true : false = true;
void _pinTypesAreExhaustive;

/** Cheap Levenshtein edit distance, for "did you mean" suggestions. */
function editDistance(a: string, b: string): number {
  const m = a.length;
  const n = b.length;
  if (m === 0) return n;
  if (n === 0) return m;
  let prev = Array.from({ length: n + 1 }, (_, i) => i);
  let curr = new Array<number>(n + 1);
  for (let i = 1; i <= m; i++) {
    curr[0] = i;
    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(prev[j] + 1, curr[j - 1] + 1, prev[j - 1] + cost);
    }
    [prev, curr] = [curr, prev];
  }
  return prev[n];
}

/** Closest candidate to `input` by case-insensitive edit distance. */
function closestCandidate(
  input: string,
  candidates: readonly string[],
): string | undefined {
  const low = input.toLowerCase();
  let best: string | undefined;
  let bestD = Infinity;
  for (const c of candidates) {
    const d = editDistance(low, c.toLowerCase());
    if (d < bestD) {
      bestD = d;
      best = c;
    }
  }
  return best;
}

/**
 * Validate a caller-supplied pin electrical type against the `PinType` enum,
 * defaulting empty/absent to "Passive". Throws an actionable error (with a
 * fuzzy "did you mean" hint and a case correction) on an unknown variant, so a
 * typo like "BiDirectional" fails at create_schematic time instead of surfacing
 * later as an opaque serde error in render_view / route_nets / export_gerber.
 */
function validatePinType(raw: unknown, where: string): SchematicPin["pin_type"] {
  if (raw === undefined || raw === null || raw === "") return "Passive";
  const s = String(raw);
  if ((PIN_TYPES as readonly string[]).includes(s)) {
    return s as SchematicPin["pin_type"];
  }
  // A pure casing slip (e.g. "BiDirectional" → "Bidirectional") gets a precise
  // suggestion; otherwise fall back to the nearest variant by edit distance.
  const cased = PIN_TYPES.find((t) => t.toLowerCase() === s.toLowerCase());
  const suggestion = cased ?? closestCandidate(s, PIN_TYPES);
  throw new Error(
    `Invalid pin type "${s}" on ${where}` +
      (suggestion ? ` — did you mean "${suggestion}"?` : "") +
      ` Valid pin types: ${PIN_TYPES.join(", ")}.`,
  );
}

/** Imperial chip-size codes for two-terminal passives. */
const CHIP_SIZE_CODES = new Set([
  "0201",
  "0402",
  "0603",
  "0805",
  "1206",
  "1210",
  "2010",
  "2512",
]);

/**
 * True when a footprint id names a two-terminal passive package — the chip
 * families (`R_0603`, `C_0805_2012Metric`, `L_...`, `D_SOD-123`) or a bare
 * chip-size code (`0603`). Such components get pins 1/2 (type Passive)
 * synthesized when the caller provides neither `part` nor `pins`.
 */
export function isTwoPinPassiveFootprint(footprint: unknown): boolean {
  if (typeof footprint !== "string") return false;
  const fp = footprint.trim();
  if (/^[RCLD]_/i.test(fp)) return true;
  return CHIP_SIZE_CODES.has(fp);
}

/** The two synthesized pins of a chip passive, in schematic-symbol layout. */
function synthesizedPassivePins(): SchematicPin[] {
  return [
    { number: "1", name: "1", pin_type: "Passive", position: { x: 0, y: 0 } },
    { number: "2", name: "2", pin_type: "Passive", position: { x: 5.08, y: 0 } },
  ];
}

export async function createSchematic(args: Record<string, unknown>) {
  const title = (args.title as string) || undefined;
  const componentsInput = (args.components as Array<Record<string, unknown>>) || [];
  const wiresInput = (args.wires as Array<Record<string, unknown>>) || [];
  const labelsInput = (args.labels as Array<Record<string, unknown>>) || [];
  const netsInput = (args.nets as Record<string, string[]>) || undefined;

  // Validate a coordinate field, throwing on non-finite input rather than
  // silently producing NaN that would propagate into schematic geometry.
  const coord = (v: unknown, where: string): number => {
    if (typeof v !== "number" || !Number.isFinite(v)) {
      throw new Error(
        `create_schematic: ${where} must be a finite number, got ${JSON.stringify(v)}`,
      );
    }
    return v;
  };
  const coordOpt = (v: unknown, where: string): number => {
    if (v === undefined || v === null) return 0;
    return coord(v, where);
  };

  const warnings: string[] = [];
  // Parts auto-resolved from the database, echoed back so the caller sees what
  // got pinned (and any datasheet / application notes).
  const resolvedParts: Array<{
    ref: string;
    part: string;
    footprint: string;
    pins: number;
    datasheet_url?: string;
    app_notes?: string[];
  }> = [];

  // Resolve each component's pins up front. Caller-provided `pins` always win
  // (the explicit override); otherwise look the part up in the parts database
  // by `part` (or `value` as a fallback) so jellybean ICs need no pin boilerplate.
  const resolvedDefs = await Promise.all(
    componentsInput.map(async (c) => {
      const explicit = (c.pins as Array<Record<string, unknown>>) || [];
      if (explicit.length > 0) return null;
      const partName = (c.part as string) || (c.value as string) || "";
      if (!partName) return null;
      const def = await kernelResolvePartDef(partName, c.footprint as string | undefined);
      if (!def) {
        // Only a hard miss when they named a part explicitly (a bare `value`
        // like "10k" is a passive, not a database lookup, and may have no pins).
        if (c.part) {
          warnings.push(
            `Part "${partName}" (ref ${c.ref ?? "?"}) is not in the parts database and ` +
              "no `pins` were provided — this component has no pins. Provide a `pins` " +
              "array, or use a known part / alias.",
          );
        }
        return null;
      }
      warnings.push(...def.warnings);
      resolvedParts.push({
        ref: c.ref as string,
        part: def.name,
        footprint: def.footprint,
        pins: def.pins.length,
        ...(def.datasheet_url ? { datasheet_url: def.datasheet_url } : {}),
        ...(def.app_notes.length > 0 ? { app_notes: def.app_notes } : {}),
      });
      return def;
    }),
  );

  const components: SchematicComponent[] = componentsInput.map((c, i) => {
    const def = resolvedDefs[i];
    const explicitPins = (c.pins as Array<Record<string, unknown>>) || [];
    const pins: SchematicPin[] =
      explicitPins.length > 0
        ? explicitPins.map((p, j) => ({
            number: p.number as string,
            name: p.name as string,
            pin_type: validatePinType(
              p.type,
              `${(c.ref as string) ?? "?"}.${(p.number as string) ?? "?"}`,
            ),
            position: {
              x: coordOpt(p.x, `components[${i}].pins[${j}].x`),
              y: coordOpt(p.y, `components[${i}].pins[${j}].y`),
            },
          }))
        : def
          ? def.pins.map((p) => ({
              number: p.number,
              name: p.name,
              pin_type: p.pin_type as SchematicPin["pin_type"],
              position: { x: p.x, y: p.y },
            }))
          : !c.part && isTwoPinPassiveFootprint(c.footprint)
            ? synthesizedPassivePins()
            : [];

    // Carry the resolved part identity + datasheet for traceability.
    const properties: Record<string, string> = {};
    if (c.part) properties.part = c.part as string;
    if (def?.datasheet_url) properties.datasheet = def.datasheet_url;

    return {
      ref: c.ref as string,
      value: (c.value as string) || (c.part as string) || def?.name || "",
      footprintId: c.footprint as string,
      position: {
        x: coord(c.x, `components[${i}].x`),
        y: coord(c.y, `components[${i}].y`),
      },
      rotation: coordOpt(c.rotation, `components[${i}].rotation`),
      pins,
      ...(Object.keys(properties).length > 0 ? { properties } : {}),
      // Inline-pad escape hatch: explicit footprint geometry that bypasses the
      // parametric engine. net/layers are (re)assigned by place_components.
      ...(Array.isArray(c.pads) && c.pads.length > 0
        ? {
            pads: (c.pads as Array<Record<string, unknown>>).map((p) => ({
              number: p.number as string,
              padType: (p.padType as PadType) || "SMD",
              shape: p.shape as Pad["shape"],
              position: (p.position as { x: number; y: number }) || { x: 0, y: 0 },
              rotation: (p.rotation as number) || 0,
              drill: (p.drill as Pad["drill"]) || undefined,
              layers: ["FCu", "FPaste", "FMask"] as PcbLayer[],
            })),
          }
        : {}),
    };
  });

  // `pins` is now optional, so flag any component that ended up with none and
  // wasn't a (separately-warned) unresolved `part` — usually a forgotten pin
  // list on a passive. A pinless component connects to nothing silently.
  componentsInput.forEach((c, i) => {
    const hadExplicitPins = ((c.pins as unknown[]) || []).length > 0;
    if (!hadExplicitPins && !c.part && components[i].pins.length === 0) {
      warnings.push(
        `Component ${(c.ref as string) ?? "?"} has no pins — give it a \`pins\` ` +
          "array or a `part` name from the database.",
      );
    }
  });

  const wires: SchematicWire[] = wiresInput.map((w, i) => ({
    start: { x: coord(w.x1, `wires[${i}].x1`), y: coord(w.y1, `wires[${i}].y1`) },
    end: { x: coord(w.x2, `wires[${i}].x2`), y: coord(w.y2, `wires[${i}].y2`) },
  }));

  const labels: SchematicLabel[] = labelsInput.map((l, i) => ({
    name: l.name as string,
    position: { x: coord(l.x, `labels[${i}].x`), y: coord(l.y, `labels[${i}].y`) },
    scope: ((l.scope as string) || "Global") as SchematicLabel["scope"],
  }));

  const schematic: SchematicSheet = {
    title,
    components,
    wires,
    junctions: [],
    labels,
    ...(netsInput ? { nets: netsInput } : {}),
  };

  // Validate the explicit netlist eagerly so bad pin refs fail this call,
  // not place_components three steps later.
  if (netsInput) validateExplicitNets(schematic, netsInput);

  // A label only joins a net when it sits exactly on a pin or wire endpoint
  // (within tolerance). A label that touches nothing silently names nothing —
  // the classic way netlists break — so say it out loud.
  for (const label of labels) {
    const touchesWire = wires.some(
      (w) =>
        (Math.abs(w.start.x - label.position.x) < POSITION_TOLERANCE &&
          Math.abs(w.start.y - label.position.y) < POSITION_TOLERANCE) ||
        (Math.abs(w.end.x - label.position.x) < POSITION_TOLERANCE &&
          Math.abs(w.end.y - label.position.y) < POSITION_TOLERANCE),
    );
    const touchesPin = components.some((comp) =>
      comp.pins.some((pin) => {
        const world = pinWorldPosition(comp, pin);
        return (
          Math.abs(world.x - label.position.x) < POSITION_TOLERANCE &&
          Math.abs(world.y - label.position.y) < POSITION_TOLERANCE
        );
      }),
    );
    if (!touchesWire && !touchesPin) {
      warnings.push(
        `Label "${label.name}" at (${label.position.x}, ${label.position.y}) doesn't touch any pin or wire endpoint — it names nothing. Move it onto a pin/wire, or declare the net in \`nets\` instead.`,
      );
    }
  }

  const doc = createDocument();
  doc.schematic = schematic;
  const documentId = registerSession(doc);

  // Resolve connectivity now and echo it back, so a broken netlist is
  // visible immediately instead of after placement.
  const derived = await deriveNets(schematic);
  warnings.push(...derived.warnings);
  const netsPreview: Record<string, string[]> = {};
  for (const [name, pins] of derived.nets) netsPreview[name] = pins;

  const connectedPins = new Set(derived.netByPin.keys());
  // Datasheet app-notes by ref, so an unconnected pin on a known part can carry
  // the relevant handling guidance instead of a bare reference.
  const appNotesByRef = new Map<string, string[]>();
  for (const rp of resolvedParts) {
    if (rp.app_notes && rp.app_notes.length > 0) {
      appNotesByRef.set(rp.ref, rp.app_notes);
    }
  }
  const unconnected: UnconnectedPin[] = [];
  for (const comp of components) {
    const partNotes = appNotesByRef.get(comp.ref);
    for (const pin of comp.pins) {
      if (pin.pin_type === "NotConnected") continue;
      if (connectedPins.has(pinKey(comp.ref, pin.number))) continue;
      const entry: UnconnectedPin = {
        ref: `${comp.ref}.${pin.number}`,
        pin_name: pin.name,
        pin_type: pin.pin_type,
        severity: unconnectedPinSeverity(pin.pin_type),
      };
      if (partNotes) {
        const hints = appNotesForPin(partNotes, pin);
        if (hints.length > 0) entry.app_notes = hints;
      }
      unconnected.push(entry);
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          document_id: documentId,
          components: components.length,
          wires: wires.length,
          labels: labels.length,
          nets: netsPreview,
          ...(resolvedParts.length > 0 ? { resolved_parts: resolvedParts } : {}),
          ...(unconnected.length > 0 ? { unconnected_pins: unconnected } : {}),
          ...(warnings.length > 0 ? { warnings } : {}),
        }),
      },
    ],
  };
}

/**
 * Refine grid positions with a deterministic force-directed pass:
 * components sharing a net attract, overlapping components repel, and
 * everything stays clamped inside the board margins.
 */
function forceDirectedRefine(
  components: SchematicComponent[],
  positions: Vec2[],
  netByPin: Map<string, string>,
  useConnectivity: boolean,
  extents: number[],
  bounds: { minX: number; minY: number; maxX: number; maxY: number },
): void {
  // net id → component indices on that net
  const netMembers = new Map<string, number[]>();
  components.forEach((comp, i) => {
    const seen = new Set<string>();
    for (const pin of comp.pins) {
      const netId = useConnectivity
        ? netByPin.get(pinKey(comp.ref, pin.number))
        : pin.name && pin.name !== "~"
          ? pin.name
          : undefined;
      if (!netId || seen.has(netId)) continue;
      seen.add(netId);
      const members = netMembers.get(netId) || [];
      members.push(i);
      netMembers.set(netId, members);
    }
  });

  const iterations = 120;
  const attract = 0.04;
  const gap = 0.6; // edge-to-edge breathing room between component courtyards

  for (let it = 0; it < iterations; it++) {
    const forces: Vec2[] = positions.map(() => ({ x: 0, y: 0 }));

    // Attraction between components sharing a net.
    for (const members of netMembers.values()) {
      for (let a = 0; a < members.length; a++) {
        for (let b = a + 1; b < members.length; b++) {
          const i = members[a];
          const j = members[b];
          const dx = positions[j].x - positions[i].x;
          const dy = positions[j].y - positions[i].y;
          forces[i].x += attract * dx;
          forces[i].y += attract * dy;
          forces[j].x -= attract * dx;
          forces[j].y -= attract * dy;
        }
      }
    }

    // Repulsion when component courtyards (extent radii) would collide —
    // size-aware, so a DPAK or bulk cap claims more room than an 0402.
    for (let i = 0; i < positions.length; i++) {
      for (let j = i + 1; j < positions.length; j++) {
        const dx = positions[j].x - positions[i].x;
        const dy = positions[j].y - positions[i].y;
        const dist = Math.max(Math.hypot(dx, dy), 0.1);
        const minSep = extents[i] + extents[j] + gap;
        if (dist >= minSep) continue;
        const push = (0.5 * (minSep - dist)) / dist;
        forces[i].x -= push * dx;
        forces[i].y -= push * dy;
        forces[j].x += push * dx;
        forces[j].y += push * dy;
      }
    }

    for (let i = 0; i < positions.length; i++) {
      // Clamp the component's *courtyard* inside the board, not just its center:
      // inset each axis by the component's half-extent (reusing the size-aware
      // `extents` the repulsion pass above already computes) so a large part —
      // e.g. an LQFP-64 — pushed against an edge lands fully on-board instead of
      // hanging half off and overlapping its neighbors. If a part is wider than
      // the available inset span (range inverts), fall back to the plain bounds
      // clamp — the cross-net legalizer below handles and reports the genuinely
      // too-tight board rather than stacking every part on the midpoint.
      const ext = extents[i];
      const loX = bounds.minX + ext;
      const hiX = bounds.maxX - ext;
      const loY = bounds.minY + ext;
      const hiY = bounds.maxY - ext;
      const nx = positions[i].x + forces[i].x;
      const ny = positions[i].y + forces[i].y;
      positions[i].x =
        hiX >= loX ? Math.min(hiX, Math.max(loX, nx)) : Math.min(bounds.maxX, Math.max(bounds.minX, nx));
      positions[i].y =
        hiY >= loY ? Math.min(hiY, Math.max(loY, ny)) : Math.min(bounds.maxY, Math.max(bounds.minY, ny));
    }
  }
}

/** A pad's copper, in component-local coordinates, for clearance legalization. */
interface LocalPad {
  x: number;
  y: number;
  /** Bounding-circle radius (over-approximates the copper, never misses an overlap). */
  r: number;
  net?: string;
}

/**
 * Hard-separate components whose cross-net pads violate copper clearance.
 *
 * The force-directed pass balances net-attraction against courtyard repulsion,
 * but that equilibrium can still settle with two components' pads — on
 * *different* nets — overlapping, baking a short into the board before any
 * routing (e.g. a VCC pad stacked on a GND pad). This pass mirrors the DRC
 * pad-clearance rule (different-net pads must clear; same-net or unnetted pads
 * may touch) and shoves the offending components apart along their
 * center-to-center axis until every cross-net pad pair clears — or a pass cap
 * is hit when the board is simply too tight. Each pad is modeled as its
 * bounding circle (radius ≥ the real copper), so clearing the circles
 * guarantees the true rectangular copper clears too.
 *
 * Returns the component pairs it could *not* separate, so the caller can refuse
 * to report success on a board that still contains a short.
 */
function legalizeCrossNetClearance(
  components: SchematicComponent[],
  positions: Vec2[],
  padLayout: LocalPad[][],
  clearance: number,
  bounds: { minX: number; minY: number; maxX: number; maxY: number },
): Array<{ a: string; b: string; gap: number }> {
  const n = positions.length;
  const eps = 1e-3;
  // Separate to a hair beyond clearance so the later 0.01mm position rounding
  // (footprint build) can't nudge a just-cleared pair back under the limit.
  const target = clearance + 0.05;

  // Worst cross-net clearance deficit between components i and j at their
  // current positions (>0 means a cross-net pad pair is closer than `goal`).
  const deficit = (i: number, j: number, goal: number): number => {
    let worst = 0;
    for (const pa of padLayout[i]) {
      if (!pa.net) continue;
      for (const pb of padLayout[j]) {
        if (!pb.net || pa.net === pb.net) continue;
        const ax = positions[i].x + pa.x;
        const ay = positions[i].y + pa.y;
        const bx = positions[j].x + pb.x;
        const by = positions[j].y + pb.y;
        const gap = Math.hypot(ax - bx, ay - by) - pa.r - pb.r;
        const d = goal - gap;
        if (d > worst) worst = d;
      }
    }
    return worst;
  };

  const maxPasses = 400;
  for (let pass = 0; pass < maxPasses; pass++) {
    let moved = false;
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const d = deficit(i, j, target);
        if (d <= eps) continue;
        moved = true;
        let dx = positions[j].x - positions[i].x;
        let dy = positions[j].y - positions[i].y;
        let len = Math.hypot(dx, dy);
        if (len < 1e-6) {
          // Coincident centers — pick a deterministic separation axis (no RNG,
          // so placement stays reproducible) from the index pair.
          const a = (((i + 1) * 73856093) ^ ((j + 1) * 19349663)) % 360;
          dx = Math.cos((a * Math.PI) / 180);
          dy = Math.sin((a * Math.PI) / 180);
          len = 1;
        }
        const ux = dx / len;
        const uy = dy / len;
        const half = d / 2 + eps;
        positions[i].x = Math.min(bounds.maxX, Math.max(bounds.minX, positions[i].x - ux * half));
        positions[i].y = Math.min(bounds.maxY, Math.max(bounds.minY, positions[i].y - uy * half));
        positions[j].x = Math.min(bounds.maxX, Math.max(bounds.minX, positions[j].x + ux * half));
        positions[j].y = Math.min(bounds.maxY, Math.max(bounds.minY, positions[j].y + uy * half));
      }
    }
    if (!moved) break;
  }

  // Report pairs that still breach the real clearance after the cap — the board
  // couldn't fit them (e.g. too small, or clamped into the same corner).
  const remaining: Array<{ a: string; b: string; gap: number }> = [];
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      const d = deficit(i, j, clearance);
      if (d > eps) {
        remaining.push({
          a: components[i].ref,
          b: components[j].ref,
          gap: Math.round((clearance - d) * 1000) / 1000,
        });
      }
    }
  }
  return remaining;
}

/** Return the polygon wound counter-clockwise (kernel extrusion convention). */
function ensureCcw(poly: Vec2[]): Vec2[] {
  return loopSignedArea(poly) < 0 ? [...poly].reverse() : poly;
}

/** Regular polygon approximation of a circle, counter-clockwise. */
function circlePolygon(center: Vec2, radius: number, segments: number): Vec2[] {
  const pts: Vec2[] = [];
  for (let i = 0; i < segments; i++) {
    const a = (i / segments) * 2 * Math.PI;
    pts.push({
      x: Math.round((center.x + radius * Math.cos(a)) * 1000) / 1000,
      y: Math.round((center.y + radius * Math.sin(a)) * 1000) / 1000,
    });
  }
  return pts;
}

/** Place components on a PCB from schematic data. */
export async function placeComponents(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const doc = ctx.doc;
  const boardThickness = (args.board_thickness as number) || 1.6;

  if (!doc.schematic) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no schematic" }],
      isError: true,
    };
  }

  // Derive pad nets from schematic connectivity (wires + junctions + labels
  // + the explicit `nets` map) — see deriveNets. Pin names only serve as net
  // ids as a fallback when there's no connectivity at all.
  const derived = await deriveNets(doc.schematic);
  const netByPin = derived.netByPin;
  const useConnectivity = netByPin.size > 0;
  const warnings: string[] = [...derived.warnings];

  // Resolve the board outline: explicit polygon > circle shorthand > rectangle.
  const outlineArg = args.outline as
    | { vertices: Vec2[]; cutouts?: Vec2[][] }
    | undefined;
  const shapeArg = args.board_shape as
    | {
        type?: string;
        outer_diameter?: number;
        inner_diameter?: number;
        center?: Vec2;
        segments?: number;
      }
    | undefined;
  const boardWidth = args.board_width as number | undefined;
  const boardHeight = args.board_height as number | undefined;

  let vertices: Vec2[];
  let cutouts: Vec2[][] | undefined;
  // Circle metadata for radial placement defaults.
  let circleCenter: Vec2 | undefined;
  let circleOuterR: number | undefined;
  let circleInnerR = 0;

  if (outlineArg) {
    if (!Array.isArray(outlineArg.vertices) || outlineArg.vertices.length < 3) {
      return {
        content: [{ type: "text" as const, text: "Error: outline.vertices needs at least 3 points" }],
        isError: true,
      };
    }
    vertices = outlineArg.vertices;
    cutouts = outlineArg.cutouts;
  } else if (shapeArg) {
    const od = shapeArg.outer_diameter;
    if (!od || od <= 0) {
      return {
        content: [{ type: "text" as const, text: "Error: board_shape.outer_diameter must be > 0" }],
        isError: true,
      };
    }
    const id = shapeArg.inner_diameter ?? 0;
    if (id < 0 || id >= od) {
      return {
        content: [
          { type: "text" as const, text: "Error: board_shape.inner_diameter must be >= 0 and < outer_diameter" },
        ],
        isError: true,
      };
    }
    const segments = Math.max(16, Math.round(shapeArg.segments ?? 64));
    circleCenter = shapeArg.center ?? { x: od / 2, y: od / 2 };
    circleOuterR = od / 2;
    circleInnerR = id / 2;
    vertices = circlePolygon(circleCenter, circleOuterR, segments);
    cutouts = id > 0 ? [circlePolygon(circleCenter, circleInnerR, segments)] : undefined;
  } else if (boardWidth && boardHeight) {
    vertices = [
      { x: 0, y: 0 },
      { x: boardWidth, y: 0 },
      { x: boardWidth, y: boardHeight },
      { x: 0, y: boardHeight },
    ];
  } else {
    return {
      content: [
        {
          type: "text" as const,
          text:
            "Error: specify the board outline — board_width + board_height (rectangle), " +
            "board_shape ({type:'circle', outer_diameter, inner_diameter?}), or " +
            "outline ({vertices, cutouts?}, e.g. from board_from_solid)",
        },
      ],
      isError: true,
    };
  }

  // Normalize winding to CCW — the kernel extruder expects it, and
  // agent-supplied polygons arrive in either orientation.
  vertices = ensureCcw(vertices);
  cutouts = cutouts?.map(ensureCcw);

  const outline: BoardOutline = {
    vertices,
    ...(cutouts && cutouts.length > 0 ? { cutouts } : {}),
    thickness: boardThickness,
  };

  // Placement bounds from the outline's bounding box.
  const bboxMinX = Math.min(...vertices.map((v) => v.x));
  const bboxMaxX = Math.max(...vertices.map((v) => v.x));
  const bboxMinY = Math.min(...vertices.map((v) => v.y));
  const bboxMaxY = Math.max(...vertices.map((v) => v.y));
  const extentW = bboxMaxX - bboxMinX;
  const extentH = bboxMaxY - bboxMinY;

  const stackup: LayerStackup = {
    layers: [
      { layer: "FCu", copperThickness: 0.035, dielectricThickness: 1.53, dielectricEr: 4.5, material: "FR4" },
      { layer: "BCu", copperThickness: 0.035 },
    ],
  };

  const rules: DesignRules = defaultDesignRules();
  const defaultRules = rules.defaultRules;

  // Grid placement sized to the outline's bounding box: split the usable
  // area into cells so every component lands inside it regardless of size.
  const components = doc.schematic.components;
  // Default to the deterministic grid (stable, good for sparse boards).
  // `force_directed` adds net-attraction + size-aware repulsion — better for
  // dense, multi-cluster boards (a power stage + MCU + connectors); `radial`
  // rings parts for annular boards.
  const strategy = (args.strategy as string) || "grid";
  const margin = Math.min(5, extentW / 8, extentH / 8);
  const usableW = extentW - 2 * margin;
  const usableH = extentH - 2 * margin;
  if (usableW <= 0 || usableH <= 0) {
    return {
      content: [{ type: "text" as const, text: "Error: Board too small for placement" }],
      isError: true,
    };
  }

  const n = components.length;
  const cols = Math.max(1, Math.round(Math.sqrt((n * usableW) / usableH)));
  const rows = Math.max(1, Math.ceil(n / cols));
  const cellW = usableW / cols;
  const cellH = usableH / Math.max(rows, 1);

  if (n > 1 && strategy !== "radial" && Math.min(cellW, cellH) < 4) {
    warnings.push(
      `Placement cells are ${Math.min(cellW, cellH).toFixed(1)}mm — components may overlap; consider a larger board`,
    );
  }

  // Resolve pad geometry up-front (precedence: inline pads > parametric engine
  // > generic placeholder) so the placer can size each component's keep-out
  // from its real footprint — and so resolution happens once for both
  // placement and the footprint build below.
  const resolutions = await Promise.all(
    components.map((c) =>
      c.pads && c.pads.length > 0
        ? Promise.resolve(null)
        : resolveFootprint(c.footprintId, c.pins.length),
    ),
  );
  const halfExtent = (shape: Pad["shape"]): number => {
    switch (shape.type) {
      case "Circle":
        return shape.diameter / 2;
      case "Rect":
      case "Oval":
      case "RoundRect":
        return Math.max(shape.width, shape.height) / 2;
      default:
        return 0.5; // Custom polygon — small default
    }
  };
  // Bounding-circle radius of a pad's copper (half-diagonal for rects) — used by
  // cross-net clearance legalization, where the circle must *contain* the copper
  // so that separating circles guarantees the real pads can't short.
  const padRadius = (shape: Pad["shape"]): number => {
    switch (shape.type) {
      case "Circle":
        return shape.diameter / 2;
      case "Rect":
      case "Oval":
      case "RoundRect":
        return Math.hypot(shape.width, shape.height) / 2;
      default:
        return 0.5; // Custom polygon — small default
    }
  };
  const componentExtent = (pads: Pad[]): number => {
    let r = 0.8; // floor so a 1-pad part still claims some room
    for (const p of pads) {
      const h = halfExtent(p.shape);
      r = Math.max(r, Math.abs(p.position.x) + h, Math.abs(p.position.y) + h);
    }
    return r;
  };
  const extents = components.map((c, i) => {
    if (c.pads && c.pads.length > 0) return componentExtent(c.pads);
    const t = resolutions[i]?.template;
    return t ? componentExtent(t.pads) : 1.0;
  });

  // Pad copper (local position + bounding radius + net) per component, mirroring
  // the same precedence the footprint build below uses (inline > engine template
  // > generic spread). Feeds cross-net clearance legalization. Net resolution
  // matches netIdForPin but is side-effect free (doesn't populate the net list).
  const padNetOf = (comp: SchematicComponent, padNumber: string): string | undefined => {
    const pin = comp.pins.find((p) => p.number === padNumber);
    if (!pin) return undefined;
    return useConnectivity
      ? netByPin.get(pinKey(comp.ref, pin.number))
      : pin.name && pin.name !== "~"
        ? pin.name
        : undefined;
  };
  const padLayout: LocalPad[][] = components.map((comp, i) => {
    if (comp.pads && comp.pads.length > 0) {
      return comp.pads.map((p) => ({
        x: p.position.x,
        y: p.position.y,
        r: padRadius(p.shape),
        net: padNetOf(comp, p.number),
      }));
    }
    const t = resolutions[i]?.template;
    if (t) {
      return t.pads.map((p) => ({
        x: p.position.x,
        y: p.position.y,
        r: padRadius(p.shape),
        net: padNetOf(comp, p.number),
      }));
    }
    // Generic fallback — must match the spread used in the footprint build.
    return comp.pins.map((pin, pi) => ({
      x: (pi - (comp.pins.length - 1) / 2) * 2.54,
      y: 0,
      r: padRadius({ type: "Rect", width: 1.0, height: 1.2 }),
      net: padNetOf(comp, pin.number),
    }));
  });

  // Cross-net pad pairs the force-directed legalizer could not separate (board
  // too tight). Non-empty means a short is baked in → the placer reports failure
  // rather than silently shipping it.
  let placementConflicts: Array<{ a: string; b: string; gap: number }> = [];

  let positions: Vec2[];
  if (strategy === "radial") {
    // Even angular spacing on a ring — the natural layout for annular
    // boards (motor stators, LED rings). Components keep input order.
    const center = circleCenter ?? {
      x: (bboxMinX + bboxMaxX) / 2,
      y: (bboxMinY + bboxMaxY) / 2,
    };
    const defaultRadius =
      circleOuterR !== undefined
        ? (circleInnerR + circleOuterR) / 2
        : 0.35 * Math.min(extentW, extentH);
    const radius = (args.radial_radius as number) || defaultRadius;
    const startAngle = (((args.radial_start_angle_deg as number) || 0) * Math.PI) / 180;
    positions = components.map((_, i) => {
      const a = startAngle + (i / Math.max(n, 1)) * 2 * Math.PI;
      return {
        x: center.x + radius * Math.cos(a),
        y: center.y + radius * Math.sin(a),
      };
    });
    if (n > 1) {
      const arcSep = (2 * Math.PI * radius) / n;
      if (arcSep < 4) {
        warnings.push(
          `Radial spacing is ${arcSep.toFixed(1)}mm between components — they may overlap; increase radial_radius or the board size`,
        );
      }
    }
  } else {
    positions = components.map((_, i) => ({
      x: bboxMinX + margin + ((i % cols) + 0.5) * cellW,
      y: bboxMinY + margin + (Math.floor(i / cols) + 0.5) * cellH,
    }));

    if (strategy === "force_directed") {
      const fdBounds = {
        minX: bboxMinX + margin,
        minY: bboxMinY + margin,
        maxX: bboxMaxX - margin,
        maxY: bboxMaxY - margin,
      };
      forceDirectedRefine(components, positions, netByPin, useConnectivity, extents, fdBounds);
      // The force-directed equilibrium can leave different-net pads overlapping
      // (a short). Shove them apart to clearance before the board is built; any
      // pair that can't be separated on this board is surfaced to the caller.
      placementConflicts = legalizeCrossNetClearance(
        components,
        positions,
        padLayout,
        defaultRules.clearance,
        fdBounds,
      );
    }
  }

  const nets: Net[] = [];
  const netSet = new Set<string>();

  const netIdForPin = (comp: SchematicComponent, pin: SchematicPin): string | undefined => {
    // Net from schematic connectivity; pin-name fallback for wireless docs.
    const netId = useConnectivity
      ? netByPin.get(pinKey(comp.ref, pin.number))
      : pin.name && pin.name !== "~"
        ? pin.name
        : undefined;
    if (netId && !netSet.has(netId)) {
      netSet.add(netId);
      nets.push({ id: netId, name: netId });
    }
    return netId;
  };

  // (Footprints were resolved up-front, before placement — see `resolutions`.)
  // Components whose footprint id did NOT resolve to a real package family —
  // surfaced to the caller instead of silently substituting wrong geometry.
  const fallbackFootprints: Array<{ ref: string; footprint: string; reason: string }> = [];

  const applyNet = (comp: SchematicComponent, pad: Pad): Pad => {
    const pin = comp.pins.find((p) => p.number === pad.number);
    const netId = pin ? netIdForPin(comp, pin) : undefined;
    // Normalize copper/mask/paste layers by mounting tech, but honor a
    // deliberate no-paste choice on SMD pads (bare-copper spring-pin / test
    // pads like Tag-Connect) so they don't get a solder-stencil aperture.
    const layers: PcbLayer[] =
      pad.padType === "THT"
        ? ["FCu", "BCu", "FMask", "BMask"]
        : (pad.layers?.includes("FPaste") ?? true)
          ? ["FCu", "FPaste", "FMask"]
          : ["FCu", "FMask"];
    return { ...pad, net: netId, layers };
  };

  const footprints: Footprint[] = components.map((comp, i) => {
    const x = Math.round(positions[i].x * 100) / 100;
    const y = Math.round(positions[i].y * 100) / 100;
    const resolution = resolutions[i];

    let pads: Pad[];
    let graphics: NonNullable<Footprint["graphics"]> = [];
    // Provenance marker the DRC engine reads to tag violations: "inline" pads
    // are author-supplied, "generated" land patterns are synthesized by the
    // parametric engine (and so are candidate footprint artifacts, not faults).
    let padSource: "inline" | "generated";

    if (comp.pads && comp.pads.length > 0) {
      // (1) Inline override — author-supplied geometry.
      pads = comp.pads.map((pad) => applyNet(comp, pad));
      padSource = "inline";
    } else if (resolution?.template) {
      // (2) Engine result — real family match or compact placeholder.
      pads = resolution.template.pads.map((pad) => applyNet(comp, pad));
      graphics = resolution.template.graphics;
      padSource = "generated";
      if (!resolution.matched) {
        fallbackFootprints.push({
          ref: comp.ref,
          footprint: comp.footprintId,
          reason: resolution.note,
        });
      }
    } else {
      // (3) Kernel unavailable / nothing to synthesize — spread generic pads.
      pads = comp.pins.map((pin, pi) => ({
        number: pin.number,
        padType: "SMD" as PadType,
        shape: { type: "Rect" as const, width: 1.0, height: 1.2 },
        position: { x: (pi - (comp.pins.length - 1) / 2) * 2.54, y: 0 },
        net: netIdForPin(comp, pin),
        layers: ["FCu", "FPaste", "FMask"] as PcbLayer[],
      }));
      padSource = "generated";
      fallbackFootprints.push({
        ref: comp.ref,
        footprint: comp.footprintId,
        reason: resolution?.note ?? "footprint kernel unavailable",
      });
    }

    return {
      ref: comp.ref,
      value: comp.value,
      footprintName: comp.footprintId,
      position: { x, y },
      rotation: 0,
      front: true,
      pads,
      ...(graphics.length > 0 ? { graphics } : {}),
      properties: { padSource },
    };
  });

  // Replay any design rules the agent set before the board existed (buffered by
  // set_design_rules). Applied here — once `nets` is populated — so net-class
  // assignments validate against the real netlist. Malformed buffered args are
  // surfaced as a warning rather than failing placement.
  let bufferedRulesApplied = false;
  const pending = (doc as DocWithPendingRules).__pendingDesignRules;
  if (pending) {
    const applied = applyDesignRuleArgs(rules, new Set(nets.map((nn) => nn.id)), pending);
    if (applied.error) {
      warnings.push(`buffered design rules ignored: ${applied.error}`);
    } else {
      bufferedRulesApplied = applied.touched;
      warnings.push(...applied.warnings.map((w) => `design rule: ${w}`));
    }
    delete (doc as DocWithPendingRules).__pendingDesignRules;
  }

  const pcb: Pcb = {
    outline,
    stackup,
    nets,
    rules,
    footprints,
    traces: [],
    vias: [],
    zones: [],
  };

  // Create (or replace) the PcbBoard DAG node instead of legacy doc.pcb.
  // Re-running place_components on a session re-lays-out the same board
  // rather than stacking a second one.
  const existingPcbIds = getPcbNodeIds(doc);
  if (existingPcbIds.length > 0) {
    const nid = existingPcbIds[0]!;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    doc.nodes[String(nid)]!.op = { type: "PcbBoard", board: pcb } as any;
    warnings.push("Document already had a PCB — its board was replaced");
  } else {
    const existingIds = Object.keys(doc.nodes).map(Number);
    const nid = existingIds.length > 0 ? Math.max(...existingIds) + 1 : 1;
    doc.nodes[String(nid)] = {
      id: nid,
      name: "PCB Board",
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      op: { type: "PcbBoard", board: pcb } as any,
    };
    doc.roots.push({ root: nid, material: "__pcb_fr4__" });
  }
  if (!doc.materials["__pcb_fr4__"]) {
    doc.materials["__pcb_fr4__"] = {
      name: "__pcb_fr4__",
      color: [0.05, 0.35, 0.15],
      roughness: 0.6,
      metallic: 0.0,
    };
  }

  const netsSummary: Record<string, string[]> = {};
  for (const [name, pins] of derived.nets) netsSummary[name] = pins;

  // Surface the pre-routing DRC subset (shorts, pad clearance, courtyard
  // overlaps, off-board parts) so the caller can fix the floorplan with
  // set_placement before routing on top of a fault — instead of only finding
  // out at run_drc, three steps later.
  const placementDrc = await summarizePlacementDrc(pcb);
  if (placementDrc.unverifiable) {
    const u = placementDrc.unverifiable;
    warnings.push(
      `placement could NOT be verified — the kernel rejected the board (${u.reason}` +
        `${u.offending_field ? `, offending field '${u.offending_field}'` : ""}); ` +
        `fix it before relying on DRC`,
    );
  } else if (!placementDrc.clean) {
    const parts: string[] = [];
    if (placementDrc.shorts.length > 0) parts.push(`${placementDrc.shorts.length} short(s)`);
    if (placementDrc.clearance_violations > 0)
      parts.push(`${placementDrc.clearance_violations} clearance`);
    if (placementDrc.courtyard_overlaps > 0)
      parts.push(`${placementDrc.courtyard_overlaps} courtyard overlap(s)`);
    if (placementDrc.off_board.length > 0)
      parts.push(`${placementDrc.off_board.length} off-board`);
    warnings.push(
      `placement DRC found ${parts.join(", ")} — see placement_drc; fix with set_placement before routing`,
    );
  }
  if (placementDrc.layout_lint && placementDrc.layout_lint.length > 0) {
    warnings.push(
      `${placementDrc.layout_lint.length} EE layout lint warning(s) — see placement_drc.layout_lint ` +
        `(crystal/decoupling/connector/separation heuristics)`,
    );
  }

  // A cross-net pad overlap is a hard short — never report success with one.
  if (placementConflicts.length > 0) {
    const pairs = placementConflicts.map((c) => `${c.a}/${c.b}`).join(", ");
    warnings.push(
      `Cross-net pad overlap couldn't be resolved on this board (${pairs}) — a short ` +
        `before any routing. Enlarge the board, or floorplan these parts with set_placement.`,
    );
  }

  // --- Board utilization + suggested outline (advisory) -----------------
  // Force-directed/grid placement can leave a generous board mostly empty
  // copper. Report how much area the parts actually occupy, plus the tightest
  // same-shape outline that still holds them, so cost-sensitive callers (and
  // agents) can right-size in one step instead of trial-and-error. Strictly
  // advisory — the caller decides whether to act on it.
  const utilization = computeUtilization(
    footprints,
    vertices,
    cutouts,
    outlineArg ? "polygon" : shapeArg ? "circle" : "rect",
    circleInnerR,
    args.edge_margin as number | undefined,
    rules.edgeClearance,
  );

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: placementConflicts.length === 0,
          footprints_placed: footprints.length,
          footprints_resolved: footprints.length - fallbackFootprints.length,
          strategy,
          placement_drc: placementDrc,
          ...(warnings.length > 0 ? { warnings } : {}),
          // Cross-net pad pairs the placer could not separate — each is a short
          // baked into the layout. `gap` is the residual copper-to-copper
          // distance (mm); below the design clearance (0.2mm) it will fail DRC.
          ...(placementConflicts.length > 0
            ? { placement_conflicts: placementConflicts }
            : {}),
          // Footprint ids that did NOT resolve to a real package family — these
          // got a generic placeholder, so their pads are approximate. Supply a
          // recognized KiCad id or inline `pads` to fix.
          ...(fallbackFootprints.length > 0
            ? { fallback_footprints: fallbackFootprints }
            : {}),
          board: {
            width: extentW,
            height: extentH,
            thickness: boardThickness,
            shape: outlineArg ? "polygon" : shapeArg ? "circle" : "rect",
            ...(cutouts && cutouts.length > 0 ? { cutouts: cutouts.length } : {}),
          },
          ...(utilization ? { utilization } : {}),
          // Design rules set before the board existed were replayed onto it.
          ...(bufferedRulesApplied ? { buffered_design_rules_applied: true } : {}),
          nets: netsSummary,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/**
 * Axis-aligned half-extents (mm) of a single pad in footprint-local coords.
 * Mirrors the placer's keep-out sizing but keeps x/y separate for a tight
 * bounding box. Pad rotation is ignored (consistent with the placer).
 */
function padHalfExtents(shape: Pad["shape"]): { hx: number; hy: number } {
  switch (shape.type) {
    case "Circle":
      return { hx: shape.diameter / 2, hy: shape.diameter / 2 };
    case "Rect":
    case "Oval":
    case "RoundRect":
      return { hx: shape.width / 2, hy: shape.height / 2 };
    case "Custom": {
      let hx = 0;
      let hy = 0;
      for (const v of shape.vertices) {
        hx = Math.max(hx, Math.abs(v.x));
        hy = Math.max(hy, Math.abs(v.y));
      }
      return { hx: hx || 0.5, hy: hy || 0.5 };
    }
    default:
      return { hx: 0.5, hy: 0.5 };
  }
}

/**
 * Board-area utilization plus an advisory right-sized outline, from the final
 * placed footprints. Component area is courtyard-approximate — summed pad
 * bounding boxes, the geometry we reliably have for every footprint (inline,
 * engine-resolved, or generic placeholder). The suggested outline keeps the
 * board's current shape and honors `edge_margin`. Returns undefined when
 * there's nothing to measure (no pads, or a degenerate board area).
 */
function computeUtilization(
  footprints: Footprint[],
  vertices: Vec2[],
  cutouts: Vec2[][] | undefined,
  shape: "polygon" | "circle" | "rect",
  innerRadius: number,
  edgeMarginArg: number | undefined,
  edgeClearance: number,
):
  | {
      board_area_mm2: number;
      component_area_mm2: number;
      utilization_pct: number;
      bounding_box: { x: number; y: number; w: number; h: number };
      suggested_outline: Record<string, unknown>;
    }
  | undefined {
  // World-space AABB of every placed footprint; sum of the boxes is the
  // occupied (courtyard-approximate) area.
  let occMinX = Infinity;
  let occMinY = Infinity;
  let occMaxX = -Infinity;
  let occMaxY = -Infinity;
  let componentArea = 0;
  for (const fp of footprints) {
    let lMinX = Infinity;
    let lMinY = Infinity;
    let lMaxX = -Infinity;
    let lMaxY = -Infinity;
    for (const pad of fp.pads) {
      const { hx, hy } = padHalfExtents(pad.shape);
      lMinX = Math.min(lMinX, pad.position.x - hx);
      lMaxX = Math.max(lMaxX, pad.position.x + hx);
      lMinY = Math.min(lMinY, pad.position.y - hy);
      lMaxY = Math.max(lMaxY, pad.position.y + hy);
    }
    if (!Number.isFinite(lMinX)) continue; // padless footprint — skip
    componentArea += (lMaxX - lMinX) * (lMaxY - lMinY);
    occMinX = Math.min(occMinX, fp.position.x + lMinX);
    occMaxX = Math.max(occMaxX, fp.position.x + lMaxX);
    occMinY = Math.min(occMinY, fp.position.y + lMinY);
    occMaxY = Math.max(occMaxY, fp.position.y + lMaxY);
  }

  // Usable board area = outer polygon minus cutouts (shoelace, sign-agnostic).
  const boardArea =
    Math.abs(loopSignedArea(vertices)) -
    (cutouts ?? []).reduce((s, c) => s + Math.abs(loopSignedArea(c)), 0);

  if (!Number.isFinite(occMinX) || boardArea <= 0) return undefined;

  const round2 = (v: number) => Math.round(v * 100) / 100;
  const ceilHalf = (v: number) => Math.ceil(v * 2) / 2; // round up to 0.5mm
  const bbW = occMaxX - occMinX;
  const bbH = occMaxY - occMinY;
  // Default keeps the suggestion DRC-safe: never tighter than the board's own
  // edge clearance, and at least 2mm so a fab edge router has room.
  const margin = Math.max(0, edgeMarginArg ?? Math.max(edgeClearance, 2));

  // Suggest the board's current shape — that's the in-place right-size.
  let suggested: Record<string, unknown>;
  if (shape === "circle") {
    // Enclosing circle of the component AABBs, recentered on the cluster.
    const cx = (occMinX + occMaxX) / 2;
    const cy = (occMinY + occMaxY) / 2;
    const od = ceilHalf(2 * (Math.hypot(bbW / 2, bbH / 2) + margin));
    suggested = {
      type: "circle",
      outer_diameter: od,
      center: { x: round2(cx), y: round2(cy) },
      ...(innerRadius > 0 ? { inner_diameter: round2(2 * innerRadius) } : {}),
      note: `Minimum enclosing circle with ${margin}mm edge clearance`,
    };
  } else {
    suggested = {
      type: "rect",
      width: ceilHalf(bbW + 2 * margin),
      height: ceilHalf(bbH + 2 * margin),
      origin: { x: round2(occMinX - margin), y: round2(occMinY - margin) },
      note: `Minimum enclosing rectangle with ${margin}mm edge clearance`,
    };
  }

  return {
    board_area_mm2: round2(boardArea),
    component_area_mm2: round2(componentArea),
    utilization_pct: Math.round((componentArea / boardArea) * 1000) / 10,
    bounding_box: { x: round2(occMinX), y: round2(occMinY), w: round2(bbW), h: round2(bbH) },
    suggested_outline: suggested,
  };
}

/** Pad center in board-world coordinates, matching the kernel router's
 *  transform (ratsnest.rs / auto.rs): translate by the footprint origin and
 *  rotate the pad offset by the footprint rotation only. The router lands trace
 *  endpoints exactly on these points, so copper can be matched to pads by
 *  coincidence. */
function padWorld(fp: Footprint, pad: Pad): Vec2 {
  const ang = ((fp.rotation ?? 0) * Math.PI) / 180;
  const cos = Math.cos(ang);
  const sin = Math.sin(ang);
  return {
    x: fp.position.x + pad.position.x * cos - pad.position.y * sin,
    y: fp.position.y + pad.position.x * sin + pad.position.y * cos,
  };
}

/** Find nets whose existing copper is *stale* — left behind by a footprint that
 *  moved (via set_placement) after the net was routed, so the route no longer
 *  matches the board. A net is flagged when either tell-tale shows up:
 *
 *  1. A *loose* trace endpoint: in a clean route every endpoint lands on a pad
 *     of its net, on a via, or on another trace's endpoint (a junction). A
 *     dangling end that anchors to nothing is copper to a pad that moved away.
 *  2. An *uncovered* pad: a current pad with no trace endpoint or via on it.
 *     This catches the case where a transition via sits on the pad's *old*
 *     location and masks the loose end (a both-ends-via'd net), which (1) misses.
 *     Pads that connect through a same-net copper pour are exempt — they
 *     legitimately carry no trace.
 *
 *  Only routable nets (>=2 pads) are considered — the same set route_nets owns.
 *  Free-form copper that isn't a pad-to-pad route (a coil/winding spiral, whose
 *  terminals dangle by design) lives on nets with <2 pads and is left alone.
 *
 *  The router lands endpoints/vias on pad centers to floating-point precision,
 *  so the tight tolerance never trips on a freshly-routed board — a non-empty
 *  result means a pad genuinely moved out from under its copper. */
function detectStaleNets(pcb: Pcb): Set<string> {
  const TOL = 0.05; // mm — far tighter than a pad pitch, far looser than float error
  const near = (a: Vec2, b: Vec2) => Math.abs(a.x - b.x) < TOL && Math.abs(a.y - b.y) < TOL;

  const padsByNet = new Map<string, { pos: Vec2; layers: PcbLayer[]; halfExtent: number }[]>();
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      if (!pad.net) continue;
      const arr = padsByNet.get(pad.net) ?? [];
      // Conservative pad reach: half its largest dimension. A trace endpoint
      // anywhere on the pad body is anchored copper (route-to-tree terminates
      // on pad bodies, not only pad centres).
      const sh = pad.shape as { width?: number; height?: number; diameter?: number };
      // Half-DIAGONAL (bounding circle): a trace overlapping a rect pad's
      // corner sits up to hypot(w,h)/2 from the centre — half the max
      // dimension misses it and flags a covered pad as stale.
      const halfExtent = sh.diameter
        ? sh.diameter / 2
        : Math.hypot(sh.width ?? 0, sh.height ?? 0) / 2;
      arr.push({ pos: padWorld(fp, pad), layers: pad.layers, halfExtent });
      padsByNet.set(pad.net, arr);
    }
  }
  const viasByNet = new Map<string, Vec2[]>();
  for (const v of pcb.vias) {
    const arr = viasByNet.get(v.net) ?? [];
    arr.push(v.position);
    viasByNet.set(v.net, arr);
  }
  const tracesByNet = new Map<string, Trace[]>();
  for (const t of pcb.traces) {
    const arr = tracesByNet.get(t.net) ?? [];
    arr.push(t);
    tracesByNet.set(t.net, arr);
  }
  // Layers each net floods with a copper pour — pads on these layers connect
  // through the plane, so "no trace on this pad" is expected, not stale.
  const zoneLayersByNet = new Map<string, Set<PcbLayer>>();
  for (const z of pcb.zones) {
    const set = zoneLayersByNet.get(z.net) ?? new Set<PcbLayer>();
    set.add(z.layer);
    zoneLayersByNet.set(z.net, set);
  }

  const stale = new Set<string>();
  for (const [net, traces] of tracesByNet) {
    const pads = padsByNet.get(net) ?? [];
    const vias = viasByNet.get(net) ?? [];
    // Only pad-to-pad routes can go stale from a moved pad; skip coils/windings
    // and other free copper (their nets have <2 pads).
    if (pads.length < 2) continue;

    // Distance from a point to a segment body — route-to-tree termination
    // legitimately ends a trace ON another trace's copper (mid-segment), not
    // only at endpoints, and that is an anchored, electrically-joined end.
    const segDist = (p: Vec2, t: Trace): number => {
      const dx = t.end.x - t.start.x;
      const dy = t.end.y - t.start.y;
      const len2 = dx * dx + dy * dy;
      const u = len2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - t.start.x) * dx + (p.y - t.start.y) * dy) / len2));
      return Math.hypot(p.x - (t.start.x + u * dx), p.y - (t.start.y + u * dy));
    };
    const onSegBody = (p: Vec2, t: Trace): boolean => segDist(p, t) <= t.width / 2 + 0.02;
    // (1) Any loose trace endpoint? Anchoring is copper-overlap: the
    // endpoint's own trace disc (its half-width) may touch the anchor's
    // body — multi-source/tree-terminated routes legally start and end
    // wherever their copper overlaps the net's existing copper.
    const anchored = (p: Vec2, selfIdx: number, halfW: number): boolean => {
      if (
        pads.some(
          (q) => Math.hypot(p.x - q.pos.x, p.y - q.pos.y) <= q.halfExtent + halfW + 0.02,
        )
      )
        return true;
      if (vias.some((q) => Math.hypot(p.x - q.x, p.y - q.y) <= halfW + 0.42)) return true;
      for (let j = 0; j < traces.length; j++) {
        if (j === selfIdx) continue;
        if (segDist(p, traces[j]) <= traces[j].width / 2 + halfW + 0.02) return true;
      }
      return false;
    };
    let isStale = traces.some(
      (t, i) => !anchored(t.start, i, t.width / 2) || !anchored(t.end, i, t.width / 2),
    );

    // (2) Any current pad uncovered by copper (and not on a same-net pour)?
    if (!isStale) {
      const zoneLayers = zoneLayersByNet.get(net);
      isStale = pads.some((pad) => {
        if (zoneLayers && pad.layers.some((l) => zoneLayers.has(l))) return false;
        // Covered = trace copper overlaps pad copper (route-to-tree endpoints
        // legally stop at the pad's edge, not its centre).
        const onTrace = traces.some(
          (t) => segDist(pad.pos, t) <= pad.halfExtent + t.width / 2 + 0.02,
        );
        const onVia = vias.some((v) =>
          Math.hypot(pad.pos.x - v.x, pad.pos.y - v.y) <= pad.halfExtent + 0.02 ? true : near(pad.pos, v),
        );
        return !onTrace && !onVia;
      });
    }

    if (isStale) stale.add(net);
  }
  return stale;
}

/** Per-net disjoint copper-group (galvanic island) counts via the kernel.
 *  Returns null when the kernel is unavailable — the connectivity guard is
 *  then OFF (and reported as such), never silently "clean". */
async function netIslandCounts(
  pcb: Pcb,
  nets: Iterable<string>,
): Promise<Map<string, number> | null> {
  const counts = new Map<string, number>();
  for (const net of nets) {
    const c = await kernelNetContinuity(pcb, net);
    if (!c) return null;
    counts.set(net, c.islands);
  }
  return counts;
}

/** Nets the connectivity guard watches: every routable (>=2 pad) net. A
 *  regression can land on a net the caller never named (stale rip-up,
 *  negotiated rip-up side effects), so the watch set is board-wide. Capped
 *  because each kernel continuity query serializes the whole board. */
const CONNECTIVITY_GUARD_MAX_NETS = 300;

/** Heuristic power/ground net names, for the `power_first` routing strategy. */
const POWER_NET_RE =
  /(^|[_\-/])(A?D?GND|VCC|VDD[A]?|VBAT|VBUS|VIN|VOUT|VSYS|VREF|PWR|POWER|\d+V\d*)(\d*)([_\-/]|$)/i;

/** Route nets on a PCB with the kernel autorouter (obstacle-avoiding). */
export async function routeNets(args: Record<string, unknown>, toolCtx?: ToolContext) {
  const progress = toolCtx?.progress;
  const ctx = resolveDocInput(args);
  const doc = ctx.doc;
  const traceWidth = (args.trace_width as number) || undefined;
  const netsFilter = (args.nets as string[]) || [];
  const lockedNets = new Set<string>(
    ((args.locked_nets as string[]) || []).map(String),
  );
  const effort = Math.min(100, Math.max(0.1, Number(args.effort) || 1));

  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  const width = traceWidth || pcb.rules.defaultRules.traceWidth;

  // Receipt: snapshot DRC before the route so the after-diff can attribute
  // exactly what this call fixed and what it introduced.
  const wantReceipt = Boolean(args.receipt);
  if (wantReceipt) progress?.(0, undefined, "snapshotting pre-route DRC");
  const beforeSnap = wantReceipt ? await drcPcb(pcb, "full", 500) : null;

  // Synthesize a netlist from pad assignments so the kernel ratsnest can
  // compute the unrouted connections (MST per net).
  const netConnections = new Map<string, Array<{ component_ref: string; pin_number: string }>>();
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      if (!pad.net) continue;
      const conns = netConnections.get(pad.net) || [];
      conns.push({ component_ref: fp.ref, pin_number: pad.number });
      netConnections.set(pad.net, conns);
    }
  }
  const netlist: NetlistResult = {
    nets: [...netConnections.entries()].map(([name, connections]) => ({ name, connections })),
  };

  // A footprint moved by set_placement after its nets were routed leaves the old
  // copper dangling (traces that no longer land on any pad). Moving a part
  // invalidates its nets' routes, so those nets are "affected" and must be
  // re-routed even when the caller didn't name them — without this, a *scoped*
  // re-route (`nets: [...]`) leaves orphaned copper on the nets it didn't touch.
  // Fold the stale nets into the route set. With no filter we already re-route
  // every net, so this only changes scoped calls.
  const staleNets = detectStaleNets(pcb);
  const effectiveFilter =
    netsFilter.length > 0 ? [...new Set([...netsFilter, ...staleNets])] : netsFilter;

  // Rip up existing copper on the nets we're about to (re)route. route_all is
  // authoritative: it returns a *complete* fresh routing for every target net,
  // so appending it on top of last run's copper would (a) stack duplicate
  // traces at 0mm self-clearance and (b) let the recomputed solution cross the
  // stale one and short other nets. Worse, the kernel's ratsnest skips nets that
  // already have a trace, so a second route_all comes back empty and the
  // no-kernel fallback below misfires — chaining naive straight segments over
  // the clean route. Re-running route_nets must *replace* the prior route, not
  // add to it. Scope the rip-up to exactly the nets route_all will route — nets
  // with >=2 pads, intersected with `effectiveFilter` — so hand-routes on other
  // nets (e.g. add_coil copper) survive.
  const targetNets = new Set<string>();
  for (const [net, conns] of netConnections) {
    if (conns.length < 2) continue;
    if (lockedNets.has(net)) continue; // preserve hand-placed copper
    if (effectiveFilter.length > 0 && !effectiveFilter.includes(net)) continue;
    targetNets.add(net);
  }

  // Copper provenance (issue #277): only *autorouted* copper is disposable.
  // Traces carrying `source: "manual"` (add_trace / add_via / coil tools) are
  // hand-placed work the router must not destroy. A net with a manual trace
  // can't be partially re-routed either — the kernel's ratsnest treats any
  // existing trace as "already routed" and skips the net, so ripping just its
  // autorouted segments would strand it with no way to close it again.
  // Such nets are therefore preserved wholesale (implicitly locked) and
  // reported in `manual_nets_preserved`; delete the manual copper
  // (delete_trace / delete_via) to hand the net back to the autorouter.
  // Copper with no `source` (pre-provenance documents, or injected directly)
  // is treated as autorouted — exactly the rip-up those documents already got.
  // Manual *vias* alone don't block re-routing (the ratsnest ignores vias);
  // they simply survive the rip-up while the net's traces are re-laid.
  const manualNets = new Set<string>();
  for (const t of pcb.traces) {
    if (t.source === "manual" && targetNets.has(t.net)) manualNets.add(t.net);
  }
  for (const n of manualNets) targetNets.delete(n);

  // Connectivity guard: snapshot per-net disjoint copper-group counts before
  // any rip-up so the result can report every net this call left WORSE
  // connected (field report: a scoped re-route took GND from 4 galvanic
  // groups to 14 and the result said nothing). Also snapshot the copper
  // itself so an unrepairable regression can be rolled back net-by-net.
  const guardNets = [...netConnections.entries()]
    .filter(([, conns]) => conns.length >= 2)
    .map(([n]) => n);
  let guardSkipped: string | null = null;
  let islandsBefore: Map<string, number> | null = null;
  let copperBefore: { traces: typeof pcb.traces; vias: typeof pcb.vias } | null = null;
  // Each continuity query serializes the whole board into the kernel, so the
  // guard is also budgeted by copper-element count, not just net count.
  const guardElements =
    pcb.traces.length +
    pcb.vias.length +
    pcb.zones.length +
    pcb.footprints.reduce((n, fp) => n + fp.pads.length, 0);
  const maxGuardElements = Number(
    process.env.VCAD_ROUTE_GUARD_MAX_ELEMENTS ?? 50_000,
  );
  if (guardNets.length > CONNECTIVITY_GUARD_MAX_NETS) {
    guardSkipped = `connectivity-regression guard skipped: ${guardNets.length} nets exceeds the ${CONNECTIVITY_GUARD_MAX_NETS}-net budget — verify with run_drc (NetIslands)`;
  } else if (guardElements > maxGuardElements) {
    guardSkipped = `connectivity-regression guard skipped: ${guardElements} copper elements exceeds the ${maxGuardElements} budget (VCAD_ROUTE_GUARD_MAX_ELEMENTS) — verify with run_drc (NetIslands)`;
  } else {
    islandsBefore = await netIslandCounts(pcb, guardNets);
    if (islandsBefore) {
      // Deep clone: Trace.start/.end and Via.position are nested Vec2 objects,
      // so a shallow spread would leave the snapshot sharing them with live
      // copper — a future in-place edit during routing would corrupt the
      // rollback baseline. structuredClone makes the snapshot fully independent.
      copperBefore = {
        traces: pcb.traces.map((t) => structuredClone(t)),
        vias: pcb.vias.map((v) => structuredClone(v)),
      };
    } else {
      guardSkipped =
        "connectivity-regression guard unavailable (kernel WASM not loaded) — verify with run_drc";
    }
  }

  let tracesRemoved = 0;
  let viasRemoved = 0;
  let zonesAdded = 0;
  if (targetNets.size > 0) {
    const beforeT = pcb.traces.length;
    const beforeV = pcb.vias.length;
    pcb.traces = pcb.traces.filter((t) => !targetNets.has(t.net) || t.source === "manual");
    pcb.vias = pcb.vias.filter((v) => !targetNets.has(v.net) || v.source === "manual");
    tracesRemoved = beforeT - pcb.traces.length;
    viasRemoved = beforeV - pcb.vias.length;
  }

  const rats = await computeRatsnest(pcb, netlist);

  const routedNets = new Set<string>();
  const fallbackNets = new Set<string>();
  const unroutedNets = new Set<string>();
  // Per-unrouted-connection diagnostics from the kernel (blocking nets, the
  // congested region, a suggested layer/via) — surfaced so the caller knows
  // *why* a net stayed open and *where*, not just that it did.
  type RouteDiagnostic = Awaited<ReturnType<typeof routeAll>>["diagnostics"][number];
  const diagnostics: RouteDiagnostic[] = [];
  // The kernel's own routability per route_all pass (connection-granular, exact).
  // The default "auto" strategy is a single pass, so we forward its value
  // verbatim as the authoritative figure rather than recomputing/rounding.
  const kernelRoutabilities: number[] = [];
  let tracesAdded = 0;
  let viasAdded = 0;

  // Auto-route the whole board in the kernel: every net is routed against one
  // growing clearance oracle and retried on the back layer with transition vias
  // that are probed before placement, so the returned copper is clearance-legal
  // by construction. Nets that can't be routed legally come back in
  // `unrouted_nets` instead of being shipped as a short.
  // Realized copper width per net (max across its segments) — lets the caller
  // confirm a power/phase net actually routed at its class width without
  // re-reading the board.
  const realizedWidths: Record<string, number> = {};
  // Nets the kernel should route: the effective set minus any the caller
  // locked and minus nets preserved for their manual copper. A preserved net
  // is neither ripped up (above) nor re-routed here, so its hand-placed
  // copper stays exactly as authored. An empty effectiveFilter means "route
  // everything", so to subtract preserved nets we make the all-set explicit;
  // with nothing preserved it stays empty (behavior unchanged).
  const preservedNets = new Set<string>([...lockedNets, ...manualNets]);
  let routeFilter = effectiveFilter;
  if (preservedNets.size > 0) {
    if (effectiveFilter.length > 0) {
      routeFilter = effectiveFilter.filter((n) => !preservedNets.has(n));
    } else {
      const allRoutable = new Set<string>();
      for (const [net, conns] of netConnections) {
        if (conns.length >= 2 && !preservedNets.has(net)) allRoutable.add(net);
      }
      routeFilter = [...allRoutable];
    }
  }
  // Nets connected through a copper plane (a zone) are stitched to the plane
  // with vias by the kernel, not trace-routed — same rule as the kernel's
  // plane_layers (a net with a zone of >=3 vertices). Also feeds `power_first`.
  const planeNets = new Set<string>();
  for (const z of pcb.zones) {
    if (z.net && z.outline.length >= 3) planeNets.add(z.net);
  }

  // Net ordering strategy. "auto" (default) routes the whole board in one
  // negotiated pass (best quality, kernel-global rip-up). The explicit
  // strategies route in priority tiers — each tier's copper becomes the next
  // tier's obstacle, so earlier (higher-priority) nets claim channels first.
  const strategy = String(args.strategy ?? "auto");
  const isPowerNet = (n: string) => planeNets.has(n) || POWER_NET_RE.test(n);
  const tierOf = (n: string): number => {
    if (strategy === "power_first") return isPowerNet(n) ? 0 : 1;
    if (strategy === "fanout_desc") return -(netConnections.get(n)?.length ?? 0);
    if (strategy === "fanout_asc") return netConnections.get(n)?.length ?? 0;
    return 0;
  };

  const applyRoute = (result: Awaited<ReturnType<typeof routeAll>>) => {
    // Synthesized copper pours first. The routing that follows *assumes* them:
    // a poured net is carried by its plane, so the kernel stitched its pads to
    // the plane instead of tracing them to each other. Dropping them here would
    // leave those nets connected to nothing.
    for (const z of result.zones ?? []) {
      pcb.zones.push(structuredClone(z));
      zonesAdded++;
    }
    for (const t of result.traces) {
      pcb.traces.push({
        start: { x: t.start.x, y: t.start.y },
        end: { x: t.end.x, y: t.end.y },
        width: t.width,
        // The kernel returns the layer as a string ("FCu"/"BCu"); always a
        // valid PcbLayer value.
        layer: t.layer as PcbLayer,
        net: t.net,
        source: "autoroute",
      });
      realizedWidths[t.net] = Math.max(realizedWidths[t.net] ?? 0, t.width);
      tracesAdded++;
    }
    for (const v of result.vias) {
      pcb.vias.push({
        position: { x: v.position.x, y: v.position.y },
        diameter: pcb.rules.defaultRules.viaDiameter,
        drill: pcb.rules.defaultRules.viaDrill,
        // Span chosen by the 3D search (blind/buried supported); older
        // kernels without spans fall back to a through via.
        startLayer: v.start_layer ?? "FCu",
        endLayer: v.end_layer ?? "BCu",
        net: v.net,
        source: "autoroute",
      });
      viasAdded++;
    }
    for (const n of result.routed_nets) routedNets.add(n);
    for (const n of result.unrouted_nets) unroutedNets.add(n);
    for (const d of result.diagnostics ?? []) diagnostics.push(d);
    if (typeof result.routability === "number") kernelRoutabilities.push(result.routability);
  };

  let routedSomething = false;
  if (strategy === "auto") {
    progress?.(0, undefined, "routing all nets (negotiated whole-board pass)");
    const result = await routeAll(pcb, width, routeFilter, effort);
    routedSomething =
      result.traces.length > 0 || result.vias.length > 0 || result.unrouted_nets.length > 0;
    if (routedSomething) applyRoute(result);
  } else {
    // Route higher-priority tiers first, each against the growing board.
    const universe =
      routeFilter.length > 0
        ? routeFilter
        : [...netConnections.keys()].filter((n) => !preservedNets.has(n));
    const tiers = [...new Set(universe.map(tierOf))].sort((a, b) => a - b);
    for (let ti = 0; ti < tiers.length; ti++) {
      const tier = tiers[ti]!;
      const nets = universe.filter((n) => tierOf(n) === tier);
      if (nets.length === 0) continue;
      progress?.(ti, tiers.length, `routing tier ${ti + 1}/${tiers.length} (${nets.length} net(s))`);
      const result = await routeAll(pcb, width, nets, effort);
      if (
        result.traces.length > 0 ||
        result.vias.length > 0 ||
        result.unrouted_nets.length > 0
      ) {
        routedSomething = true;
        applyRoute(result);
      }
    }
  }

  if (!routedSomething && rats.length === 0) {
    // No kernel at all: computeRatsnest returns [] and the auto-router is empty
    // — chain pads directly so the tool still produces connectivity (legacy
    // behavior; flagged because it may cross copper).
    for (const [netId, conns] of netConnections) {
      if (conns.length < 2) continue;
      // Never reroute a locked net or one preserved for its manual copper.
      if (preservedNets.has(netId)) continue;
      if (effectiveFilter.length > 0 && !effectiveFilter.includes(netId)) continue;
      const positions = conns.map((c) => {
        const fp = pcb.footprints.find((f) => f.ref === c.component_ref)!;
        const pad = fp.pads.find((p) => p.number === c.pin_number)!;
        return { x: fp.position.x + pad.position.x, y: fp.position.y + pad.position.y };
      });
      for (let i = 0; i < positions.length - 1; i++) {
        pcb.traces.push({
          start: positions[i],
          end: positions[i + 1],
          width,
          layer: "FCu",
          net: netId,
          source: "autoroute",
        });
        tracesAdded++;
      }
      routedNets.add(netId);
      fallbackNets.add(netId);
    }
  }

  // Connectivity-regression guard, part 2: any net whose copper now forms
  // MORE disjoint groups than before this call regressed — the router (or a
  // stale rip-up side effect) broke connectivity it was supposed to preserve.
  // Retry the regressed nets once against the otherwise-final board; a net
  // still worse after the retry gets its pre-call copper restored (rollback),
  // so a route_nets call can never silently degrade a net's connectivity.
  const connectivityRegressions: Record<
    string,
    { groups_before: number; groups_after: number; action: "repaired-by-retry" | "rolled_back" }
  > = {};
  if (islandsBefore && copperBefore) {
    progress?.(0, undefined, "verifying net connectivity");
    const before = islandsBefore;
    const worseNets = async (nets: Iterable<string>): Promise<Map<string, number> | null> => {
      const counts = await netIslandCounts(pcb, nets);
      if (!counts) return null;
      const worse = new Map<string, number>();
      for (const [n, after] of counts) {
        const b = before.get(n) ?? 0;
        if (b > 0 && after > b) worse.set(n, after);
      }
      return worse;
    };
    let worse = await worseNets(guardNets);
    if (worse && worse.size > 0) {
      // Retry: rip the regressed nets' autorouted copper and route just them
      // again — the rest of the board (this pass's other copper included) is
      // now a fixed obstacle field, which often resolves the negotiated
      // rip-up collateral that caused the regression.
      const retrySet = new Set(
        [...worse.keys()].filter((n) => !preservedNets.has(n)),
      );
      if (retrySet.size > 0) {
        pcb.traces = pcb.traces.filter((t) => !retrySet.has(t.net) || t.source === "manual");
        pcb.vias = pcb.vias.filter((v) => !retrySet.has(v.net) || v.source === "manual");
        progress?.(0, undefined, `re-routing ${retrySet.size} net(s) with regressed connectivity`);
        const retried = await routeAll(pcb, width, [...retrySet], effort);
        applyRoute(retried);
      }
      const stillWorse = (await worseNets(worse.keys())) ?? worse;
      for (const [n, afterRoute] of worse) {
        if (!stillWorse.has(n)) {
          connectivityRegressions[n] = {
            groups_before: before.get(n)!,
            groups_after: afterRoute,
            action: "repaired-by-retry",
          };
          continue;
        }
        // Rollback: restore the net's pre-call copper verbatim. The restored
        // copper is checked against the final board by the caller's next DRC
        // (and by the `receipt` after-snapshot below when requested).
        pcb.traces = pcb.traces.filter((t) => t.net !== n);
        pcb.vias = pcb.vias.filter((v) => v.net !== n);
        for (const t of copperBefore.traces) if (t.net === n) pcb.traces.push(t);
        for (const v of copperBefore.vias) if (v.net === n) pcb.vias.push(v);
        connectivityRegressions[n] = {
          groups_before: before.get(n)!,
          groups_after: stillWorse.get(n)!,
          action: "rolled_back",
        };
      }
    }
  }

  const planeStitched = [...routedNets].filter((n) => planeNets.has(n)).sort();

  // Overall routability in [0, 1]: how close the board is to fully routed. A
  // single-pass "auto" route forwards the kernel's exact connection-granular
  // figure verbatim (authoritative — no divergence, no rounding). Tier
  // strategies run several passes over disjoint net sets with no shared
  // connection count, so there we fall back to a net-level estimate. Either way
  // we keep full precision so small boards aren't misrepresented (2 of 3 reads
  // 0.6667, not 0.67).
  const attemptedNets = routedNets.size + unroutedNets.size;
  const routability =
    kernelRoutabilities.length === 1
      ? kernelRoutabilities[0]
      : attemptedNets === 0
        ? 1
        : routedNets.size / attemptedNets;
  // Keep one diagnostic per still-unrouted net (the kernel may report several
  // connections for a multi-pin net; the first is the most useful summary).
  const dedupedDiagnostics: RouteDiagnostic[] = [];
  const seenDiagNets = new Set<string>();
  for (const d of diagnostics) {
    if (!unroutedNets.has(d.net) || seenDiagNets.has(d.net)) continue;
    seenDiagNets.add(d.net);
    dedupedDiagnostics.push(d);
  }

  const warnings: string[] = [];
  if (unroutedNets.size > 0) {
    warnings.push(
      `${unroutedNets.size} net(s) could not be routed without shorting and were left unrouted (no copper added) — they need another layer or rip-up rerouting: ${[...unroutedNets].join(", ")}`,
    );
  }
  if (fallbackNets.size > 0) {
    warnings.push(
      `${fallbackNets.size} net(s) used direct fallback segments that may cross other copper — run run_drc to verify`,
    );
  }
  // Stale nets the caller didn't ask for but we ripped up and re-routed because a
  // pad had moved out from under their copper. Surface them so a scoped re-route
  // is honest about the extra cleanup it did. Nets preserved for manual copper
  // were NOT actually cleared, so they don't belong in this message.
  const staleCleared = [...staleNets].filter(
    (n) => !netsFilter.includes(n) && !manualNets.has(n),
  );
  if (netsFilter.length > 0 && staleCleared.length > 0) {
    warnings.push(
      `ripped up orphaned copper on ${staleCleared.length} net(s) whose pads moved after the last route and re-routed them: ${staleCleared.join(", ")}`,
    );
  }
  if (manualNets.size > 0) {
    warnings.push(
      `${manualNets.size} net(s) carry hand-placed copper (add_trace / add_via / coil tools) and were preserved as-is — neither ripped up nor re-routed: ${[...manualNets].sort().join(", ")}. Delete that copper (delete_trace / delete_via) first if route_nets should re-own the net`,
    );
  }

  if (guardSkipped) warnings.push(guardSkipped);
  const regressedNets = Object.keys(connectivityRegressions).sort();
  if (regressedNets.length > 0) {
    const rolled = regressedNets.filter(
      (n) => connectivityRegressions[n]!.action === "rolled_back",
    );
    warnings.push(
      `connectivity regression on ${regressedNets.length} net(s) — this call left their copper in more disjoint groups than before (${regressedNets
        .map((n) => `${n}: ${connectivityRegressions[n]!.groups_before}→${connectivityRegressions[n]!.groups_after}`)
        .join(", ")}). Regressed nets were re-routed once${
        rolled.length > 0
          ? `; ${rolled.join(", ")} stayed worse and had their pre-call copper restored (rolled back) — the restored copper may now conflict with newly routed nets, run run_drc`
          : " and repaired"
      }`,
    );
  }

  const receiptField: Record<string, unknown> = {};
  // Only build a receipt when both snapshots actually verified — a receipt
  // diffed against an unverifiable (kernel-rejected) board would be meaningless.
  // But a requested receipt is never SILENTLY dropped: an unverifiable
  // snapshot yields `receipt_error` with the reason instead of nothing.
  if (wantReceipt && beforeSnap && beforeSnap.success) {
    const after = await drcPcb(pcb, "full", 500);
    // Only build a receipt when the after-snapshot also verified — an
    // unverifiable (kernel-rejected) board has no byNetPair to diff against.
    if (!after.success) {
      receiptField.receipt_error = `receipt requested but the after-route DRC snapshot could not verify the board: ${after.reason}`;
    } else {
      const entry = buildEntry(
        { tool: "route_nets", args: { nets: netsFilter, trace_width: traceWidth }, before: beforeSnap, after },
        0,
      );
      const shortPairs: [string, string][] = [];
      for (const bp of after.byNetPair) {
        if (bp.rule === "Short" && bp.nets[0] && bp.nets[1]) shortPairs.push(bp.nets);
      }
      receiptField.receipt = {
        ...agentView(entry, ctx.documentId ?? ""),
        // Total violation counts of the two snapshots — `errors` (from
        // agentView) is only the error-severity slice; agents diff the board
        // on the full count.
        violations: { before: entry.before.violations, after: entry.after.violations },
        nets_routed: [...routedNets].sort(),
        nets_unrouted: [...unroutedNets].sort(),
        traces_added: tracesAdded,
        traces_removed: tracesRemoved,
        vias_added: viasAdded,
        vias_removed: viasRemoved,
        plane_stitched: planeStitched,
        short_pairs: shortPairs,
      };
    }
  } else if (wantReceipt && beforeSnap && !beforeSnap.success) {
    receiptField.receipt_error = `receipt requested but the before-route DRC snapshot could not verify the board: ${beforeSnap.reason}`;
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          nets_routed: routedNets.size,
          routability,
          traces_added: tracesAdded,
          // Copper pours the router synthesized for high-current nets: those
          // nets are now carried by a plane and stitched to it, not traced.
          ...(zonesAdded > 0 ? { zones_added: zonesAdded } : {}),
          // Copper hygiene: re-routing rips the prior route up first, so a
          // re-route reports both what it removed and what it laid — `added`
          // alone reads like monotonic growth even when copper is being
          // replaced, not stacked.
          ...(tracesRemoved > 0 ? { traces_removed: tracesRemoved } : {}),
          ...(viasRemoved > 0 ? { vias_removed: viasRemoved } : {}),
          ...(staleCleared.length > 0 ? { stale_nets_cleared: staleCleared } : {}),
          ...(lockedNets.size > 0 ? { locked_nets: [...lockedNets] } : {}),
          ...(manualNets.size > 0
            ? { manual_nets_preserved: [...manualNets].sort() }
            : {}),
          ...(planeStitched.length > 0 ? { plane_stitched: planeStitched } : {}),
          ...(Object.keys(realizedWidths).length > 0
            ? { track_widths_mm: realizedWidths }
            : {}),
          ...(unroutedNets.size > 0 ? { unrouted_nets: [...unroutedNets] } : {}),
          ...(dedupedDiagnostics.length > 0 ? { unrouted_diagnostics: dedupedDiagnostics } : {}),
          ...(fallbackNets.size > 0 ? { fallback_nets: [...fallbackNets] } : {}),
          ...(regressedNets.length > 0
            ? { connectivity_regressions: connectivityRegressions }
            : {}),
          ...(warnings.length > 0 ? { warnings } : {}),
          ...receiptField,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/** Run DRC checks on a PCB. */
// ============================================================================
// DRC result aggregation — summary-first, opt-in full detail
// ============================================================================

/** A single DRC violation, structurally compatible with both the kernel
 *  result and the scalar fallback (which omits actual/required). */
/** Where a violation comes from — lets a caller discount footprint artifacts
 *  from the headline count. Mirrors the kernel `DrcProvenance` (snake_case). */
type DrcProvenance = "intra_footprint" | "inter_component" | "routing";

interface DrcViol {
  rule: string;
  severity: string;
  message: string;
  position?: Vec2;
  actual?: number;
  required?: number;
  /** Footprint-internal, between components, or routing/board-level. */
  provenance?: DrcProvenance;
  /** True when a generated (synthesized) footprint land pattern is involved. */
  generated?: boolean;
}

/** Per (rule, net-pair) rollup. netB is "" for single-net rules. */
interface DrcNetPairCount {
  nets: [string, string];
  rule: string;
  count: number;
  worstActual: number;
  worstRequired: number;
}

/** Group each DRC rule by what it means for the board, so callers can tell an
 *  *incomplete* layout (ratsnest left to route) from an *illegal* one (copper
 *  conflicts / fab-rule breaks). UnconnectedNet and UnstitchedPad are
 *  "incomplete" rules (route it / stitch it). NetIslands is a hard defect (a
 *  net's copper built as ≥2 galvanically-isolated islands); it carries Error
 *  severity regardless of bucket. SameNetBypass is a Warning-severity
 *  connectivity defect: same-net copper touching far from any intended
 *  junction, short-circuiting the conductor between the points (fatal to
 *  two-terminal structures like spiral coils). */
const DRC_CATEGORY: Record<string, "connectivity" | "clearance" | "manufacturing"> = {
  UnconnectedNet: "connectivity",
  UnstitchedPad: "connectivity",
  NetIslands: "connectivity",
  SameNetBypass: "connectivity",
  Clearance: "clearance",
  Short: "clearance",
  MinTraceWidth: "manufacturing",
  MinDrill: "manufacturing",
  AnnularRing: "manufacturing",
  EdgeClearance: "manufacturing",
  HoleToHole: "manufacturing",
  SilkscreenClearance: "manufacturing",
  CourtyardOverlap: "manufacturing",
  AcidTrap: "manufacturing",
  Keepout: "manufacturing",
};

/** Counts split by category — `connectivity` is unrouted nets (a to-do),
 *  `clearance`+`manufacturing` are genuine violations. */
interface DrcCategories {
  connectivity: number;
  clearance: number;
  manufacturing: number;
}

/** Counts split by where the conflict originates, so a caller can tell a
 *  synthesized land-pattern artifact (intra_footprint, usually generated) from
 *  a real placement (inter_component) or routing fault. */
interface DrcProvenanceCounts {
  intra_footprint: number;
  inter_component: number;
  routing: number;
}

/** Summary-first DRC payload: counts + worst-case + a capped representative
 *  sample by default; the full violation array only when detail==="full". */
interface DrcSummary {
  success: true;
  violations: number;
  errors: number;
  warnings: number;
  /** Counts split into connectivity (incomplete) vs clearance/manufacturing (illegal). */
  categories: DrcCategories;
  /** Counts split by origin (footprint-internal / between-components / routing). */
  byProvenance: DrcProvenanceCounts;
  /** Violations involving a generated (synthesized) footprint land pattern —
   *  candidate artifacts, NOT necessarily real faults. */
  generatedArtifacts: number;
  /** Violations the headline count should be judged on: total minus generated
   *  land-pattern artifacts. */
  realViolations: number;
  byRule: Record<string, number>;
  byNetPair: DrcNetPairCount[];
  worstClearance:
    | { actual: number; required: number; position?: Vec2; nets: [string, string] }
    | null;
  sample: DrcViol[];
  sampleCapped: boolean;
  detail: "summary" | "full";
  details?: DrcViol[];
}

/** DRC outcome that the kernel could not run because it refused the board (e.g.
 *  a malformed layer name like `In1.Cu`). A *parse failure*, not a clean board —
 *  surfacing it as "0 violations" would be a false-clean. Distinguished from
 *  {@link DrcSummary} by `success: false` so every caller must branch on it. */
export interface DrcUnverifiable {
  success: false;
  status: "errored";
  reason: string;
  offending_field?: string;
}

/** Pull net names out of a kernel DRC message (`... net 'A' ... net 'B' ...`).
 *  Returns a lexically-sorted pair so (A,B) and (B,A) collapse; "" when absent. */
export function parseNetPair(message: string): [string, string] {
  const names: string[] = [];
  const re = /net '([^']+)'/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(message)) !== null) names.push(m[1]!);
  const a = names[0] ?? "";
  const b = names[1] ?? "";
  // Single-net (or net-less) rules keep the real net first, "" in slot 2.
  if (!a || !b) return [a || b, ""];
  return a <= b ? [a, b] : [b, a];
}

/** Aggregate raw violations into a summary, with a representative sample drawn
 *  round-robin across (rule, net-pair) buckets (not just the first N of one
 *  rule). The kernel emits no structured net fields, so pairs are parsed from
 *  the message text — a stable format (see crates/vcad-ecad-pcb DRC). */
export function aggregateDrc(
  violations: DrcViol[],
  sampleSize: number,
  detail: "summary" | "full",
): DrcSummary {
  const finite = (n: number | undefined): n is number => Number.isFinite(n);
  const byRule: Record<string, number> = {};
  const buckets = new Map<string, DrcNetPairCount & { items: DrcViol[] }>();
  let worst: DrcSummary["worstClearance"] = null;

  for (const v of violations) {
    byRule[v.rule] = (byRule[v.rule] ?? 0) + 1;
    const [a, b] = parseNetPair(v.message);
    const key = `${v.rule}|${a}|${b}`;
    let e = buckets.get(key);
    if (!e) {
      e = {
        nets: [a, b],
        rule: v.rule,
        count: 0,
        worstActual: v.actual ?? NaN,
        worstRequired: v.required ?? NaN,
        items: [],
      };
      buckets.set(key, e);
    }
    e.count++;
    e.items.push(v);
    if (
      finite(v.actual) &&
      finite(v.required) &&
      (!finite(e.worstActual) ||
        !finite(e.worstRequired) ||
        v.actual - v.required < e.worstActual - e.worstRequired)
    ) {
      e.worstActual = v.actual;
      e.worstRequired = v.required;
    }
    if (
      v.rule === "Clearance" &&
      finite(v.actual) &&
      finite(v.required) &&
      (!worst || v.actual - v.required < worst.actual - worst.required)
    ) {
      worst = { actual: v.actual, required: v.required, position: v.position, nets: [a, b] };
    }
  }

  const byNetPair: DrcNetPairCount[] = [...buckets.values()]
    .map((e) => ({
      nets: e.nets,
      rule: e.rule,
      count: e.count,
      worstActual: e.worstActual,
      worstRequired: e.worstRequired,
    }))
    .sort((x, y) => y.count - x.count)
    .slice(0, 50);

  // Representative sample: round-robin across buckets so it isn't first-N-of-one-rule.
  const lists = [...buckets.values()].map((e) => e.items.slice());
  const cap = Math.max(0, Math.round(sampleSize));
  const sample: DrcViol[] = [];
  let progressed = true;
  while (sample.length < cap && progressed) {
    progressed = false;
    for (const l of lists) {
      if (sample.length >= cap) break;
      const v = l.shift();
      if (v) {
        sample.push(v);
        progressed = true;
      }
    }
  }

  const categories: DrcCategories = { connectivity: 0, clearance: 0, manufacturing: 0 };
  for (const [rule, count] of Object.entries(byRule)) {
    categories[DRC_CATEGORY[rule] ?? "manufacturing"] += count;
  }

  // Provenance breakdown + the trustworthy "real vs artifact" split. A missing
  // provenance (e.g. kernel-less fallback path) is treated as routing.
  const byProvenance: DrcProvenanceCounts = {
    intra_footprint: 0,
    inter_component: 0,
    routing: 0,
  };
  let generatedArtifacts = 0;
  for (const v of violations) {
    byProvenance[v.provenance ?? "routing"] += 1;
    if (v.generated) generatedArtifacts += 1;
  }

  const summary: DrcSummary = {
    success: true,
    violations: violations.length,
    errors: violations.filter((v) => v.severity === "Error").length,
    warnings: violations.filter((v) => v.severity === "Warning").length,
    categories,
    byProvenance,
    generatedArtifacts,
    realViolations: violations.length - generatedArtifacts,
    byRule,
    byNetPair,
    worstClearance: worst,
    sample,
    sampleCapped: sample.length < violations.length,
    detail,
  };
  if (detail === "full") summary.details = violations;
  return summary;
}

/** Route a declared differential pair (P/N) coupled + length-matched, committing the legs. */
export async function routeDiffPair(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const netP = String(args.net_p ?? "");
  const netN = String(args.net_n ?? "");
  if (!netP || !netN) {
    return {
      content: [{ type: "text" as const, text: "Error: 'net_p' and 'net_n' are required" }],
      isError: true,
    };
  }

  const res = await kernelRouteDiffPair(pcb, netP, netN);
  if (!res.success || !res.p || !res.n) {
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: false,
            reason:
              "could not resolve the pair — each of net_p/net_n needs exactly two pads, " +
              "or the kernel is unavailable",
          }),
        },
      ],
    };
  }

  // Leg width: the pair's diff-pair-class width, else the default.
  const cls = (pcb.rules.classRules ?? []).find(
    (c) =>
      c.diffPairGap != null &&
      (pcb.rules.netClassAssignments?.[c.name] ?? []).includes(netP),
  );
  const width = cls?.diffPairWidth ?? cls?.traceWidth ?? pcb.rules.defaultRules.traceWidth;

  let added = 0;
  for (const leg of [res.p, res.n]) {
    for (const [s, e] of leg.segments) {
      pcb.traces.push({
        start: { x: s.x, y: s.y },
        end: { x: e.x, y: e.y },
        width,
        layer: "FCu",
        net: leg.net,
        // Router output: rippable by a route_nets re-route of these nets.
        source: "autoroute",
      });
      added++;
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ success: true, traces_added: added, ...docResultPayload(ctx) }),
      },
    ],
  };
}

/** Length-match a group of nets by meandering the shorter ones, committing the copper. */
export async function lengthMatchTraces(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  const nets = Array.isArray(args.nets) ? (args.nets as unknown[]).map(String) : [];
  if (nets.length === 0) return fail("'nets' must be a non-empty array of net names");
  const checkOnly = args.check_only === true;
  const style = args.style === undefined ? undefined : String(args.style);
  if (style !== undefined && style !== "trombone" && style !== "sawtooth") {
    return fail("style must be 'trombone' or 'sawtooth'");
  }

  const result = await kernelMatchTraceLengths(pcb, nets, {
    target_length: args.target_length as number | undefined,
    tolerance: args.tolerance as number | undefined,
    max_amplitude: args.max_amplitude as number | undefined,
    spacing: args.spacing as number | undefined,
    style: style as "trombone" | "sawtooth" | undefined,
    check_only: checkOnly,
  });
  if (!result) {
    return fail("length matching unavailable — the ECAD kernel WASM is not loaded");
  }

  const round = (v: number) => Math.round(v * 1000) / 1000;
  const report = result.nets.map((n) => ({
    net: n.net,
    length_before_mm: round(n.length_before),
    length_after_mm: round(n.length_after),
    deviation_mm: round(n.length_after - result.target_length),
    matched: n.matched,
    tuned: n.tuned,
    ...(n.skip_reason ? { skip_reason: n.skip_reason } : {}),
    ...(n.tuned ? { traces_replaced: n.new_traces?.length ?? 0 } : {}),
  }));

  if (checkOnly) {
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            check_only: true,
            target_length_mm: round(result.target_length),
            tolerance_mm: result.tolerance,
            all_matched: result.all_matched,
            nets: report,
          }),
        },
      ],
    };
  }

  // Commit: each tuned net's replacement copper supplants ALL of its straight
  // traces (the meanders re-emit the untouched spans too).
  const tuned = result.nets.filter((n) => n.tuned && n.new_traces && n.new_traces.length > 0);
  const touched = new Set(tuned.map((n) => n.net));
  const newPoints: Vec2[] = tuned.flatMap((n) =>
    (n.new_traces ?? []).flatMap((t) => [t.start, t.end]),
  );
  const drcCap = await beginDrcDelta(
    pcb,
    touched.size > 0 ? boundsOfPoints(newPoints, 3) : null,
  );

  if (touched.size > 0) {
    pcb.traces = pcb.traces.filter((t) => !touched.has(t.net));
    for (const n of tuned) pcb.traces.push(...(n.new_traces ?? []));
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          target_length_mm: round(result.target_length),
          tolerance_mm: result.tolerance,
          all_matched: result.all_matched,
          nets_tuned: tuned.length,
          nets: report,
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/** Read-only audit of one net's routing: length, vias, clearance margin, DRC issues. */
export async function critiqueRoute(args: Record<string, unknown>) {
  const { doc } = resolveDocInput(args);
  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const net = String(args.net ?? "");
  if (!net) {
    return {
      content: [{ type: "text" as const, text: "Error: 'net' is required" }],
      isError: true,
    };
  }
  const outcome = await kernelCritiqueRoute(pcb, net);
  // Unverifiable ≠ "no issues". The kernel could not parse the board (or isn't
  // loaded) — surface it as an error the agent can branch on, not a silent null.
  if (outcome.status === "errored") return ecadUnverifiable("critique_route", outcome);
  return { content: [{ type: "text" as const, text: JSON.stringify(outcome.value) }] };
}

/** Run DRC on a board and return the summary-first payload. Shared by the
 *  run_drc tool and the inline receipt wrap in the mutators. */
export async function drcPcb(
  pcb: Pcb,
  detail: "summary" | "full" = "summary",
  sampleSize = 20,
): Promise<DrcSummary | DrcUnverifiable> {
  // Kernel DRC: copper clearance (trace↔copper and pad↔pad shorts), trace
  // width, drill, annular ring, edge clearance, hole-to-hole. Falls back to
  // basic scalar checks when the kernel WASM is unavailable.
  //
  // Size guard: a board past this budget risks exhausting the shared WASM
  // instance's linear memory, and a wasm OOM can take down the whole server
  // (killing every session) rather than just this call. Fail closed with a
  // structured error the agent can branch on. The CM5 reverse-engineering
  // board (10 layers, ~6.5k traces, ~3k vias, ~3k pads, 107 zones ≈ 13k
  // elements) checks in seconds, so the default cap leaves ample headroom.
  const elementCount =
    pcb.traces.length +
    pcb.vias.length +
    pcb.zones.length +
    pcb.footprints.reduce((n, fp) => n + fp.pads.length, 0);
  const maxElements = Number(process.env.VCAD_DRC_MAX_ELEMENTS ?? 200_000);
  if (elementCount > maxElements) {
    return {
      success: false,
      status: "errored",
      reason:
        `Board too large for DRC: ${elementCount} copper elements ` +
        `(traces + vias + pads + zones) exceeds the ${maxElements} budget. ` +
        `Run DRC on a region (verify-on-write covers edits), or raise ` +
        `VCAD_DRC_MAX_ELEMENTS if this host has the memory for it.`,
    };
  }
  let violations: DrcViol[];
  if (await isEcadAvailable()) {
    // The kernel can refuse a board it can't deserialize (e.g. a malformed
    // layer name). That's NOT a clean board — propagate the errored outcome
    // instead of swallowing it into an empty (false-clean) violation list.
    const outcome = await kernelRunDrc(pcb);
    if (outcome.status === "errored") {
      return {
        success: false,
        status: "errored",
        reason: outcome.reason,
        ...(outcome.offending_field ? { offending_field: outcome.offending_field } : {}),
      };
    }
    violations = outcome.value as unknown as DrcViol[];
  } else {
    violations = [];

    // Check min trace width
    for (const trace of pcb.traces) {
      if (trace.width < pcb.rules.defaultRules.traceWidth) {
        violations.push({
          rule: "MinTraceWidth",
          severity: "Error",
          message: `Trace width ${trace.width}mm < minimum ${pcb.rules.defaultRules.traceWidth}mm`,
          position: trace.start,
        });
      }
    }

    // Check min drill
    for (const via of pcb.vias) {
      if (via.drill < pcb.rules.minDrill) {
        violations.push({
          rule: "MinDrill",
          severity: "Error",
          message: `Via drill ${via.drill}mm < minimum ${pcb.rules.minDrill}mm`,
          position: via.position,
        });
      }
    }

    // Check annular ring
    for (const via of pcb.vias) {
      const annularRing = (via.diameter - via.drill) / 2;
      if (annularRing < pcb.rules.minAnnularRing) {
        violations.push({
          rule: "AnnularRing",
          severity: "Error",
          message: `Via annular ring ${annularRing.toFixed(3)}mm < minimum ${pcb.rules.minAnnularRing}mm`,
          position: via.position,
        });
      }
    }

    // Check edge clearance for traces
    const boardMinX = Math.min(...pcb.outline.vertices.map((v) => v.x));
    const boardMaxX = Math.max(...pcb.outline.vertices.map((v) => v.x));
    const boardMinY = Math.min(...pcb.outline.vertices.map((v) => v.y));
    const boardMaxY = Math.max(...pcb.outline.vertices.map((v) => v.y));
    const edgeClr = pcb.rules.edgeClearance;

    for (const trace of pcb.traces) {
      for (const pt of [trace.start, trace.end]) {
        const hw = trace.width / 2;
        if (
          pt.x - hw < boardMinX + edgeClr ||
          pt.x + hw > boardMaxX - edgeClr ||
          pt.y - hw < boardMinY + edgeClr ||
          pt.y + hw > boardMaxY - edgeClr
        ) {
          violations.push({
            rule: "EdgeClearance",
            severity: "Error",
            message: `Trace too close to board edge (min ${edgeClr}mm)`,
            position: pt,
          });
        }
      }
    }
  }

  return aggregateDrc(violations, sampleSize, detail);
}

// ============================================================================
// drc_delta — verify-on-write for every copper-mutating tool
// ============================================================================
//
// Field report: add_motor_winding returned a bare document_version for a board
// it had just shorted in 3 places and islanded in 3 more — the agent only
// learned from a separate run_drc. Every copper mutator now wraps its mutation
// in a before/after DRC snapshot and reports what it *introduced* (the
// route_nets `receipt` diff, extracted into `drcDelta`); `drc_delta.clean` is
// the one-step branch an agent needs.
//
// Cost control: boards with >= DRC_DELTA_FULL_BOARD_MAX elements scope the
// geometric checks to the mutation's inflated bbox via the kernel's region DRC
// (`check_drc_in_region`); connectivity always runs board-global — it is the
// only rule class a local copper edit can violate remotely. Smaller boards
// just take full-board snapshots.

/** Counts of introduced violations split by what they mean for the board.
 *  Unlike run_drc's categories, shorts get their own bucket — the worst class,
 *  so an agent can triage without parsing rule names. */
export interface DrcDeltaCategories {
  shorts: number;
  clearance: number;
  connectivity: number;
  manufacturing: number;
}

/** Verify-on-write verdict attached to every copper-mutating tool result:
 *  what this one call broke (and fixed), from before/after DRC snapshots. */
export interface DrcDelta {
  /** True iff the mutation introduced no new violations. */
  clean: boolean;
  /** Violations this mutation introduced (multiset diff, exact). */
  introduced: number;
  /** Violations this mutation fixed. */
  resolved: number;
  /** Introduced counts by category — `shorts` is the drop-everything bucket. */
  by_category: DrcDeltaCategories;
  /** Worst-first capped sample of the introduced violations, with positions. */
  sample: DrcViol[];
  sample_capped: boolean;
  /** "full" board snapshots, or geometric checks scoped to the mutation's
   *  inflated bbox ("region" — connectivity still board-global). */
  scope: "full" | "region";
  /** Present when a snapshot could not be verified (kernel refused the board).
   *  `clean` is forced false — an unverifiable board is NOT a clean one. */
  unverifiable?: { reason: string; offending_field?: string };
}

/** Category of one rule for the delta triage. */
function deltaCategory(rule: string): keyof DrcDeltaCategories {
  if (rule === "Short") return "shorts";
  return DRC_CATEGORY[rule] ?? "manufacturing";
}

/** Sample cap for `drc_delta.sample` — enough to act on, small enough to read. */
const DRC_DELTA_SAMPLE_CAP = 10;

/**
 * Diff two DRC snapshots into the violations a mutation INTRODUCED (and
 * resolved). The extracted core of the route_nets receipt: the same multiset
 * identity (`@vcad/core` `diffViolations`), reduced to the one verdict a
 * mutator should self-report. Both snapshots must come from the same scope
 * (full board, or the same region) — the callers guarantee that.
 */
export function drcDelta(
  before: DrcSummary | DrcUnverifiable,
  after: DrcSummary | DrcUnverifiable,
  scope: "full" | "region" = "full",
): DrcDelta {
  const empty: DrcDeltaCategories = {
    shorts: 0,
    clearance: 0,
    connectivity: 0,
    manufacturing: 0,
  };
  if (!before.success || !after.success) {
    const bad = !before.success ? before : (after as DrcUnverifiable);
    return {
      clean: false,
      introduced: 0,
      resolved: 0,
      by_category: empty,
      sample: [],
      sample_capped: false,
      scope,
      unverifiable: {
        reason: bad.reason,
        ...(bad.offending_field ? { offending_field: bad.offending_field } : {}),
      },
    };
  }

  // Snapshots are taken with detail:"full", so `details` is the complete list
  // and the diff is exact; `sample` only backstops a foreign snapshot.
  const { introduced, fixed } = diffViolations(
    before.details ?? before.sample,
    after.details ?? after.sample,
  );

  const by_category: DrcDeltaCategories = { ...empty };
  for (const v of introduced) by_category[deltaCategory(v.rule)] += 1;

  // Worst-first: shorts → connectivity → clearance → manufacturing, errors
  // before warnings — so a truncated sample never hides the short.
  const rank: Record<keyof DrcDeltaCategories, number> = {
    shorts: 0,
    connectivity: 1,
    clearance: 2,
    manufacturing: 3,
  };
  const sorted = [...(introduced as DrcViol[])].sort(
    (a, b) =>
      rank[deltaCategory(a.rule)] - rank[deltaCategory(b.rule)] ||
      (a.severity === "Error" ? 0 : 1) - (b.severity === "Error" ? 0 : 1),
  );

  return {
    clean: introduced.length === 0,
    introduced: introduced.length,
    resolved: fixed.length,
    by_category,
    sample: sorted.slice(0, DRC_DELTA_SAMPLE_CAP),
    sample_capped: introduced.length > DRC_DELTA_SAMPLE_CAP,
    scope,
  };
}

/** Boards with fewer elements than this always take full-board snapshots —
 *  the region machinery only pays off past it. */
const DRC_DELTA_FULL_BOARD_MAX = 2000;

/** Board size in copper-ish elements, for the full-vs-region scope decision. */
function pcbElementCount(pcb: Pcb): number {
  let pads = 0;
  for (const fp of pcb.footprints) pads += fp.pads.length;
  return (
    pcb.traces.length +
    (pcb.traceArcs?.length ?? 0) +
    pcb.vias.length +
    pcb.zones.length +
    pads
  );
}

/** The largest clearance any net class demands — the region inflation that
 *  keeps every element the mutation could conflict with in scope. */
function maxBoardClearance(pcb: Pcb): number {
  let c = pcb.rules.defaultRules.clearance;
  for (const cls of pcb.rules.classRules ?? []) c = Math.max(c, cls.clearance);
  return c;
}

/** World-space bbox of a mutation's copper, inflated by the element radius.
 *  Returns "full" when any coordinate is non-finite (bad args are the tool's
 *  problem — the delta must not silently scope them out). */
function boundsOfPoints(
  points: Vec2[],
  inflate = 0,
): { min: Vec2; max: Vec2 } | "full" {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const p of points) {
    minX = Math.min(minX, p.x);
    minY = Math.min(minY, p.y);
    maxX = Math.max(maxX, p.x);
    maxY = Math.max(maxY, p.y);
  }
  const b = {
    min: { x: minX - inflate, y: minY - inflate },
    max: { x: maxX + inflate, y: maxY + inflate },
  };
  const vals = [b.min.x, b.min.y, b.max.x, b.max.y];
  return vals.every(Number.isFinite) ? b : "full";
}

/** Mutation footprint for the drc_delta scope: a copper bbox, "full" for
 *  board-scale mutations (outline changes, motor windings), or null when the
 *  mutation moves no copper at all (set_stackup) — a no-op delta is expected
 *  but still verified. */
type DrcDeltaBounds = { min: Vec2; max: Vec2 } | "full" | null;

/** Region-scoped DRC snapshot — same summary shape as {@link drcPcb}. */
async function drcPcbInRegion(
  pcb: Pcb,
  region: { min: Vec2; max: Vec2 },
): Promise<DrcSummary | DrcUnverifiable> {
  if (await isEcadAvailable()) {
    const outcome = await kernelRunDrcInRegion(pcb, region.min, region.max);
    if (outcome.status === "errored") {
      return {
        success: false,
        status: "errored",
        reason: outcome.reason,
        ...(outcome.offending_field ? { offending_field: outcome.offending_field } : {}),
      };
    }
    return aggregateDrc(outcome.value as unknown as DrcViol[], 20, "full");
  }
  // No kernel: the scalar fallback is linear and cheap — run it unscoped.
  return drcPcb(pcb, "full", 20);
}

/** An in-flight verify-on-write capture: the before snapshot is taken, the
 *  after snapshot and diff happen in {@link DrcDeltaCapture.finish}. */
export interface DrcDeltaCapture {
  /** Take the after snapshot (same scope as before) and diff. Call once,
   *  after the mutation has been applied to the same live `pcb`. */
  finish(): Promise<DrcDelta>;
}

/**
 * Start a verify-on-write capture for a mutation about to land in `bounds`.
 * Call after arg validation (so error paths don't pay for a snapshot) and
 * immediately before the first board mutation; `finish()` after the last one.
 */
export async function beginDrcDelta(
  pcb: Pcb,
  bounds: DrcDeltaBounds,
): Promise<DrcDeltaCapture> {
  const full = bounds === "full" || pcbElementCount(pcb) < DRC_DELTA_FULL_BOARD_MAX;
  let region: { min: Vec2; max: Vec2 } | null = null;
  if (!full) {
    // `!full` already excludes bounds === "full" (aliased-condition narrowing).
    if (bounds) {
      // Inflate by the worst clearance in play (plus the kernel's own 1mm
      // search margin) so borderline subjects at the bbox edge stay in scope.
      const pad = maxBoardClearance(pcb) + 1.0;
      region = {
        min: { x: bounds.min.x - pad, y: bounds.min.y - pad },
        max: { x: bounds.max.x + pad, y: bounds.max.y + pad },
      };
    } else {
      // No copper moved: a degenerate region keeps the geometric checks empty
      // while connectivity still verifies globally.
      region = { min: { x: 0, y: 0 }, max: { x: 0, y: 0 } };
    }
  }
  const scope: "full" | "region" = region ? "region" : "full";
  const snapshot = (): Promise<DrcSummary | DrcUnverifiable> =>
    region ? drcPcbInRegion(pcb, region) : drcPcb(pcb, "full", 20);
  const before = await snapshot();
  return {
    async finish(): Promise<DrcDelta> {
      const after = await snapshot();
      return drcDelta(before, after, scope);
    },
  };
}

// ============================================================================
// Post-placement DRC — the lightweight subset that makes sense before routing
// ============================================================================

/** A short between two nets, with the components whose pads cause it. */
export interface PlacementShort {
  /** The two shorted nets (lexically sorted). */
  nets: [string, string];
  /** Reference designators of the footprints whose pads overlap. */
  refs: string[];
}

/** Placement-stage DRC: the faults that are real *before* any copper is routed
 *  — overlapping pads (shorts), too-close pads (clearance), colliding
 *  courtyards, and components hanging off the board. Trace/via-only rules
 *  (trace width, annular ring, trace clearance) are intentionally excluded:
 *  there are no traces yet. `clean` is the single branch the caller needs. */
export interface PlacementDrc {
  clean: boolean;
  shorts: PlacementShort[];
  clearance_violations: number;
  courtyard_overlaps: number;
  /** Refs of footprints placed off the board outline (or inside a cutout). */
  off_board: string[];
  /** Present when the kernel could not check the board at all (e.g. a malformed
   *  layer name). `clean` is forced `false` — an unverifiable floorplan is NOT a
   *  clean one. */
  unverifiable?: { reason: string; offending_field?: string };
  /** Heuristic EE layout lint (crystal placement, decoupling distance,
   *  connector edge access, high-current vs. sensitive separation). Advisory —
   *  never affects `clean`. Present only when at least one warning fired. */
  layout_lint?: LayoutLintWarning[];
}

/** One heuristic EE-layout warning. Advisory: names the offending refs and the
 *  measured vs. threshold distance so the caller can fix with set_placement. */
export interface LayoutLintWarning {
  kind:
    | "crystal_far_from_ic"
    | "decoupling_cap_far_from_pin"
    | "connector_not_on_edge"
    | "high_current_near_sensitive";
  /** Reference designators involved (offender first). */
  refs: string[];
  /** Measured distance in mm (meaning depends on `kind`). */
  distance_mm: number;
  /** The heuristic threshold that was crossed, in mm. */
  threshold_mm: number;
  /** Human-readable explanation naming refs, pads/nets, and distances. */
  message: string;
}

// Lint thresholds (mm). Heuristics, not physics — chosen from common EE
// layout guidance: crystals want <5mm loop to the oscillator pins, decouplers
// want <3mm to the pin they decouple, connectors belong at the board edge,
// and high-current copper wants standoff from USB/analog signals.
const LINT_CRYSTAL_MAX_MM = 5;
const LINT_DECAP_MAX_MM = 3;
const LINT_CONNECTOR_EDGE_MAX_MM = 5;
const LINT_HIGH_CURRENT_SEP_MIN_MM = 2;

const LINT_GROUND_NET_RE = /gnd|vss|0v\b/i;
const LINT_SUPPLY_NET_RE = /^(v|\+)|vcc|vdd|vbat|vbus|pwr|3v3|5v/i;
const SENSITIVE_NET_RE = /usb|(^|_)d[+-]$|(^|_)dp$|(^|_)dm$|adc|analog|(^|_)ain|vref/i;
const HIGH_CURRENT_CLASS_RE = /pwr|power|high[-_ ]?current|motor|\bhv\b|hi[-_ ]?amp/i;

const refPrefix = (ref: string): string => /^[A-Za-z]+/.exec(ref)?.[0]?.toUpperCase() ?? "";
const dist2d = (a: Vec2, b: Vec2): number => Math.hypot(a.x - b.x, a.y - b.y);
const round2 = (n: number): number => Math.round(n * 100) / 100;

/**
 * Heuristic EE layout lint over a placed (possibly unrouted) board. Pure
 * geometry + net names — no kernel round-trip — so it runs on every
 * place_components / set_placement result. Four checks:
 *  - a crystal (ref Y or X) whose oscillator nets reach an IC (U*) pin more than
 *    5mm away (the RP2040-crystal-11mm-from-XIN failure mode);
 *  - a two-pad decoupling cap (C*, one pad ground-ish, one power-ish) more
 *    than 3mm from the nearest IC pin on the same supply net;
 *  - a connector (ref J, P, CN, or USB) whose pads all sit >5mm from the board edge;
 *  - a pad on a high-current net class closer than 2mm to a USB/analog pad.
 */
export function layoutLint(pcb: Pcb): LayoutLintWarning[] {
  const warnings: LayoutLintWarning[] = [];
  const fps = pcb.footprints;

  // World-space pads, grouped by net.
  interface WPad {
    ref: string;
    prefix: string;
    pad: Pad;
    pos: Vec2;
  }
  const allPads: WPad[] = [];
  const byNet = new Map<string, WPad[]>();
  for (const fp of fps) {
    const prefix = refPrefix(fp.ref);
    for (const pad of fp.pads) {
      const w: WPad = { ref: fp.ref, prefix, pad, pos: padWorld(fp, pad) };
      allPads.push(w);
      if (pad.net) {
        const arr = byNet.get(pad.net) ?? [];
        arr.push(w);
        byNet.set(pad.net, arr);
      }
    }
  }

  // ── Crystal → IC oscillator-pin distance ─────────────────────────────────
  for (const fp of fps) {
    const prefix = refPrefix(fp.ref);
    if (prefix !== "Y" && prefix !== "X" && prefix !== "XTAL") continue;
    // Worst oscillator net: for each non-power crystal net, the nearest IC pad
    // sharing it; report the farthest of those (both XIN and XOUT must be close).
    let worst: { dist: number; icRef: string; icPad: string; net: string } | null = null;
    for (const pad of fp.pads) {
      const net = pad.net;
      if (!net || LINT_GROUND_NET_RE.test(net) || LINT_SUPPLY_NET_RE.test(net)) continue;
      const from = padWorld(fp, pad);
      let nearest: { dist: number; icRef: string; icPad: string } | null = null;
      for (const other of byNet.get(net) ?? []) {
        if (other.ref === fp.ref || other.prefix !== "U") continue;
        const d = dist2d(from, other.pos);
        if (!nearest || d < nearest.dist) {
          nearest = { dist: d, icRef: other.ref, icPad: other.pad.number };
        }
      }
      if (nearest && (!worst || nearest.dist > worst.dist)) worst = { ...nearest, net };
    }
    if (worst && worst.dist > LINT_CRYSTAL_MAX_MM) {
      warnings.push({
        kind: "crystal_far_from_ic",
        refs: [fp.ref, worst.icRef],
        distance_mm: round2(worst.dist),
        threshold_mm: LINT_CRYSTAL_MAX_MM,
        message:
          `Crystal ${fp.ref} is ${round2(worst.dist)}mm from ${worst.icRef} pad ` +
          `${worst.icPad} (net '${worst.net}') — oscillator loops want <` +
          `${LINT_CRYSTAL_MAX_MM}mm; move ${fp.ref} next to its XIN/XOUT pins.`,
      });
    }
  }

  // ── Decoupling cap → supply-pin distance ─────────────────────────────────
  for (const fp of fps) {
    if (refPrefix(fp.ref) !== "C" || fp.pads.length !== 2) continue;
    const nets = fp.pads.map((p) => p.net ?? "");
    const gndIdx = nets.findIndex((n) => LINT_GROUND_NET_RE.test(n));
    const pwrIdx = nets.findIndex((n) => n && LINT_SUPPLY_NET_RE.test(n) && !LINT_GROUND_NET_RE.test(n));
    if (gndIdx < 0 || pwrIdx < 0) continue; // not a decoupler
    const pwrPad = fp.pads[pwrIdx]!;
    const from = padWorld(fp, pwrPad);
    let nearest: { dist: number; icRef: string; icPad: string } | null = null;
    for (const other of byNet.get(pwrPad.net!) ?? []) {
      if (other.ref === fp.ref || other.prefix !== "U") continue;
      const d = dist2d(from, other.pos);
      if (!nearest || d < nearest.dist) {
        nearest = { dist: d, icRef: other.ref, icPad: other.pad.number };
      }
    }
    if (nearest && nearest.dist > LINT_DECAP_MAX_MM) {
      warnings.push({
        kind: "decoupling_cap_far_from_pin",
        refs: [fp.ref, nearest.icRef],
        distance_mm: round2(nearest.dist),
        threshold_mm: LINT_DECAP_MAX_MM,
        message:
          `Decoupling cap ${fp.ref} is ${round2(nearest.dist)}mm from ${nearest.icRef} ` +
          `pad ${nearest.icPad} (net '${pwrPad.net}') — decouplers want <` +
          `${LINT_DECAP_MAX_MM}mm to the pin they decouple.`,
      });
    }
  }

  // ── Connectors at the board edge ─────────────────────────────────────────
  const outline = pcb.outline.vertices ?? [];
  if (outline.length >= 3) {
    const edgeDist = (p: Vec2): number => {
      let min = Infinity;
      for (let i = 0; i < outline.length; i++) {
        const a = outline[i]!;
        const b = outline[(i + 1) % outline.length]!;
        min = Math.min(min, pointSegDist(p, a, b));
      }
      return min;
    };
    for (const fp of fps) {
      const prefix = refPrefix(fp.ref);
      if (prefix !== "J" && prefix !== "P" && prefix !== "CN" && prefix !== "USB") continue;
      const pts = fp.pads.length > 0 ? fp.pads.map((p) => padWorld(fp, p)) : [fp.position];
      const d = Math.min(...pts.map(edgeDist));
      if (d > LINT_CONNECTOR_EDGE_MAX_MM) {
        warnings.push({
          kind: "connector_not_on_edge",
          refs: [fp.ref],
          distance_mm: round2(d),
          threshold_mm: LINT_CONNECTOR_EDGE_MAX_MM,
          message:
            `Connector ${fp.ref} sits ${round2(d)}mm from the board edge — connectors ` +
            `want edge access (<${LINT_CONNECTOR_EDGE_MAX_MM}mm); move it to the outline.`,
        });
      }
    }
  }

  // ── High-current net class vs. USB/analog separation ─────────────────────
  const highCurrentNets = new Set<string>();
  const rules = pcb.rules;
  if (rules) {
    const defaultW = rules.defaultRules.traceWidth;
    const assignments = rules.netClassAssignments ?? {};
    for (const cls of rules.classRules ?? []) {
      const heavy =
        HIGH_CURRENT_CLASS_RE.test(cls.name) || cls.traceWidth >= Math.max(1, 2 * defaultW);
      if (!heavy) continue;
      for (const net of assignments[cls.name] ?? []) highCurrentNets.add(net);
    }
  }
  if (highCurrentNets.size > 0) {
    // One warning per (high-current net, sensitive net) pair at its closest
    // approach — not one per pad pair, which would flood dense boards.
    const closest = new Map<
      string,
      { dist: number; hi: WPad; lo: WPad; hiNet: string; loNet: string }
    >();
    const sensitive = allPads.filter((p) => p.pad.net && SENSITIVE_NET_RE.test(p.pad.net));
    for (const hi of allPads) {
      const hiNet = hi.pad.net;
      if (!hiNet || !highCurrentNets.has(hiNet)) continue;
      for (const lo of sensitive) {
        const loNet = lo.pad.net!;
        if (loNet === hiNet || lo.ref === hi.ref) continue;
        const d = dist2d(hi.pos, lo.pos);
        if (d >= LINT_HIGH_CURRENT_SEP_MIN_MM) continue;
        const key = `${hiNet}\x00${loNet}`;
        const cur = closest.get(key);
        if (!cur || d < cur.dist) closest.set(key, { dist: d, hi, lo, hiNet, loNet });
      }
    }
    for (const c of closest.values()) {
      warnings.push({
        kind: "high_current_near_sensitive",
        refs: [c.hi.ref, c.lo.ref],
        distance_mm: round2(c.dist),
        threshold_mm: LINT_HIGH_CURRENT_SEP_MIN_MM,
        message:
          `High-current net '${c.hiNet}' (${c.hi.ref} pad ${c.hi.pad.number}) is ` +
          `${round2(c.dist)}mm from '${c.loNet}' (${c.lo.ref} pad ${c.lo.pad.number}) — ` +
          `keep ≥${LINT_HIGH_CURRENT_SEP_MIN_MM}mm between high-current and USB/analog pads.`,
      });
    }
  }

  return warnings;
}

/** Pull the two refs and two nets out of a pad↔pad clearance message:
 *  `Clearance violation: pad C1.1 net 'VCC' to pad J1.2 net 'GND': …`. */
function parsePadClearance(
  message: string,
): { refs: [string, string]; nets: [string, string] } | null {
  const m = /pad (\S+?)\.\S+ net '([^']+)' to pad (\S+?)\.\S+ net '([^']+)'/.exec(message);
  if (!m) return null;
  return { refs: [m[1]!, m[3]!], nets: [m[2]!, m[4]!] };
}

/** Sorted, NUL-joined net-pair key so (A,B) and (B,A) collapse. */
function netPairKey(a: string, b: string): string {
  return a <= b ? `${a}\x00${b}` : `${b}\x00${a}`;
}

/** Run the lightweight pre-routing DRC subset against a freshly-placed board.
 *  Reuses the kernel DRC (single source of truth for pad geometry, net-ties,
 *  diff-pairs) and keeps only the rules that are meaningful with no copper —
 *  so it agrees with what `run_drc` will later report. Shared by
 *  `place_components` and `set_placement` so the move→re-check loop never has
 *  to fall through to a full route→DRC pass. */
export async function summarizePlacementDrc(pcb: Pcb): Promise<PlacementDrc> {
  // Heuristic EE lint is pure geometry — runs even when the kernel DRC can't.
  const lint = layoutLint(pcb);
  const withLint = (drcPart: Omit<PlacementDrc, "layout_lint">): PlacementDrc => ({
    ...drcPart,
    ...(lint.length > 0 ? { layout_lint: lint } : {}),
  });
  // `full` so we get every violation, not a capped sample.
  const drc = await drcPcb(pcb, "full");
  // The kernel couldn't parse the board — report it as unverifiable (NOT clean)
  // instead of letting an empty `.details` masquerade as a clean floorplan.
  if (!drc.success) {
    return withLint({
      clean: false,
      shorts: [],
      clearance_violations: 0,
      courtyard_overlaps: 0,
      off_board: [],
      unverifiable: {
        reason: drc.reason,
        ...(drc.offending_field ? { offending_field: drc.offending_field } : {}),
      },
    });
  }
  const viols = drc.details ?? [];

  // Pad↔pad clearance messages carry both refs and nets. With no traces on the
  // board yet, every Clearance violation is pad↔pad.
  const clearanceViols = viols.filter((v) => v.rule === "Clearance");
  const padPairs = clearanceViols
    .map((v) => ({ parsed: parsePadClearance(v.message), actual: v.actual ?? Infinity }))
    .filter(
      (p): p is { parsed: NonNullable<ReturnType<typeof parsePadClearance>>; actual: number } =>
        p.parsed !== null,
    );

  // A short = two different-net pads whose copper overlaps. The kernel Short
  // rule names the nets; the coincident (≈0mm) pad-pair clearance names the
  // refs. Take net-pairs from both signals so a short is caught either way.
  const shortPairs = new Set<string>();
  for (const v of viols) {
    if (v.rule !== "Short") continue;
    const [a, b] = parseNetPair(v.message);
    if (a && b) shortPairs.add(netPairKey(a, b));
  }
  for (const p of padPairs) {
    if (p.actual < 1e-3) shortPairs.add(netPairKey(p.parsed.nets[0], p.parsed.nets[1]));
  }

  const shorts: PlacementShort[] = [...shortPairs].map((key) => {
    const [a, b] = key.split("\x00") as [string, string];
    const refs = new Set<string>();
    for (const p of padPairs) {
      if (netPairKey(p.parsed.nets[0], p.parsed.nets[1]) === key) {
        refs.add(p.parsed.refs[0]);
        refs.add(p.parsed.refs[1]);
      }
    }
    return { nets: [a, b], refs: [...refs] };
  });

  // Genuine clearance violations are too-close pads that are NOT overlapping —
  // the overlaps are already reported as shorts above, so don't double-count.
  const clearanceViolations = clearanceViols.filter(
    (v) => !(typeof v.actual === "number" && v.actual < 1e-3),
  ).length;

  const courtyardOverlaps = viols.filter((v) => v.rule === "CourtyardOverlap").length;

  // Off-board: a footprint origin outside the outline (or inside a cutout) —
  // the same definition set_placement warns on. The kernel edge-clearance rule
  // only covers traces and vias, so footprints need this explicit check.
  const offBoard: string[] = [];
  const outline = pcb.outline.vertices ?? [];
  const cutouts = pcb.outline.cutouts ?? [];
  if (outline.length >= 3) {
    for (const fp of pcb.footprints) {
      const onBoard =
        pointInPolygon(fp.position, outline) &&
        !cutouts.some((c) => c.length >= 3 && pointInPolygon(fp.position, c));
      if (!onBoard) offBoard.push(fp.ref);
    }
  }

  return withLint({
    clean:
      shorts.length === 0 &&
      clearanceViolations === 0 &&
      courtyardOverlaps === 0 &&
      offBoard.length === 0,
    shorts,
    clearance_violations: clearanceViolations,
    courtyard_overlaps: courtyardOverlaps,
    off_board: offBoard,
  });
}

// ============================================================================
// Author-time zone-overlap DRC — the one fault add_zone can create on its own
// ============================================================================

/** One copper-overlap conflict: the candidate pour shares copper with an
 *  existing pour on the same layer that carries a different net. */
export interface ZoneOverlap {
  /** The copper layer both pours sit on. */
  layer: PcbLayer;
  /** `[candidate net, conflicting existing net]`. */
  nets: [string, string];
  /** Axis-aligned bounds of the overlapping copper region, board-local mm. */
  bbox: { min: Vec2; max: Vec2 };
}

/** Pre-author DRC for a copper pour: would this outline overlap an existing
 *  pour on the same layer with a different net? That overlap is a dead short
 *  (the kernel's `pours_touch` Short rule), so `clean` is the single branch the
 *  caller needs before committing the zone — the same shape philosophy as
 *  {@link summarizePlacementDrc}. Pure-geometry (no kernel round-trip) because
 *  the check must localize the conflict with a bbox and run at author time. */
export interface ZoneDrc {
  clean: boolean;
  overlaps: ZoneOverlap[];
}

/** Compare a candidate pour outline against the existing zones, flagging every
 *  same-layer / different-net pour whose copper it overlaps. Same-net pours
 *  (intentional, they merge) and other-layer pours (separated by dielectric)
 *  are never conflicts. */
export function summarizeZoneDrc(
  zones: Zone[],
  candidate: { outline: Vec2[]; net: string; layer: PcbLayer },
): ZoneDrc {
  const overlaps: ZoneOverlap[] = [];
  if (candidate.outline.length >= 3) {
    for (const z of zones) {
      if (z.layer !== candidate.layer || z.net === candidate.net) continue;
      if (!z.outline || z.outline.length < 3) continue;
      const bbox = polygonOverlapBbox(candidate.outline, z.outline);
      if (bbox) overlaps.push({ layer: candidate.layer, nets: [candidate.net, z.net], bbox });
    }
  }
  return { clean: overlaps.length === 0, overlaps };
}

/**
 * A fail-closed DRC verdict for the fab-readiness gate. Unlike {@link drcPcb}
 * (which swallows a kernel failure into "0 violations" and looks clean), this
 * runs DRC through the error-surfacing {@link tryRunDrc} probe so a board that
 * can't be parsed/checked reads as `unverifiable` — NEVER `clean`.
 *
 * - `clean`: DRC ran and found no errors.
 * - `violations`: DRC ran and found ≥1 error (short, clearance, unconnected
 *   net, fab-rule break). The summary carries counts + a representative sample.
 * - `unverifiable`: DRC could not run (kernel missing, or the board threw a
 *   serde/parse trap). `reason` explains; the board is not certifiable.
 */
export type DrcVerdict =
  | { status: "clean"; summary: DrcSummary }
  | { status: "violations"; summary: DrcSummary }
  | { status: "unverifiable"; reason: string };

/** Run the fail-closed DRC verdict against a PCB (shared by validate_for_fab
 *  and the export_gerber clean-DRC gate, so they can never disagree). */
export async function drcVerdict(pcb: Pcb, sampleSize = 20): Promise<DrcVerdict> {
  const probe = await tryRunDrc(pcb);
  if (!probe.ok) {
    return {
      status: "unverifiable",
      reason:
        probe.reason === "unavailable"
          ? `DRC engine unavailable (${probe.message}) — cannot certify the board`
          : `DRC could not run — the board failed to serialize/parse: ${probe.message}`,
    };
  }
  const summary = aggregateDrc(probe.value as unknown as DrcViol[], sampleSize, "summary");
  // Unconnected nets are Error severity in the kernel, so `errors` already
  // captures the "board with unconnected nets" case the fab gate must block.
  return { status: summary.errors === 0 ? "clean" : "violations", summary };
}

/** Run DRC against a session/inline document and return the summary-first payload. */
export async function runDrc(args: Record<string, unknown>) {
  const { doc } = resolveDocInput(args);
  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const detail: "summary" | "full" = args.detail === "full" ? "full" : "summary";
  const sampleSize =
    typeof args.sample_size === "number" ? Math.max(0, Math.round(args.sample_size)) : 20;
  const summary = await drcPcb(pcb, detail, sampleSize);
  // Unverifiable ≠ clean. The kernel could not parse the board, so report it as
  // an error the agent can branch on — never as a passing "0 violations".
  if (!summary.success) return ecadUnverifiable("run_drc", summary);
  return {
    content: [{ type: "text" as const, text: JSON.stringify(summary) }],
  };
}

/** Run ERC checks on a schematic. */
export async function runErc(args: Record<string, unknown>) {
  const { doc } = resolveDocInput(args);

  if (!doc.schematic) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no schematic" }],
      isError: true,
    };
  }

  const sheet = doc.schematic;

  // Hard verifiability gate on the RAW sheet. The rules below judge the
  // *derived* connectivity (synthesizeNettedSheet), which keeps only connected
  // pins — so a malformed field on an unconnected pin would be silently dropped
  // and the board could read as `verified`. Reject a raw sheet the kernel can't
  // even deserialize up front, as a loud "unverifiable". Skipped without WASM so
  // the pure-TS checks still run in WASM-less environments.
  if (await isEcadAvailable()) {
    const ercOutcome = await kernelRunErc(sheet);
    if (ercOutcome.status === "errored") {
      return ecadUnverifiable("run_erc", ercOutcome);
    }
  }

  const violations: Array<{
    severity: string;
    message: string;
    position?: Vec2;
  }> = [];

  // Check for duplicate reference designators
  const refs = new Map<string, number>();
  for (const comp of sheet.components) {
    refs.set(comp.ref, (refs.get(comp.ref) || 0) + 1);
  }
  for (const [ref, count] of refs) {
    if (count > 1) {
      violations.push({
        severity: "Error",
        message: `Duplicate reference designator: ${ref} (appears ${count} times)`,
      });
    }
  }

  // Unconnected pins, judged from the same connectivity model the rest of
  // the pipeline uses (wires + labels + explicit nets, rotation-aware) —
  // so run_erc can never contradict the netlist create_schematic reported.
  const derived = await deriveNets(sheet);
  for (const comp of sheet.components) {
    for (const pin of comp.pins) {
      if (pin.pin_type === "NotConnected") continue;
      if (derived.netByPin.has(pinKey(comp.ref, pin.number))) continue;
      violations.push({
        severity: pin.pin_type === "PowerInput" ? "Error" : "Warning",
        message: `Unconnected pin: ${comp.ref} pin ${pin.number} (${pin.name})`,
        position: pinWorldPosition(comp, pin),
      });
    }
  }

  // Kernel ERC: pin-type conflicts (output driving output) and floating power
  // inputs. These rules need a netlist; the kernel's own netlist is
  // coordinate-only, so we hand it the *derived* connectivity (which also
  // bridges global labels and the explicit `nets` map) re-expressed as an
  // explicit netlist over only the connected pins. That keeps these rules
  // judging exactly the nets create_schematic reported — and, because
  // unconnected pins are excluded, a stray power pin is reported once (above),
  // never doubled here.
  const kernelOutcome = await kernelCheckErc(synthesizeNettedSheet(sheet, derived));
  let verified = false;
  let unverifiedReason: string | undefined;
  if (kernelOutcome.status === "ok") {
    verified = true;
    for (const v of kernelOutcome.violations) {
      if (!isPinTypeOrPowerViolation(v.message)) continue;
      violations.push({
        severity: v.severity,
        message: v.message,
        ...(v.position ? { position: v.position } : {}),
      });
    }
  } else if (kernelOutcome.status === "unavailable") {
    // Fail closed: the kernel rules did not run, so the schematic is not
    // verified for pin-type/power conflicts — never report this as clean.
    unverifiedReason =
      "kernel ECAD WASM unavailable — pin-type conflict and floating-power rules were not evaluated";
  } else {
    unverifiedReason = `kernel ERC could not parse the schematic: ${kernelOutcome.message}`;
  }

  // Errors first, then by message — matches the kernel's ordering convention.
  violations.sort((a, b) => {
    const sev = (a.severity === "Error" ? 0 : 1) - (b.severity === "Error" ? 0 : 1);
    return sev !== 0 ? sev : a.message.localeCompare(b.message);
  });

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          verified,
          ...(unverifiedReason ? { unverified_reason: unverifiedReason } : {}),
          violations: violations.length,
          errors: violations.filter(v => v.severity === "Error").length,
          warnings: violations.filter(v => v.severity === "Warning").length,
          details: violations,
        }),
      },
    ],
  };
}

/** A kernel ERC message we own here vs. one already reported by the TS checks
 *  above (duplicate-ref, unconnected). The kernel's pin-type and power messages
 *  are pinned by its own unit tests ("multiple outputs", "no power source"). */
function isPinTypeOrPowerViolation(message: string): boolean {
  return message.startsWith("Pin conflict on net") || message.includes("has no power source");
}

/**
 * Re-express derived connectivity as a kernel-consumable sheet: the explicit
 * `nets` map carries every connected pin's net, wires/labels are dropped (the
 * map already encodes them), and each component keeps only its connected pins
 * so the kernel's per-pin netlist never invents singleton nets for open pins.
 * Pin electrical types are preserved verbatim, which is what gives the kernel's
 * pin-type and power rules signal.
 */
function synthesizeNettedSheet(sheet: SchematicSheet, derived: DerivedNets): SchematicSheet {
  const connectedByRef = new Map<string, Set<string>>();
  for (const key of derived.netByPin.keys()) {
    const sep = key.indexOf(PIN_SEP);
    const ref = key.slice(0, sep);
    const pin = key.slice(sep + 1);
    let set = connectedByRef.get(ref);
    if (!set) connectedByRef.set(ref, (set = new Set()));
    set.add(pin);
  }
  const nets: Record<string, string[]> = {};
  for (const [name, pins] of derived.nets) nets[name] = pins;
  return {
    ...sheet,
    wires: [],
    labels: [],
    junctions: [],
    components: sheet.components.map((comp) => ({
      ...comp,
      pins: comp.pins.filter((pin) => connectedByRef.get(comp.ref)?.has(pin.number)),
    })),
    nets,
  };
}

/** Export Gerber files for a PCB. */
export async function exportGerber(args: Record<string, unknown>) {
  const { doc } = resolveDocInput(args);
  const outputDir = args.output_dir as string | undefined;
  const pcb = getDocPcb(doc);

  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  const validity = validatePcb(pcb);
  if (!validity.valid) {
    return pcbValidationError("export_gerber", validity, args.document_id ? String(args.document_id) : undefined);
  }

  // Clean-DRC gate (default ON for agents): never emit a fab bundle from a board
  // that isn't DRC-clean. Fail closed — an unverifiable board (won't parse) is
  // blocked too, since shipping Gerbers we couldn't certify is the failure mode
  // this guards against. Pass require_clean_drc:false to force a known-dirty export.
  const requireCleanDrc = args.require_clean_drc !== false;
  if (requireCleanDrc) {
    const verdict = await drcVerdict(pcb);
    if (verdict.status !== "clean") {
      const blocker =
        verdict.status === "unverifiable"
          ? verdict.reason
          : `${verdict.summary.errors} DRC error(s) must be resolved before fabrication`;
      // A blocked export is a VERDICT, not a tool failure — same posture as
      // validate_for_fab's `verdict: "blocked"`. The tool ran, evaluated the
      // gate, and reported a structured refusal with escape hatches; isError
      // here would (and did) make every working gate trip read as a tool
      // crash in clients and telemetry.
      return {
        content: [
          {
            type: "text" as const,
            text: JSON.stringify({
              success: false,
              blocked: true,
              reason: blocker,
              drc: verdict.status === "unverifiable" ? { status: "unverifiable" } : verdict.summary,
              hint:
                "Resolve the DRC errors (run run_drc / validate_for_fab for details). " +
                "To get editable files out of the dirty board now, use export_kicad " +
                "(native .kicad_pcb/.kicad_sch — no DRC gate) or open_in_browser. " +
                "Or re-run export_gerber with require_clean_drc:false to force the Gerber bundle anyway.",
            }),
          },
        ],
      };
    }
  }

  const files = await exportFabFiles(pcb);
  if (files === null) {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: ECAD export unavailable (kernel WASM not loaded)",
        },
      ],
      isError: true,
    };
  }

  if (outputDir) {
    // Node-only path: write the files to disk. Imported dynamically so this
    // module stays loadable in browser bundles (e.g. the HTTP MCP frontend).
    try {
      const fs = await import("node:fs/promises");
      const { resolveWithinRoot } = await import("./safe-path.js");
      // Validate both the directory and each filename against cwd so a
      // crafted output_dir or file name can't escape the workspace.
      const dir = resolveWithinRoot(outputDir);
      await fs.mkdir(dir, { recursive: true });
      for (const f of files) {
        await fs.writeFile(resolveWithinRoot(f.name, dir), f.content, "utf8");
      }
      return {
        content: [
          {
            type: "text" as const,
            text: JSON.stringify({
              success: true,
              message: `Wrote ${files.length} fabrication files`,
              output_dir: outputDir,
              files: files.map((f) => ({ name: f.name, bytes: f.content.length })),
            }),
          },
        ],
      };
    } catch (e) {
      // Sandboxed/hosted servers can't write arbitrary paths — fall through to
      // the inline/offload decision so the caller still gets the files.
      const reason = e instanceof Error ? e.message : String(e);
      return deliverFabFiles(files, `could not write to '${outputDir}': ${reason}`);
    }
  }

  return deliverFabFiles(files);
}

/**
 * Return a fab bundle to the caller without overflowing the model's context.
 * Under the inline cap the files ride back inline (today's behavior); over it,
 * the bundle is written to the artifact store and only a compact
 * { artifact_url, manifest } handle is returned. A ~168 KB Gerber bundle is
 * over the default cap, so it offloads by default — the whole point: the fab
 * files stop transiting model context and an order can reference them by id.
 */
function deliverFabFiles(files: FabFile[], diskFailReason?: string) {
  const total = bundleBytes(files);
  const cap = maxInlineArtifactBytes();
  const base = diskFailReason
    ? `Generated ${files.length} fabrication files (${diskFailReason})`
    : `Generated ${files.length} fabrication files`;

  if (total <= cap) {
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            message: `${base}; returning contents inline`,
            bytes: total,
            files,
          }),
        },
      ],
    };
  }

  const handle = storeArtifact(files);
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          message: `${base}; ${total} bytes exceeds the ${cap}-byte inline limit — written to the artifact store`,
          bytes: total,
          artifact_id: handle.artifact_id,
          artifact_url: handle.artifact_url,
          manifest: handle.manifest,
          expires_at: handle.expires_at,
          // The handle quote_manufacturing / place_order accept verbatim, so the
          // fab files reach an order without ever re-entering model context.
          fab_artifact: {
            artifact_id: handle.artifact_id,
            artifact_url: handle.artifact_url,
            bytes: handle.bytes,
            manifest: handle.manifest,
          },
          note:
            "Fab files are at artifact_url (the manifest lists each file with bytes + sha256; " +
            "download one at <artifact_url>/<file>). Pass artifact_id to quote_manufacturing / " +
            "place_order so the bundle never transits model context.",
        }),
      },
    ],
  };
}

/**
 * Return a linked KiCad project bundle to the caller. When output_dir is
 * writable the three files land on disk; otherwise they ride inline under the
 * cap, or offload to the artifact store above it (same mechanism as Gerbers).
 */
async function deliverKicadProject(
  files: FabFile[],
  name: string,
  outputDir: string | undefined,
  documentId: string | undefined,
) {
  const total = bundleBytes(files);
  const docField = documentId ? { document_id: documentId } : {};

  if (outputDir) {
    try {
      const fs = await import("node:fs/promises");
      const { resolveWithinRoot } = await import("./safe-path.js");
      const dir = resolveWithinRoot(outputDir);
      await fs.mkdir(dir, { recursive: true });
      const paths: string[] = [];
      for (const f of files) {
        const path = resolveWithinRoot(f.name, dir);
        await fs.writeFile(path, f.content, "utf8");
        paths.push(path);
      }
      const payload = {
        success: true,
        format: "kicad_project" as const,
        name,
        bytes: total,
        paths,
        note: `Linked KiCad project written; open ${name}.kicad_pro in KiCad 9 to cross-probe schematic and board.`,
        ...docField,
      };
      return {
        content: [{ type: "text" as const, text: JSON.stringify(payload) }],
        structuredContent: { export_kicad: payload },
      };
    } catch {
      // Sandboxed/hosted host — fall through to inline/artifact delivery.
    }
  }

  const cap = maxInlineArtifactBytes();
  if (total <= cap) {
    const payload = {
      success: true,
      format: "kicad_project" as const,
      name,
      bytes: total,
      files,
      ...docField,
    };
    return {
      content: [{ type: "text" as const, text: JSON.stringify(payload) }],
      structuredContent: { export_kicad: payload },
    };
  }

  const handle = storeArtifact(files);
  const payload = {
    success: true,
    format: "kicad_project" as const,
    name,
    bytes: total,
    artifact_id: handle.artifact_id,
    artifact_url: handle.artifact_url,
    manifest: handle.manifest,
    expires_at: handle.expires_at,
    note:
      "Project files are at artifact_url (the manifest lists each file with bytes + sha256; " +
      "download one at <artifact_url>/<file>).",
    ...docField,
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: { export_kicad: payload },
  };
}

/**
 * Export the session's board or schematic as a native, editable KiCad 9 file.
 *
 * `.kicad_pcb` serializes the board (the inverse of import_pcb); `.kicad_sch`
 * serializes the schematic. The point is a round trip: a human can open the
 * agent's design in KiCad to finish routing the nets the autorouter couldn't
 * close, then re-import. Large outputs respect the inline byte cap — over the
 * cap, the caller is steered to `output_dir` or open_in_browser.
 */
export async function exportKicad(args: Record<string, unknown>) {
  const { doc, documentId } = resolveDocInput(args);
  const filename =
    typeof args.filename === "string" && args.filename.trim()
      ? (args.filename as string)
      : "board.kicad_pcb";
  const outputDir = args.output_dir as string | undefined;

  const ext = filename.includes(".")
    ? filename.toLowerCase().split(".").pop()
    : undefined;

  // Linked project bundle: .kicad_pro (or a bare name) exports all three
  // files — <name>.kicad_pro / .kicad_sch / .kicad_pcb — with board
  // footprints carrying (path …) references to their schematic symbol uuids
  // so KiCad can cross-probe.
  if (ext === "kicad_pro" || ext === undefined) {
    const name = ext === undefined ? filename : filename.slice(0, -".kicad_pro".length);
    if (!name) {
      return ecadError("Project filename must have a basename, e.g. 'board.kicad_pro'.");
    }
    const sheet = (doc as Document & { schematic?: SchematicSheet }).schematic;
    if (!sheet) {
      return ecadError(
        "A KiCad project bundle needs a schematic; this document has none. " +
          "Create one with create_schematic, or export the board alone as .kicad_pcb.",
      );
    }
    const pcb = getDocPcb(doc);
    if (!pcb) {
      return ecadError(
        "A KiCad project bundle needs a board; this document has none. " +
          "Export the schematic alone as .kicad_sch.",
      );
    }
    const bundle = await exportKicadProject(sheet, pcb, name);
    if (!bundle) {
      return ecadError("KiCad project export unavailable (kernel WASM not loaded or outdated)");
    }
    const files = bundle.map(([n, c]) => ({ name: n, content: c }));
    return deliverKicadProject(files, name, outputDir, documentId);
  }

  let content: string | null;
  let format: "kicad_pcb" | "kicad_sch";

  if (ext === "kicad_sch") {
    format = "kicad_sch";
    const sheet = (doc as Document & { schematic?: SchematicSheet }).schematic;
    if (!sheet) {
      return ecadError(
        "Document has no schematic to export. Create one with create_schematic.",
      );
    }
    content = await exportKicadSch(sheet);
  } else if (ext === "kicad_pcb") {
    format = "kicad_pcb";
    const pcb = getDocPcb(doc);
    if (!pcb) {
      return ecadError("Document has no PCB to export.");
    }
    content = await exportKicadPcb(pcb);
  } else {
    return ecadError(
      `Unsupported KiCad extension '.${ext ?? ""}'. Use .kicad_pcb (board) or .kicad_sch (schematic).`,
    );
  }

  if (content === null) {
    return ecadError("KiCad export unavailable (kernel WASM not loaded)");
  }

  const bytes = Buffer.byteLength(content, "utf8");

  // Disk path: write to output_dir when provided and the host allows it.
  if (outputDir) {
    try {
      const fs = await import("node:fs/promises");
      const { resolveWithinRoot } = await import("./safe-path.js");
      const dir = resolveWithinRoot(outputDir);
      await fs.mkdir(dir, { recursive: true });
      const path = resolveWithinRoot(filename, dir);
      await fs.writeFile(path, content, "utf8");
      const payload = { success: true, filename, format, bytes, path, ...(documentId ? { document_id: documentId } : {}) };
      return {
        content: [{ type: "text" as const, text: JSON.stringify(payload) }],
        structuredContent: { export_kicad: payload },
      };
    } catch (e) {
      // Sandboxed/hosted host — fall through to inline delivery below.
      const reason = e instanceof Error ? e.message : String(e);
      const cap = maxInlineExportBytes();
      if (bytes > cap) {
        return ecadError(
          `Could not write to '${outputDir}' (${reason}) and the ${bytes}-byte file is over the ${cap}-byte inline cap. ` +
            "Use open_in_browser, or a writable output_dir.",
        );
      }
      const payload = {
        success: true,
        filename,
        format,
        bytes,
        content,
        note_delivery: `Could not write to '${outputDir}' (${reason}); file content returned inline.`,
        ...(documentId ? { document_id: documentId } : {}),
      };
      return {
        content: [{ type: "text" as const, text: JSON.stringify(payload) }],
        structuredContent: { export_kicad: payload },
      };
    }
  }

  // Inline path: bounded so the file does not flood the model's context.
  const cap = maxInlineExportBytes();
  if (bytes > cap) {
    return ecadError(
      `KiCad file is ${bytes} bytes — over the ${cap}-byte inline limit. ` +
        "Pass output_dir to write it to disk, or use open_in_browser.",
    );
  }
  const payload = {
    success: true,
    filename,
    format,
    bytes,
    content,
    ...(documentId ? { document_id: documentId } : {}),
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: { export_kicad: payload },
  };
}

// ============================================================================
// validate_for_fab — the single fab-readiness oracle
// ============================================================================

/** Pull the offending field name out of a serde error message, when it names
 *  one (`missing field \`thickness\``, `unknown field \`foo\``). Returns null
 *  for location-only messages (`invalid type: null, expected f64 at …`). */
function extractSerdeField(message: string): string | null {
  const m = /(?:missing|unknown) field `([^`]+)`/.exec(message);
  return m ? m[1]! : null;
}

/** A feature present on the board that the fab/export pipeline can't faithfully
 *  represent — surfaced loudly rather than silently producing a wrong bundle. */
interface UnsupportedFeature {
  feature: string;
  count: number;
  detail: string;
  fix: string;
}

/** True for a via that spans the outer copper pair (a normal through-via). */
function isThroughVia(via: Via): boolean {
  const a = via.startLayer;
  const b = via.endLayer;
  return (a === "FCu" && b === "BCu") || (a === "BCu" && b === "FCu");
}

/** Scan the board for features the Gerber/Excellon export can't faithfully
 *  produce. Heuristic and intentionally narrow — it flags the known-unsupported
 *  set, not "everything that could ever be wrong". Today: blind/buried vias,
 *  which our drill writer collapses to plain through-holes. */
function detectUnsupportedFeatures(pcb: Pcb): UnsupportedFeature[] {
  const out: UnsupportedFeature[] = [];

  const blindBuried = pcb.vias.filter((v) => !isThroughVia(v));
  if (blindBuried.length > 0) {
    out.push({
      feature: "blind/buried via",
      count: blindBuried.length,
      detail:
        `${blindBuried.length} via(s) span inner layers (not FCu↔BCu). The Excellon ` +
        "drill export drills every via straight through, so blind/buried vias would " +
        "fabricate as through-holes — electrically wrong.",
      fix: "Re-route these connections with through-vias (FCu↔BCu), or split the board so no via needs controlled depth.",
    });
  }

  return out;
}

/**
 * Full fab-readiness gate in one verdict. Runs every check an agent would
 * otherwise have to assemble by hand — DRC (fail-closed), renderability, and
 * actual Gerber serialization — plus a scan for unsupported features, then
 * rolls them into one `ready` boolean with the exact blockers and suggested
 * fixes. Read-only: it mutates nothing.
 *
 * Fail-closed throughout: a board that can't be parsed/serialized is reported
 * `unverifiable` (never silently "clean" or "ready").
 */
/**
 * Get a routed board fab-ready in one call, and return the DRC-delta receipt
 * that says what was actually achieved.
 *
 * Runs the whole pipeline in the kernel: optional (logged) rule calibration,
 * the verdict ladder over unrouted connections, the strip-and-re-route fix loop,
 * the dangling-copper prune. Mutates the session document with the fixed board.
 *
 * The receipt is the point. On an imported fixture "zero DRC violations" is not
 * achievable, so the report never gives one number — it gives the pair: the same
 * board with all routing stripped (the floor the board arrived with) and the
 * finished board, with the difference charged to the routing. A run that cannot
 * drive that difference to zero comes back `converged: false` with the remaining
 * offenders named, and `export_gerber`'s clean-DRC gate still stands: this is the
 * supported way to GET clean, never a way around the gate.
 */
export async function fabPrep(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  // Fail closed: every claim this tool makes is a DRC claim, so without the
  // kernel there is nothing to say and guessing would be worse than refusing.
  if (!(await isEcadAvailable())) {
    return ecadError(
      "fab_prep requires the kernel DRC/routing engine (ECAD WASM unavailable) — refusing to " +
        "report an unverifiable fab-readiness verdict",
    );
  }

  const dryRun = Boolean(args.dry_run);
  const options = {
    calibrate_rules: Boolean(args.calibrate_rules),
    route_remaining: args.route_remaining === undefined ? true : Boolean(args.route_remaining),
    prune_dangling: args.prune_dangling === undefined ? true : Boolean(args.prune_dangling),
    max_rounds: Math.max(0, Math.round(Number(args.max_rounds ?? 8))),
    accept_rules: Array.isArray(args.accept_rules) ? (args.accept_rules as string[]) : [],
    verdict: {
      budget: Math.max(1, Math.round(Number(args.budget ?? 5_000_000))),
      max_cluster: Math.max(1, Math.round(Number(args.max_cluster ?? 6))),
    },
  };

  const out = await kernelRunFabPrep(pcb, options);
  if (!out.ok) {
    return ecadError(`fab_prep could not run: ${out.message}`);
  }
  const { report, pcb: fixed } = out.value;

  if (!dryRun) {
    // The kernel returns a whole board; copy the copper it changed back onto the
    // session's board rather than swapping the object, so anything else holding
    // a reference to it (preview, receipts) sees the update.
    pcb.traces = fixed.traces;
    pcb.traceArcs = fixed.traceArcs;
    pcb.vias = fixed.vias;
    pcb.rules = fixed.rules;
  }

  const payload = {
    success: true,
    ...(dryRun ? { dry_run: true } : {}),
    converged: report.converged,
    ...(report.blocker ? { blocker: report.blocker } : {}),
    headline: fabPrepHeadline(report),
    // Both numbers, always. A caller that sees only one of them is being told
    // something other than what this run established.
    drc_delta: {
      baseline_total: report.delta.baseline_total,
      final_total: report.delta.final_total,
      route_attributable_total: report.delta.route_attributable_total,
      route_attributable_blocking: report.delta.route_attributable_fixable,
      route_attributable_accepted: report.delta.route_attributable_accepted,
      by_rule: report.delta.rules,
      baseline_note:
        "baseline = this same board with every trace and via stripped, checked under the same " +
        "rules. It is not zero on an imported fixture and is not supposed to be; the router is " +
        "answerable for the difference, not the total.",
    },
    connectivity: report.connectivity,
    ...(report.accepted_rules.length > 0 ? { accepted_rules: report.accepted_rules } : {}),
    calibration: {
      requested: report.calibration_requested,
      applied: report.calibration.applied,
      refused: report.calibration.refused,
    },
    initial_verdict: report.initial_verdict,
    rounds: report.rounds,
    pruned: { traces: report.pruned_traces, vias: report.pruned_vias },
    // Cap the offender list: a non-converging run on a dense board can name
    // thousands, and the full list lives in the board's own DRC.
    offenders: report.delta.offenders.slice(0, 25),
    offender_count: report.delta.offenders.length,
    next_action: report.converged
      ? "export_gerber (the clean-DRC gate will now pass)"
      : "resolve the offenders above (fix_drc / route_nets / set_placement), then re-run fab_prep",
    ...docResultPayload(ctx),
  };
  return { content: [{ type: "text" as const, text: JSON.stringify(payload) }] };
}

/** One-sentence statement of what a fab-prep run established. */
function fabPrepHeadline(report: FabPrepReport): string {
  if (!report.converged) {
    return (
      `NOT FAB-READY — ${report.delta.route_attributable_fixable} route-attributable ` +
      `violation(s) remain (${report.blocker ?? "loop did not converge"})`
    );
  }
  const waived =
    report.delta.route_attributable_accepted > 0
      ? `, ${report.delta.route_attributable_accepted} waived under ${report.accepted_rules.join("+")}`
      : "";
  return (
    `zero route-attributable violations${waived} — ${report.delta.final_total} on the finished ` +
    `board, ${report.delta.baseline_total} on the same board stripped of all routing`
  );
}

export async function validateForFab(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  const blockers: string[] = [];
  const unverifiable: string[] = [];
  const fixes: string[] = [];

  // 1. DRC — fail-closed: a parse failure is 'unverifiable', not clean.
  const drc = await drcVerdict(pcb);
  let drcPayload: Record<string, unknown>;
  if (drc.status === "unverifiable") {
    unverifiable.push(`DRC: ${drc.reason}`);
    drcPayload = { status: "unverifiable", reason: drc.reason };
    fixes.push("Make the board serialize/parse, then re-run validate_for_fab for a real DRC verdict.");
  } else {
    drcPayload = { status: drc.status, ...drc.summary };
    if (drc.status === "violations") {
      const topRules = Object.entries(drc.summary.byRule)
        .sort((a, b) => b[1] - a[1])
        .map(([rule, n]) => `${rule}×${n}`)
        .join(", ");
      blockers.push(`DRC: ${drc.summary.errors} error(s) — ${topRules}`);
      if (drc.summary.categories.connectivity > 0)
        fixes.push("Route the remaining nets (route_nets) — unconnected nets can't fabricate.");
      if (drc.summary.categories.clearance > 0)
        fixes.push("Resolve clearance/short violations (move pads apart or reroute).");
      if (drc.summary.categories.manufacturing > 0)
        fixes.push("Meet the fab rules (widen traces, enlarge drills/annular rings) — see set_design_rules / run_drc.");
    }
  }

  // 2. Renderability — can the board geometry be evaluated/visualized?
  const render = await tryPcbPreviewMeshes(pcb);
  let renderPayload: Record<string, unknown>;
  if (render.ok) {
    if (render.value.length > 0) {
      renderPayload = { ok: true, meshes: render.value.length };
    } else {
      renderPayload = { ok: false, reason: "preview produced no geometry (board solid did not evaluate)" };
      blockers.push("Renderability: the board produced no preview geometry (empty board solid).");
      fixes.push("Check the board outline has ≥3 valid vertices and a positive thickness.");
    }
  } else if (render.reason === "error") {
    renderPayload = { ok: false, reason: render.message };
    blockers.push(`Renderability: board geometry failed to evaluate — ${render.message}`);
  } else {
    renderPayload = { ok: false, unverifiable: true, reason: render.message };
    unverifiable.push(`Renderability: ${render.message}`);
  }

  // 3. Gerber-exportability — actually attempt serialization; on failure report
  //    the exact field if the serde error names one.
  const gerber = await tryExportFabFiles(pcb);
  let gerberPayload: Record<string, unknown>;
  if (gerber.ok) {
    gerberPayload = { ok: true, files: gerber.value.length };
  } else if (gerber.reason === "error") {
    const field = extractSerdeField(gerber.message);
    gerberPayload = { ok: false, ...(field ? { field } : {}), reason: gerber.message };
    blockers.push(
      `Gerber export: serialization failed${field ? ` on field '${field}'` : ""} — ${gerber.message}`,
    );
    fixes.push(
      field
        ? `Provide a valid '${field}' on the board so the fab serializer accepts it.`
        : "Fix the malformed board field the serializer rejected (see the error above).",
    );
  } else {
    gerberPayload = { ok: false, unverifiable: true, reason: gerber.message };
    unverifiable.push(`Gerber export: ${gerber.message}`);
  }

  // 4. Unsupported features — present in the IR but not faithfully fabricable.
  const unsupported = detectUnsupportedFeatures(pcb);
  for (const u of unsupported) {
    blockers.push(`Unsupported feature — ${u.detail}`);
    fixes.push(u.fix);
  }

  // Fail-closed: ready only when nothing is blocked AND nothing is unverifiable.
  const ready = blockers.length === 0 && unverifiable.length === 0;
  const verdict = ready ? "ready" : blockers.length > 0 ? "blocked" : "unverifiable";

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          ready,
          verdict,
          ...docResultPayload(ctx),
          drc: drcPayload,
          renderable: renderPayload,
          gerber_exportable: gerberPayload,
          unsupported_features: unsupported,
          blockers,
          unverifiable,
          suggested_fixes: [...new Set(fixes)],
        }),
      },
    ],
  };
}

/** Calculate trace impedance. */
// ----------------------------------------------------------------------------
// Impedance physics — pure closed-form leaves (IPC-2141-style).
//
// These are the single source of truth: calc_impedance reads them (verify) and
// size_impedance inverts them (optimize), so the impedance an agent solves for
// is bit-identical to the impedance verify later reports. Generalizing these to
// `<S: Scalar>` in the Rust twin (crates/vcad-ecad-sim) is the symbolic-autodiff
// foothold; the TS versions here drive the finite-difference MVP solver below.
// ----------------------------------------------------------------------------

/** Effective microstrip width accounting for copper thickness. */
function microstripWe(w: number, t: number, h: number): number {
  return (
    w +
    (t / Math.PI) *
      Math.log(
        (4 * Math.E) /
          Math.sqrt(
            Math.pow(t / h, 2) + Math.pow(t / (w * Math.PI + 1.1 * t * Math.PI), 2),
          ),
      )
  );
}

/** Single-ended microstrip characteristic impedance (Ω). Monotonic ↓ in w. */
function microstripZ0(w: number, t: number, h: number, er: number): number {
  const we = microstripWe(w, t, h);
  return (87 / Math.sqrt(er + 1.41)) * Math.log((5.98 * h) / (0.8 * we + t));
}

/** Microstrip effective permittivity. */
function microstripErEff(w: number, t: number, h: number, er: number): number {
  const we = microstripWe(w, t, h);
  return (er + 1) / 2 + ((er - 1) / 2) * Math.pow(1 + (12 * h) / we, -0.5);
}

/** Single-ended stripline characteristic impedance (Ω). Monotonic ↓ in w. */
function striplineZ0(w: number, t: number, h: number, er: number): number {
  return (60 / Math.sqrt(er)) * Math.log((4 * h) / (0.67 * Math.PI * (0.8 * w + t)));
}

/** Edge-coupling factor for a differential pair; zDiff = 2·z0·k. ↑ in spacing.
 *  Empirical constants per family, matching the Rust reference
 *  (vcad-ecad-sim/src/impedance.rs): microstrip (0.48, 0.96),
 *  stripline (0.347, 2.9). */
function diffCouplingK(traceType: string, spacing: number, h: number): number {
  const [a, b] = traceType.includes("stripline") ? [0.347, 2.9] : [0.48, 0.96];
  return 1 - a * Math.exp((-b * spacing) / h);
}

/** Single-ended z0 for a trace type (microstrip family vs stripline family). */
function singleEndedZ0(traceType: string, w: number, t: number, h: number, er: number): number {
  return traceType.includes("stripline")
    ? striplineZ0(w, t, h, er)
    : microstripZ0(w, t, h, er);
}

// ----------------------------------------------------------------------------
// Bounded Gauss–Newton / Levenberg–Marquardt — a compact TS prototype of the
// generic LM driver the differentiable-design roadmap extracts in Rust. Solves
// least-squares residuals over 1–3 box-constrained continuous parameters with a
// finite-difference Jacobian (exact-enough for the dense, near-convex MVP).
// ----------------------------------------------------------------------------

/** Solve A·x = b for small dense systems (Gaussian elimination, partial pivot). */
function solveDense(A: number[][], b: number[]): number[] | null {
  const n = b.length;
  const M = A.map((row, i) => [...row, b[i]!]);
  for (let c = 0; c < n; c++) {
    let piv = c;
    for (let r = c + 1; r < n; r++) {
      if (Math.abs(M[r]![c]!) > Math.abs(M[piv]![c]!)) piv = r;
    }
    if (Math.abs(M[piv]![c]!) < 1e-15) return null;
    const tmp = M[c]!;
    M[c] = M[piv]!;
    M[piv] = tmp;
    const pivRow = M[c]!;
    for (let r = 0; r < n; r++) {
      if (r === c) continue;
      const row = M[r]!;
      const f = row[c]! / pivRow[c]!;
      for (let k = c; k <= n; k++) row[k] = row[k]! - f * pivRow[k]!;
    }
  }
  // Gauss–Jordan leaves a diagonal system: x[i] = M[i][n] / M[i][i].
  return M.map((row, i) => row[n]! / row[i]!);
}

const sumSq = (v: number[]) => v.reduce((s, x) => s + x * x, 0);

/**
 * Minimize ‖residual(x)‖² over the box [lo, hi]. Forward-difference Jacobian,
 * Marquardt damping, projected steps. Returns the best x found and its cost.
 */
function lmSolve(
  residual: (x: number[]) => number[],
  x0: number[],
  lo: number[],
  hi: number[],
  maxIter = 80,
): { x: number[]; cost: number; converged: boolean } {
  const clamp = (x: number[]) => x.map((v, i) => Math.min(hi[i]!, Math.max(lo[i]!, v)));
  let x = clamp(x0);
  let r = residual(x);
  let cost = sumSq(r);
  let lambda = 1e-3;
  const n = x.length;

  for (let it = 0; it < maxIter && cost > 1e-12; it++) {
    // Forward-difference Jacobian J (m×n).
    const m = r.length;
    const J: number[][] = Array.from({ length: m }, () => new Array(n).fill(0));
    for (let j = 0; j < n; j++) {
      const step = Math.max(1e-7, Math.abs(x[j]!) * 1e-6);
      const xp = x.slice();
      xp[j]! += step;
      const rp = residual(xp);
      for (let i = 0; i < m; i++) J[i]![j] = (rp[i]! - r[i]!) / step;
    }
    // Normal equations JᵀJ, Jᵀr.
    const JtJ: number[][] = Array.from({ length: n }, () => new Array(n).fill(0));
    const Jtr = new Array(n).fill(0);
    for (let a = 0; a < n; a++) {
      for (let b = 0; b < n; b++) {
        let s = 0;
        for (let i = 0; i < m; i++) s += J[i]![a]! * J[i]![b]!;
        JtJ[a]![b] = s;
      }
      let s = 0;
      for (let i = 0; i < m; i++) s += J[i]![a]! * r[i]!;
      Jtr[a] = s;
    }
    // Damped step, with backtracking on the damping until the cost drops.
    let stepped = false;
    for (let tries = 0; tries < 10 && !stepped; tries++) {
      const A = JtJ.map((row, a) =>
        row.map((v, b) => (a === b ? v + lambda * (Math.abs(v) || 1) : v)),
      );
      const dx = solveDense(A, Jtr.map((v) => -v));
      if (!dx) {
        lambda *= 10;
        continue;
      }
      const xn = clamp(x.map((v, i) => v + dx[i]!));
      const rn = residual(xn);
      const cn = sumSq(rn);
      if (cn < cost) {
        x = xn;
        r = rn;
        cost = cn;
        lambda = Math.max(1e-9, lambda * 0.5);
        stepped = true;
      } else {
        lambda *= 10;
      }
    }
    if (!stepped) break; // converged or stuck at a bound
  }
  return { x, cost, converged: cost < 1e-6 };
}

/** Coarse grid scan for a robust LM seed (handles non-convex landscapes). */
function seedScan(
  residual: (x: number[]) => number[],
  lo: number[],
  hi: number[],
  perDim = 12,
): number[] {
  const n = lo.length;
  const best = { x: lo.slice(), cost: Infinity };
  const idx = new Array(n).fill(0);
  const total = Math.pow(perDim, n);
  for (let c = 0; c < total; c++) {
    const x = idx.map((q, d) => lo[d]! + ((hi[d]! - lo[d]!) * q) / (perDim - 1));
    const cost = sumSq(residual(x));
    if (cost < best.cost) {
      best.cost = cost;
      best.x = x;
    }
    for (let d = 0; d < n; d++) {
      if (++idx[d] < perDim) break;
      idx[d] = 0;
    }
  }
  return best.x;
}

// ============================================================================
// Realized-copper gate — a closed-form PASS is only trustworthy if the copper
// it describes is actually one continuous conductor. The PI/SI calculators are
// pure by default; when a caller ties a result to a board net (document_id +
// net), we verify the realized plane/trace before letting a PASS stand. A PASS
// on a split/absent plane is worse than no number — it is actively misleading.
// ============================================================================

/** Outcome of resolving the realized-copper context for a calculator call. */
type RealizedGate =
  | { kind: "none" }
  | { kind: "incomplete"; message: string }
  | { kind: "unchecked"; reason: string }
  | { kind: "ok"; continuity: NetContinuity };

/**
 * Resolve the realized-copper continuity of a board net referenced alongside a
 * pure calculator call. `document_id` + `net` are both required to verify; one
 * without the other is a usage error, and neither means "model-only".
 */
async function resolveRealizedNet(args: Record<string, unknown>): Promise<RealizedGate> {
  const id = args.document_id ? String(args.document_id) : "";
  const net = typeof args.net === "string" ? args.net.trim() : "";
  if (!id && !net) return { kind: "none" };
  if (!id || !net) {
    return {
      kind: "incomplete",
      message:
        "pass BOTH document_id and net to verify against realized copper (got only one)",
    };
  }
  let pcb: Pcb | null = null;
  try {
    pcb = getDocPcb(getSession(id));
  } catch (e) {
    return { kind: "unchecked", reason: (e as Error).message };
  }
  if (!pcb) return { kind: "unchecked", reason: `document '${id}' has no PCB` };
  const continuity = await kernelNetContinuity(pcb, net);
  if (!continuity) return { kind: "unchecked", reason: "continuity engine unavailable" };
  return { kind: "ok", continuity };
}

/** Compact, agent-facing summary of a net's realized-plane continuity. */
function realizedPlaneReport(c: NetContinuity): Record<string, unknown> {
  return {
    net: c.net,
    realized: c.realized,
    continuous: c.continuous,
    islands: c.islands,
    coverage_pct: Math.round(c.coverage * 1000) / 10,
    connected_pads: c.connected_pads,
    total_pads: c.total_pads,
    stitching_vias: c.vias,
    ...(c.worst_island
      ? {
          worst_island: {
            pad_count: c.worst_island.pad_count,
            node_count: c.worst_island.node_count,
            position: c.worst_island.position,
          },
        }
      : {}),
  };
}

/** Why a net's realized copper fails verification, or null if it's sound. */
function planeBlockReason(c: NetContinuity, conductor: string): string | null {
  if (!c.realized) {
    return `net '${c.net}' has no realized copper — there is no ${conductor} to verify`;
  }
  if (!c.continuous) {
    const pct = Math.round(c.coverage * 1000) / 10;
    return `net '${c.net}' copper is split into ${c.islands} galvanic islands (only ${pct}% of pads reach the main plane) — an electrically open ${conductor}`;
  }
  return null;
}

/**
 * Apply the realized-copper gate to a calculator payload. Attaches the realized
 * plane report, and — when the plane/trace is split or absent — REFUSES the
 * verdict: flips `passKey` (if any) to false and stamps a blocked result with
 * coverage stats, stitching-via count, and the worst island, replacing the
 * model summary. Mutates `payload` in place.
 */
function applyRealizedGate(
  payload: Record<string, unknown>,
  gate: RealizedGate,
  opts: { conductor: string; noun: string; passKey?: string },
): void {
  if (gate.kind === "none") {
    payload.realized_check =
      "model-only — pass document_id + net to verify the number against realized copper";
    return;
  }
  if (gate.kind === "unchecked") {
    payload.realized_check = `not verified against realized copper — ${gate.reason}`;
    return;
  }
  if (gate.kind !== "ok") return;
  const c = gate.continuity;
  payload.realized_plane = realizedPlaneReport(c);
  const reason = planeBlockReason(c, opts.conductor);
  if (!reason) {
    payload.realized_verified = true;
    return;
  }
  if (opts.passKey) payload[opts.passKey] = false;
  payload.blocked = true;
  payload.verdict = "blocked";
  payload.unverifiable_reason = "unverifiable on disconnected plane";
  payload.summary = `${opts.noun} NOT certified — ${reason}. Stitch the copper (add_via / route_nets), then re-run; a closed-form PASS on a dead plane would mislead.`;
}

export async function calcImpedance(args: Record<string, unknown>) {
  const traceWidth = args.trace_width as number;
  const copperThickness = (args.copper_thickness as number) || 0.035;
  const dielectricHeight = args.dielectric_height as number;
  const er = (args.dielectric_er as number) || 4.5;
  const traceType = (args.trace_type as string) || "microstrip";
  const spacing = (args.spacing as number) || 0;

  const h = dielectricHeight;
  const w = traceWidth;
  const t = copperThickness;

  // Route by family so diff_stripline gets the stripline formula — the same
  // dispatch sizeImpedance uses, so the two tools always agree on a geometry.
  const isStriplineFamily = traceType.includes("stripline");
  const z0 = singleEndedZ0(traceType, w, t, h, er);
  // Stripline is fully embedded in the dielectric, so er_eff == er.
  const erEff = isStriplineFamily ? er : microstripErEff(w, t, h, er);
  const delayPsPerMm = 3.336 * Math.sqrt(erEff);

  // Differential pair calculations
  let zDiff: number | undefined;
  if (spacing > 0 && (traceType === "diff_microstrip" || traceType === "diff_stripline")) {
    zDiff = 2 * z0 * diffCouplingK(traceType, spacing, h);
  }

  const result: Record<string, unknown> = {
    z0: Math.round(z0 * 100) / 100,
    er_eff: Math.round(erEff * 1000) / 1000,
    delay_ps_per_mm: Math.round(delayPsPerMm * 1000) / 1000,
    trace_type: traceType,
    inputs: {
      trace_width: traceWidth,
      copper_thickness: copperThickness,
      dielectric_height: dielectricHeight,
      dielectric_er: er,
    },
  };

  if (zDiff !== undefined) {
    result.z_diff = Math.round(zDiff * 100) / 100;
    result.spacing = spacing;
  }

  // Gate on realized copper: an impedance for a trace that isn't actually
  // realized (unrouted / split) is a number with nothing behind it.
  const gate = await resolveRealizedNet(args);
  if (gate.kind === "incomplete") return ecadError(gate.message);
  applyRealizedGate(result, gate, { conductor: "trace", noun: "Impedance" });

  // Receipt claims for the quantities predicted (method reflects the branch
  // actually taken above — see em-claims.ts for the family).
  const model = isStriplineFamily ? "ipc2141-stripline" : "ipc2141-microstrip";
  const claimInputs = {
    trace_width: traceWidth,
    copper_thickness: copperThickness,
    dielectric_height: dielectricHeight,
    dielectric_er: er,
    trace_type: traceType,
  };
  const claims = [
    emClaim("characteristic_impedance", result.z0 as number, "ohm", model, claimInputs),
    emClaim("effective_permittivity", result.er_eff as number, "dimensionless", model, claimInputs),
    emClaim("propagation_delay", result.delay_ps_per_mm as number, "ps/mm", model, claimInputs),
  ];
  if (result.z_diff !== undefined) {
    claims.push(
      emClaim("differential_impedance", result.z_diff as number, "ohm", "edge-coupled-diff-pair", {
        ...claimInputs,
        spacing,
      }),
    );
  }
  result.claims = claims;

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(result),
      },
    ],
  };
}

// ============================================================================
// size_impedance — the first differentiable design tool (intent → optimize)
// ============================================================================

/**
 * Solve trace geometry for a target characteristic impedance — the inverse of
 * calc_impedance, and the first end-to-end intent→optimize→verify loop. PURE:
 * takes a target + stackup, returns the solved width (and spacing, for diff
 * pairs) AS DATA, recomputed-and-checked against the SAME impedance model, then
 * snapped to a fab grid and re-verified. A bounded Gauss–Newton/LM over the
 * continuous geometry (the differentiable sub-problem once the layer/stackup is
 * fixed); DFM min-width/spacing are hard box bounds and a binding floor is
 * REPORTED, never silently clamped. No board, no session, no mutation.
 */
export function sizeImpedance(args: Record<string, unknown>) {
  const fail = ecadError;

  const traceType = (args.trace_type as string) || "microstrip";
  const isDiff = traceType === "diff_microstrip" || traceType === "diff_stripline";
  const h = args.dielectric_height as number;
  const er = (args.dielectric_er as number) ?? 4.5;
  const t = (args.copper_thickness as number) ?? 0.035;
  const targetDiff = isDiff ? ((args.target_diff_z0 as number) ?? 100) : undefined;
  const targetZ0 =
    (args.target_z0 as number) ?? (isDiff ? (targetDiff as number) / 2 : 50);
  const minW = (args.min_width as number) ?? 0.1;
  const maxW = (args.max_width as number) ?? 5;
  const minS = (args.min_spacing as number) ?? minW;
  const maxS = (args.max_spacing as number) ?? 5;
  const grid = (args.fab_grid_mm as number) ?? 0.0254;
  const tolPct = (args.tolerance_pct as number) ?? 5;

  if (!(h > 0)) return fail("dielectric_height must be > 0 mm");
  if (!(t < h)) {
    return fail(
      `copper_thickness (${t}mm) must be < dielectric_height (${h}mm) — the impedance model is invalid otherwise`,
    );
  }
  if (!(maxW > minW)) return fail("max_width must be > min_width");
  if (isDiff && !(maxS > minS)) return fail("max_spacing must be > min_spacing");
  if (!(targetZ0 > 0)) return fail("target_z0 must be > 0");
  if (isDiff && !((targetDiff as number) > 0)) return fail("target_diff_z0 must be > 0");

  const seZ0 = (w: number) => singleEndedZ0(traceType, w, t, h, er);
  const snap = (v: number, loB: number, hiB: number) =>
    Math.min(hiB, Math.max(loB, Math.round(v / grid) * grid));
  const near = (a: number, b: number) => Math.abs(a - b) <= grid * 1.5;

  // Residuals (Ω), the LM box, and the solve.
  const residual = isDiff
    ? (x: number[]) => [
        seZ0(x[0]!) - targetZ0,
        2 * seZ0(x[0]!) * diffCouplingK(traceType, x[1]!, h) - (targetDiff as number),
      ]
    : (x: number[]) => [seZ0(x[0]!) - targetZ0];
  const lo = isDiff ? [minW, minS] : [minW];
  const hi = isDiff ? [maxW, maxS] : [maxW];

  const seed = seedScan(residual, lo, hi);
  const { x: cont } = lmSolve(residual, seed, lo, hi);

  // Snap to the fab grid and RE-VERIFY against the same model.
  const wSnap = snap(cont[0]!, minW, maxW);
  const sSnap = isDiff ? snap(cont[1]!, minS, maxS) : undefined;
  const z0Meas = seZ0(wSnap);
  const diffMeas = isDiff ? 2 * z0Meas * diffCouplingK(traceType, sSnap as number, h) : undefined;

  const within = (meas: number, target: number) =>
    Math.abs(meas - target) <= (tolPct / 100) * target;
  const z0Ok = within(z0Meas, targetZ0);
  const diffOk = isDiff ? within(diffMeas as number, targetDiff as number) : true;
  const withinTolerance = z0Ok && diffOk;

  // A bound that the solution sits on is a binding DFM constraint, not a fit.
  const active: string[] = [];
  if (near(wSnap, minW)) active.push("min_width");
  if (near(wSnap, maxW)) active.push("max_width");
  if (isDiff && near(sSnap as number, minS)) active.push("min_spacing");
  if (isDiff && near(sSnap as number, maxS)) active.push("max_spacing");

  const r2 = (v: number) => Math.round(v * 100) / 100;
  const r4 = (v: number) => Math.round(v * 1e4) / 1e4;

  let summary: string;
  let reason: string | undefined;
  if (withinTolerance) {
    summary = isDiff
      ? `${traceType} → width ${r4(wSnap)}mm, spacing ${r4(sSnap as number)}mm: ${r2(z0Meas)}Ω SE / ${r2(diffMeas as number)}Ω diff, within ±${tolPct}%`
      : `${traceType} → width ${r4(wSnap)}mm: ${r2(z0Meas)}Ω, within ±${tolPct}% of ${targetZ0}Ω`;
  } else {
    const bound = active.length
      ? ` — DFM bound active (${active.join(", ")})`
      : " — outside the searched geometry box";
    reason = isDiff
      ? `closest achievable on this stackup is ${r2(z0Meas)}Ω SE / ${r2(diffMeas as number)}Ω diff (targets ${targetZ0}/${targetDiff})${bound}`
      : `no width in [${minW}, ${maxW}]mm reaches ${targetZ0}Ω on this stackup; closest is ${r4(wSnap)}mm → ${r2(z0Meas)}Ω${bound}`;
    summary = `${traceType}: ${reason}`;
  }

  const payload: Record<string, unknown> = {
    success: true,
    summary,
    trace_type: traceType,
    within_tolerance: withinTolerance,
    tolerance_pct: tolPct,
    width_mm: r4(wSnap),
    ...(isDiff ? { spacing_mm: r4(sSnap as number) } : {}),
    continuous: {
      width_mm: r4(cont[0]!),
      ...(isDiff ? { spacing_mm: r4(cont[1]!) } : {}),
    },
    snapped: !near(cont[0]!, wSnap) || (isDiff ? !near(cont[1]!, sSnap as number) : false),
    fab_grid_mm: grid,
    target: { z0: targetZ0, ...(isDiff ? { diff_z0: targetDiff } : {}) },
    // "Proof": metrics recomputed from the chosen geometry via the same model.
    measured: {
      z0: r2(z0Meas),
      ...(isDiff ? { diff_z0: r2(diffMeas as number) } : {}),
      recomputed_from_geometry: true,
    },
    ...(active.length ? { active_constraints: active } : {}),
    ...(reason ? { reason } : {}),
  };

  // Receipt claims about the recommended (snapped) geometry.
  const seModel = traceType.includes("stripline") ? "ipc2141-stripline" : "ipc2141-microstrip";
  const claimInputs = {
    trace_type: traceType,
    trace_width: r4(wSnap),
    ...(isDiff ? { spacing: r4(sSnap as number) } : {}),
    copper_thickness: t,
    dielectric_height: h,
    dielectric_er: er,
  };
  payload.claims = [
    emClaim("characteristic_impedance", r2(z0Meas), "ohm", seModel, claimInputs),
    ...(isDiff
      ? [
          emClaim(
            "differential_impedance",
            r2(diffMeas as number),
            "ohm",
            "edge-coupled-diff-pair",
            claimInputs,
          ),
        ]
      : []),
  ];

  return { content: [{ type: "text" as const, text: JSON.stringify(payload) }] };
}

// ============================================================================
// size_pdn — gradient sizing of a power-distribution resistor mesh
// ============================================================================

/**
 * Size copper-segment widths across a PDN resistor mesh so each load node's
 * IR-drop meets its budget with minimal copper. Builds the reduced conductance
 * (Laplacian) matrix, solves G·V = I for node voltages via the shared dense
 * solver, and drives drop → budget with the bounded LM tuner (finite-difference
 * Jacobian over the forward solve). PURE: takes a mesh + budgets, returns the
 * per-segment widths AS DATA with the drops recomputed-and-checked from a
 * forward solve. The Rust `PdnSystem` is the scalable kernel backend (analytic
 * implicit-function adjoint); this is the agent-facing path for small meshes.
 */
export async function sizePdn(args: Record<string, unknown>) {
  const fail = ecadError;

  const nodes = Math.round(args.nodes as number);
  const edges = args.edges as Array<{ a: number; b: number; length: number }>;
  const loads = (args.loads as Array<{ node: number; current: number }>) || [];
  const targets = args.targets as Array<{ node: number; max_drop: number }>;
  const t = (args.copper_thickness as number) ?? 0.035;
  const rho = (args.resistivity as number) ?? 1.68e-5;
  const sigma = 1 / rho;
  const minW = (args.min_width as number) ?? 0.1;
  const maxW = (args.max_width as number) ?? 5;
  const grid = (args.fab_grid_mm as number) ?? 0.0254;
  const tolPct = (args.tolerance_pct as number) ?? 5;

  if (!(nodes >= 2)) return fail("nodes must be >= 2 (node 0 is the reference)");
  if (!Array.isArray(edges) || edges.length === 0) return fail("provide at least one edge");
  if (!Array.isArray(targets) || targets.length === 0) return fail("provide at least one target");
  if (!(maxW > minW)) return fail("max_width must be > min_width");

  // Realized-copper gate: if this PDN mesh is tied to a board net, the sizing
  // PASS is only certified when that power plane is galvanically continuous.
  const gate = await resolveRealizedNet(args);
  if (gate.kind === "incomplete") return fail(gate.message);
  const respondPdn = (payload: Record<string, unknown>) => {
    applyRealizedGate(payload, gate, {
      conductor: "plane",
      noun: "PDN sizing",
      passKey: "within_budget",
    });
    return { content: [{ type: "text" as const, text: JSON.stringify(payload) }] };
  };
  // One IR-drop receipt claim per budgeted node — same model regardless of
  // which engine (TS solver or Rust adjoint) produced the widths.
  const pdnClaims = (drops: number[], segments: number) =>
    targets.map((tg, i) =>
      emClaim("ir_drop", Math.round(drops[i]! * 1e6) / 1e6, "V", "dc-resistor-mesh", {
        node: tg.node,
        max_drop_v: tg.max_drop,
        segments,
      }),
    );

  // Optionally route into the Rust kernel engine (implicit-function adjoint)
  // via WASM. Falls through to the TS solver if the artifact isn't available.
  if ((args.engine as string) === "exact" && ecadDiffEngineAvailable()) {
    const exact = sizePdnExact({
      nodes,
      edges,
      loads: loads.map((l) => [l.node, l.current]),
      targets: targets.map((tg) => [tg.node, tg.max_drop]),
      sigma: 1 / rho,
      thickness: t,
      min_width: minW,
      max_width: maxW,
      seed_width: (minW + maxW) / 4,
    });
    if (exact && !exact.error && Array.isArray(exact.widths_mm)) {
      const r6 = (v: number) => Math.round(v * 1e6) / 1e6;
      const widths = (exact.widths_mm as number[]).map((v) => Math.round(v * 1e4) / 1e4);
      const drops = exact.drops_v as number[];
      const withinBudget = targets.every((tg, i) => drops[i]! <= tg.max_drop * (1 + tolPct / 100));
      const overBudget = targets
        .map((tg, i) => ({ node: tg.node, drop: r6(drops[i]!), budget: tg.max_drop }))
        .filter((x) => x.drop > x.budget * (1 + tolPct / 100));
      return respondPdn({
        success: true,
        engine: "rust-adjoint",
        summary: withinBudget
          ? `sized ${widths.length} segment(s) via the Rust adjoint engine; all ${targets.length} node(s) within budget`
          : `Rust engine: ${overBudget.length} node(s) over budget within the width bounds`,
        within_budget: withinBudget,
        tolerance_pct: tolPct,
        widths_mm: widths,
        measured_drops_v: drops.map(r6),
        targets,
        converged: exact.converged,
        ...(overBudget.length ? { over_budget: overBudget } : {}),
        claims: pdnClaims(drops, widths.length),
      });
    }
  }

  const m = nodes - 1; // reduced system size (node 0 eliminated)
  const reduced = (n: number) => (n === 0 ? -1 : n - 1);
  const conductance = (w: number, len: number) => (sigma * t * w) / len;

  const injection = () => {
    const inj = new Array<number>(m).fill(0);
    for (const l of loads) {
      const r = reduced(l.node);
      if (r >= 0) inj[r] = -l.current; // a load draws current → negative injection
    }
    return inj;
  };
  const buildG = (widths: number[]) => {
    const g: number[][] = Array.from({ length: m }, () => new Array<number>(m).fill(0));
    edges.forEach((e, i) => {
      const c = conductance(widths[i]!, e.length);
      const ra = reduced(e.a);
      const rb = reduced(e.b);
      if (ra >= 0) g[ra]![ra]! += c;
      if (rb >= 0) g[rb]![rb]! += c;
      if (ra >= 0 && rb >= 0) {
        g[ra]![rb]! -= c;
        g[rb]![ra]! -= c;
      }
    });
    return g;
  };
  const forward = (widths: number[]) => solveDense(buildG(widths), injection());
  const vfull = (v: number[] | null, node: number) => {
    const r = reduced(node);
    return r < 0 || !v ? 0 : v[r]!;
  };
  const dropsOf = (widths: number[]) => {
    const v = forward(widths);
    return targets.map((tg) => -vfull(v, tg.node));
  };

  // The mesh must be solvable at the seed (connected to the reference).
  if (!forward(new Array<number>(edges.length).fill((minW + maxW) / 2))) {
    return fail("PDN mesh is singular — every node needs a conductive path to node 0");
  }

  const ne = edges.length;
  const lo = new Array<number>(ne).fill(minW);
  const hi = new Array<number>(ne).fill(maxW);
  const residual = (widths: number[]) => {
    const v = forward(widths);
    if (!v) return targets.map(() => 1e6);
    return targets.map((tg) => -vfull(v, tg.node) - tg.max_drop);
  };
  // Seed analytically: scaling EVERY width by k scales every conductance by k,
  // so G→kG, V→V/k, and every IR-drop→drop/k exactly. So the uniform width that
  // hits the tightest budget is one reference solve away — it lands LM on (or
  // just outside) the feasible set, and it only has to refine. This sidesteps
  // the 1/w curvature that stalls Gauss-Newton from a far seed, and unlike
  // size_impedance's grid seed-scan it stays O(1) in the segment count.
  const wRef = Math.min(maxW, Math.max(minW, 1.0));
  const dropsRef = dropsOf(new Array<number>(ne).fill(wRef));
  let k = 0;
  targets.forEach((tg, i) => {
    if (dropsRef[i]! > 0 && tg.max_drop > 0) k = Math.max(k, dropsRef[i]! / tg.max_drop);
  });
  if (!(k > 0)) k = 1;
  const seed = new Array<number>(ne).fill(Math.min(maxW, Math.max(minW, wRef * k)));
  const { x: cont } = lmSolve(residual, seed, lo, hi, 200);

  // Snap UP to the fab grid: more copper than the continuous optimum, so a met
  // budget stays met after quantization (never silently nudged over).
  const snap = (v: number) => Math.min(maxW, Math.max(minW, Math.ceil(v / grid) * grid));
  const widths = cont.map(snap);
  const measured = dropsOf(widths);

  const r4 = (v: number) => Math.round(v * 1e4) / 1e4;
  const r6 = (v: number) => Math.round(v * 1e6) / 1e6;
  const withinBudget = targets.every((tg, i) => measured[i]! <= tg.max_drop * (1 + tolPct / 100));

  const active: string[] = [];
  const near = (v: number, b: number) => Math.abs(v - b) <= grid * 1.5;
  if (widths.some((w) => near(w, minW))) active.push("min_width");
  if (widths.some((w) => near(w, maxW))) active.push("max_width");

  const overBudget = targets
    .map((tg, i) => ({ node: tg.node, drop: r6(measured[i]!), budget: tg.max_drop }))
    .filter((x) => x.drop > x.budget * (1 + tolPct / 100));

  const summary = withinBudget
    ? `sized ${ne} segment(s); all ${targets.length} node(s) within their IR-drop budget (±${tolPct}%)`
    : `cannot meet budget on ${overBudget.length} node(s) within [${minW}, ${maxW}]mm copper — ${
        active.includes("max_width") ? "widen max_width or shorten segments" : "mesh-limited"
      }`;

  return respondPdn({
    success: true,
    summary,
    within_budget: withinBudget,
    tolerance_pct: tolPct,
    widths_mm: widths.map(r4),
    continuous_widths_mm: cont.map(r4),
    measured_drops_v: measured.map(r6),
    targets,
    fab_grid_mm: grid,
    ...(active.length ? { active_constraints: active } : {}),
    ...(overBudget.length ? { over_budget: overBudget } : {}),
    // One IR-drop claim per budgeted node, at the sized copper widths.
    claims: pdnClaims(measured, ne),
  });
}

// ============================================================================
// calc_coil / size_coil — planar-magnetics analyzer + sizer
// ============================================================================

/**
 * Analyze a planar spiral coil: inductance (modified Wheeler), DC resistance,
 * copper length, and L/R time constant. The evaluate-family analyzer for the
 * planar-magnetics archetype (inductors, sensor coils, motor stators), built on
 * the same closed-form leaves the differentiable kernel exposes. PURE.
 */
export function calcCoil(args: Record<string, unknown>) {
  const fail = ecadError;
  const innerR = args.inner_radius as number;
  const outerR = args.outer_radius as number;
  const turns = args.turns as number;
  const w = args.trace_width as number;
  const t = (args.copper_thickness as number) ?? 0.035;
  const rho = (args.resistivity as number) ?? 1.68e-5;
  const geometry = (args.geometry as string) || "circular";

  if (!(innerR >= 0)) return fail("inner_radius must be >= 0");
  if (!(outerR > innerR)) return fail("outer_radius must be > inner_radius");
  if (!(turns > 0)) return fail("turns must be > 0");
  if (!(w > 0)) return fail("trace_width must be > 0");

  const inductanceNh = coilInductanceNh(turns, innerR, outerR, geometry);
  const wireLen = coilWireLengthMm(turns, innerR, outerR);
  const resistance = (rho * wireLen) / (w * t);
  const r3 = (v: number) => Math.round(v * 1000) / 1000;

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          geometry,
          turns,
          inductance_nh: r3(inductanceNh),
          dc_resistance_ohm: r3(resistance),
          wire_length_mm: r3(wireLen),
          // L/R time constant in microseconds.
          time_constant_us: r3((inductanceNh * 1e-9) / resistance / 1e-6),
          inputs: { inner_radius: innerR, outer_radius: outerR, trace_width: w, copper_thickness: t },
          claims: [
            emClaim("inductance", r3(inductanceNh), "nH", "wheeler-mohan-1999", {
              turns,
              inner_radius: innerR,
              outer_radius: outerR,
              geometry,
            }),
            emClaim("dc_resistance", r3(resistance), "ohm", "dc-trace-resistance", {
              wire_length_mm: r3(wireLen),
              trace_width: w,
              copper_thickness: t,
              resistivity: rho,
            }),
          ],
        }),
      },
    ],
  };
}

/**
 * Inverse of calc_coil: solve the turn count for a target inductance in a given
 * annulus. Wheeler's L ∝ turns², so the turn count is closed-form; we report the
 * continuous and integer-rounded turns, the inductance actually achieved, and
 * whether that many turns physically fit the radial band (else fit-limited).
 * PURE — returns data, builds no geometry.
 */
export function sizeCoil(args: Record<string, unknown>) {
  const fail = ecadError;
  const targetNh = args.target_inductance_nh as number;
  const innerR = args.inner_radius as number;
  const outerR = args.outer_radius as number;
  const w = args.trace_width as number;
  const clearance = (args.clearance as number) ?? w;
  const t = (args.copper_thickness as number) ?? 0.035;
  const rho = (args.resistivity as number) ?? 1.68e-5;
  const geometry = (args.geometry as string) || "circular";
  const tolPct = (args.tolerance_pct as number) ?? 5;

  if (!(targetNh > 0)) return fail("target_inductance_nh must be > 0");
  if (!(innerR >= 0)) return fail("inner_radius must be >= 0");
  if (!(outerR > innerR)) return fail("outer_radius must be > inner_radius");
  if (!(w > 0)) return fail("trace_width must be > 0");

  // Invert L = K1·μ0·n²·d_avg / (1 + K2·ρ)  →  n = sqrt(L·(1+K2ρ)/(K1·μ0·d_avg)).
  const { k1, k2 } = COIL_GEOMETRY[geometry] ?? COIL_GEOMETRY.circular!;
  const dAvgM = (innerR + outerR) * 1e-3;
  const fill = (outerR - innerR) / (outerR + innerR);
  const nContinuous = Math.sqrt((targetNh * 1e-9 * (1 + k2 * fill)) / (k1 * MU0 * dAvgM));

  // How many turns physically fit the radial band at this pitch?
  const pitch = w + clearance;
  const maxTurnsFit = Math.floor((outerR - innerR) / pitch);
  const turns = Math.max(1, Math.min(maxTurnsFit, Math.round(nContinuous)));
  const fits = Math.round(nContinuous) <= maxTurnsFit && nContinuous >= 1;

  const achievedNh = coilInductanceNh(turns, innerR, outerR, geometry);
  const wireLen = coilWireLengthMm(turns, innerR, outerR);
  const resistance = (rho * wireLen) / (w * t);
  const within = Math.abs(achievedNh - targetNh) <= (tolPct / 100) * targetNh;
  const r3 = (v: number) => Math.round(v * 1000) / 1000;

  const summary = fits
    ? `${turns} turn(s) → ${r3(achievedNh)}nH (target ${targetNh}nH, ${within ? "within" : "outside"} ±${tolPct}%)`
    : `target ${targetNh}nH needs ${nContinuous.toFixed(1)} turns but only ${maxTurnsFit} fit the ${r3(outerR - innerR)}mm band at ${pitch}mm pitch — widen the annulus or thin the trace`;

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          summary,
          geometry,
          turns,
          turns_continuous: r3(nContinuous),
          target_inductance_nh: targetNh,
          achieved_inductance_nh: r3(achievedNh),
          within_tolerance: within && fits,
          fits,
          max_turns_fit: maxTurnsFit,
          dc_resistance_ohm: r3(resistance),
          wire_length_mm: r3(wireLen),
          // Claims describe the integer-turn coil actually recommended.
          claims: [
            emClaim("inductance", r3(achievedNh), "nH", "wheeler-mohan-1999", {
              turns,
              inner_radius: innerR,
              outer_radius: outerR,
              geometry,
            }),
            emClaim("dc_resistance", r3(resistance), "ohm", "dc-trace-resistance", {
              wire_length_mm: r3(wireLen),
              trace_width: w,
              copper_thickness: t,
              resistivity: rho,
            }),
          ],
        }),
      },
    ],
  };
}

// ============================================================================
// calc_rf — RLC frequency-domain (AC) analysis: |Z(f)|, S11, resonance, Q
// ============================================================================

/**
 * Frequency-domain (AC) analysis of an RLC resonator — the RF/AC analyzer the
 * surface was missing (calc_impedance is geometry-only; CircuitSim is transient
 * only). Sweeps the complex impedance over frequency and reports |Z|, phase,
 * and S11/return-loss against a reference Z0, plus resonance, Q, and the best
 * match in the band. Closed-form, deterministic, PURE.
 */
export function calcRf(args: Record<string, unknown>) {
  const fail = ecadError;
  const topology = (args.topology as string) || "series_rlc";
  const r = args.r_ohm as number;
  const l = args.l_henry as number;
  const c = args.c_farad as number;
  const z0 = (args.z0_ohm as number) ?? 50;
  if (!(r >= 0)) return fail("r_ohm must be >= 0");
  if (!(l > 0)) return fail("l_henry must be > 0");
  if (!(c > 0)) return fail("c_farad must be > 0");
  if (topology !== "series_rlc" && topology !== "parallel_rlc") {
    return fail("topology must be 'series_rlc' or 'parallel_rlc'");
  }

  const f0 = 1 / (2 * Math.PI * Math.sqrt(l * c)); // resonant frequency
  const q = topology === "series_rlc"
    ? (r > 0 ? (1 / r) * Math.sqrt(l / c) : Infinity)
    : r * Math.sqrt(c / l);
  const fStart = (args.f_start_hz as number) ?? f0 * 0.1;
  const fStop = (args.f_stop_hz as number) ?? f0 * 10;
  const points = Math.min(256, Math.max(3, Math.round((args.points as number) ?? 21)));
  if (!(fStop > fStart && fStart > 0)) return fail("require 0 < f_start_hz < f_stop_hz");

  // Impedance at one frequency, per topology → {re, im}.
  const impedance = (f: number): [number, number] => {
    const w = 2 * Math.PI * f;
    if (topology === "series_rlc") {
      return [r, w * l - 1 / (w * c)];
    }
    // Parallel RLC: Y = 1/R + j(ωC − 1/ωL); Z = 1/Y.
    const gRe = r > 0 ? 1 / r : 1e12;
    const gIm = w * c - 1 / (w * l);
    const den = gRe * gRe + gIm * gIm;
    return [gRe / den, -gIm / den];
  };
  const mag = (re: number, im: number) => Math.hypot(re, im);
  // |S11| with S11 = (Z − Z0)/(Z + Z0), Z complex, Z0 real.
  const returnLossDb = (zre: number, zim: number) => {
    const s11 = mag(zre - z0, zim) / mag(zre + z0, zim);
    return s11 > 0 ? -20 * Math.log10(s11) : Infinity;
  };

  const samples: Array<{ f_hz: number; z_mag_ohm: number; z_phase_deg: number; return_loss_db: number }> = [];
  let best = { f_hz: 0, return_loss_db: -Infinity };
  const logStart = Math.log10(fStart);
  const logStop = Math.log10(fStop);
  for (let i = 0; i < points; i++) {
    const f = Math.pow(10, logStart + ((logStop - logStart) * i) / (points - 1));
    const [zre, zim] = impedance(f);
    const rl = returnLossDb(zre, zim);
    samples.push({
      f_hz: Math.round(f),
      z_mag_ohm: Math.round(mag(zre, zim) * 1000) / 1000,
      z_phase_deg: Math.round((Math.atan2(zim, zre) * 180) / Math.PI * 100) / 100,
      return_loss_db: Number.isFinite(rl) ? Math.round(rl * 100) / 100 : 999,
    });
    if (rl > best.return_loss_db) best = { f_hz: Math.round(f), return_loss_db: rl };
  }

  const [z0re, z0im] = impedance(f0);
  const r3 = (v: number) => Math.round(v * 1000) / 1000;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          topology,
          resonance_hz: Math.round(f0),
          q_factor: Number.isFinite(q) ? r3(q) : 999999,
          z_at_resonance_ohm: r3(mag(z0re, z0im)),
          best_match: {
            f_hz: best.f_hz,
            return_loss_db: Number.isFinite(best.return_loss_db) ? r3(best.return_loss_db) : 999,
          },
          z0_ohm: z0,
          samples,
          claims: [
            emClaim("resonant_frequency", Math.round(f0), "Hz", "rlc-analytic", {
              topology,
              r_ohm: r,
              l_henry: l,
              c_farad: c,
            }),
            ...(Number.isFinite(q)
              ? [
                  emClaim("q_factor", r3(q), "dimensionless", "rlc-analytic", {
                    topology,
                    r_ohm: r,
                    l_henry: l,
                    c_farad: c,
                  }),
                ]
              : []),
          ],
        }),
      },
    ],
  };
}

// ============================================================================
// add_coil — first-class spiral trace primitive
// ============================================================================

const round3 = (v: number) => Math.round(v * 1000) / 1000;

/**
 * Generate an Archimedean spiral of copper traces — the primitive a PCB
 * motor stator or planar inductor is actually made of. Validates the
 * turn-to-turn gap against clearance, assigns every segment to a net, and
 * can drop a via at the inner endpoint (which is otherwise trapped).
 */
export async function addCoil(
  args: Record<string, unknown>,
  opts?: {
    /** Composite tools (add_coil_array / add_motor_winding) wrap the WHOLE
     *  batch in one drc_delta capture — per-coil snapshots would multiply the
     *  DRC cost by the coil count for no extra signal. */
    skipDrcDelta?: boolean;
  },
) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: Document has no PCB — run place_components first (or open a document that has a board)",
        },
      ],
      isError: true,
    };
  }

  const center = args.center as Vec2;
  const turns = args.turns as number;
  const innerR = args.inner_radius as number;
  const outerR = args.outer_radius as number;
  const traceWidth = args.trace_width as number;
  const net = String(args.net ?? "");
  const direction = (args.direction as string) || "ccw";
  const startAngleDeg = (args.start_angle_deg as number) || 0;
  const segmentsPerTurn = Math.min(
    720,
    Math.max(12, Math.round((args.segments_per_turn as number) || 48)),
  );
  const clearance = (args.clearance as number) ?? pcb.rules.defaultRules.clearance;

  const fail = ecadError;

  if (!center || typeof center.x !== "number" || typeof center.y !== "number") {
    return fail("center must be {x, y} in mm");
  }
  if (!(turns > 0)) return fail("turns must be > 0");
  if (!(innerR >= 0)) return fail("inner_radius must be >= 0");
  if (!(outerR > innerR)) return fail("outer_radius must be > inner_radius");
  if (!(traceWidth > 0)) return fail("trace_width must be > 0");
  if (!net) return fail("net is required — coil copper must belong to a net");
  const layerRes = validateCopperLayer((args.layer as string) || "FCu");
  if ("error" in layerRes) return fail(layerRes.error);
  const layer = layerRes.layer;
  if (direction !== "ccw" && direction !== "cw") {
    return fail(`direction must be "ccw" or "cw", got "${direction}"`);
  }

  // The radial pitch must fit the trace plus the turn-to-turn gap.
  const pitch = (outerR - innerR) / turns;
  const gap = pitch - traceWidth;
  if (gap + 1e-9 < clearance) {
    const maxTurns = Math.floor((outerR - innerR) / (traceWidth + clearance));
    return fail(
      `coil doesn't fit: radial pitch ${pitch.toFixed(3)}mm leaves a ${gap.toFixed(3)}mm gap between turns, ` +
        `below the ${clearance}mm clearance. With trace_width ${traceWidth}mm the annulus ` +
        `${innerR}–${outerR}mm fits at most ${maxTurns} turn(s) — reduce turns, narrow the trace, ` +
        `or widen the annulus.`,
    );
  }

  // Sample the spiral: r grows linearly with angle (Archimedean). Samples
  // that round to the previous point are dropped — no zero-length traces.
  const sign = direction === "cw" ? -1 : 1;
  const theta0 = (startAngleDeg * Math.PI) / 180;
  const steps = Math.max(2, Math.ceil(turns * segmentsPerTurn));
  const pts: Vec2[] = [];
  for (let s = 0; s <= steps; s++) {
    const t = s / steps;
    const theta = theta0 + sign * t * turns * 2 * Math.PI;
    const r = innerR + t * (outerR - innerR);
    const p = {
      x: round3(center.x + r * Math.cos(theta)),
      y: round3(center.y + r * Math.sin(theta)),
    };
    const prev = pts[pts.length - 1];
    if (!prev || prev.x !== p.x || prev.y !== p.y) pts.push(p);
  }

  // Tangential lead-out: prepend a terminal off the inner spoke so the inner
  // via no longer lands on the same radius as the outer endpoint (a same-net
  // bypass-short hazard). Tangent at the inner end (angle θ0): radial dir is
  // (cosθ0, sinθ0); the tangent that follows the winding sense is sign·(-sinθ0, cosθ0).
  const innerLeadOut = (args.inner_lead_out as number) || 0;
  if (innerLeadOut > 0 && pts.length) {
    const tx = -Math.sin(theta0) * sign;
    const ty = Math.cos(theta0) * sign;
    const T = {
      x: round3(pts[0].x + tx * innerLeadOut),
      y: round3(pts[0].y + ty * innerLeadOut),
    };
    if (T.x !== pts[0].x || T.y !== pts[0].y) pts.unshift(T);
  }

  // The coil's copper (spiral + lead-out + any inner/stitch vias, which all
  // land on spiral points) stays inside the sampled polyline's bbox.
  const drcCap = opts?.skipDrcDelta
    ? null
    : await beginDrcDelta(
        pcb,
        boundsOfPoints(pts, Math.max(traceWidth, pcb.rules.defaultRules.viaDiameter) / 2),
      );

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  // ---- Multilayer stacked coil ------------------------------------------
  const layersIn = Array.isArray(args.layers) ? (args.layers as string[]) : undefined;
  if (layersIn && layersIn.length >= 2) {
    for (const l of layersIn) {
      const lr = validateCopperLayer(l);
      if ("error" in lr) return fail(`layers entry: ${lr.error}`);
    }
    const stitchVias: Array<{ position: Vec2; startLayer: PcbLayer; endLayer: PcbLayer }> = [];

    // The stack is ONE continuous spiral in θ: the angle marches monotonically
    // across every layer while the radius zigzags inner↔outer, with a stitch via
    // at each turnaround. That constant angular sweep is the whole point — series
    // current then circulates the same way on every layer, so their axial fields
    // ADD. (Reusing one spiral on all layers instead makes current run outer→inner
    // on even layers and inner→outer on odd ones through identical copper, which
    // reverses the circulation each layer and cancels the stack out.)
    //
    // Consecutive layers also meet at exactly the same (r, θ) by construction, so
    // the stitch vias land on a shared point without any extra alignment work.
    const dTheta = sign * turns * 2 * Math.PI;
    const spiralSteps = Math.max(2, Math.ceil(turns * segmentsPerTurn));

    // Turnaround angles, staggered. Turnarounds at the same radius recur every
    // two layers, and for an integer `turns` they would otherwise land on the
    // identical spoke — stacking two vias in one hole. Fan each successive
    // turnaround by `stagger` so same-radius holes clear each other; the binding
    // case is the inner radius, where a given angle buys the least arc.
    const drill = pcb.rules.defaultRules.viaDrill;
    const holePitch = drill + 0.5 + 0.05; // hole-to-hole minimum, plus margin
    const staggerR = Math.max(innerR, traceWidth);
    const stagger = Math.min(Math.PI / 4, holePitch / (2 * staggerR));
    // Boundary li sits between layer li and li+1; boundary -1 is the free start.
    const boundary = (li: number) =>
      li < 0 ? theta0 + dTheta : theta0 - li * dTheta - sign * (li + 1) * stagger;

    const layerPts: Vec2[][] = [];
    for (let li = 0; li < layersIn.length; li++) {
      // Every layer sweeps the same way — that is what makes the fields add.
      // Even layers run outer→inner, odd layers inner→outer.
      const thetaFrom = boundary(li - 1);
      const thetaTo = boundary(li);
      const outward = li % 2 === 1;
      const rFrom = outward ? innerR : outerR;
      const rTo = outward ? outerR : innerR;
      const lp: Vec2[] = [];
      for (let s = 0; s <= spiralSteps; s++) {
        const t = s / spiralSteps;
        const theta = thetaFrom + t * (thetaTo - thetaFrom);
        const r = rFrom + t * (rTo - rFrom);
        const p = {
          x: round3(center.x + r * Math.cos(theta)),
          y: round3(center.y + r * Math.sin(theta)),
        };
        const prev = lp[lp.length - 1];
        if (!prev || prev.x !== p.x || prev.y !== p.y) lp.push(p);
      }
      layerPts.push(lp);
    }

    // Tangential lead-out at each inner turnaround: with integer `turns` the
    // inner and outer terminals share a spoke, so the stitch via would otherwise
    // sit at the same angle as the outer endpoint (a same-net bypass-short
    // hazard). Displace both sides of the turnaround to the same new point so the
    // via still lands on shared copper.
    if (innerLeadOut > 0) {
      for (let li = 0; li + 1 < layersIn.length; li++) {
        if (li % 2 !== 0) continue; // odd→even turnarounds are at the outer radius
        const end = layerPts[li][layerPts[li].length - 1];
        const thetaEnd = boundary(li);
        // Continue along the direction of travel (the sweep runs by −dθ).
        const tx = Math.sin(thetaEnd) * sign;
        const ty = -Math.cos(thetaEnd) * sign;
        const T = {
          x: round3(end.x + tx * innerLeadOut),
          y: round3(end.y + ty * innerLeadOut),
        };
        if (T.x !== end.x || T.y !== end.y) {
          layerPts[li].push(T);
          layerPts[li + 1].unshift(T);
        }
      }
    }

    let totalLengthMm = 0;
    let totalTraces = 0;
    let totalResistance = 0;
    for (let li = 0; li < layersIn.length; li++) {
      const lyr = layersIn[li] as PcbLayer;
      const lp = layerPts[li];
      for (let i = 0; i + 1 < lp.length; i++) {
        pcb.traces.push({
          start: lp[i],
          end: lp[i + 1],
          width: traceWidth,
          layer: lyr,
          net,
          source: "manual",
        });
        totalTraces++;
      }
      const layerLen = segLength(lp);
      totalLengthMm += layerLen;
      const cuT =
        pcb.stackup.layers.find((s) => s.layer === lyr)?.copperThickness ?? 0.035;
      totalResistance += (1.68e-5 * layerLen) / (traceWidth * cuT);
      // Stitch to the next layer at this layer's exit terminal, which is exactly
      // where the next layer begins.
      if (li + 1 < layersIn.length) {
        const stitchPt = lp[lp.length - 1];
        pcb.vias.push({
          position: stitchPt,
          diameter: pcb.rules.defaultRules.viaDiameter,
          drill: pcb.rules.defaultRules.viaDrill,
          startLayer: lyr,
          endLayer: layersIn[li + 1] as PcbLayer,
          net,
          source: "manual",
        });
        stitchVias.push({ position: stitchPt, startLayer: lyr, endLayer: layersIn[li + 1] as PcbLayer });
      }
    }
    // External terminals: layer[0] starts at the outer radius; the last layer's
    // free end is inner when the layer count is odd, outer when it is even.
    const lastLp = layerPts[layerPts.length - 1];
    const terminalA = layerPts[0][0]; // layer[0] outer
    const terminalB = lastLp[lastLp.length - 1];
    const lastFreeInner = layersIn.length % 2 === 1;
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            net,
            turns,
            direction,
            layers_used: layersIn,
            multilayer: true,
            note:
              "Multilayer stacked coil: `layer` and inner_via/via_to_layer ignored. " +
              "One continuous spiral in angle across the stack — the radius zigzags " +
              "inner↔outer with a stitch via at each turnaround — so every layer " +
              "circulates the same way and their fields add.",
            total_traces: totalTraces,
            total_length_mm: Math.round(totalLengthMm * 100) / 100,
            total_resistance_ohms: Math.round(totalResistance * 1000) / 1000,
            stitch_vias: stitchVias,
            terminals: { a: terminalA, b: terminalB },
            inner_endpoint: lastFreeInner ? terminalB : layerPts[0][layerPts[0].length - 1],
            outer_endpoint: terminalA,
            ...(drcCap ? { drc_delta: await drcCap.finish() } : {}),
            ...docResultPayload(ctx),
          }),
        },
      ],
    };
  }

  // ---- Single-layer coil ------------------------------------------------
  let lengthMm = 0;
  let tracesAdded = 0;
  for (let i = 0; i + 1 < pts.length; i++) {
    pcb.traces.push({
      start: pts[i],
      end: pts[i + 1],
      width: traceWidth,
      layer,
      net,
      source: "manual",
    });
    tracesAdded++;
    lengthMm += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
  }

  let via: { position: Vec2; startLayer: PcbLayer; endLayer: PcbLayer } | undefined;
  if (args.inner_via) {
    const viaToRes = validateCopperLayer((args.via_to_layer as string) || "BCu");
    if ("error" in viaToRes) return fail(`via_to_layer: ${viaToRes.error}`);
    const viaTo = viaToRes.layer;
    const v = {
      position: pts[0],
      diameter: pcb.rules.defaultRules.viaDiameter,
      drill: pcb.rules.defaultRules.viaDrill,
      startLayer: layer,
      endLayer: viaTo,
      net,
      source: "manual" as const,
    };
    pcb.vias.push(v);
    via = { position: v.position, startLayer: layer, endLayer: viaTo };
  }

  // DC resistance estimate: ρ_cu = 1.68e-5 Ω·mm, cross-section = width × copper thickness.
  const copperT =
    pcb.stackup.layers.find((l) => l.layer === layer)?.copperThickness ?? 0.035;
  const resistance = (1.68e-5 * lengthMm) / (traceWidth * copperT);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          net,
          layer,
          turns,
          direction,
          traces_added: tracesAdded,
          length_mm: Math.round(lengthMm * 100) / 100,
          estimated_dc_resistance_ohms: Math.round(resistance * 1000) / 1000,
          inner_endpoint: pts[0],
          outer_endpoint: pts[pts.length - 1],
          ...(via ? { via } : {}),
          ...(drcCap ? { drc_delta: await drcCap.finish() } : {}),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/** Total polyline length of an ordered point list. */
function segLength(pts: Vec2[]): number {
  let len = 0;
  for (let i = 0; i + 1 < pts.length; i++) {
    len += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
  }
  return len;
}

// ============================================================================
// add_coil_array — a ring of coils (a realizer primitive, no phase logic)
// ============================================================================

/**
 * Fan `add_coil` out over a ring: `count` spirals evenly spaced on a circle of
 * `pitch_radius` about `center`. Pure geometry — net assignment is caller-
 * supplied (`net_sequence` cycles per coil) and `chirality` is a winding-sense
 * convenience with NO phase/polarity meaning. For an electrically-correct phase
 * and polarity per coil, plan with `winding_layout` first and map its result.
 */
export async function addCoilArray(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: Document has no PCB — run place_components first (or open a document that has a board)",
        },
      ],
      isError: true as const,
    };
  }

  const fail = ecadError;

  const count = Math.round(args.count as number);
  const center = args.center as Vec2;
  const pitchRadius = args.pitch_radius as number;
  const startAngleDeg = (args.start_angle_deg as number) || 0;
  const netSequence = Array.isArray(args.net_sequence)
    ? (args.net_sequence as string[])
    : undefined;
  const net = args.net ? String(args.net) : undefined;
  const chirality = (args.chirality as string) || "uniform";

  if (!(count >= 1)) return fail("count must be >= 1");
  if (!center || typeof center.x !== "number" || typeof center.y !== "number") {
    return fail("center must be {x, y} in mm");
  }
  if (!(pitchRadius >= 0)) return fail("pitch_radius must be >= 0");
  if (!netSequence?.length && !net) {
    return fail("provide `net` or a non-empty `net_sequence` — coil copper must belong to a net");
  }

  const dirFor = (i: number): string => {
    if (chirality === "alternating") return i % 2 === 0 ? "ccw" : "cw";
    if (chirality === "cw" || chirality === "ccw") return chirality;
    return "ccw"; // 'uniform'
  };

  // Re-use the same session/doc so each delegated addCoil mutates this board.
  const childDoc = ctx.documentId ? { document_id: ctx.documentId } : { document: ctx.doc };

  // One capture around the WHOLE ring (each coil reaches outer_radius from a
  // center on the pitch circle); the delegated addCoil calls skip their own.
  const outerR = typeof args.outer_radius === "number" ? (args.outer_radius as number) : NaN;
  const reach = pitchRadius + outerR + pcb.rules.defaultRules.viaDiameter;
  const drcCap = await beginDrcDelta(
    pcb,
    boundsOfPoints([
      { x: center.x - reach, y: center.y - reach },
      { x: center.x + reach, y: center.y + reach },
    ]),
  );

  const results: Array<Record<string, unknown>> = [];
  const errors: string[] = [];
  let totalTraces = 0;

  for (let i = 0; i < count; i++) {
    const angleDeg = startAngleDeg + (i * 360) / count;
    const angle = (angleDeg * Math.PI) / 180;
    const coilCenter = {
      x: round3(center.x + pitchRadius * Math.cos(angle)),
      y: round3(center.y + pitchRadius * Math.sin(angle)),
    };
    const coilNet = netSequence?.length ? netSequence[i % netSequence.length] : net;
    const direction = dirFor(i);
    const res = await addCoil({
      ...childDoc,
      center: coilCenter,
      turns: args.turns,
      inner_radius: args.inner_radius,
      outer_radius: args.outer_radius,
      trace_width: args.trace_width,
      net: coilNet,
      layer: args.layer,
      direction,
      start_angle_deg: angleDeg,
      clearance: args.clearance,
      segments_per_turn: args.segments_per_turn,
      inner_via: args.inner_via,
      via_to_layer: args.via_to_layer,
    }, { skipDrcDelta: true });
    if (res.isError) {
      errors.push(`coil ${i} (net ${coilNet}): ${res.content[0]!.text}`);
      continue;
    }
    const payload = JSON.parse(res.content[0]!.text) as Record<string, unknown>;
    const traces = (payload.traces_added as number) ?? 0;
    totalTraces += traces;
    results.push({ index: i, center: coilCenter, net: coilNet, direction, traces_added: traces });
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: errors.length === 0,
          coils_added: results.length,
          total_traces: totalTraces,
          results,
          ...(errors.length ? { errors } : {}),
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
    // Hard error only when nothing landed; partial success returns errors[].
    ...(errors.length && results.length === 0 ? { isError: true as const } : {}),
  };
}

// ============================================================================
// winding_layout — pure polyphase winding planner (data only, no session)
// ============================================================================

/** Greatest common divisor of two non-negative integers. */
function gcdInt(a: number, b: number): number {
  a = Math.abs(a);
  b = Math.abs(b);
  while (b) {
    const r = a % b;
    a = b;
    b = r;
  }
  return a;
}

/** One coil in a winding plan: tooth-relative + electrical only (no copper). */
interface WindingCoil {
  slot: number;
  angleDeg: number;
  electricalDeg: number;
  phase: number;
  net: string;
  polarity: 1 | -1;
  turns: number;
}

/** Medium-agnostic result of winding_layout — a netlist with geometry hints. */
interface WindingPlan {
  feasible: boolean;
  reason?: string;
  slots: number;
  poles: number;
  phases: number;
  layer: "single" | "double";
  connection: "wye" | "delta";
  coils: WindingCoil[];
  windingFactor: number;
  pitchFactor: number;
  distributionFactor: number;
  slotsPerPolePerPhase: number;
  coilsPerPhase: number;
  phaseSeries: Record<string, number[]>;
  neutralNet?: string;
}

/**
 * Plan a balanced polyphase concentrated (fractional-slot) winding. PURE: takes
 * a spec, returns a WindingPlan as data — no document, no session mutation, no
 * geometry. The plan describes the ELECTROMAGNETIC layout (which tooth, which
 * phase, which winding direction); realizers (PCB spirals via add_coil_array, or
 * a future swept-wire solid) consume it unchanged. Uses the star-of-slots /
 * EMF-phasor method: each slot maps to a phasor at its electrical angle and is
 * binned to the nearest of 2·phases belts to read off phase + polarity; the
 * fundamental winding factor is kp·kd.
 */
export function windingLayout(args: Record<string, unknown>) {
  const fail = ecadError;

  const slots = Math.round(args.slots as number);
  const poles = Math.round(args.poles as number);
  const phases = args.phases != null ? Math.round(args.phases as number) : 3;
  const turns = (args.turns_per_coil as number) ?? 1;
  const connection = (args.connection as string) === "delta" ? "delta" : "wye";
  const layer = (args.layer as string) === "single" ? "single" : "double";
  const phaseNetsIn = Array.isArray(args.phase_nets) ? (args.phase_nets as string[]) : undefined;

  if (!Number.isFinite(slots) || slots < 1) return fail("slots must be an integer >= 1");
  if (!Number.isFinite(poles) || poles < 2 || poles % 2 !== 0) {
    return fail("poles must be an even integer >= 2 (the pole count 2p)");
  }
  if (!Number.isFinite(phases) || phases < 1) return fail("phases must be an integer >= 1");
  if (!(turns > 0)) return fail("turns_per_coil must be > 0");

  const defaultNet = (j: number) => (phases === 3 ? ["PHA", "PHB", "PHC"][j]! : `PH${j + 1}`);
  const phaseNet = (j: number) => phaseNetsIn?.[j] ?? defaultNet(j);
  const neutralNet = connection === "wye" ? String(args.neutral_net ?? "WIND_N") : undefined;

  const p = poles / 2; // pole pairs
  const alpha = (360 * p) / slots; // electrical deg between adjacent slots
  const W = 180 / phases; // belt width; there are 2·phases belts

  // Feasibility (star-of-slots balance test).
  const t = gcdInt(slots, p);
  let feasible = true;
  let reason: string | undefined;
  if (slots % phases !== 0) {
    feasible = false;
    reason = `unbalanced: slots (${slots}) must be divisible by phases (${phases}) for equal coils per phase`;
  } else if ((slots / t) % phases !== 0) {
    feasible = false;
    reason = `unbalanced: slots/gcd(slots,p) = ${slots}/${t} = ${slots / t} is not divisible by phases (${phases}) — no symmetric ${phases}-phase winding for ${slots}s/${poles}p`;
  } else if (layer === "single") {
    feasible = false;
    reason =
      slots % 2 !== 0
        ? `single-layer winding impossible for an odd slot count (${slots}); use layer='double'`
        : `single-layer planning is not yet implemented — use layer='double' (one coil per tooth)`;
  }

  // Assign each tooth/coil a phase + polarity from its slot phasor.
  const coils: WindingCoil[] = [];
  const phaseSeries: Record<string, number[]> = {};
  for (let k = 0; k < slots; k++) {
    const electricalDeg = (((alpha * k) % 360) + 360) % 360;
    // Nearest of 2·phases belts, boundary-safe: shift by half a belt, then floor.
    const shifted = (((electricalDeg + W / 2) % 360) + 360) % 360;
    const belt = Math.floor(shifted / W) % (2 * phases);
    const phase = (phases - (belt % phases)) % phases;
    const polarity: 1 | -1 = belt % 2 === 0 ? 1 : -1;
    const net = phaseNet(phase);
    coils.push({
      slot: k,
      angleDeg: (k / slots) * 360,
      electricalDeg: Math.round(electricalDeg * 1000) / 1000,
      phase,
      net,
      polarity,
      turns,
    });
    if (!phaseSeries[net]) phaseSeries[net] = [];
    phaseSeries[net]!.push(k);
  }

  // Winding factors. kp for a single-tooth concentrated coil (1-slot throw).
  const pitchFactor = Math.abs(Math.sin((p * Math.PI) / slots));
  // kd from the signed phasor sum of phase 0's coils (equal across phases when balanced).
  const phase0 = coils.filter((c) => c.phase === 0);
  let re = 0;
  let im = 0;
  for (const c of phase0) {
    const rad = (c.electricalDeg * Math.PI) / 180;
    re += c.polarity * Math.cos(rad);
    im += c.polarity * Math.sin(rad);
  }
  const distributionFactor = phase0.length ? Math.hypot(re, im) / phase0.length : 0;
  const windingFactor = pitchFactor * distributionFactor;

  const r6 = (n: number) => Math.round(n * 1e6) / 1e6;
  const plan: WindingPlan = {
    feasible,
    ...(reason ? { reason } : {}),
    slots,
    poles,
    phases,
    layer,
    connection,
    coils,
    windingFactor: r6(windingFactor),
    pitchFactor: r6(pitchFactor),
    distributionFactor: r6(distributionFactor),
    slotsPerPolePerPhase: r6(slots / (poles * phases)),
    coilsPerPhase: slots / phases,
    phaseSeries,
    ...(neutralNet ? { neutralNet } : {}),
  };

  // A winding factor is only a claim about a buildable winding.
  const payload = {
    ...plan,
    ...(feasible
      ? {
          claims: [
            emClaim("winding_factor", plan.windingFactor, "dimensionless", "star-of-slots", {
              slots,
              poles,
              phases,
              layer,
            }),
          ],
        }
      : {}),
  };

  return { content: [{ type: "text" as const, text: JSON.stringify(payload) }] };
}

// ============================================================================
// set_board_outline — resize/reshape the board without re-placing parts
// ============================================================================

/** Shared {x, y} JSON schema fragment. */
const vec2Schema = {
  type: "object" as const,
  properties: { x: { type: "number" as const }, y: { type: "number" as const } },
  required: ["x", "y"],
};

/** JSON Schema for set_board_outline tool. */
export const setBoardOutlineSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    board_width: {
      type: "number" as const,
      description:
        "Rectangle width in mm — origin-corner outline (0,0)→(w,h). Pair with board_height.",
    },
    board_height: {
      type: "number" as const,
      description: "Rectangle height in mm. Pair with board_width.",
    },
    board_shape: {
      type: "object" as const,
      description:
        "Circular/annular outline: {outer_diameter, inner_diameter?, center?, segments?}.",
      properties: {
        outer_diameter: { type: "number" as const },
        inner_diameter: { type: "number" as const },
        center: vec2Schema,
        segments: { type: "number" as const },
      },
    },
    outline: {
      type: "object" as const,
      description:
        "Explicit polygon outline: {vertices: [{x,y}, ...], cutouts?: [[{x,y}, ...]]}.",
      properties: {
        vertices: { type: "array" as const, items: vec2Schema },
        cutouts: {
          type: "array" as const,
          items: { type: "array" as const, items: vec2Schema },
        },
      },
    },
    thickness: {
      type: "number" as const,
      description: "Board thickness in mm (defaults to the current board thickness).",
    },
  },
  required: ["document_id"],
};

/**
 * Replace the board outline in place — resize a rectangle, swap in a circular
 * or arbitrary polygon — WITHOUT touching component placement, traces, vias, or
 * zones. The kernel re-extrudes the new `outline.vertices` (minus `cutouts`) on
 * the next eval; everything else is preserved exactly.
 *
 * Footprints keep their positions. Any whose origin ends up off the new board
 * (outside the outline, or inside a cutout) is reported in `off_board` — never
 * silently relocated, so the caller decides whether to move them or grow the
 * board. This is the non-destructive counterpart to re-running place_components
 * with new dimensions (which would reset the floorplan).
 */
export async function setBoardOutline(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  // Resolve the new outline: explicit polygon > circle shorthand > rectangle.
  const outlineArg = args.outline as
    | { vertices?: Vec2[]; cutouts?: Vec2[][] }
    | undefined;
  const shapeArg = args.board_shape as
    | {
        outer_diameter?: number;
        inner_diameter?: number;
        center?: Vec2;
        segments?: number;
      }
    | undefined;
  const boardWidth = args.board_width as number | undefined;
  const boardHeight = args.board_height as number | undefined;

  let vertices: Vec2[];
  let cutouts: Vec2[][] | undefined;

  if (outlineArg) {
    if (!Array.isArray(outlineArg.vertices) || outlineArg.vertices.length < 3) {
      return ecadError("outline.vertices needs at least 3 points");
    }
    vertices = outlineArg.vertices;
    cutouts = outlineArg.cutouts;
  } else if (shapeArg) {
    const od = shapeArg.outer_diameter;
    if (!od || od <= 0) return ecadError("board_shape.outer_diameter must be > 0");
    const id = shapeArg.inner_diameter ?? 0;
    if (id < 0 || id >= od) {
      return ecadError("board_shape.inner_diameter must be >= 0 and < outer_diameter");
    }
    const segments = Math.max(16, Math.round(shapeArg.segments ?? 64));
    const center = shapeArg.center ?? { x: od / 2, y: od / 2 };
    vertices = circlePolygon(center, od / 2, segments);
    cutouts = id > 0 ? [circlePolygon(center, id / 2, segments)] : undefined;
  } else if (boardWidth && boardHeight) {
    if (!(boardWidth > 0) || !(boardHeight > 0)) {
      return ecadError("board_width and board_height must be > 0");
    }
    vertices = [
      { x: 0, y: 0 },
      { x: boardWidth, y: 0 },
      { x: boardWidth, y: boardHeight },
      { x: 0, y: boardHeight },
    ];
  } else {
    return ecadError(
      "specify the new outline — board_width + board_height (rectangle), " +
        "board_shape ({outer_diameter, inner_diameter?}), or outline ({vertices, cutouts?})",
    );
  }

  // Match the kernel extruder's CCW expectation (agent polygons arrive either way).
  vertices = ensureCcw(vertices);
  cutouts = cutouts?.map(ensureCcw);

  const thickness = (args.thickness as number) ?? pcb.outline.thickness ?? 1.6;

  // A new outline re-judges EVERY copper element's edge clearance (and can
  // strand copper off-board) — inherently board-wide, so never region-scoped.
  const drcCap = await beginDrcDelta(pcb, "full");

  pcb.outline = {
    vertices,
    ...(cutouts && cutouts.length > 0 ? { cutouts } : {}),
    thickness,
  };

  // Components keep their positions; flag any now off-board (origin outside the
  // outline or inside a cutout) — the same definition placement DRC uses.
  const offBoard: string[] = [];
  for (const fp of pcb.footprints) {
    const onBoard =
      pointInPolygon(fp.position, vertices) &&
      !(cutouts ?? []).some((c) => c.length >= 3 && pointInPolygon(fp.position, c));
    if (!onBoard) offBoard.push(fp.ref);
  }

  const xs = vertices.map((v) => v.x);
  const ys = vertices.map((v) => v.y);
  const width = round3(Math.max(...xs) - Math.min(...xs));
  const height = round3(Math.max(...ys) - Math.min(...ys));

  // A rewritten outline invalidates vertex indices — drop constraints whose
  // indices are now out of range (reported), and warn about the rest.
  const droppedConstraints = pruneOutlineConstraints(ctx.doc, (node) => {
    const p = getNodePcb(ctx.doc, node);
    return p ? (p.outline.vertices?.length ?? 0) : undefined;
  });
  const constraintWarning = constraintStaleWarning(ctx.doc);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          ...(droppedConstraints.length > 0
            ? { dropped_constraints: droppedConstraints }
            : {}),
          ...(constraintWarning ? { constraint_warning: constraintWarning } : {}),
          outline: {
            width,
            height,
            vertices: vertices.length,
            cutouts: cutouts?.length ?? 0,
            thickness,
          },
          components_kept: pcb.footprints.length,
          ...(offBoard.length > 0 ? { off_board: offBoard } : {}),
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// list_footprints / search_footprints — parametric footprint discovery
// ============================================================================

/** A footprint family the parametric engine can resolve. `example` is a
 *  canonical id that resolves to this family (verified by a drift-guard test
 *  against the kernel `resolveFootprint`). */
interface FootprintFamily {
  family: string;
  label: string;
  aliases: string[];
  kind: "passive" | "ic" | "transistor" | "diode" | "power" | "connector";
  pins: string;
  pitch_mm: string;
  example: string;
  example_pins: number;
}

/**
 * Discovery catalog mirroring the parametric footprint engine
 * (`crates/vcad-ecad-symbols/src/footprint.rs` `match_family`). Hand-curated
 * (the engine match is imperative, not a table), but every `example` is kept
 * honest by `footprint_examples_resolve` in the test suite, which runs each
 * one through the kernel resolver and asserts a real family match.
 */
const FOOTPRINT_FAMILIES: FootprintFamily[] = [
  { family: "Chip", label: "Two-terminal SMD passive (R/C/L)", aliases: ["0201", "0402", "0603", "0805", "1206", "1210", "2010", "2512", "1608Metric", "2012Metric"], kind: "passive", pins: "2", pitch_mm: "n/a", example: "0805", example_pins: 2 },
  { family: "SOT-23", label: "Small-outline transistor", aliases: ["SOT-23", "SOT-23-3", "SOT-23-5", "SOT-23-6", "SOT-23-8", "TSOT-23"], kind: "transistor", pins: "3, 5, 6, 8", pitch_mm: "0.95", example: "SOT-23", example_pins: 3 },
  { family: "SOT-223", label: "SOT-223 power package (tab)", aliases: ["SOT-223", "SOT-223-3", "SOT-223-5"], kind: "power", pins: "4, 5, 8", pitch_mm: "2.3", example: "SOT-223", example_pins: 4 },
  { family: "SC-70", label: "SC-70 / SOT-353 / SOT-363", aliases: ["SC-70", "SC-70-5", "SOT-353", "SOT-363"], kind: "transistor", pins: "3, 5, 6", pitch_mm: "0.65", example: "SC-70-5", example_pins: 5 },
  { family: "SOT-89", label: "SOT-89 power transistor", aliases: ["SOT-89"], kind: "power", pins: "3", pitch_mm: "1.5", example: "SOT-89", example_pins: 3 },
  { family: "SOD", label: "Small-outline diode", aliases: ["SOD-123", "SOD-323", "SOD-523", "SOD-882"], kind: "diode", pins: "2", pitch_mm: "n/a", example: "SOD-123", example_pins: 2 },
  { family: "DO-214", label: "DO-214 SMD diode (SMA/SMB/SMC)", aliases: ["D_SMA", "D_SMB", "D_SMC", "DO-214AC", "DO-214AA", "DO-214AB", "SMA", "SMB", "SMC"], kind: "diode", pins: "2", pitch_mm: "n/a", example: "D_SMA", example_pins: 2 },
  { family: "QFN", label: "Quad flat no-lead (thermal pad)", aliases: ["QFN", "VQFN", "UQFN", "WQFN", "TQFN", "DHVQFN"], kind: "ic", pins: "multiple of 4", pitch_mm: "0.4–0.5", example: "QFN-16", example_pins: 16 },
  { family: "DFN", label: "Dual flat no-lead / SON", aliases: ["DFN", "SON", "WSON"], kind: "ic", pins: "even", pitch_mm: "0.4–0.5", example: "DFN-8", example_pins: 8 },
  { family: "QFP", label: "Quad flat package (LQFP/TQFP/PQFP)", aliases: ["QFP", "LQFP", "TQFP", "PQFP", "CQFP"], kind: "ic", pins: "multiple of 4, ≥8", pitch_mm: "0.4–0.8", example: "LQFP-48", example_pins: 48 },
  { family: "SOIC", label: "Small-outline IC (SO/SOP)", aliases: ["SOIC", "SO", "SOP"], kind: "ic", pins: "even, ≥4", pitch_mm: "1.27", example: "SOIC-8", example_pins: 8 },
  { family: "SSOP", label: "Shrink small-outline (QSOP)", aliases: ["SSOP", "QSOP"], kind: "ic", pins: "even, ≥4", pitch_mm: "0.635–0.65", example: "SSOP-16", example_pins: 16 },
  { family: "TSSOP", label: "Thin shrink small-outline", aliases: ["TSSOP"], kind: "ic", pins: "even, ≥4", pitch_mm: "0.65", example: "TSSOP-20", example_pins: 20 },
  { family: "HTSSOP", label: "TSSOP with exposed thermal pad (PowerPad)", aliases: ["HTSSOP", "PowerPad", "PowerPAD", "TSSOP-EP", "HTSSOP-16", "PWP"], kind: "ic", pins: "even, ≥4 (+EP)", pitch_mm: "0.65", example: "HTSSOP-16-1EP_4.4x5mm_P0.65mm_EP3.4x5mm", example_pins: 16 },
  { family: "MSOP", label: "Mini small-outline", aliases: ["MSOP"], kind: "ic", pins: "even, ≥4", pitch_mm: "0.65", example: "MSOP-8", example_pins: 8 },
  { family: "VSSOP", label: "Very-thin shrink small-outline", aliases: ["VSSOP"], kind: "ic", pins: "even, ≥4", pitch_mm: "0.5", example: "VSSOP-8", example_pins: 8 },
  { family: "DIP", label: "Dual in-line (through-hole)", aliases: ["DIP", "PDIP"], kind: "ic", pins: "even, ≥4", pitch_mm: "2.54", example: "DIP-8", example_pins: 8 },
  { family: "DPAK", label: "DPAK / TO-252 power tab", aliases: ["DPAK", "TO-252"], kind: "power", pins: "2, 3", pitch_mm: "2.28", example: "TO-252", example_pins: 3 },
  { family: "D2PAK", label: "D2PAK / TO-263 power tab", aliases: ["D2PAK", "DDPAK", "TO-263"], kind: "power", pins: "2, 3", pitch_mm: "2.54", example: "TO-263", example_pins: 3 },
  { family: "TO-220", label: "TO-220 / TO-247 (through-hole)", aliases: ["TO-220", "TO-247"], kind: "power", pins: "2, 3", pitch_mm: "2.54", example: "TO-220", example_pins: 3 },
  { family: "PinHeader", label: "Pin header / socket (RxC grid)", aliases: ["PinHeader", "PinSocket", "Socket_Strip", "IDC-Header"], kind: "connector", pins: "rows × cols", pitch_mm: "2.54 (default)", example: "PinHeader_2x05_P2.54mm", example_pins: 10 },
  { family: "ScrewTerminal", label: "Screw terminal block", aliases: ["TerminalBlock", "Screw_Terminal"], kind: "connector", pins: "positions", pitch_mm: "5.08 (default)", example: "TerminalBlock_1x02_P5.08mm", example_pins: 2 },
  { family: "Electrolytic", label: "Radial electrolytic capacitor", aliases: ["CP_Radial", "C_Radial"], kind: "passive", pins: "2", pitch_mm: "2.0–5.0", example: "CP_Radial_D6.3mm_P2.50mm", example_pins: 2 },
  { family: "JST-PH", label: "JST PH wire-to-board (2.0mm THT)", aliases: ["JST_PH", "JST-PH"], kind: "connector", pins: "2+", pitch_mm: "2.0", example: "JST_PH_2", example_pins: 2 },
  { family: "JST-XH", label: "JST XH wire-to-board (2.5mm THT)", aliases: ["JST_XH", "JST-XH"], kind: "connector", pins: "2+", pitch_mm: "2.5", example: "JST_XH_3", example_pins: 3 },
  { family: "JST-EH", label: "JST EH wire-to-board (2.5mm THT)", aliases: ["JST_EH", "JST-EH"], kind: "connector", pins: "2+", pitch_mm: "2.5", example: "JST_EH_2", example_pins: 2 },
  { family: "JST-SH", label: "JST SH wire-to-board (1.0mm SMD)", aliases: ["JST_SH", "JST-SH"], kind: "connector", pins: "2+", pitch_mm: "1.0", example: "JST_SH_4", example_pins: 4 },
  { family: "JST-GH", label: "JST GH wire-to-board (1.25mm SMD)", aliases: ["JST_GH", "JST-GH"], kind: "connector", pins: "2+", pitch_mm: "1.25", example: "JST_GH_4", example_pins: 4 },
  { family: "Molex-PicoBlade", label: "Molex Pico-Blade (1.25mm SMD)", aliases: ["PicoBlade", "Pico-Blade", "53048", "53261"], kind: "connector", pins: "2+", pitch_mm: "1.25", example: "Molex_PicoBlade_1x04_P1.25mm", example_pins: 4 },
  { family: "Tag-Connect", label: "Tag-Connect spring-pin programming pads", aliases: ["Tag-Connect", "TagConnect", "TC2030", "TC2050"], kind: "connector", pins: "6 (TC2030), 10 (TC2050)", pitch_mm: "1.27", example: "TC2030", example_pins: 6 },
  { family: "USB-C", label: "USB-C receptacle (simplified; 16-pin USB 2.0 subset or full 24)", aliases: ["USB_C", "USB-C", "Type-C", "TypeC", "USB-C-16"], kind: "connector", pins: "up to 24 (+4 shield posts)", pitch_mm: "0.5–1.0", example: "USB-C", example_pins: 24 },
  { family: "USB-Micro-B", label: "USB micro-B receptacle (5 contacts + shield posts)", aliases: ["USB_Micro-B", "USB-Micro", "USB Micro", "MicroUSB", "Micro_USB", "Micro-B"], kind: "connector", pins: "5 (+4 shield posts)", pitch_mm: "0.65", example: "USB_Micro-B_Molex-105017-0001", example_pins: 5 },
  { family: "Crystal", label: "SMD crystal (2-pad 5032/6035/7050, 4-pad 3225/2520/2016/1612)", aliases: ["Crystal", "Crystal_SMD", "3225", "2520", "2016", "1612", "5032", "6035", "7050", "XTAL"], kind: "passive", pins: "2 or 4", pitch_mm: "n/a", example: "Crystal_SMD_3225-4Pin_3.2x2.5mm", example_pins: 4 },
];

/** Relevance score of a family for a query (0..1) — exact alias, substring,
 *  then edit-distance similarity, over family/label/aliases. */
function scoreFootprintFamily(query: string, fam: FootprintFamily): number {
  const q = query.toLowerCase().trim();
  if (!q) return 0;
  const cands = [fam.family, fam.label, ...fam.aliases].map((s) => s.toLowerCase());
  let best = 0;
  for (const c of cands) {
    if (c === q) {
      best = Math.max(best, 1);
    } else if (c.includes(q) || q.includes(c)) {
      best = Math.max(best, 0.75);
    } else {
      const sim = 1 - editDistance(q, c) / Math.max(q.length, c.length);
      best = Math.max(best, sim * 0.6);
    }
  }
  return best;
}

/** JSON Schema for list_footprints tool. */
export const listFootprintsSchema = {
  type: "object" as const,
  properties: {
    kind: {
      type: "string" as const,
      description:
        "Optional filter: passive | ic | transistor | diode | power | connector.",
    },
  },
  required: [],
};

/**
 * List the footprint families the parametric engine resolves, each with a
 * canonical `example` id to drop into create_schematic's `footprint`. Removes
 * the guess-and-fail loop over id spellings ("SOIC8" vs "SOIC-8").
 */
export function listFootprints(args: Record<string, unknown>) {
  const kind = args.kind != null ? String(args.kind).toLowerCase() : undefined;
  const families = FOOTPRINT_FAMILIES.filter((f) => !kind || f.kind === kind).map((f) => ({
    family: f.family,
    label: f.label,
    kind: f.kind,
    aliases: f.aliases,
    pins: f.pins,
    pitch_mm: f.pitch_mm,
    example: f.example,
  }));
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          count: families.length,
          ...(kind ? { kind } : {}),
          families,
        }),
      },
    ],
  };
}

/** JSON Schema for search_footprints tool. */
export const searchFootprintsSchema = {
  type: "object" as const,
  properties: {
    query: {
      type: "string" as const,
      description:
        "Family name, alias, or partial id to match (e.g. 'SOIC 8', 'jst', 'qfn').",
    },
  },
  required: ["query"],
};

/**
 * Fuzzy-search footprint families by name/alias/label and return ranked
 * matches with a canonical example id — so an agent can resolve "what's the id
 * for an 8-pin small-outline?" without a failed create_schematic round-trip.
 */
export function searchFootprints(args: Record<string, unknown>) {
  const query = String(args.query ?? "");
  if (!query.trim()) {
    return ecadError("query is required (a family name, alias, or partial id)");
  }
  const ranked = FOOTPRINT_FAMILIES.map((f) => ({ f, score: scoreFootprintFamily(query, f) }))
    .filter((r) => r.score >= 0.34)
    .sort((a, b) => b.score - a.score)
    .slice(0, 10)
    .map((r) => ({
      family: r.f.family,
      label: r.f.label,
      kind: r.f.kind,
      aliases: r.f.aliases,
      pins: r.f.pins,
      pitch_mm: r.f.pitch_mm,
      example: r.f.example,
      score: Math.round(r.score * 100) / 100,
    }));
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ success: true, query, count: ranked.length, matches: ranked }),
      },
    ],
  };
}

// ============================================================================
// get_pad_positions — absolute board-frame pad coordinates (read-only)
// ============================================================================

/** JSON Schema for get_pad_positions tool. */
export const getPadPositionsSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    net: {
      type: "string" as const,
      description:
        "Optional net filter — return only pads on this net (e.g. 'GND', 'IMU_CS').",
    },
    ref: {
      type: "string" as const,
      description:
        "Optional reference-designator filter — return only pads on this component (e.g. 'U1').",
    },
  },
  required: ["document_id"],
};

/** Copper layers a pad sits on, in board order (drops paste/mask/silk). */
function padCopperLayers(pad: Pad): PcbLayer[] {
  return pad.layers.filter((l) => isCopperLayer(l));
}

/**
 * Return every footprint pad's absolute board-frame (x, y), copper layer, and
 * net — the primitive manual routing (add_trace / add_via / add_via_array)
 * needs so trace endpoints land exactly on pads instead of being eyeballed from
 * component centers. Read-only; mutates nothing.
 *
 * Coordinates compose the footprint placement with the pad's local offset,
 * applying the footprint rotation — the same transform Gerber and
 * pick-and-place export use: worldX = fp.x + padX·cosθ − padY·sinθ,
 * worldY = fp.y + padX·sinθ + padY·cosθ. Optional `net` and `ref` filters
 * narrow the result for targeted routing.
 */
export function getPadPositions(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const netFilter = args.net != null ? String(args.net) : undefined;
  const refFilter = args.ref != null ? String(args.ref) : undefined;

  const pads: Array<{
    ref: string;
    pin: string;
    x: number;
    y: number;
    rotation: number;
    net: string | null;
    layer: string | null;
    layers: string[];
    pad_type: string;
    pad_shape: Pad["shape"];
  }> = [];

  for (const fp of pcb.footprints) {
    if (refFilter !== undefined && fp.ref !== refFilter) continue;
    const theta = ((fp.rotation ?? 0) * Math.PI) / 180;
    const cos = Math.cos(theta);
    const sin = Math.sin(theta);
    for (const pad of fp.pads) {
      const net = pad.net ?? null;
      if (netFilter !== undefined && net !== netFilter) continue;
      const lx = pad.position.x * cos - pad.position.y * sin;
      const ly = pad.position.x * sin + pad.position.y * cos;
      const copper = padCopperLayers(pad);
      pads.push({
        ref: fp.ref,
        pin: pad.number,
        x: round3(fp.position.x + lx),
        y: round3(fp.position.y + ly),
        rotation: round3((fp.rotation ?? 0) + (pad.rotation ?? 0)),
        net,
        layer: copper[0] ?? null,
        layers: copper,
        pad_type: pad.padType,
        pad_shape: pad.shape,
      });
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          count: pads.length,
          ...(netFilter !== undefined ? { net: netFilter } : {}),
          ...(refFilter !== undefined ? { ref: refFilter } : {}),
          pads,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// get_footprint — introspect a land pattern in BOTH local and board frames
// ============================================================================

/** Local-frame courtyard AABB of a footprint: prefer an explicit courtyard
 *  graphic (FCrtYd/BCrtYd rect), else the pad bounding box. Null when empty. */
function localCourtyardAabb(
  pads: Pad[],
  graphics: Footprint["graphics"],
): { min: Vec2; max: Vec2 } | null {
  for (const g of graphics ?? []) {
    if (g.type === "Rect" && (g.layer === "FCrtYd" || g.layer === "BCrtYd")) {
      return {
        min: { x: Math.min(g.start.x, g.end.x), y: Math.min(g.start.y, g.end.y) },
        max: { x: Math.max(g.start.x, g.end.x), y: Math.max(g.start.y, g.end.y) },
      };
    }
  }
  // Fall back to the pad bounding box (half-extent of each pad's copper).
  if (pads.length === 0) return null;
  const half = (shape: Pad["shape"]): { hx: number; hy: number } => {
    switch (shape.type) {
      case "Circle":
        return { hx: shape.diameter / 2, hy: shape.diameter / 2 };
      case "Rect":
      case "Oval":
      case "RoundRect":
        return { hx: shape.width / 2, hy: shape.height / 2 };
      default:
        return { hx: 0.5, hy: 0.5 };
    }
  };
  let minX = Infinity,
    minY = Infinity,
    maxX = -Infinity,
    maxY = -Infinity;
  for (const p of pads) {
    const { hx, hy } = half(p.shape);
    minX = Math.min(minX, p.position.x - hx);
    minY = Math.min(minY, p.position.y - hy);
    maxX = Math.max(maxX, p.position.x + hx);
    maxY = Math.max(maxY, p.position.y + hy);
  }
  return { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } };
}

/** Map a footprint-local point into the board frame: rotate CCW by `rotDeg`
 *  about the origin, then translate to `origin`. Back-side mirrors local X
 *  first (KiCad convention) so a flipped connector lands correctly. */
function toBoardFrame(
  local: Vec2,
  origin: Vec2,
  rotDeg: number,
  front: boolean,
): Vec2 {
  const theta = (rotDeg * Math.PI) / 180;
  const cos = Math.cos(theta);
  const sin = Math.sin(theta);
  const lx = front ? local.x : -local.x;
  return {
    x: round3(origin.x + lx * cos - local.y * sin),
    y: round3(origin.y + lx * sin + local.y * cos),
  };
}

/** Board-frame AABB of a local AABB under a placement — recomputed from the
 *  four transformed corners (rotation can tilt the box). */
function boardCourtyardAabb(
  local: { min: Vec2; max: Vec2 } | null,
  origin: Vec2,
  rotDeg: number,
  front: boolean,
): { min: Vec2; max: Vec2 } | null {
  if (!local) return null;
  const corners: Vec2[] = [
    { x: local.min.x, y: local.min.y },
    { x: local.max.x, y: local.min.y },
    { x: local.max.x, y: local.max.y },
    { x: local.min.x, y: local.max.y },
  ].map((c) => toBoardFrame(c, origin, rotDeg, front));
  return {
    min: { x: Math.min(...corners.map((c) => c.x)), y: Math.min(...corners.map((c) => c.y)) },
    max: { x: Math.max(...corners.map((c) => c.x)), y: Math.max(...corners.map((c) => c.y)) },
  };
}

/** JSON Schema for get_footprint tool. */
export const getFootprintSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    footprint: {
      type: "string" as const,
      description:
        "Footprint id to resolve PRE-placement (KiCad-style, e.g. " +
        "'Connector_JST:JST_PH_2' or 'QFN-40_5x5mm_P0.4mm'). Returns the " +
        "land pattern in the footprint-local frame; pass `at`/`rotation` to " +
        "also project it into a hypothetical board placement.",
    },
    pins: {
      type: "number" as const,
      description:
        "Declared pin count, used to resolve fallback geometry and parse the " +
        "count when the id omits it (footprint mode). Defaults to a count " +
        "parsed from the id, else 2.",
    },
    ref: {
      type: "string" as const,
      description:
        "Reference designator of a PLACED footprint to introspect (e.g. 'J1') " +
        "— reads its real board-frame transform, nets, and courtyard from the " +
        "session document. Requires document_id. Use instead of `footprint`.",
    },
    at: {
      ...vec2Schema,
      description:
        "Hypothetical placement origin (board mm) for the board-frame " +
        "projection in `footprint` mode. Defaults to (0,0).",
    },
    rotation: {
      type: "number" as const,
      description:
        "Hypothetical placement rotation in degrees, CCW about the origin " +
        "(footprint mode). Defaults to 0.",
    },
    side: {
      type: "string" as const,
      enum: ["front", "back"],
      description: "Hypothetical board side for the projection (footprint mode). Default 'front'.",
    },
  },
  required: [],
};

const ROTATION_CONVENTION =
  "Rotation is in degrees, counter-clockwise about the footprint origin. " +
  "board = origin + R(rotation)·(localX, localY), where " +
  "R(θ) = [[cosθ, -sinθ], [sinθ, cosθ]]. Back-side placements mirror local X first.";

/**
 * Introspect a footprint's land pattern in BOTH the footprint-local frame and
 * the board frame, so an agent can see exactly where pads (especially connector
 * pins) land instead of rendering and eyeballing. Two modes:
 *
 *  - `ref`: a footprint already placed on the session board — reports its real
 *    origin/rotation/side, per-pad nets, and courtyard.
 *  - `footprint`: a footprint id resolved by the parametric engine
 *    pre-placement — reports the local land pattern, and (with `at`/`rotation`/
 *    `side`) the board-frame projection for a hypothetical placement.
 *
 * Read-only. Shares the board-frame transform with get_pad_positions.
 */
export async function getFootprint(args: Record<string, unknown>) {
  const refArg = args.ref != null ? String(args.ref) : undefined;
  const fpArg = args.footprint != null ? String(args.footprint) : undefined;

  if (!refArg && !fpArg) {
    return ecadError("pass `ref` (a placed footprint) or `footprint` (an id to resolve)");
  }

  // ---- Mode 1: a placed footprint, read from the session board. ----
  if (refArg) {
    const ctx = resolveDocInput(args);
    const pcb = getDocPcb(ctx.doc);
    if (!pcb) {
      return ecadError(
        "Document has no PCB — run place_components first, or use `footprint` to resolve an id pre-placement",
      );
    }
    const fp = pcb.footprints.find((f) => f.ref === refArg);
    if (!fp) {
      const have = pcb.footprints.map((f) => f.ref).join(", ");
      return ecadError(`no footprint '${refArg}' on the board (have: ${have || "none"})`);
    }
    const origin = fp.position;
    const rotation = fp.rotation ?? 0;
    const front = fp.front ?? true;
    const localCy = localCourtyardAabb(fp.pads, fp.graphics);
    const pads = fp.pads.map((pad) => {
      const local = { x: round3(pad.position.x), y: round3(pad.position.y) };
      const board = toBoardFrame(pad.position, origin, rotation, front);
      const copper = pad.layers.filter((l) => /Cu$/.test(l));
      return {
        pin: pad.number,
        pad_type: pad.padType,
        pad_shape: pad.shape,
        net: pad.net ?? null,
        layers: copper,
        drill_mm: pad.drill?.diameter ?? null,
        local: { ...local, rotation: round3(pad.rotation ?? 0) },
        board: { ...board, rotation: round3(rotation + (pad.rotation ?? 0)) },
      };
    });
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            mode: "placed",
            ref: fp.ref,
            footprint: fp.footprintName,
            value: fp.value,
            generated: fp.properties?.padSource === "generated",
            rotation_convention: ROTATION_CONVENTION,
            origin: {
              x: round3(origin.x),
              y: round3(origin.y),
              rotation: round3(rotation),
              side: front ? "front" : "back",
            },
            courtyard: {
              local: localCy,
              board: boardCourtyardAabb(localCy, origin, rotation, front),
            },
            count: pads.length,
            pads,
            ...docResultPayload(ctx),
          }),
        },
      ],
    };
  }

  // ---- Mode 2: resolve a footprint id pre-placement (local frame + optional
  //      hypothetical board projection). ----
  const declared =
    typeof args.pins === "number" && args.pins > 0 ? Math.round(args.pins as number) : 0;
  const resolution = await resolveFootprint(fpArg!, declared);
  if (!resolution || !resolution.template) {
    return ecadError(
      resolution?.note ??
        `could not resolve footprint '${fpArg}' (kernel unavailable, or no pins to synthesize from — pass \`pins\`)`,
    );
  }
  const template = resolution.template;
  const at = (args.at as Vec2 | undefined) ?? { x: 0, y: 0 };
  const rotation = typeof args.rotation === "number" ? (args.rotation as number) : 0;
  const front = args.side !== "back";
  const localCy = localCourtyardAabb(template.pads, template.graphics);
  const pads = template.pads.map((pad) => {
    const local = { x: round3(pad.position.x), y: round3(pad.position.y) };
    const board = toBoardFrame(pad.position, at, rotation, front);
    const copper = pad.layers.filter((l) => /Cu$/.test(l));
    return {
      pin: pad.number,
      pad_type: pad.padType,
      pad_shape: pad.shape,
      layers: copper,
      drill_mm: pad.drill?.diameter ?? null,
      local: { ...local, rotation: round3(pad.rotation ?? 0) },
      board: { ...board, rotation: round3(rotation + (pad.rotation ?? 0)) },
    };
  });
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          mode: "resolved",
          footprint: fpArg,
          resolved_name: template.name,
          family: resolution.family,
          matched: resolution.matched,
          generated: true,
          note: resolution.note,
          rotation_convention: ROTATION_CONVENTION,
          origin: {
            x: round3(at.x),
            y: round3(at.y),
            rotation: round3(rotation),
            side: front ? "front" : "back",
          },
          courtyard: {
            local: localCy,
            board: boardCourtyardAabb(localCy, at, rotation, front),
          },
          count: pads.length,
          pads,
        }),
      },
    ],
  };
}

// describe_pcb — compact, structured snapshot of the session board
// ============================================================================

/** JSON Schema for describe_pcb tool. */
export const describePcbSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
  },
  required: ["document_id"],
};

/** Axis-aligned bounding box of a point set (rounded); null when empty. */
function pointsBbox(
  pts: Vec2[],
): { minX: number; minY: number; maxX: number; maxY: number } | null {
  if (pts.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const p of pts) {
    if (p.x < minX) minX = p.x;
    if (p.y < minY) minY = p.y;
    if (p.x > maxX) maxX = p.x;
    if (p.y > maxY) maxY = p.y;
  }
  return {
    minX: round3(minX),
    minY: round3(minY),
    maxX: round3(maxX),
    maxY: round3(maxY),
  };
}

/** Increment a string-keyed tally in place. */
function tally(map: Record<string, number>, key: string): void {
  map[key] = (map[key] ?? 0) + 1;
}

/** Cap on the number of net names echoed back (the rest are summarized by count). */
const DESCRIBE_NET_NAME_CAP = 64;

/**
 * Return a lightweight, *structured* snapshot of the session PCB — board size +
 * outline, stackup (canonical layer names + copper weights), net classes /
 * design rules, zones (net/layer/bbox/fill), trace and via counts by net and by
 * layer, component count, the current DRC status, and an
 * exportability/renderability probe. Read-only; mutates nothing.
 *
 * Unlike get_document / read — which return only the opaque document_id for a
 * PCB session — this lets an agent (or a human debugging a stuck session)
 * actually inspect the board as data. The export/render probe serializes the
 * board for fab output and 3D preview and reports whether each succeeds,
 * surfacing the dangerous "DRC-clean but unexportable" state (e.g. a board
 * solid that fails to evaluate) that no other inspection tool catches.
 *
 * Counts and small arrays only — never full trace/pad/zone geometry.
 */
export async function describePcb(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  // --- Board outline + size ------------------------------------------------
  const verts = pcb.outline.vertices;
  const bbox = pointsBbox(verts);
  const cutouts = pcb.outline.cutouts ?? [];
  const board = {
    width: bbox ? round3(bbox.maxX - bbox.minX) : 0,
    height: bbox ? round3(bbox.maxY - bbox.minY) : 0,
    thickness: pcb.outline.thickness,
    bbox,
    outline_vertices: verts.length,
    cutouts: cutouts.length,
    // Outer polygon minus cutouts (shoelace, sign-agnostic).
    area_mm2: round3(
      Math.abs(loopSignedArea(verts)) -
        cutouts.reduce((s, c) => s + Math.abs(loopSignedArea(c)), 0),
    ),
  };

  // --- Stackup (canonical layer names + copper weight) ---------------------
  // 1 oz/ft² of copper ≈ 0.035 mm; report both so a human reads "1 oz" while a
  // tool keeps the exact thickness.
  const OZ_PER_MM = 1 / 0.035;
  const stackupLayers = pcb.stackup.layers.map((l) => ({
    layer: l.layer,
    ...(l.copperThickness != null
      ? {
          copper_thickness_mm: round3(l.copperThickness),
          copper_oz: Math.round(l.copperThickness * OZ_PER_MM * 100) / 100,
        }
      : {}),
    ...(l.dielectricThickness != null
      ? { dielectric_mm: round3(l.dielectricThickness) }
      : {}),
    ...(l.dielectricEr != null ? { er: l.dielectricEr } : {}),
    ...(l.material != null ? { material: l.material } : {}),
  }));
  const copperLayers = pcb.stackup.layers.filter((l) => /Cu$/.test(l.layer)).length;

  // --- Design rules / net classes ------------------------------------------
  const dr = pcb.rules;
  const ruleClass = (c: NetClassRules) => ({
    name: c.name,
    traceWidth: c.traceWidth,
    clearance: c.clearance,
    viaDiameter: c.viaDiameter,
    viaDrill: c.viaDrill,
    ...(c.diffPairGap != null ? { diffPairGap: c.diffPairGap } : {}),
    ...(c.diffPairWidth != null ? { diffPairWidth: c.diffPairWidth } : {}),
  });
  const netClassAssignments = dr.netClassAssignments
    ? Object.fromEntries(
        Object.entries(dr.netClassAssignments).map(([k, v]) => [k, v.length]),
      )
    : undefined;
  const designRules = {
    default: ruleClass(dr.defaultRules),
    ...(dr.classRules && dr.classRules.length > 0
      ? { classes: dr.classRules.map(ruleClass) }
      : {}),
    ...(netClassAssignments ? { netClassAssignments } : {}),
    edgeClearance: dr.edgeClearance,
    holeToHole: dr.holeToHole,
    minAnnularRing: dr.minAnnularRing,
    minDrill: dr.minDrill,
  };

  // --- Zones: { net, layer, bbox, fill } -----------------------------------
  const zones = pcb.zones.map((z) => ({
    net: z.net,
    layer: z.layer,
    bbox: pointsBbox(z.outline),
    fill: z.fillType ?? "Solid",
    ...(z.holes && z.holes.length > 0 ? { holes: z.holes.length } : {}),
  }));

  // --- Traces / vias: counts by net and by layer ---------------------------
  const traceByNet: Record<string, number> = {};
  const traceByLayer: Record<string, number> = {};
  for (const t of pcb.traces) {
    tally(traceByNet, t.net);
    tally(traceByLayer, t.layer);
  }
  for (const a of pcb.traceArcs ?? []) {
    tally(traceByNet, a.net);
    tally(traceByLayer, a.layer);
  }
  const viaByNet: Record<string, number> = {};
  const viaByLayer: Record<string, number> = {};
  for (const v of pcb.vias) {
    tally(viaByNet, v.net);
    tally(viaByLayer, `${v.startLayer}-${v.endLayer}`);
  }

  // --- Components / footprints ---------------------------------------------
  const fpByName: Record<string, number> = {};
  let padCount = 0;
  for (const fp of pcb.footprints) {
    tally(fpByName, fp.footprintName);
    padCount += fp.pads.length;
  }

  // --- DRC status (counts only; no sample) ---------------------------------
  const drcSummary = await drcPcb(pcb, "summary", 0);
  // Unverifiable ≠ clean: when the kernel can't evaluate the board, report the
  // status instead of fabricating zero-violation counts (same fail-closed
  // semantics as run_drc, which returns ecadUnverifiable on `!success`).
  const drc = drcSummary.success
    ? {
        violations: drcSummary.violations,
        errors: drcSummary.errors,
        warnings: drcSummary.warnings,
        categories: drcSummary.categories,
        byRule: drcSummary.byRule,
        worstClearance: drcSummary.worstClearance,
        // Same-net copper touching far from any intended junction — invisible
        // to clearance/short rules but fatal to two-terminal structures
        // (coils, shunts). Surfaced by name so a "clean-looking" board that
        // silently short-circuits its own winding is impossible to miss.
        sameNetBypass: drcSummary.byRule["SameNetBypass"] ?? 0,
        // `clean` ignores connectivity (unrouted nets are a to-do, not a defect) —
        // same semantics as placement_drc.clean.
        clean: drcSummary.categories.clearance + drcSummary.categories.manufacturing === 0,
      }
    : {
        unverifiable: true,
        reason: drcSummary.reason,
        ...(drcSummary.offending_field ? { offending_field: drcSummary.offending_field } : {}),
      };

  // --- Exportability / renderability probe ---------------------------------
  // Actually serialize the board for fab + preview and report success. This is
  // the only check that catches a board that passes DRC but cannot be exported
  // or rendered (e.g. the board solid fails to evaluate). `null` means we could
  // not determine it because the ECAD WASM was unavailable.
  const wasmAvailable = await isEcadAvailable();
  let gerberExportable: boolean | null = null;
  let gerberFileCount = 0;
  let renderable: boolean | null = null;
  let previewMeshCount = 0;
  if (wasmAvailable) {
    const fab = await exportFabFiles(pcb);
    gerberExportable = fab !== null && fab.length > 0;
    gerberFileCount = fab?.length ?? 0;
    const meshes = await pcbPreviewMeshes(pcb);
    renderable = meshes.length > 0;
    previewMeshCount = meshes.length;
  }
  const exportability = {
    wasm_available: wasmAvailable,
    gerber_exportable: gerberExportable,
    gerber_file_count: gerberFileCount,
    renderable,
    preview_mesh_count: previewMeshCount,
  };

  const netNames = pcb.nets.map((n) => n.name);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          board,
          stackup: { layers: stackupLayers, copper_layers: copperLayers },
          nets: {
            count: pcb.nets.length,
            names: netNames.slice(0, DESCRIBE_NET_NAME_CAP),
            ...(netNames.length > DESCRIBE_NET_NAME_CAP
              ? { names_truncated: true }
              : {}),
          },
          design_rules: designRules,
          zones,
          traces: {
            segments: pcb.traces.length,
            arcs: (pcb.traceArcs ?? []).length,
            by_net: traceByNet,
            by_layer: traceByLayer,
          },
          vias: {
            count: pcb.vias.length,
            by_net: viaByNet,
            by_layer: viaByLayer,
          },
          components: {
            count: pcb.footprints.length,
            pads: padCount,
            by_footprint: fpByName,
          },
          ...(pcb.netTies && pcb.netTies.length > 0
            ? { net_ties: pcb.netTies.length }
            : {}),
          drc,
          exportability,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_trace — push straight copper segments between consecutive points
// ============================================================================

/** JSON Schema for add_trace tool. */
export const addTraceSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    points: {
      type: "array" as const,
      items: vec2Schema,
      description: "Polyline vertices (>= 2); a Trace is emitted between each consecutive pair.",
    },
    layer: { type: "string" as const, description: "Copper layer (default 'FCu')" },
    net: { type: "string" as const, description: "Net name the copper belongs to" },
    width: {
      type: "number" as const,
      description: "Trace width, mm (default: design-rule traceWidth)",
    },
  },
  required: ["document_id", "points", "net"],
};

/**
 * Push straight copper traces between each consecutive pair of `points` onto a
 * copper layer of the board. Ensures the net exists. The atomic routing
 * primitive add_coil / add_motor_winding lean on.
 */
export async function addTrace(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const points = Array.isArray(args.points) ? (args.points as Vec2[]) : undefined;
  const net = String(args.net ?? "");
  const width = (args.width as number) ?? pcb.rules.defaultRules.traceWidth;

  if (!points || points.length < 2) return fail("points must be an array of >= 2 {x, y}");
  for (const p of points) {
    if (!p || typeof p.x !== "number" || typeof p.y !== "number") {
      return fail("every point must be {x, y} in mm");
    }
  }
  if (!net) return fail("net is required — copper must belong to a net");
  const layerRes = validateCopperLayer((args.layer as string) || "FCu");
  if ("error" in layerRes) return fail(layerRes.error);
  const layer = layerRes.layer;
  if (!(width > 0)) return fail("width must be > 0");

  const drcCap = await beginDrcDelta(pcb, boundsOfPoints(points, width / 2));

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  let tracesAdded = 0;
  let lengthMm = 0;
  for (let i = 0; i + 1 < points.length; i++) {
    const start = { x: round3(points[i].x), y: round3(points[i].y) };
    const end = { x: round3(points[i + 1].x), y: round3(points[i + 1].y) };
    // Tagged manual: route_nets preserves this copper instead of ripping it
    // up on a re-route (issue #277).
    const trace: Trace = { start, end, width, layer, net, source: "manual" };
    pcb.traces.push(trace);
    tracesAdded++;
    lengthMm += Math.hypot(end.x - start.x, end.y - start.y);
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          traces_added: tracesAdded,
          length_mm: Math.round(lengthMm * 1000) / 1000,
          net,
          layer,
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_via — push a layer-spanning via
// ============================================================================

/** JSON Schema for add_via tool. */
export const addViaSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    position: { ...vec2Schema, description: "Via center on the board, mm" },
    net: { type: "string" as const, description: "Net the via belongs to" },
    start_layer: { type: "string" as const, description: "Span start layer (default 'FCu')" },
    end_layer: { type: "string" as const, description: "Span end layer (default 'BCu')" },
    diameter: {
      type: "number" as const,
      description: "Via pad diameter, mm (default: design-rule viaDiameter)",
    },
    drill: {
      type: "number" as const,
      description: "Via drill diameter, mm (default: design-rule viaDrill)",
    },
  },
  required: ["document_id", "position", "net"],
};

/**
 * Push a single via (layer-spanning connection) onto the board. Ensures the net
 * exists. The escape primitive for trapped spiral ends and inter-layer hops.
 */
export async function addVia(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const position = args.position as Vec2;
  const net = String(args.net ?? "");
  const diameter = (args.diameter as number) ?? pcb.rules.defaultRules.viaDiameter;
  const drill = (args.drill as number) ?? pcb.rules.defaultRules.viaDrill;

  if (!position || typeof position.x !== "number" || typeof position.y !== "number") {
    return fail("position must be {x, y} in mm");
  }
  if (!net) return fail("net is required — a via must belong to a net");
  const startRes = validateCopperLayer((args.start_layer as string) || "FCu");
  if ("error" in startRes) return fail(`start_layer: ${startRes.error}`);
  const startLayer = startRes.layer;
  const endRes = validateCopperLayer((args.end_layer as string) || "BCu");
  if ("error" in endRes) return fail(`end_layer: ${endRes.error}`);
  const endLayer = endRes.layer;

  const drcCap = await beginDrcDelta(pcb, boundsOfPoints([position], diameter / 2));

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  const pos = { x: round3(position.x), y: round3(position.y) };
  // Tagged manual: survives route_nets rip-up (issue #277).
  const via: Via = { position: pos, diameter, drill, startLayer, endLayer, net, source: "manual" };
  pcb.vias.push(via);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          net,
          position: pos,
          start_layer: startLayer,
          end_layer: endLayer,
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// set_stackup — set copper weight / layer materials in the board stackup
// ============================================================================

/** Copper weight conversion: 1 oz/ft² ≈ 0.0348 mm of copper. */
const OZ_TO_MM = 0.0348;

/** JSON Schema for set_stackup tool. */
export const setStackupSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    copper_oz: {
      type: "number" as const,
      description:
        "Copper weight in oz/ft² applied to ALL copper layers (1 oz = 0.0348 mm). " +
        "The knob that fixes coil DC-resistance estimates, which assumed 1 oz.",
    },
    layers: {
      type: "array" as const,
      description: "Per-layer overrides; entries naming a missing copper layer are created.",
      items: {
        type: "object" as const,
        properties: {
          layer: { type: "string" as const },
          copper_oz: { type: "number" as const, description: "Copper weight, oz/ft²" },
          copper_thickness_mm: { type: "number" as const, description: "Copper thickness, mm (overrides copper_oz)" },
          dielectric_thickness_mm: { type: "number" as const },
          material: { type: "string" as const },
        },
        required: ["layer"],
      },
    },
  },
  required: ["document_id"],
};

/**
 * Mutate the board stackup: set a uniform copper weight across every copper
 * layer (`copper_oz`) and/or apply per-layer thickness/material overrides.
 * Closes the gap where coil DC-resistance was permanently computed at 1 oz
 * because nothing could set copper weight.
 */
export async function setStackup(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const copperOz = args.copper_oz as number | undefined;
  const perLayer = Array.isArray(args.layers)
    ? (args.layers as Array<Record<string, unknown>>)
    : undefined;

  if (copperOz == null && (!perLayer || perLayer.length === 0)) {
    return fail("provide `copper_oz` and/or a non-empty `layers` array");
  }
  if (copperOz != null && !(copperOz > 0)) return fail("copper_oz must be > 0");

  // Stackup edits move no copper, so a no-op delta is the expected outcome —
  // but it is verified, not assumed (null bounds: connectivity still runs).
  const drcCap = await beginDrcDelta(pcb, null);

  const stackup = pcb.stackup.layers;

  // 1) Uniform copper weight across all copper layers.
  if (copperOz != null) {
    const t = round3(copperOz * OZ_TO_MM);
    for (const l of stackup) {
      if (isCopperLayer(l.layer)) l.copperThickness = t;
    }
  }

  // 2) Per-layer overrides; create copper-layer entries that are missing.
  for (const ov of perLayer ?? []) {
    if (ov.layer == null || String(ov.layer).trim() === "") {
      return fail("each layers entry needs a `layer`");
    }
    const layerRes = validateLayer(ov.layer);
    if ("error" in layerRes) return fail(layerRes.error);
    const layerName = layerRes.layer;
    let entry = stackup.find((l) => l.layer === layerName);
    if (!entry) {
      if (!isCopperLayer(layerName)) {
        return fail(`cannot create non-copper layer "${layerName}" — only copper layers are auto-added`);
      }
      entry = { layer: layerName } as StackupLayer;
      stackup.push(entry);
    }
    if (ov.copper_thickness_mm != null) {
      entry.copperThickness = round3(ov.copper_thickness_mm as number);
    } else if (ov.copper_oz != null) {
      entry.copperThickness = round3((ov.copper_oz as number) * OZ_TO_MM);
    }
    if (ov.dielectric_thickness_mm != null) {
      entry.dielectricThickness = ov.dielectric_thickness_mm as number;
    }
    if (ov.material != null) entry.material = String(ov.material);
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          stackup: pcb.stackup.layers,
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// set_placement — explicit per-component placement (the floorplan primitive)
// ============================================================================

/** JSON Schema for set_placement tool. */
export const setPlacementSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    placements: {
      type: "array" as const,
      description:
        "Absolute per-component placements in the board-local frame (mm). Each " +
        "entry targets a placed footprint by `ref`. Realizes a deliberate " +
        "floorplan the auto-placer (grid/force_directed/radial) can't — thermal " +
        "rings, a quiet IMU corner, rim connectors. Off-board, in-cutout, and " +
        "stacked positions are reported as warnings, not silently accepted.",
      items: {
        type: "object" as const,
        properties: {
          ref: { type: "string" as const, description: "Reference designator, e.g. 'U1', 'Q3', 'J2'" },
          x: { type: "number" as const, description: "Absolute X in the board frame, mm" },
          y: { type: "number" as const, description: "Absolute Y in the board frame, mm" },
          rotation: { type: "number" as const, description: "Absolute rotation, degrees CCW" },
          side: {
            type: "string" as const,
            description: "'top' (default) or 'bottom' — 'bottom' mirrors the footprint to the back copper",
          },
        },
        required: ["ref"],
      },
    },
  },
  required: ["document_id", "placements"],
};

/**
 * Place footprints at explicit board-frame coordinates — the realize step for a
 * hand-authored floorplan. Looks each component up by `ref`, sets position /
 * rotation / side, and validates each landing against the board outline
 * (off-board, inside-a-cutout, and stacked-on-another-footprint become
 * warnings). Mutates the session document.
 */
export async function setPlacement(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = (text: string) => ({
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  });
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  const placements = Array.isArray(args.placements)
    ? (args.placements as Array<Record<string, unknown>>)
    : undefined;
  if (!placements || placements.length === 0) {
    return fail("placements must be a non-empty array of {ref, x?, y?, rotation?, side?}");
  }

  const byRef = new Map<string, Footprint>(
    pcb.footprints.map((f) => [f.ref, f] as [string, Footprint]),
  );
  const outline = pcb.outline.vertices ?? [];
  const cutouts = pcb.outline.cutouts ?? [];

  const moved: string[] = [];
  const rotated: string[] = [];
  const flipped: string[] = [];
  const unknownRefs: string[] = [];
  const warnings: string[] = [];

  for (const p of placements) {
    const ref = String(p.ref ?? "");
    if (!ref) {
      warnings.push("skipped a placement with no `ref`");
      continue;
    }
    const fp = byRef.get(ref);
    if (!fp) {
      unknownRefs.push(ref);
      continue;
    }
    const hasX = typeof p.x === "number";
    const hasY = typeof p.y === "number";
    if (hasX !== hasY) {
      warnings.push(`${ref}: give both x and y (or neither) — ignoring partial position`);
    } else if (hasX && hasY) {
      const pos = { x: round3(p.x as number), y: round3(p.y as number) };
      fp.position = pos;
      moved.push(ref);
      if (outline.length >= 3) {
        if (!pointInPolygon(pos, outline)) {
          warnings.push(`${ref} at (${pos.x}, ${pos.y}) is outside the board outline`);
        } else if (cutouts.some((c) => c.length >= 3 && pointInPolygon(pos, c))) {
          warnings.push(`${ref} at (${pos.x}, ${pos.y}) sits inside a board cutout`);
        }
      }
    }
    if (typeof p.rotation === "number") {
      fp.rotation = p.rotation as number;
      rotated.push(ref);
    }
    const side = p.side != null ? String(p.side).toLowerCase() : undefined;
    if (side === "bottom" || side === "top") {
      const front = side === "top";
      if (fp.front !== front) {
        fp.front = front;
        flipped.push(ref);
      }
    } else if (side != null) {
      warnings.push(`${ref}: side "${String(p.side)}" — use 'top' or 'bottom'`);
    }
  }

  // Two footprints at the same rounded point is almost always a mistake — the
  // auto-placer never does it, so it's worth surfacing.
  const seen = new Map<string, string>();
  for (const ref of moved) {
    const fp = byRef.get(ref)!;
    const key = `${fp.position.x},${fp.position.y}`;
    const prev = seen.get(key);
    if (prev) warnings.push(`${ref} and ${prev} share the exact position ${key} — likely overlap`);
    else seen.set(key, ref);
  }

  if (unknownRefs.length > 0) {
    warnings.push(
      `unknown refs (not on this board): ${unknownRefs.join(", ")} — ` +
        `refs come from the schematic via place_components`,
    );
  }
  if (moved.length === 0 && rotated.length === 0 && flipped.length === 0) {
    return fail(
      `no footprints updated${unknownRefs.length ? ` — all refs unknown: ${unknownRefs.join(", ")}` : ""}`,
    );
  }

  // Explicit placements deliberately do NOT auto-solve constraints — that
  // would silently undo the caller's just-requested positions when they
  // conflict. Warn instead so agency stays with the caller.
  const staleWarning = constraintStaleWarning(ctx.doc);
  if (staleWarning) warnings.push(staleWarning);

  // Re-check the floorplan after the move so the caller can branch on the
  // result in one call — the move→re-check loop never has to reach run_drc.
  const placementDrc = await summarizePlacementDrc(pcb);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          moved: moved.length,
          rotated: rotated.length,
          flipped: flipped.length,
          placement_drc: placementDrc,
          ...(unknownRefs.length > 0 ? { unknown_refs: unknownRefs } : {}),
          ...(warnings.length > 0 ? { warnings } : {}),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_zone — copper pour (ground / power plane) on a net
// ============================================================================

/** JSON Schema for add_zone tool. */
export const addZoneSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    net: { type: "string" as const, description: "Net the pour belongs to (e.g. 'GND', 'VBAT')" },
    layer: { type: "string" as const, description: "Copper layer (default 'FCu')" },
    fill_board: {
      type: "boolean" as const,
      description:
        "Pour the whole board outline (a full plane), treating board cutouts as " +
        "voids. The usual ground/power-plane shortcut. Ignores `outline`.",
    },
    outline: {
      type: "array" as const,
      items: { ...vec2Schema },
      description:
        "Explicit pour polygon, board-local mm (>= 3 points) — for a partial " +
        "plane (e.g. a VBAT pour over the FET drains). Omit with fill_board:true " +
        "for a board-spanning plane.",
    },
    clearance: {
      type: "number" as const,
      description: "Copper-to-copper gap around the pour, mm (default: design-rule clearance)",
    },
    thermal_relief: {
      type: "string" as const,
      description:
        "Pour-to-same-net-pad connection: 'Relief' (default, spoke thermals — " +
        "solderable), 'Direct' (solid copper), or 'None'.",
    },
    thermal_gap: { type: "number" as const, description: "Relief gap around a connected pad, mm" },
    thermal_spoke_width: { type: "number" as const, description: "Relief spoke width, mm" },
    fill_type: { type: "string" as const, description: "'Solid' (default) or 'Hatched'" },
    min_area: {
      type: "number" as const,
      description: "Discard isolated copper islands smaller than this, mm²",
    },
    priority: {
      type: "number" as const,
      description: "Higher-priority pours fill first and win overlaps (default 0)",
    },
    allow_overlap: {
      type: "boolean" as const,
      description:
        "Author the pour even if it overlaps an existing different-net pour on " +
        "the same layer. Default false: such an overlap is a copper short and is " +
        "rejected at author time (see the returned zone_drc). Only set true if a " +
        "higher-priority pour will legitimately clip this one.",
    },
  },
  required: ["document_id", "net"],
};

/**
 * Add a copper zone (pour) — the primitive for ground and power planes, which
 * are fills on a net+layer, not traces. The stored zone is an outline + rules;
 * the fab/DRC pipeline computes the actual filled copper. `fill_board` uses the
 * board outline and treats its cutouts as voids. Mutates the session document.
 */
export function addZone(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = (text: string) => ({
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  });
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const net = String(args.net ?? "");
  if (!net) return fail("net is required — a pour must belong to a net");
  const layerRes = validateCopperLayer((args.layer as string) || "FCu");
  if ("error" in layerRes) return fail(layerRes.error);
  const layer = layerRes.layer;

  const reliefArg = (args.thermal_relief as string) || "Relief";
  if (!["Direct", "Relief", "None"].includes(reliefArg)) {
    return fail("thermal_relief must be 'Direct', 'Relief', or 'None'");
  }
  const fillArg = (args.fill_type as string) || "Solid";
  if (!["Solid", "Hatched"].includes(fillArg)) {
    return fail("fill_type must be 'Solid' or 'Hatched'");
  }

  const fillBoard = args.fill_board === true;
  const explicit = Array.isArray(args.outline)
    ? (args.outline as Array<Record<string, unknown>>)
    : undefined;

  let rawOutline: Vec2[];
  let holes: Vec2[][] | undefined;
  let fillMode: "board" | "polygon";
  if (explicit && !fillBoard) {
    if (explicit.length < 3) return fail("outline needs >= 3 points");
    for (const v of explicit) {
      if (!v || typeof v.x !== "number" || typeof v.y !== "number") {
        return fail("every outline point must be {x, y} in mm");
      }
    }
    rawOutline = explicit.map((v) => ({ x: v.x as number, y: v.y as number }));
    fillMode = "polygon";
  } else {
    const verts = pcb.outline.vertices ?? [];
    if (verts.length < 3) {
      return fail("board has no outline to fill — pass an explicit `outline` polygon");
    }
    rawOutline = verts.map((v) => ({ x: v.x, y: v.y }));
    const cuts = pcb.outline.cutouts ?? [];
    if (cuts.length > 0) {
      holes = cuts.map((c) => c.map((v) => ({ x: round3(v.x), y: round3(v.y) })));
    }
    fillMode = "board";
  }

  const outline = ensureCcw(rawOutline).map((v) => ({ x: round3(v.x), y: round3(v.y) }));
  const clearance = (args.clearance as number) ?? pcb.rules.defaultRules.clearance;

  // Author-time copper-overlap guard. Two different-net pours sharing copper on
  // the same layer is a dead short; the kernel reports it as a `Short` (via
  // pours_touch in drc.rs), but only once run_drc runs — far too late if a
  // power plane was split into 3V3/5V/VBAT islands that overlap. Catch it here
  // and localize the conflict with a bbox so an agent can branch before routing.
  const zoneDrc = summarizeZoneDrc(pcb.zones, { outline, net, layer });
  if (!zoneDrc.clean && args.allow_overlap !== true) {
    const others = [...new Set(zoneDrc.overlaps.map((o) => o.nets[1]))];
    const { min, max } = zoneDrc.overlaps[0]!.bbox;
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: false,
            error:
              `Copper pour on net '${net}' (${layer}) overlaps existing ` +
              `different-net pour${others.length > 1 ? "s" : ""} ` +
              `${others.map((n) => `'${n}'`).join(", ")} on the same layer — ` +
              `this is a short. Overlap bbox: [${min.x}, ${min.y}] → ` +
              `[${max.x}, ${max.y}]. Clip the pour, move it, give a contained ` +
              `higher-priority pour precedence, or pass allow_overlap:true to ` +
              `author it anyway.`,
            zone_drc: zoneDrc,
            ...docResultPayload(ctx),
          }),
        },
      ],
      isError: true as const,
    };
  }

  if (!pcb.nets.some((n) => n.id === net)) pcb.nets.push({ id: net, name: net });

  const zone: Zone = {
    outline,
    net,
    layer,
    clearance,
    fillType: fillArg as NonNullable<Zone["fillType"]>,
    thermalRelief: reliefArg as NonNullable<Zone["thermalRelief"]>,
    priority: typeof args.priority === "number" ? (args.priority as number) : 0,
    ...(holes ? { holes } : {}),
    ...(typeof args.min_area === "number" ? { minArea: args.min_area as number } : {}),
    ...(typeof args.thermal_gap === "number" ? { thermalGap: args.thermal_gap as number } : {}),
    ...(typeof args.thermal_spoke_width === "number"
      ? { thermalSpokeWidth: args.thermal_spoke_width as number }
      : {}),
  };
  pcb.zones.push(zone);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          net,
          layer,
          fill: fillMode,
          vertices: outline.length,
          ...(holes ? { holes: holes.length } : {}),
          clearance,
          thermal_relief: reliefArg,
          zones_total: pcb.zones.length,
          zone_drc: zoneDrc,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// delete_zone / delete_trace / delete_via — remove routed copper from the board
// ============================================================================
//
// add_zone / add_trace / add_via are append-only — before these tools there was
// no way to take a bad pour, trace, or via back out, so one wrong add_zone
// forced a full session rebuild (re-sending the large create_schematic), which
// violates the "never re-send the document" contract. These remove a single
// element by index (or by an unambiguous net/layer match) and report a compact
// `changed` diff of what left the board, mirroring the other mutators. For a
// "take back the very last thing I did" the `undo` tool is the broader hammer.

/** A removed (or, for undo, re-added) board element, for the `changed` diff.
 *  For `netTie` entries `net` is the joined nets as "A+B+C" (a tie has no
 *  single net of its own). */
interface PcbElementChange {
  action: "removed" | "added";
  kind: "zone" | "trace" | "traceArc" | "via" | "netTie";
  index: number;
  net: string;
  layer?: string;
}

/** The copper layer of a board element for matching/reporting — vias span two
 *  layers, so they report none (match by net/position instead). */
function elementLayer(kind: PcbElementChange["kind"], el: Zone | Trace | Via): string | undefined {
  return kind === "via" ? undefined : (el as Zone | Trace).layer;
}

/** Shared body for delete_zone / delete_trace / delete_via. Resolves the target
 *  element (by `index`, or by an unambiguous net[/layer] match), splices it out,
 *  and returns the standard result with a `changed: [{action:'removed', …}]`. */
async function deletePcbElement(args: Record<string, unknown>, kind: "zone" | "trace" | "via") {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  // The collection is one of three element arrays; treat it generically for
  // index/splice and read net/layer per element for matching + reporting.
  const coll = (kind === "zone" ? pcb.zones : kind === "trace" ? pcb.traces : pcb.vias) as Array<
    Zone | Trace | Via
  >;
  const plural = `${kind}s`;
  if (coll.length === 0) return fail(`board has no ${plural} to delete`);

  const wantNet = args.net != null ? String(args.net) : undefined;
  const wantLayer = args.layer != null ? String(args.layer) : undefined;
  const hasIndex = typeof args.index === "number";
  const matchesFilter = (el: Zone | Trace | Via): boolean =>
    (wantNet === undefined || el.net === wantNet) &&
    (wantLayer === undefined || elementLayer(kind, el) === wantLayer);

  let index: number;
  if (hasIndex) {
    index = args.index as number;
    if (!Number.isInteger(index) || index < 0 || index >= coll.length) {
      return fail(`index ${index} out of range — board has ${coll.length} ${plural} (0..${coll.length - 1})`);
    }
    // When a net/layer is also given, it's a guard against deleting the wrong
    // element — confirm the indexed element actually matches.
    if (!matchesFilter(coll[index]!)) {
      const el = coll[index]!;
      const got = elementLayer(kind, el) ? `${el.net}/${elementLayer(kind, el)}` : el.net;
      return fail(`${kind} at index ${index} is on ${got}, not the net/layer you specified`);
    }
  } else if (wantNet !== undefined || wantLayer !== undefined) {
    const matches = coll.flatMap((el, i) => (matchesFilter(el) ? [i] : []));
    const sel = [wantNet, wantLayer].filter(Boolean).join("/");
    if (matches.length === 0) return fail(`no ${kind} matches ${sel}`);
    if (matches.length > 1) {
      return fail(
        `${matches.length} ${plural} match ${sel} (indices ${matches.join(", ")}) — pass \`index\` to pick one`,
      );
    }
    index = matches[0]!;
  } else {
    return fail(`pass \`index\` (0..${coll.length - 1}) or a \`net\` to identify the ${kind} to delete`);
  }

  // Removing copper can't create clearance faults, but it CAN sever a net
  // into islands or orphan a plane stitch — the delta's connectivity pass
  // (always board-global) is what catches that.
  const target = coll[index]!;
  const bounds =
    kind === "zone"
      ? boundsOfPoints((target as Zone).outline)
      : kind === "trace"
        ? boundsOfPoints(
            [(target as Trace).start, (target as Trace).end],
            (target as Trace).width / 2,
          )
        : boundsOfPoints([(target as Via).position], (target as Via).diameter / 2);
  const drcCap = await beginDrcDelta(pcb, bounds);

  const [removed] = coll.splice(index, 1);
  const change: PcbElementChange = {
    action: "removed",
    kind,
    index,
    net: removed!.net,
    ...(elementLayer(kind, removed!) ? { layer: elementLayer(kind, removed!) } : {}),
  };

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          deleted: change,
          [`${plural}_total`]: coll.length,
          changed: [change],
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/** Shared JSON-Schema props for the per-element delete tools. */
const deleteElementProps = {
  ...docInputProperties,
  index: {
    type: "number" as const,
    description: "Zero-based position in the board's collection (the order add_* appended them).",
  },
  net: {
    type: "string" as const,
    description:
      "Net filter. With `index`, a guard (the indexed element must be on this net); without it, identifies the element when exactly one matches.",
  },
};

/** JSON Schema for delete_zone. */
export const deleteZoneSchema = {
  type: "object" as const,
  properties: {
    ...deleteElementProps,
    layer: { type: "string" as const, description: "Copper-layer filter (e.g. 'FCu', 'BCu')." },
  },
  required: ["document_id"],
};

/** Remove a copper pour (zone) from the board — the take-back for a bad
 *  add_zone, without rebuilding the session. Mutates the session document. */
export function deleteZone(args: Record<string, unknown>) {
  return deletePcbElement(args, "zone");
}

/** JSON Schema for delete_trace. */
export const deleteTraceSchema = {
  type: "object" as const,
  properties: {
    ...deleteElementProps,
    layer: { type: "string" as const, description: "Copper-layer filter (e.g. 'FCu', 'BCu')." },
  },
  required: ["document_id"],
};

/** Remove a single routed trace segment from the board. Mutates the session. */
export function deleteTrace(args: Record<string, unknown>) {
  return deletePcbElement(args, "trace");
}

/** JSON Schema for delete_via. */
export const deleteViaSchema = {
  type: "object" as const,
  properties: { ...deleteElementProps },
  required: ["document_id"],
};

/** Remove a single via from the board. Mutates the session document. */
export function deleteVia(args: Record<string, unknown>) {
  return deletePcbElement(args, "via");
}

// ============================================================================
// get_copper — read/query routed copper (the discovery side of the algebra)
// ============================================================================
//
// add_trace/add_via/add_zone write copper and delete_trace/delete_via/
// delete_zone remove it by index — but nothing let an agent SEE what copper
// exists: describe_pcb only aggregates counts, so finding "the trace shorting
// GND to PHA near the star point" meant exporting the whole document. This is
// the read companion: filter by layer/net/bbox/kind and get each element back
// with the same `index` (per-collection, add order) the delete_* tools accept.

/** The queryable copper collections, in report order. */
type CopperKind = "trace" | "traceArc" | "via" | "zone";
const COPPER_KINDS: readonly CopperKind[] = ["trace", "traceArc", "via", "zone"];

/** Axis-aligned bbox as {min, max}. */
interface Bbox {
  min: Vec2;
  max: Vec2;
}

/** Conservative bounding box of a copper element (copper extent included). */
function copperElementBbox(kind: CopperKind, el: Trace | TraceArc | Via | Zone): Bbox {
  switch (kind) {
    case "trace": {
      const t = el as Trace;
      const r = t.width / 2;
      return {
        min: { x: Math.min(t.start.x, t.end.x) - r, y: Math.min(t.start.y, t.end.y) - r },
        max: { x: Math.max(t.start.x, t.end.x) + r, y: Math.max(t.start.y, t.end.y) + r },
      };
    }
    case "traceArc": {
      // Full-circle bound — conservative (a quarter arc reports the whole
      // circle's box), but never misses copper on a bbox query.
      const a = el as TraceArc;
      const r = a.radius + a.width / 2;
      return {
        min: { x: a.center.x - r, y: a.center.y - r },
        max: { x: a.center.x + r, y: a.center.y + r },
      };
    }
    case "via": {
      const v = el as Via;
      const r = v.diameter / 2;
      return {
        min: { x: v.position.x - r, y: v.position.y - r },
        max: { x: v.position.x + r, y: v.position.y + r },
      };
    }
    case "zone": {
      const z = el as Zone;
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const p of z.outline) {
        if (p.x < minX) minX = p.x;
        if (p.y < minY) minY = p.y;
        if (p.x > maxX) maxX = p.x;
        if (p.y > maxY) maxY = p.y;
      }
      return { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } };
    }
  }
}

/** True when a via's barrel spans `layer` (start/end plus every copper layer
 *  between, by stackup order) — a through via FCu→BCu carries In1Cu too. */
function viaSpansLayer(via: Via, layer: PcbLayer): boolean {
  const lo = COPPER_LAYERS.indexOf(via.startLayer);
  const hi = COPPER_LAYERS.indexOf(via.endLayer);
  const at = COPPER_LAYERS.indexOf(layer);
  if (lo < 0 || hi < 0 || at < 0) return via.startLayer === layer || via.endLayer === layer;
  return at >= Math.min(lo, hi) && at <= Math.max(lo, hi);
}

/** Hard cap on elements per get_copper page — keeps a dense board's response
 *  bounded; the caller pages with `offset` (the result carries `total`). */
const GET_COPPER_CAP = 200;

/** JSON Schema for get_copper tool. */
export const getCopperSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    kind: {
      type: "string" as const,
      description:
        "Restrict to one collection: 'trace', 'traceArc', 'via', or 'zone'. " +
        "Omit for all four.",
    },
    layer: {
      type: "string" as const,
      description:
        "Copper-layer filter (e.g. 'FCu', 'BCu', 'In1Cu'). Traces/arcs/zones " +
        "match their own layer; a via matches every layer its barrel spans.",
    },
    net: { type: "string" as const, description: "Net filter (exact id, e.g. 'GND')." },
    bbox: {
      type: "object" as const,
      description:
        "Spatial filter, board-local mm: keep elements whose bounding box " +
        "overlaps the rectangle x..x+w, y..y+h (conservative for arcs).",
      properties: {
        x: { type: "number" as const },
        y: { type: "number" as const },
        w: { type: "number" as const },
        h: { type: "number" as const },
      },
      required: ["x", "y", "w", "h"],
    },
    offset: {
      type: "number" as const,
      description: "Skip this many matches (pagination; default 0).",
    },
    limit: {
      type: "number" as const,
      description: `Max elements returned (default and cap ${GET_COPPER_CAP}).`,
    },
  },
  required: ["document_id"],
};

/**
 * Query the board's routed copper — traces, trace arcs, vias, zones — with
 * optional layer/net/bbox/kind filters. Each element is returned with its
 * `kind` and `index`: the exact addressing delete_trace / delete_via /
 * delete_zone accept, so a query result can drive a surgical delete without
 * ever exporting the document. Read-only.
 */
export function getCopper(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  let kinds: readonly CopperKind[] = COPPER_KINDS;
  if (args.kind != null) {
    const k = String(args.kind);
    if (!COPPER_KINDS.includes(k as CopperKind)) {
      return fail(`kind '${k}' is not valid; legal: ${COPPER_KINDS.join(", ")}`);
    }
    kinds = [k as CopperKind];
  }

  let wantLayer: PcbLayer | undefined;
  if (args.layer != null) {
    const layerRes = validateCopperLayer(args.layer);
    if ("error" in layerRes) return fail(layerRes.error);
    wantLayer = layerRes.layer;
  }
  const wantNet = args.net != null ? String(args.net) : undefined;

  let queryBox: Bbox | undefined;
  if (args.bbox != null) {
    const b = args.bbox as Record<string, unknown>;
    if (
      typeof b.x !== "number" || typeof b.y !== "number" ||
      typeof b.w !== "number" || typeof b.h !== "number"
    ) {
      return fail("bbox must be {x, y, w, h} in board-local mm");
    }
    if ((b.w as number) < 0 || (b.h as number) < 0) return fail("bbox w and h must be >= 0");
    queryBox = {
      min: { x: b.x as number, y: b.y as number },
      max: { x: (b.x as number) + (b.w as number), y: (b.y as number) + (b.h as number) },
    };
  }

  const offset = typeof args.offset === "number" ? Math.max(0, Math.floor(args.offset)) : 0;
  const limit =
    typeof args.limit === "number"
      ? Math.min(GET_COPPER_CAP, Math.max(1, Math.floor(args.limit)))
      : GET_COPPER_CAP;

  const overlaps = (a: Bbox, b: Bbox): boolean =>
    a.min.x <= b.max.x && a.max.x >= b.min.x && a.min.y <= b.max.y && a.max.y >= b.min.y;

  const layerMatches = (kind: CopperKind, el: Trace | TraceArc | Via | Zone): boolean => {
    if (wantLayer === undefined) return true;
    if (kind === "via") return viaSpansLayer(el as Via, wantLayer);
    return (el as Trace | TraceArc | Zone).layer === wantLayer;
  };

  const rv = (v: Vec2): Vec2 => ({ x: round3(v.x), y: round3(v.y) });
  const rbox = (b: Bbox): Bbox => ({ min: rv(b.min), max: rv(b.max) });

  /** The reported view of one element — identity first, geometry after. */
  const describe = (
    kind: CopperKind,
    index: number,
    el: Trace | TraceArc | Via | Zone,
  ): Record<string, unknown> => {
    switch (kind) {
      case "trace": {
        const t = el as Trace;
        return {
          kind, index, net: t.net, layer: t.layer,
          start: rv(t.start), end: rv(t.end), width: t.width,
          ...(t.source ? { source: t.source } : {}),
        };
      }
      case "traceArc": {
        const a = el as TraceArc;
        return {
          kind, index, net: a.net, layer: a.layer,
          center: rv(a.center), radius: round3(a.radius),
          start_angle: a.startAngle, end_angle: a.endAngle, width: a.width,
        };
      }
      case "via": {
        const v = el as Via;
        return {
          kind, index, net: v.net, layers: [v.startLayer, v.endLayer],
          position: rv(v.position), diameter: v.diameter, drill: v.drill,
          ...(v.source ? { source: v.source } : {}),
        };
      }
      case "zone": {
        // A pour's outline can run to hundreds of vertices (board fills) —
        // report its bbox + vertex count; the index is enough to delete it.
        const z = el as Zone;
        return {
          kind, index, net: z.net, layer: z.layer,
          bbox: rbox(copperElementBbox("zone", z)), vertices: z.outline.length,
          ...(z.holes && z.holes.length > 0 ? { holes: z.holes.length } : {}),
          clearance: z.clearance, priority: z.priority ?? 0,
        };
      }
    }
  };

  const collections: Record<CopperKind, Array<Trace | TraceArc | Via | Zone>> = {
    trace: pcb.traces,
    traceArc: pcb.traceArcs ?? [],
    via: pcb.vias,
    zone: pcb.zones,
  };

  // Match in deterministic order (traces, arcs, vias, zones; index order
  // within each) so offset-paging never skips or repeats an element.
  const matched: Array<{ kind: CopperKind; index: number; el: Trace | TraceArc | Via | Zone }> = [];
  const totalByKind: Partial<Record<CopperKind, number>> = {};
  for (const kind of kinds) {
    collections[kind].forEach((el, index) => {
      if (wantNet !== undefined && el.net !== wantNet) return;
      if (!layerMatches(kind, el)) return;
      if (queryBox && !overlaps(copperElementBbox(kind, el), queryBox)) return;
      matched.push({ kind, index, el });
      totalByKind[kind] = (totalByKind[kind] ?? 0) + 1;
    });
  }

  const page = matched.slice(offset, offset + limit);
  const elements = page.map((m) => describe(m.kind, m.index, m.el));

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          total: matched.length,
          total_by_kind: totalByKind,
          count: elements.length,
          offset,
          ...(offset + elements.length < matched.length
            ? { next_offset: offset + elements.length }
            : {}),
          elements,
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_net_tie / delete_net_tie — intentional net junctions
// ============================================================================
//
// Wye motor windings, split grounds joined at one stitch, and current-sense
// shunts all REQUIRE two named nets to touch on purpose — without a declared
// tie, DRC correctly reports the junction as a short. Until now only the
// add_motor_winding realizer could author a NetTie; an agent hand-building a
// wye had to do offline JSON surgery on the saved .vcad. These tools complete
// the algebra: author and remove ties on the live session.

/** Tolerance for matching a tie by position in delete_net_tie, mm. */
const TIE_POSITION_TOL = 1e-3;

/** The reported view of one net tie: the stored fields plus its index (the
 *  addressing delete_net_tie accepts) and the DRC scope it resolves to. */
function describeNetTie(tie: NetTie, index: number): Record<string, unknown> {
  return {
    index,
    nets: tie.nets,
    ...(tie.position ? { position: tie.position } : {}),
    ...(tie.radius !== undefined ? { radius: tie.radius } : {}),
    scope: tie.position && tie.radius !== undefined ? "region" : "board_wide",
  };
}

/** The full tie list, as returned by both tie tools after a mutation. */
function netTieList(pcb: Pcb): Array<Record<string, unknown>> {
  return (pcb.netTies ?? []).map((t, i) => describeNetTie(t, i));
}

/** JSON Schema for add_net_tie tool. */
export const addNetTieSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Names of the nets joined at this tie (>= 2, all must exist on the " +
        "board) — e.g. the three phases + neutral of a wye, or GND + AGND.",
    },
    position: {
      ...vec2Schema,
      description:
        "Center of the allowed join region, board-local mm. Give WITH `radius` " +
        "to scope the exemption; omit both for a board-wide tie.",
    },
    radius: {
      type: "number" as const,
      description:
        "Radius of the allowed join region, mm (> 0). Requires `position`. " +
        "Size it to cover the junction copper with margin (~1 trace width): " +
        "DRC judges each contact at an estimated contact point (e.g. a trace " +
        "midpoint), not the exact geometric touch.",
    },
  },
  required: ["document_id", "nets"],
};

/**
 * Declare an intentional junction between two or more nets (a net-tie) so DRC
 * treats them as one electrical node where they meet — the primitive behind
 * wye/star neutral points, split-ground stitches, and current-sense shunt
 * taps. Region-scoped (position+radius) ties exempt clearance/short checks
 * only for contacts inside the region — and connectivity: nets joined through
 * copper are legal when each has a tie-covered contact there (a stray crossing
 * of the same nets elsewhere still fires). Region-less ties exempt the pair
 * board-wide. Mutates the session document.
 */
export async function addNetTie(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const rawNets = Array.isArray(args.nets) ? (args.nets as unknown[]) : undefined;
  if (!rawNets) return fail("nets is required — the >= 2 net names this tie joins");
  const nets: string[] = [];
  for (const n of rawNets) {
    const name = String(n ?? "").trim();
    if (!name) return fail("every entry in nets must be a non-empty net name");
    if (!nets.includes(name)) nets.push(name);
  }
  if (nets.length < 2) return fail("a net tie joins at least 2 distinct nets");

  // A tie on a net that doesn't exist is a typo, not an intention — and it
  // would silently exempt nothing. Validate against the board's netlist.
  const known = new Set(pcb.nets.map((n) => n.id));
  const unknown = nets.filter((n) => !known.has(n));
  if (unknown.length > 0) {
    const names = pcb.nets.map((n) => n.id);
    const shown = names.slice(0, 30).join(", ");
    return fail(
      `unknown net${unknown.length > 1 ? "s" : ""} ${unknown.map((n) => `'${n}'`).join(", ")} — ` +
        `board nets: ${shown}${names.length > 30 ? `, … (${names.length} total)` : ""}`,
    );
  }

  // The kernel only forms a scoped region when BOTH position and radius are
  // present (NetTieGroups in drc.rs); one without the other would silently
  // degrade to a board-wide exemption. Fail closed instead.
  const hasPosition = args.position != null;
  const hasRadius = args.radius != null;
  if (hasPosition !== hasRadius) {
    return fail(
      "position and radius must be given together — one without the other " +
        "would silently become a board-wide exemption. Pass both to scope the " +
        "tie to a region, or neither for an explicit board-wide tie.",
    );
  }
  let position: Vec2 | undefined;
  let radius: number | undefined;
  if (hasPosition) {
    const p = args.position as Record<string, unknown>;
    if (typeof p.x !== "number" || typeof p.y !== "number") {
      return fail("position must be {x, y} in board-local mm");
    }
    radius = args.radius as number;
    if (typeof radius !== "number" || !(radius > 0)) return fail("radius must be > 0 mm");
    position = { x: round3(p.x as number), y: round3(p.y as number) };
    radius = round3(radius);
  }

  // A tie mutates DRC *semantics*, not copper: it exempts (and, deleted,
  // re-convicts) short/clearance findings. A region-scoped tie only changes
  // verdicts inside its circle; a board-wide tie can change them anywhere the
  // tied nets' copper meets.
  const drcCap = await beginDrcDelta(
    pcb,
    position && radius !== undefined
      ? boundsOfPoints([position], radius)
      : "full",
  );

  pcb.netTies = pcb.netTies ?? [];
  const tie: NetTie = {
    nets,
    ...(position ? { position } : {}),
    ...(radius !== undefined ? { radius } : {}),
  };
  pcb.netTies.push(tie);
  const index = pcb.netTies.length - 1;

  const change: PcbElementChange = {
    action: "added",
    kind: "netTie",
    index,
    net: nets.join("+"),
  };

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          tie: describeNetTie(tie, index),
          net_ties: netTieList(pcb),
          net_ties_total: pcb.netTies.length,
          changed: [change],
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

/** JSON Schema for delete_net_tie tool. */
export const deleteNetTieSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    index: {
      type: "number" as const,
      description:
        "Zero-based position in pcb.netTies (as reported by add_net_tie / get_document).",
    },
    nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Match ties joining exactly this net set (order-insensitive). With " +
        "`index`, a guard; without it, identifies the tie when exactly one matches.",
    },
    position: {
      ...vec2Schema,
      description:
        `Match region-scoped ties centered here (±${TIE_POSITION_TOL} mm) — ` +
        "disambiguates when the same net set is tied at several junctions " +
        "(e.g. the three X–Y ties of a delta winding).",
    },
  },
  required: ["document_id"],
};

/** Remove a net tie by `index`, or by matching `nets` (set equality) and/or
 *  `position` — the take-back for a bad add_net_tie. The junction's copper (if
 *  any) stays; DRC will report it as a short again. Mutates the session. */
export async function deleteNetTie(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  const ties = pcb.netTies ?? [];
  if (ties.length === 0) return fail("board has no net ties to delete");

  let wantNets: string[] | undefined;
  if (args.nets != null) {
    if (!Array.isArray(args.nets)) return fail("nets must be an array of net names");
    wantNets = (args.nets as unknown[]).map((n) => String(n ?? "").trim()).sort();
  }
  let wantPos: Vec2 | undefined;
  if (args.position != null) {
    const p = args.position as Record<string, unknown>;
    if (typeof p.x !== "number" || typeof p.y !== "number") {
      return fail("position must be {x, y} in board-local mm");
    }
    wantPos = { x: p.x as number, y: p.y as number };
  }

  const matchesFilter = (tie: NetTie): boolean => {
    if (wantNets) {
      const have = [...tie.nets].sort();
      if (have.length !== wantNets.length || have.some((n, i) => n !== wantNets![i])) return false;
    }
    if (wantPos) {
      if (!tie.position) return false;
      if (
        Math.abs(tie.position.x - wantPos.x) > TIE_POSITION_TOL ||
        Math.abs(tie.position.y - wantPos.y) > TIE_POSITION_TOL
      ) {
        return false;
      }
    }
    return true;
  };

  let index: number;
  if (typeof args.index === "number") {
    index = args.index as number;
    if (!Number.isInteger(index) || index < 0 || index >= ties.length) {
      return fail(`index ${index} out of range — board has ${ties.length} net ties (0..${ties.length - 1})`);
    }
    // A nets/position given alongside the index is a guard against deleting
    // the wrong tie — confirm the indexed tie actually matches.
    if (!matchesFilter(ties[index]!)) {
      return fail(
        `net tie at index ${index} joins [${ties[index]!.nets.join(", ")}]` +
          `${ties[index]!.position ? ` at (${ties[index]!.position!.x}, ${ties[index]!.position!.y})` : ""}, ` +
          "not the nets/position you specified",
      );
    }
  } else if (wantNets || wantPos) {
    const matches = ties.flatMap((t, i) => (matchesFilter(t) ? [i] : []));
    const sel = [
      wantNets ? `[${wantNets.join(", ")}]` : undefined,
      wantPos ? `(${wantPos.x}, ${wantPos.y})` : undefined,
    ]
      .filter(Boolean)
      .join(" at ");
    if (matches.length === 0) return fail(`no net tie matches ${sel}`);
    if (matches.length > 1) {
      return fail(
        `${matches.length} net ties match ${sel} (indices ${matches.join(", ")}) — pass \`index\` or \`position\` to pick one`,
      );
    }
    index = matches[0]!;
  } else {
    return fail(
      `pass \`index\` (0..${ties.length - 1}), \`nets\`, or \`position\` to identify the tie to delete`,
    );
  }

  // Removing a tie un-exempts whatever it was excusing — copper contact at
  // the junction reads as a Short again. Scope to the tie's region when it
  // has one; a board-wide tie could have been excusing contact anywhere.
  const target = ties[index]!;
  const drcCap = await beginDrcDelta(
    pcb,
    target.position && target.radius != null
      ? boundsOfPoints([target.position], target.radius)
      : "full",
  );

  const [removed] = ties.splice(index, 1);
  const change: PcbElementChange = {
    action: "removed",
    kind: "netTie",
    index,
    net: removed!.nets.join("+"),
  };

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          deleted: describeNetTie(removed!, index),
          net_ties: netTieList(pcb),
          net_ties_total: ties.length,
          changed: [change],
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// undo — rewind the last mutation on a session (snapshot stack)
// ============================================================================

/** Multiset diff of two element arrays (before vs after), keyed by full JSON so
 *  identical-but-reordered elements don't show as churn. Emits removed entries
 *  first, then added — enough to describe what an undo (or any swap) did. */
function diffElementArray(
  kind: PcbElementChange["kind"],
  before: Array<Zone | Trace | Via>,
  after: Array<Zone | Trace | Via>,
): PcbElementChange[] {
  const countKeys = (arr: Array<Zone | Trace | Via>) => {
    const m = new Map<string, number>();
    for (const el of arr) m.set(JSON.stringify(el), (m.get(JSON.stringify(el)) ?? 0) + 1);
    return m;
  };
  const beforeKeys = countKeys(before);
  const afterKeys = countKeys(after);
  const out: PcbElementChange[] = [];
  const describe = (action: "removed" | "added", el: Zone | Trace | Via, index: number) => ({
    action,
    kind,
    index,
    net: el.net,
    ...(elementLayer(kind, el) ? { layer: elementLayer(kind, el) } : {}),
  });
  before.forEach((el, i) => {
    const key = JSON.stringify(el);
    if ((afterKeys.get(key) ?? 0) > 0) afterKeys.set(key, afterKeys.get(key)! - 1);
    else out.push(describe("removed", el, i));
  });
  after.forEach((el, i) => {
    const key = JSON.stringify(el);
    if ((beforeKeys.get(key) ?? 0) > 0) beforeKeys.set(key, beforeKeys.get(key)! - 1);
    else out.push(describe("added", el, i));
  });
  return out;
}

/** Board-element diff between two PCBs (or null when neither has a board). */
function diffPcbElements(before: Pcb | null, after: Pcb | null): PcbElementChange[] {
  if (!before || !after) return [];
  // Ties carry no single `net` — view them with the joined names so the
  // generic differ (which keys on full JSON and reads `.net`) can report them.
  const tieView = (ties?: NetTie[]) =>
    (ties ?? []).map((t) => ({ ...t, net: t.nets.join("+") }));
  return [
    ...diffElementArray("zone", before.zones ?? [], after.zones ?? []),
    ...diffElementArray("trace", before.traces ?? [], after.traces ?? []),
    ...diffElementArray("traceArc", (before.traceArcs ?? []) as never, (after.traceArcs ?? []) as never),
    ...diffElementArray("via", before.vias ?? [], after.vias ?? []),
    ...diffElementArray("netTie", tieView(before.netTies) as never, tieView(after.netTies) as never),
  ];
}

/** JSON Schema for the undo tool. */
export const undoSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id whose last mutation to rewind.",
    },
  },
  required: ["document_id"],
};

/**
 * Rewind the most recent mutation on a session by restoring the snapshot taken
 * before it. The dispatch layer snapshots the whole Document before every
 * mutating tool, so this generalizes across the board: it takes back the last
 * add_zone / add_trace / add_via / delete_* / route_nets / place_components —
 * or a CAD create/update/delete — without re-sending the document. Repeated
 * calls walk further back through the stack. Reports a compact `changed` diff
 * of the board elements the rewind moved (when the session has a PCB).
 */
export function undo(args: Record<string, unknown>) {
  const id = args.document_id ? String(args.document_id) : "";
  if (!id) return ecadError("undo needs a `document_id` (the session to rewind)");
  // Resolve first so a bad/foreign id throws the pinned "Unknown document_id"
  // before any state is touched — ownership is enforced here.
  const current = getSession(id);
  const beforePcb = getDocPcb(current);
  const beforePcbCopy = beforePcb ? (JSON.parse(JSON.stringify(beforePcb)) as Pcb) : null;

  const restored = undoLastSnapshot(id);
  if (!restored) {
    return ecadError("nothing to undo — no mutation has been recorded for this session");
  }
  const changed = diffPcbElements(beforePcbCopy, getDocPcb(restored));

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          undone: true,
          remaining_undos: historyDepth(id),
          changed,
          document_id: id,
        }),
      },
    ],
  };
}

// ============================================================================
// set_design_rules — configurable DRC rules + net classes
// ============================================================================

/** JSON Schema for set_design_rules tool. */
export const setDesignRulesSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    clearance: { type: "number" as const, description: "Default copper-to-copper clearance, mm" },
    track_width: { type: "number" as const, description: "Default minimum trace width, mm" },
    via_diameter: { type: "number" as const, description: "Default via pad diameter, mm" },
    via_drill: { type: "number" as const, description: "Default via drill diameter, mm" },
    diff_pair_gap: { type: "number" as const, description: "Default differential-pair gap, mm" },
    diff_pair_width: { type: "number" as const, description: "Default differential-pair trace width, mm" },
    edge_clearance: { type: "number" as const, description: "Copper-to-board-edge clearance, mm" },
    hole_to_hole: { type: "number" as const, description: "Minimum hole-to-hole spacing, mm" },
    min_annular_ring: { type: "number" as const, description: "Minimum via annular ring, mm" },
    min_drill: { type: "number" as const, description: "Minimum drill diameter, mm" },
    classes: {
      type: "array" as const,
      description:
        "Net classes, each overriding the default clearance/width for its nets — " +
        "DRC honors per-class clearance and trace width. This is how to enforce a " +
        "high-voltage / power class (give VBAT and the phase nets a wide " +
        "clearance) separate from signal nets. NOTE: creepage is not yet a " +
        "distinct rule — model HV spacing as a high-clearance class here.",
      items: {
        type: "object" as const,
        properties: {
          name: { type: "string" as const, description: "Class name, e.g. 'power', 'hv'" },
          nets: {
            type: "array" as const,
            items: { type: "string" as const },
            description: "Net ids assigned to this class",
          },
          clearance: { type: "number" as const },
          track_width: { type: "number" as const },
          via_diameter: { type: "number" as const },
          via_drill: { type: "number" as const },
          diff_pair_gap: { type: "number" as const },
          diff_pair_width: { type: "number" as const },
        },
        required: ["name", "nets"],
      },
    },
  },
  required: ["document_id"],
};

/**
 * Mutate the board design rules consumed by run_drc (and used as defaults by
 * route_nets / add_coil). Sets default clearances/widths and/or net classes so a
 * power or high-voltage class can carry wider clearance than signal nets. run_drc
 * already reads pcb.rules — this is the writer for it. Mutates the session
 * document.
 */
export function setDesignRules(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = (text: string) => ({
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  });

  // No board yet: don't dead-end. Validate the shape (so a class with no nets or
  // an empty call still fails fast), then buffer the rules on the document and
  // replay them when place_components builds the board. The agent can set rules
  // in any order; the canonical next step (place_components) rides back as a
  // next_action so it stays discoverable.
  if (!pcb) {
    const probe = applyDesignRuleArgs(defaultDesignRules(), new Set<string>(), args, {
      checkNets: false,
    });
    if (probe.error) return fail(probe.error);
    if (!probe.touched) {
      return fail("provide at least one rule field (clearance, track_width, …) or `classes`");
    }
    (ctx.doc as DocWithPendingRules).__pendingDesignRules = stripDocArgs(args);
    const next: NextAction[] = [
      {
        action:
          "No board yet — these design rules are buffered and apply automatically when the board is built. Run place_components next.",
        tool: "place_components",
        ...(ctx.documentId ? { args: { document_id: ctx.documentId } } : {}),
      },
    ];
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            buffered: true,
            ...(probe.classNames ? { classes: probe.classNames } : {}),
            ...(probe.warnings.length > 0 ? { warnings: probe.warnings } : {}),
            next_actions: next,
            ...docResultPayload(ctx),
          }),
        },
      ],
      structuredContent: { next_actions: next },
    };
  }

  const rules = pcb.rules;
  const res = applyDesignRuleArgs(rules, new Set(pcb.nets.map((n) => n.id)), args);
  if (res.error) return fail(res.error);
  if (!res.touched) {
    return fail("provide at least one rule field (clearance, track_width, …) or `classes`");
  }

  const dr = rules.defaultRules;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          rules: {
            clearance: dr.clearance,
            track_width: dr.traceWidth,
            via_diameter: dr.viaDiameter,
            via_drill: dr.viaDrill,
            edge_clearance: rules.edgeClearance,
            hole_to_hole: rules.holeToHole,
            min_annular_ring: rules.minAnnularRing,
            min_drill: rules.minDrill,
          },
          ...(res.classNames ? { classes: res.classNames } : {}),
          ...(res.warnings.length > 0 ? { warnings: res.warnings } : {}),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// size_trace_for_current — IPC-2221 ampacity → trace width
// ============================================================================

/** JSON Schema for size_trace_for_current tool. */
export const sizeTraceForCurrentSchema = {
  type: "object" as const,
  properties: {
    current_a: { type: "number" as const, description: "Continuous current the trace must carry, A" },
    copper_oz: {
      type: "number" as const,
      description: "Copper weight, oz/ft² (default 1; 1 oz ≈ 0.0348 mm). Match set_stackup.",
    },
    temp_rise_c: {
      type: "number" as const,
      description: "Allowed conductor temperature rise above ambient, °C (default 10)",
    },
    layer: {
      type: "string" as const,
      description:
        "'outer' (default, in free air) or 'inner' (buried; derated ~2× because " +
        "it sheds heat poorly, so it needs much more width for the same current).",
    },
    fab_grid_mm: {
      type: "number" as const,
      description: "Snap the solved width UP to this manufacturing grid, mm (default 0.0254 = 1 mil)",
    },
  },
  required: ["current_a"],
};

/**
 * IPC-2221 closed-form conductor ampacity, solved for width:
 *   I = k · ΔT^0.44 · A^0.725   (A = cross-section in mil², k = 0.048 outer / 0.024 inner)
 * Pure calc, no document — the ampacity sibling of size_impedance/size_pdn.
 * Conservative vs IPC-2152 chart data (which credits board conduction and gives
 * narrower traces) — the safe default for power nets.
 */
export function sizeTraceForCurrent(args: Record<string, unknown>) {
  const fail = (text: string) => ({
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  });
  const current = args.current_a as number;
  if (typeof current !== "number" || !(current > 0)) return fail("current_a must be > 0");
  const copperOz = typeof args.copper_oz === "number" ? (args.copper_oz as number) : 1;
  if (!(copperOz > 0)) return fail("copper_oz must be > 0");
  const dT = typeof args.temp_rise_c === "number" ? (args.temp_rise_c as number) : 10;
  if (!(dT > 0)) return fail("temp_rise_c must be > 0");
  const layer = String(args.layer ?? "outer").toLowerCase() === "inner" ? "inner" : "outer";
  const grid =
    typeof args.fab_grid_mm === "number" && (args.fab_grid_mm as number) > 0
      ? (args.fab_grid_mm as number)
      : 0.0254;

  const k = layer === "inner" ? 0.024 : 0.048;
  const MIL_PER_MM = 1 / 0.0254;
  const thicknessMm = copperOz * OZ_TO_MM;
  const thicknessMil = thicknessMm * MIL_PER_MM;

  // A_mil2 = (I / (k·ΔT^0.44))^(1/0.725)
  const areaMil2 = Math.pow(current / (k * Math.pow(dT, 0.44)), 1 / 0.725);
  const widthMil = areaMil2 / thicknessMil;
  const widthMmRaw = widthMil / MIL_PER_MM;
  const widthMm = Math.ceil(widthMmRaw / grid) * grid;
  const crossMm2 = widthMm * thicknessMm;

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          standard: "IPC-2221",
          current_a: current,
          temp_rise_c: dT,
          copper_oz: copperOz,
          copper_thickness_mm: round3(thicknessMm),
          layer,
          width_mm: round3(widthMm),
          width_mm_raw: round3(widthMmRaw),
          cross_section_mm2: round3(crossMm2),
          cross_section_mil2: Math.round(areaMil2 * 100) / 100,
          note:
            `IPC-2221 closed form (k=${k}). Conservative vs IPC-2152; inner ` +
            `layers derate ~2×. Width snapped up to a ${grid}mm grid.`,
        }),
      },
    ],
  };
}

// ============================================================================
// add_via_array — grid / batch of vias (thermal field, plane stitching)
// ============================================================================

/** JSON Schema for add_via_array tool. */
export const addViaArraySchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    net: { type: "string" as const, description: "Net for every via in the array (e.g. 'GND')" },
    region: {
      type: "object" as const,
      description:
        "Rectangular region (board-local mm) filled with a via grid at `pitch` " +
        "spacing — thermal-via fields under FETs, GND-plane stitching.",
      properties: {
        x: { type: "number" as const },
        y: { type: "number" as const },
        w: { type: "number" as const },
        h: { type: "number" as const },
      },
      required: ["x", "y", "w", "h"],
    },
    points: {
      type: "array" as const,
      items: { ...vec2Schema },
      description: "Explicit via centers (board-local mm) — alternative to `region` for hand stitching.",
    },
    pitch: { type: "number" as const, description: "Grid spacing for region mode, mm (default 1.0)" },
    start_layer: { type: "string" as const, description: "Span start layer (default 'FCu')" },
    end_layer: { type: "string" as const, description: "Span end layer (default 'BCu')" },
    diameter: { type: "number" as const, description: "Via pad diameter, mm (default: design-rule viaDiameter)" },
    drill: { type: "number" as const, description: "Via drill, mm (default: design-rule viaDrill)" },
    clip_to_board: {
      type: "boolean" as const,
      description: "Drop grid vias outside the board outline / inside a cutout (default true)",
    },
  },
  required: ["document_id", "net"],
};

/** Max vias one call will place — a guard against a fine pitch over a big region. */
const MAX_VIA_ARRAY = 2000;

/**
 * Place many vias at once: a regular grid over a rectangular `region` (thermal
 * vias, plane stitching) or an explicit list of `points`. Grid vias are clipped
 * to the board outline by default. Mutates the session document.
 */
export async function addViaArray(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = (text: string) => ({
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  });
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const net = String(args.net ?? "");
  const diameter = (args.diameter as number) ?? pcb.rules.defaultRules.viaDiameter;
  const drill = (args.drill as number) ?? pcb.rules.defaultRules.viaDrill;
  if (!net) return fail("net is required — a via must belong to a net");
  const startRes = validateCopperLayer((args.start_layer as string) || "FCu");
  if ("error" in startRes) return fail(`start_layer: ${startRes.error}`);
  const startLayer = startRes.layer;
  const endRes = validateCopperLayer((args.end_layer as string) || "BCu");
  if ("error" in endRes) return fail(`end_layer: ${endRes.error}`);
  const endLayer = endRes.layer;

  const explicitPoints = Array.isArray(args.points)
    ? (args.points as Array<Record<string, unknown>>)
    : undefined;
  const region = args.region as { x?: unknown; y?: unknown; w?: unknown; h?: unknown } | undefined;

  const candidates: Vec2[] = [];
  let mode: "region" | "points";
  if (explicitPoints && explicitPoints.length > 0) {
    for (const p of explicitPoints) {
      if (!p || typeof p.x !== "number" || typeof p.y !== "number") {
        return fail("every point must be {x, y} in mm");
      }
      candidates.push({ x: p.x as number, y: p.y as number });
    }
    mode = "points";
  } else if (
    region &&
    typeof region.x === "number" &&
    typeof region.y === "number" &&
    typeof region.w === "number" &&
    typeof region.h === "number"
  ) {
    const x0 = region.x as number;
    const y0 = region.y as number;
    const w = region.w as number;
    const h = region.h as number;
    if (!(w > 0) || !(h > 0)) return fail("region.w and region.h must be > 0");
    const pitch =
      typeof args.pitch === "number" && (args.pitch as number) > 0 ? (args.pitch as number) : 1.0;
    const cols = Math.floor(w / pitch) + 1;
    const rows = Math.floor(h / pitch) + 1;
    if (cols * rows > MAX_VIA_ARRAY) {
      return fail(
        `grid would be ${cols * rows} vias (> ${MAX_VIA_ARRAY}) — increase pitch or shrink region`,
      );
    }
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        candidates.push({ x: x0 + c * pitch, y: y0 + r * pitch });
      }
    }
    mode = "region";
  } else {
    return fail("provide either `region` {x,y,w,h} or a non-empty `points` array");
  }

  // Clip grid vias to the board (default on). Points mode is taken as authored.
  const clip = args.clip_to_board !== false && mode === "region";
  const outline = pcb.outline.vertices ?? [];
  const cutouts = pcb.outline.cutouts ?? [];
  let skipped = 0;
  const kept: Vec2[] = [];
  for (const p of candidates) {
    if (clip && outline.length >= 3) {
      if (!pointInPolygon(p, outline)) { skipped++; continue; }
      if (cutouts.some((cc) => cc.length >= 3 && pointInPolygon(p, cc))) { skipped++; continue; }
    }
    kept.push(p);
  }

  if (kept.length === 0) {
    return fail(
      mode === "region"
        ? "no vias landed inside the board outline — check region coordinates"
        : "no via points given",
    );
  }

  const drcCap = await beginDrcDelta(pcb, boundsOfPoints(kept, diameter / 2));

  if (!pcb.nets.some((n) => n.id === net)) pcb.nets.push({ id: net, name: net });

  for (const p of kept) {
    pcb.vias.push({
      position: { x: round3(p.x), y: round3(p.y) },
      diameter,
      drill,
      startLayer,
      endLayer,
      net,
      source: "manual",
    });
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          net,
          mode,
          vias_added: kept.length,
          ...(skipped > 0 ? { skipped_outside_board: skipped } : {}),
          start_layer: startLayer,
          end_layer: endLayer,
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_motor_winding — plan → copper realizer (closes the winding loop)
// ============================================================================

/** Point at polar (r, ang·rad) around c. Not rounded — round at push time. */
function windPolar(c: Vec2, r: number, ang: number): Vec2 {
  return { x: c.x + r * Math.cos(ang), y: c.y + r * Math.sin(ang) };
}

/** Angle of p around c, radians normalized to [0, 2π). */
function windAngle(c: Vec2, p: Vec2): number {
  const a = Math.atan2(p.y - c.y, p.x - c.x);
  return a < 0 ? a + 2 * Math.PI : a;
}

/** Minimum centerline distance between segments ab and cd. */
function windSegSegDist(a: Vec2, b: Vec2, c: Vec2, d: Vec2): number {
  if (segmentCrossing(a, b, c, d)) return 0;
  return Math.min(
    pointSegDist(a, c, d),
    pointSegDist(b, c, d),
    pointSegDist(c, a, b),
    pointSegDist(d, a, b),
  );
}

/** Approximate closest-approach point between segments ab and cd. */
function windSegClosestPt(a: Vec2, b: Vec2, c: Vec2, d: Vec2): Vec2 {
  const clamp = (p: Vec2, s: Vec2, e: Vec2): Vec2 => {
    const dx = e.x - s.x;
    const dy = e.y - s.y;
    const len2 = dx * dx + dy * dy;
    if (len2 === 0) return s;
    const t = Math.max(0, Math.min(1, ((p.x - s.x) * dx + (p.y - s.y) * dy) / len2));
    return { x: s.x + t * dx, y: s.y + t * dy };
  };
  let best: [Vec2, Vec2] = [a, clamp(a, c, d)];
  let bestD = Infinity;
  for (const [p, q] of [
    [a, clamp(a, c, d)],
    [b, clamp(b, c, d)],
    [c, clamp(c, a, b)],
    [d, clamp(d, a, b)],
  ] as Array<[Vec2, Vec2]>) {
    const dd = Math.hypot(p.x - q.x, p.y - q.y);
    if (dd < bestD) {
      bestD = dd;
      best = [p, q];
    }
  }
  return { x: (best[0].x + best[1].x) / 2, y: (best[0].y + best[1].y) / 2 };
}

/**
 * Arc polyline at radius r around c from angle a0 to a1 (radians). Sweeps
 * CCW when a1 > a0, CW when a1 < a0. Chord sagitta capped at 0.02mm so the
 * polyline never strays meaningfully off its ring radius.
 */
function windArcPoints(c: Vec2, r: number, a0: number, a1: number): Vec2[] {
  const sweep = a1 - a0;
  const maxStep = Math.max(0.01, 2 * Math.acos(Math.max(0, 1 - 0.02 / Math.max(r, 0.02))));
  const n = Math.max(1, Math.ceil(Math.abs(sweep) / maxStep));
  const pts: Vec2[] = [];
  for (let i = 0; i <= n; i++) pts.push(windPolar(c, r, a0 + (sweep * i) / n));
  return pts;
}

/** JSON Schema for add_motor_winding tool. */
export const addMotorWindingSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    slots: { type: "number" as const, description: "Slot/tooth count" },
    poles: { type: "number" as const, description: "Pole count 2p (even >= 2)" },
    phases: { type: "number" as const, description: "Phase count (default 3)" },
    turns_per_coil: { type: "number" as const, description: "Turns per coil (default 1)" },
    connection: {
      type: "string" as const,
      description: "'wye' (default, star neutral) or 'delta' (loop)",
    },
    center: { ...vec2Schema, description: "Ring center on the board, mm" },
    pitch_radius: {
      type: "number" as const,
      description: "Radius of the ring the coil CENTERS sit on, mm",
    },
    inner_radius: { type: "number" as const, description: "Each coil's inner turn radius, mm" },
    outer_radius: { type: "number" as const, description: "Each coil's outer turn radius, mm" },
    trace_width: { type: "number" as const, description: "Copper trace width, mm" },
    copper_layer: { type: "string" as const, description: "Layer the spirals are drawn on (default 'FCu')" },
    return_layer: {
      type: "string" as const,
      description:
        "Layer for the interconnect's radial drops and terminal vias; the " +
        "interconnect arcs ride the spiral layer (default 'BCu')",
    },
    phase_nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Override phase net names (default PHA/PHB/PHC...)",
    },
    neutral_net: { type: "string" as const, description: "Wye neutral net name (default 'WIND_N')" },
    clearance: { type: "number" as const, description: "Turn-to-turn clearance, mm" },
    segments_per_turn: { type: "number" as const, description: "Spiral polyline resolution" },
  },
  required: [
    "document_id",
    "slots",
    "poles",
    "center",
    "pitch_radius",
    "inner_radius",
    "outer_radius",
    "trace_width",
  ],
};

/**
 * One-shot motor-winding realizer: plans a balanced polyphase winding with
 * winding_layout, drops a spiral coil per tooth, series-connects coils within
 * each phase, and terminates the phases (wye star or delta loop) with a
 * region-scoped NetTie at each junction so DRC treats the join as intentional.
 *
 * The interconnect is planar by construction:
 *
 * - Each coil's terminals land on vias placed clear of the spiral body — the
 *   inner terminal leads to a via at the coil center (the bore is copper-free),
 *   the outer leads radially outward past the last turn. A via dropped directly
 *   on the spiral endpoint would overlap the adjacent turn (turn pitch is
 *   usually smaller than via radius + trace half-width) and silently short it.
 * - Every phase gets one arc ring in the coil-free bore around the winding
 *   center: series hops ride that ring on the spiral layer, reached by radial
 *   drops on the return layer, joined only through vias. Rings are staggered
 *   by via-to-via pitch, so arcs never cross anything.
 * - Approach angles are jogged so no return-layer radial runs along a terminal
 *   ray past a same-net via it doesn't terminate at (which would electrically
 *   bypass a coil).
 * - The wye star is a short radial junction across the phase-ring ends at an
 *   angle chosen on real board material (never over a bore cutout, never at
 *   the board center), carrying the neutral net under a scoped NetTie.
 * - Phase (and neutral) feeds are routed to same-net pads when any exist, via
 *   staggered escape arcs outside the coil ring, each candidate checked for
 *   collisions before it is committed; unroutable feeds are reported, never
 *   guessed.
 *
 * A post-build audit re-checks both invariants (no same-net via bypass, no
 * cross-net contact outside a tie region) and reports violations as errors.
 */
export async function addMotorWinding(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const fail = ecadError;
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const center = args.center as Vec2;
  const pitchRadius = args.pitch_radius as number;
  const innerR = args.inner_radius as number;
  const outerR = args.outer_radius as number;
  const traceWidth = args.trace_width as number;
  const turnsPerCoil = (args.turns_per_coil as number) ?? 1;
  const connection = (args.connection as string) === "delta" ? "delta" : "wye";

  if (!center || typeof center.x !== "number" || typeof center.y !== "number") {
    return fail("center must be {x, y} in mm");
  }
  if (!(pitchRadius >= 0)) return fail("pitch_radius must be >= 0");
  if (!(outerR > innerR)) return fail("outer_radius must be > inner_radius");
  if (!(traceWidth > 0)) return fail("trace_width must be > 0");
  const copperRes = validateCopperLayer((args.copper_layer as string) || "FCu");
  if ("error" in copperRes) return fail(`copper_layer: ${copperRes.error}`);
  const copperLayer = copperRes.layer;
  const returnRes = validateCopperLayer((args.return_layer as string) || "BCu");
  if ("error" in returnRes) return fail(`return_layer: ${returnRes.error}`);
  const returnLayer = returnRes.layer;

  // a. Plan the winding.
  const planRes = windingLayout({
    slots: args.slots,
    poles: args.poles,
    phases: args.phases,
    turns_per_coil: turnsPerCoil,
    connection,
    layer: "double",
    phase_nets: args.phase_nets,
    neutral_net: args.neutral_net,
  });
  if ("isError" in planRes && planRes.isError) {
    return fail(planRes.content[0]!.text.replace(/^Error:\s*/, ""));
  }
  const plan = JSON.parse(planRes.content[0]!.text) as WindingPlan;
  if (!plan.feasible) return fail(plan.reason ?? "infeasible winding");

  const childDoc = ctx.documentId ? { document_id: ctx.documentId } : { document: ctx.doc };
  const errors: string[] = [];
  let coilsPlaced = 0;
  let vias = 0;
  let interconnectTraces = 0;

  const TAU = Math.PI * 2;
  const rules = pcb.rules;
  const viaDia = rules.defaultRules.viaDiameter;
  const viaDrill = rules.defaultRules.viaDrill;
  const clearance = (args.clearance as number) ?? rules.defaultRules.clearance;
  const halfW = traceWidth / 2;
  const viaR = viaDia / 2;
  const MARGIN = 0.05;

  // Pads by net — feed targets (pre-existing pads only; this tool adds none).
  const padsByNet = new Map<string, Array<{ pos: Vec2; onReturnLayer: boolean; size: number }>>();
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      if (!pad.net) continue;
      const arr = padsByNet.get(pad.net) ?? [];
      arr.push({
        pos: padWorld(fp, pad),
        onReturnLayer: pad.layers.includes(returnLayer),
        size: padApproxRadius(pad),
      });
      padsByNet.set(pad.net, arr);
    }
  }
  const neutral =
    connection === "wye"
      ? (plan.neutralNet ?? (args.neutral_net ? String(args.neutral_net) : "WIND_N"))
      : undefined;
  const neutralFeedWanted = !!(neutral && (padsByNet.get(neutral)?.length ?? 0) > 0);

  // Interconnect geometry, validated up front so an unroutable request fails
  // BEFORE any copper lands: one arc ring per phase (plus one for delta return
  // links / the neutral feed) in the coil-free bore. Ring pitch is the worst
  // pairwise requirement — via-to-via clearance or drilled hole-to-hole — so
  // staggered rings can never interact.
  const planPhases = Object.keys(plan.phaseSeries).length;
  const ringSpacing = Math.max(viaDia + clearance, viaDrill + rules.holeToHole) + MARGIN;
  const ringOuterBound = pitchRadius - outerR - (viaR + halfW + clearance + MARGIN);
  const nRings =
    connection === "wye" ? planPhases + (neutralFeedWanted ? 1 : 0) : planPhases * 2;
  const ringInnermost = ringOuterBound - (nRings - 1) * ringSpacing;
  const interconnectNeeded = plan.coils.length > 1 || connection === "wye";
  const onMaterial = (p: Vec2): boolean =>
    pointInPolygon(p, pcb.outline.vertices) &&
    !(pcb.outline.cutouts ?? []).some((c) => c.length >= 3 && pointInPolygon(p, c));

  if (interconnectNeeded) {
    // The rings must land on real board material: clear of any outline cutout
    // that intrudes into the bore (e.g. a shaft bore), by edge clearance.
    let cutoutReach = 0;
    for (const cut of pcb.outline.cutouts ?? []) {
      if (cut.length < 3) continue;
      const dists = cut.map((v) => Math.hypot(v.x - center.x, v.y - center.y));
      if (Math.min(...dists) < ringOuterBound + viaR + rules.edgeClearance) {
        cutoutReach = Math.max(cutoutReach, Math.max(...dists));
      }
    }
    const minInner = Math.max(viaR + rules.edgeClearance, cutoutReach + viaR + rules.edgeClearance);
    if (!(ringInnermost >= minInner)) {
      return fail(
        `interconnect doesn't fit: ${nRings} arc ring(s) at ${round3(ringSpacing)}mm pitch ` +
          `need radii ${round3(ringInnermost)}–${round3(ringOuterBound)}mm around the winding ` +
          `center, but the innermost usable radius is ${round3(minInner)}mm` +
          (cutoutReach > 0 ? ` (a bore/cutout reaches ${round3(cutoutReach)}mm)` : "") +
          `. Increase pitch_radius, reduce outer_radius, or shrink the bore.`,
      );
    }
  }

  // Jog angle: sized so a radial run at (terminal ray ± jog) clears every via
  // sitting ON the terminal ray, at every radius down to the innermost ring —
  // the no-bypass invariant. Also spaces same-ring via pairs at one coil.
  const viaSep = Math.max(viaDia + clearance, viaDrill + rules.holeToHole) + MARGIN;
  const jog = interconnectNeeded
    ? Math.asin(Math.min(1, viaSep / Math.max(ringInnermost, viaSep)))
    : 0;

  // Wye star angle: past the last coil ray (plus jogs), short of the first
  // coil's feed jog window near 2π — and on real board material, so the
  // junction never lands over a bore cutout (and never at the board center).
  let starAngle: number | undefined;
  if (connection === "wye" && interconnectNeeded) {
    const maxCoilAngle = ((plan.coils.length - 1) / Math.max(1, plan.coils.length)) * TAU;
    const wFrom = maxCoilAngle + 2 * jog;
    const wTo = TAU - 2.5 * jog;
    const rTop = ringOuterBound;
    const rBot = ringOuterBound - (planPhases - 1) * ringSpacing;
    const rMid = (rTop + rBot) / 2;
    const steps = 24;
    for (let i = 0; i <= steps && wTo > wFrom; i++) {
      const a = wFrom + ((wTo - wFrom) * i) / steps;
      const jc = windPolar(center, rMid, a);
      const probe = (rTop - rBot) / 2 + viaR + rules.edgeClearance;
      let ok = onMaterial(jc);
      for (let k = 0; k < 8 && ok; k++) {
        ok = onMaterial(windPolar(jc, probe, (k * TAU) / 8));
      }
      if (ok) {
        starAngle = a;
        break;
      }
    }
    if (starAngle == null) {
      return fail(
        "no board material for the star junction in the reachable angular window — " +
          "the outline/cutouts leave nowhere to land the neutral. Adjust the outline " +
          "or the winding placement.",
      );
    }
  }

  // The field-report tool: it used to return a bare document_version for a
  // board it had just shorted. Full-board capture — a winding spans the stator
  // annulus, the bore interconnect rings, AND rim feed escapes out to the
  // board edge, so a bbox scope would cover most of the board anyway.
  const drcCap = await beginDrcDelta(pcb, "full");

  const tracesBase = pcb.traces.length;
  const viasBase = pcb.vias.length;
  const pushTrace = (a: Vec2, b: Vec2, layer: PcbLayer, net: string): boolean => {
    const ra = { x: round3(a.x), y: round3(a.y) };
    const rb = { x: round3(b.x), y: round3(b.y) };
    if (ra.x === rb.x && ra.y === rb.y) return false;
    pcb.traces.push({ start: ra, end: rb, width: traceWidth, layer, net, source: "manual" });
    return true;
  };
  const pushVia = (p: Vec2, net: string): Vec2 => {
    const rp = { x: round3(p.x), y: round3(p.y) };
    pcb.vias.push({
      position: rp,
      diameter: viaDia,
      drill: viaDrill,
      startLayer: copperLayer,
      endLayer: returnLayer,
      net,
      source: "manual",
    });
    vias++;
    return rp;
  };
  const pushPolyline = (pts: Vec2[], layer: PcbLayer, net: string): void => {
    for (let i = 0; i + 1 < pts.length; i++) {
      if (pushTrace(pts[i]!, pts[i + 1]!, layer, net)) interconnectTraces++;
    }
  };

  // b. One spiral per tooth. Terminal strategy: the spiral's own endpoints are
  //    NOT via sites — a via on the spiral start/end overlaps the adjacent turn
  //    (turn pitch < via radius + trace half-width), silently bypassing it.
  //    Instead the inner endpoint leads to a via at the coil CENTER (guaranteed
  //    copper-free bore) and the outer endpoint leads radially outward far
  //    enough for the via pad to clear the outermost turn.
  const leadOut = viaR + halfW + clearance + MARGIN;
  interface CoilTerm {
    net: string;
    centerVia: Vec2;
    centerAngle: number;
    outerVia: Vec2;
    outerAngle: number;
  }
  const coilTerminals = new Map<number, CoilTerm>();
  const coilTraceRanges: Array<[number, number]> = [];
  for (const coil of plan.coils) {
    const angle = (coil.angleDeg * Math.PI) / 180;
    const coilCenter = {
      x: round3(center.x + pitchRadius * Math.cos(angle)),
      y: round3(center.y + pitchRadius * Math.sin(angle)),
    };
    const dir = coil.polarity === 1 ? "ccw" : "cw";
    const beforeTraces = pcb.traces.length;
    const res = await addCoil({
      ...childDoc,
      center: coilCenter,
      turns: turnsPerCoil,
      inner_radius: innerR,
      outer_radius: outerR,
      trace_width: traceWidth,
      net: coil.net,
      layer: copperLayer,
      direction: dir,
      start_angle_deg: coil.angleDeg,
      clearance: args.clearance,
      segments_per_turn: args.segments_per_turn,
      inner_via: false,
    }, { skipDrcDelta: true });
    if (res.isError) {
      errors.push(`coil slot ${coil.slot} (net ${coil.net}): ${res.content[0]!.text}`);
      continue;
    }
    coilTraceRanges.push([beforeTraces, pcb.traces.length]);
    const payload = JSON.parse(res.content[0]!.text) as {
      inner_endpoint: Vec2;
      outer_endpoint: Vec2;
    };
    coilsPlaced++;
    // Inner lead-in: spiral start → coil center, via there (spiral layer run).
    if (pushTrace(payload.inner_endpoint, coilCenter, copperLayer, coil.net)) {
      interconnectTraces++;
    }
    const centerVia = pushVia(coilCenter, coil.net);
    // Outer lead-out: spiral end → radially outward, via clear of the last turn.
    const outDirAng = windAngle(coilCenter, payload.outer_endpoint);
    const rOut = Math.hypot(
      payload.outer_endpoint.x - coilCenter.x,
      payload.outer_endpoint.y - coilCenter.y,
    );
    const outerViaPos = windPolar(coilCenter, rOut + leadOut, outDirAng);
    if (pushTrace(payload.outer_endpoint, outerViaPos, copperLayer, coil.net)) {
      interconnectTraces++;
    }
    const outerVia = pushVia(outerViaPos, coil.net);
    coilTerminals.set(coil.slot, {
      net: coil.net,
      centerVia,
      centerAngle: windAngle(center, centerVia),
      outerVia,
      outerAngle: windAngle(center, outerVia),
    });
  }

  // Phases that actually placed coils, in plan order.
  const phaseList = Object.keys(plan.phaseSeries).filter((n) =>
    plan.phaseSeries[n]!.some((s) => coilTerminals.has(s)),
  );
  const seriesRing = (p: number) => ringOuterBound - p * ringSpacing;
  const buildInterconnect =
    coilsPlaced > 0 && (coilsPlaced > 1 || connection === "wye");

  // Return-layer drop from an outer terminal via to a ring: short jog chord at
  // the terminal radius (off the terminal ray), then a radial to the ring, then
  // a layer-hop via. Returns the ring angle the arc starts at.
  const buildDescent = (term: CoilTerm, ringR: number, net: string): number => {
    const aFrom = term.outerAngle + jog;
    const rO = Math.hypot(term.outerVia.x - center.x, term.outerVia.y - center.y);
    const jogEnd = windPolar(center, rO, aFrom);
    if (pushTrace(term.outerVia, jogEnd, returnLayer, net)) interconnectTraces++;
    const hop = windPolar(center, ringR, aFrom);
    if (pushTrace(jogEnd, hop, returnLayer, net)) interconnectTraces++;
    pushVia(hop, net);
    return aFrom;
  };

  // d. Series links per phase: ride the phase's ring on the spiral layer from
  //    (previous coil's outer drop) to the next coil's center-via ray, then hop
  //    back to the return layer and run the radial up to the center via.
  const phaseChain = new Map<string, { first: CoilTerm; last: CoilTerm }>();
  phaseList.forEach((netName, p) => {
    const present = plan.phaseSeries[netName]!.filter((s) => coilTerminals.has(s));
    const ringR = seriesRing(p);
    const first = coilTerminals.get(present[0]!)!;
    let last = first;
    for (let i = 1; i < present.length; i++) {
      const prev = coilTerminals.get(present[i - 1]!)!;
      const cur = coilTerminals.get(present[i]!)!;
      const aFrom = buildDescent(prev, ringR, netName);
      let aTo = cur.centerAngle;
      while (aTo <= aFrom + 1e-9) aTo += TAU;
      pushPolyline(windArcPoints(center, ringR, aFrom, aTo), copperLayer, netName);
      const hopEnd = pushVia(windPolar(center, ringR, aTo), netName);
      if (pushTrace(hopEnd, cur.centerVia, returnLayer, netName)) interconnectTraces++;
      last = cur;
    }
    phaseChain.set(netName, { first, last });
  });

  // e. Termination + scoped net-tie at each junction.
  pcb.netTies = pcb.netTies ?? [];
  let netTiesAdded = 0;
  let starJunction: Vec2 | undefined;
  let neutralFeedVia: Vec2 | undefined;

  if (connection === "wye" && buildInterconnect && phaseList.length > 0 && starAngle != null) {
    const rTop = seriesRing(0);
    const rBot = seriesRing(phaseList.length - 1);

    // Each phase ring continues past its last coil to the star angle…
    phaseList.forEach((netName, p) => {
      const ringR = seriesRing(p);
      const { last } = phaseChain.get(netName)!;
      const aFrom = buildDescent(last, ringR, netName);
      let aTo = starAngle!;
      while (aTo <= aFrom + 1e-9) aTo += TAU;
      pushPolyline(windArcPoints(center, ringR, aFrom, aTo), copperLayer, netName);
    });
    // …and the neutral is a short radial across the ring ends, on the spiral
    // layer, touching each phase exactly at its arc end.
    const neutralNet = neutral!;
    if (!pcb.nets.some((n) => n.id === neutralNet)) {
      pcb.nets.push({ id: neutralNet, name: neutralNet });
    }
    const jTop = windPolar(center, rTop, starAngle);
    const jBot = windPolar(center, rBot, starAngle);
    if (pushTrace(jTop, jBot, copperLayer, neutralNet)) interconnectTraces++;
    if (neutralFeedWanted) {
      const rN = ringOuterBound - phaseList.length * ringSpacing;
      const jN = windPolar(center, rN, starAngle);
      if (pushTrace(jBot, jN, copperLayer, neutralNet)) interconnectTraces++;
      neutralFeedVia = pushVia(jN, neutralNet);
    }
    starJunction = { x: round3((jTop.x + jBot.x) / 2), y: round3((jTop.y + jBot.y) / 2) };
    pcb.netTies.push({
      nets: [...phaseList, neutralNet],
      position: starJunction,
      radius: round3((rTop - rBot) / 2 + (neutralFeedWanted ? ringSpacing : 0) + traceWidth + 1),
    });
    netTiesAdded++;
  } else if (connection === "delta" && buildInterconnect && phaseList.length > 1) {
    // Delta: phase[i]'s end rides its own link ring to phase[i+1]'s first-coil
    // ray, then ascends to that coil's center via — the junction, tied there.
    const n = phaseList.length;
    for (let p = 0; p < n; p++) {
      const a = phaseList[p]!;
      const b = phaseList[(p + 1) % n]!;
      const endTerm = phaseChain.get(a)!.last;
      const startTerm = phaseChain.get(b)!.first;
      const ringR = ringOuterBound - (n + p) * ringSpacing;
      const aFrom = buildDescent(endTerm, ringR, a);
      let aTo = startTerm.centerAngle;
      while (aTo <= aFrom + 1e-9) aTo += TAU;
      pushPolyline(windArcPoints(center, ringR, aFrom, aTo), copperLayer, a);
      const hopEnd = pushVia(windPolar(center, ringR, aTo), a);
      // Ascend to phase b's center via — the X–Y junction. The last stretch is
      // its own short segment so DRC's contact-point estimate (a trace's
      // midpoint) lands inside the tie region.
      const junctionR = Math.hypot(
        startTerm.centerVia.x - center.x,
        startTerm.centerVia.y - center.y,
      );
      const approach = windPolar(center, junctionR - viaSep, aTo);
      if (pushTrace(hopEnd, approach, returnLayer, a)) interconnectTraces++;
      if (pushTrace(approach, startTerm.centerVia, returnLayer, a)) interconnectTraces++;
      pcb.netTies.push({
        nets: [a, b],
        position: startTerm.centerVia,
        radius: round3(viaR + traceWidth + clearance + 1),
      });
      netTiesAdded++;
    }
  }

  // f. Phase/neutral feeds to same-net pads, when any exist. Escape scheme:
  //    jog off the first coil's center-via ray (opposite side from the series
  //    escapes), radial out on the return layer past the outer terminals, a
  //    staggered escape arc to the pad's angle, then a drop onto the pad (via
  //    if the pad doesn't reach the return layer). Every candidate is checked
  //    against all existing copper before it is committed; ring assignments
  //    and arc directions are searched, and anything unroutable is reported.
  const phaseFeeds: Record<string, Vec2> = {};
  const feedsRouted: string[] = [];
  const feedsUnrouted: string[] = [];
  {
    interface FeedSpec {
      net: string;
      start: Vec2;
      startRadius: number;
      angle: number;
      useJog: boolean;
    }
    const specs: FeedSpec[] = [];
    for (const netName of phaseList) {
      const first = phaseChain.get(netName)?.first;
      if (!first) continue;
      phaseFeeds[netName] = first.centerVia;
      if ((padsByNet.get(netName)?.length ?? 0) > 0) {
        specs.push({
          net: netName,
          start: first.centerVia,
          startRadius: Math.hypot(first.centerVia.x - center.x, first.centerVia.y - center.y),
          angle: first.centerAngle,
          useJog: true,
        });
      }
    }
    if (neutralFeedWanted && neutralFeedVia && starJunction) {
      phaseFeeds[neutral!] = neutralFeedVia;
      specs.push({
        net: neutral!,
        start: neutralFeedVia,
        startRadius: Math.hypot(neutralFeedVia.x - center.x, neutralFeedVia.y - center.y),
        angle: windAngle(center, neutralFeedVia),
        useJog: false,
      });
    }

    if (specs.length > 0) {
      const outerViaRadii = [...coilTerminals.values()].map((t) =>
        Math.hypot(t.outerVia.x - center.x, t.outerVia.y - center.y),
      );
      const rEscBase =
        (outerViaRadii.length ? Math.max(...outerViaRadii) : pitchRadius + outerR) +
        viaR +
        halfW +
        clearance +
        MARGIN;
      // Outer material limit: nearest outline edge from the winding center.
      let rimDist = Infinity;
      const ov = pcb.outline.vertices;
      for (let i = 0; i < ov.length; i++) {
        rimDist = Math.min(rimDist, pointSegDist(center, ov[i]!, ov[(i + 1) % ov.length]!));
      }
      const rEscMax = rimDist - rules.edgeClearance - viaR;

      type Candidate = {
        traces: Array<{ a: Vec2; b: Vec2; layer: PcbLayer }>;
        vias: Vec2[];
      };
      const buildFeed = (spec: FeedSpec, ringIdx: number, dirSign: 1 | -1): Candidate | null => {
        const rEsc = rEscBase + ringIdx * ringSpacing;
        if (rEsc + halfW > rEscMax) return null;
        const pads = padsByNet.get(spec.net)!;
        const target = pads.reduce((best, p) =>
          Math.hypot(p.pos.x - spec.start.x, p.pos.y - spec.start.y) <
          Math.hypot(best.pos.x - spec.start.x, best.pos.y - spec.start.y)
            ? p
            : best,
        );
        const aEsc = spec.useJog ? spec.angle - jog : spec.angle;
        const traces: Candidate["traces"] = [];
        const cvias: Vec2[] = [];
        let prev = spec.start;
        if (spec.useJog) {
          const jogEnd = windPolar(center, spec.startRadius, aEsc);
          traces.push({ a: prev, b: jogEnd, layer: returnLayer });
          prev = jogEnd;
        }
        const escTop = windPolar(center, rEsc, aEsc);
        traces.push({ a: prev, b: escTop, layer: returnLayer });
        let aPad = windAngle(center, target.pos);
        if (dirSign > 0) {
          while (aPad <= aEsc + 1e-9) aPad += TAU;
        } else {
          while (aPad >= aEsc - 1e-9) aPad -= TAU;
        }
        // Stop the arc short of the pad's ray so the last chord stays clear of
        // the pad drop-via — the final diagonal then terminates AT the via
        // instead of riding past it (which the bypass audit would flag).
        const padR = Math.hypot(target.pos.x - center.x, target.pos.y - center.y);
        const lim = viaR + halfW + clearance + MARGIN;
        let standoff = 0;
        if (Math.abs(rEsc - padR) < lim && rEsc > 0 && padR > 0) {
          const cosD = (rEsc * rEsc + padR * padR - lim * lim) / (2 * rEsc * padR);
          standoff = Math.acos(Math.max(-1, Math.min(1, cosD)));
        }
        const arcEnd = aPad - dirSign * standoff;
        if (Math.abs(arcEnd - aEsc) > 1e-6 && dirSign * (arcEnd - aEsc) > 0) {
          const arc = windArcPoints(center, rEsc, aEsc, arcEnd);
          for (let i = 0; i + 1 < arc.length; i++) {
            traces.push({ a: arc[i]!, b: arc[i + 1]!, layer: returnLayer });
          }
          traces.push({ a: arc[arc.length - 1]!, b: target.pos, layer: returnLayer });
        } else {
          traces.push({ a: escTop, b: target.pos, layer: returnLayer });
        }
        if (!target.onReturnLayer) cvias.push(target.pos);
        return { traces, vias: cvias };
      };

      // Collision check against all existing copper (and board material).
      const feedClear = (cand: Candidate, net: string): boolean => {
        for (const t of cand.traces) {
          if (!onMaterial(t.a) || !onMaterial(t.b)) return false;
          for (const other of pcb.traces) {
            if (other.net === net || other.layer !== t.layer) continue;
            if (windSegSegDist(t.a, t.b, other.start, other.end) < halfW + other.width / 2 + clearance) {
              return false;
            }
          }
          for (const v of pcb.vias) {
            if (v.net === net) continue;
            if (pointSegDist(v.position, t.a, t.b) < v.diameter / 2 + halfW + clearance) return false;
          }
          for (const fp of pcb.footprints) {
            for (const pad of fp.pads) {
              if (pad.net === net) continue;
              const pw = padWorld(fp, pad);
              if (pointSegDist(pw, t.a, t.b) < padApproxRadius(pad) + halfW + clearance) return false;
            }
          }
        }
        for (const v of cand.vias) {
          for (const other of pcb.vias) {
            const d = Math.hypot(other.position.x - v.x, other.position.y - v.y);
            if (other.net !== net && d < viaDia + clearance) return false;
            if (d < viaDrill + rules.holeToHole && d > 1e-6) return false;
          }
          for (const other of pcb.traces) {
            if (other.net === net) continue;
            if (pointSegDist(v, other.start, other.end) < viaR + other.width / 2 + clearance) {
              return false;
            }
          }
        }
        return true;
      };

      const commit = (cand: Candidate, net: string) => {
        for (const t of cand.traces) {
          if (pushTrace(t.a, t.b, t.layer, net)) interconnectTraces++;
        }
        for (const v of cand.vias) pushVia(v, net);
      };

      // Search ring assignments (≤4 feeds → ≤24 permutations), directions
      // greedily per feed; first fully-clear assignment wins.
      const perms = (idx: number[]): number[][] =>
        idx.length <= 1
          ? [idx]
          : idx.flatMap((v, i) => perms([...idx.slice(0, i), ...idx.slice(i + 1)]).map((r) => [v, ...r]));
      let done = false;
      for (const perm of perms(specs.map((_, i) => i))) {
        const staged: Array<{ cand: Candidate; net: string }> = [];
        let ok = true;
        for (let s = 0; s < specs.length && ok; s++) {
          const spec = specs[s]!;
          let placed = false;
          for (const dirSign of [1, -1] as const) {
            const cand = buildFeed(spec, perm[s]!, dirSign);
            if (!cand) continue;
            // check against pcb + already-staged candidates
            const stagedHit = staged.some((st) =>
              st.net !== spec.net &&
              st.cand.traces.some((a) =>
                cand.traces.some(
                  (b) => a.layer === b.layer && windSegSegDist(a.a, a.b, b.a, b.b) < traceWidth + clearance,
                ),
              ),
            );
            if (!stagedHit && feedClear(cand, spec.net)) {
              staged.push({ cand, net: spec.net });
              placed = true;
              break;
            }
          }
          if (!placed) ok = false;
        }
        if (ok) {
          for (const st of staged) commit(st.cand, st.net);
          feedsRouted.push(...staged.map((st) => st.net));
          done = true;
          break;
        }
      }
      if (!done) {
        // Fall back to routing whatever fits, first-fit; report the rest.
        for (const spec of specs) {
          let placed = false;
          for (let ringIdx = 0; ringIdx < specs.length && !placed; ringIdx++) {
            for (const dirSign of [1, -1] as const) {
              const cand = buildFeed(spec, ringIdx, dirSign);
              if (cand && feedClear(cand, spec.net)) {
                commit(cand, spec.net);
                placed = true;
                break;
              }
            }
          }
          (placed ? feedsRouted : feedsUnrouted).push(spec.net);
        }
      }
    }
  }

  // g. Post-build audit: re-verify the two invariants on the copper this call
  //    added. (1) No trace within clearance of a same-net via it doesn't
  //    terminate at — the silent-bypass case. (2) No cross-net copper contact
  //    or sub-clearance approach outside a declared tie region.
  const auditIssues: string[] = [];
  {
    const near = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.y - b.y) <= 1e-3;
    const tieExempt = (a: string, b: string, at: Vec2): boolean =>
      (pcb.netTies ?? []).some(
        (t) =>
          t.nets.includes(a) &&
          t.nets.includes(b) &&
          (!t.position ||
            t.radius == null ||
            Math.hypot(at.x - t.position.x, at.y - t.position.y) <= t.radius),
      );
    const isCoilTrace = (idx: number) =>
      coilTraceRanges.some(([s, e]) => idx >= s && idx < e);
    const bboxFar = (a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2, lim: number) =>
      Math.min(a1.x, b1.x) > Math.max(a2.x, b2.x) + lim ||
      Math.min(a2.x, b2.x) > Math.max(a1.x, b1.x) + lim ||
      Math.min(a1.y, b1.y) > Math.max(a2.y, b2.y) + lim ||
      Math.min(a2.y, b2.y) > Math.max(a1.y, b1.y) + lim;

    for (let vi = viasBase; vi < pcb.vias.length; vi++) {
      const v = pcb.vias[vi]!;
      for (let ti = tracesBase; ti < pcb.traces.length; ti++) {
        const t = pcb.traces[ti]!;
        if (t.net !== v.net) continue;
        const lim = v.diameter / 2 + t.width / 2 + clearance - 1e-6;
        if (bboxFar(t.start, t.end, v.position, v.position, lim)) continue;
        const d = pointSegDist(v.position, t.start, t.end);
        if (d >= lim) continue;
        if (near(v.position, t.start) || near(v.position, t.end)) continue;
        auditIssues.push(
          `audit: net '${t.net}' trace (${t.start.x},${t.start.y})→(${t.end.x},${t.end.y}) passes ` +
            `${round3(d)}mm from a same-net via at (${v.position.x},${v.position.y}) it doesn't ` +
            `terminate at — electrical bypass`,
        );
      }
    }

    for (let ti = tracesBase; ti < pcb.traces.length; ti++) {
      const t = pcb.traces[ti]!;
      const tIsCoil = isCoilTrace(ti);
      for (let tj = tracesBase; tj < pcb.traces.length; tj++) {
        if (tj <= ti || (tIsCoil && isCoilTrace(tj))) continue;
        const u = pcb.traces[tj]!;
        if (u.net === t.net || u.layer !== t.layer) continue;
        const lim = t.width / 2 + u.width / 2 + clearance - 1e-6;
        if (bboxFar(t.start, t.end, u.start, u.end, lim)) continue;
        const d = windSegSegDist(t.start, t.end, u.start, u.end);
        if (d >= lim) continue;
        const at = windSegClosestPt(t.start, t.end, u.start, u.end);
        if (tieExempt(t.net, u.net, at)) continue;
        auditIssues.push(
          `audit: nets '${t.net}'/'${u.net}' copper ${round3(d)}mm apart at ` +
            `(${round3(at.x)},${round3(at.y)}) on ${t.layer}, outside any tie region`,
        );
      }
      for (let vi = viasBase; vi < pcb.vias.length; vi++) {
        const v = pcb.vias[vi]!;
        if (v.net === t.net) continue;
        const lim = v.diameter / 2 + t.width / 2 + clearance - 1e-6;
        if (bboxFar(t.start, t.end, v.position, v.position, lim)) continue;
        const d = pointSegDist(v.position, t.start, t.end);
        if (d >= lim) continue;
        if (tieExempt(t.net, v.net, v.position)) continue;
        auditIssues.push(
          `audit: net '${t.net}' trace ${round3(d)}mm from net '${v.net}' via at ` +
            `(${v.position.x},${v.position.y}), outside any tie region`,
        );
      }
    }
  }
  errors.push(...auditIssues);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: errors.length === 0,
          coils_placed: coilsPlaced,
          coils_failed: plan.coils.length - coilsPlaced,
          interconnect_traces: interconnectTraces,
          vias_added: vias,
          net_ties_added: netTiesAdded,
          connection,
          winding_factor: plan.windingFactor,
          ...(starJunction ? { star_junction: starJunction } : {}),
          phase_feeds: phaseFeeds,
          ...(feedsRouted.length ? { feeds_routed: feedsRouted } : {}),
          ...(feedsUnrouted.length ? { feeds_unrouted: feedsUnrouted } : {}),
          interconnect_note:
            "Interconnect is planar by construction: per-phase staggered-radius arcs " +
            "on the spiral layer in the coil-free bore, radial drops on the return " +
            "layer, joined only through vias, with approach angles jogged so no trace " +
            "rides a terminal ray past a same-net via. A post-build audit re-checked " +
            "these invariants" +
            (auditIssues.length ? " and FOUND VIOLATIONS (see errors)." : "."),
          ...(errors.length ? { errors } : {}),
          drc_delta: await drcCap.finish(),
          ...docResultPayload(ctx),
        }),
      },
    ],
    ...(errors.length && coilsPlaced === 0 ? { isError: true as const } : {}),
  };
}

/** Coarse pad radius from its shape, for feed-escape collision checks. */
function padApproxRadius(pad: Pad): number {
  const s = pad.shape as { type: string; diameter?: number; width?: number; height?: number };
  if (s.type === "Circle") return (s.diameter ?? 1) / 2;
  return Math.max(s.width ?? 1, s.height ?? 1) / 2;
}

// ============================================================================
// calc_motor — first-order analytical motor performance (Kt/Ke/speed/torque)
// ============================================================================

export const calcMotorSchema = {
  type: "object" as const,
  properties: {
    mode: {
      type: "string" as const,
      description:
        "'pm' (default): permanent-magnet machine — Kt/Ke from air-gap flux; " +
        "requires inner/outer radius, phase_resistance_ohm, supply_voltage_v. " +
        "'induction': thin-sheet axial induction rotor (drag-cup / PCB cage) — " +
        "torque from eddy currents; requires phase_current_a, electrical_freq_hz, " +
        "effective_gap_mm, sheet_conductance_s, inner/outer radius.",
    },
    pole_pairs: { type: "number" as const, description: "Pole pairs p (electrical periods per mechanical rev)." },
    turns_per_phase: { type: "number" as const, description: "Series turns per phase N." },
    winding_factor: { type: "number" as const, description: "Winding factor kw (default 0.95). Use the value from winding_layout for accuracy." },
    inner_radius_mm: { type: "number" as const, description: "Inner (bore) stator radius, mm. In induction mode: inner radius of the active annulus the field sweeps." },
    outer_radius_mm: { type: "number" as const, description: "Outer stator radius, mm. In induction mode: outer radius of the active annulus." },
    phase_resistance_ohm: { type: "number" as const, description: "Per-phase resistance, ohms (e.g. the add_coil DC estimate). PM mode only (required)." },
    supply_voltage_v: { type: "number" as const, description: "DC bus / supply voltage, volts. PM mode only (required)." },
    airgap_flux_tesla: {
      type: "number" as const,
      description: "PM mode: air-gap flux density B_gap (T). Omit to COMPUTE it from `magnet` via the MEC model.",
    },
    magnet: {
      type: "object" as const,
      description:
        "PM mode: optional magnet/geometry to compute B_gap when airgap_flux_tesla is omitted (NdFeB defaults). " +
        "Fields: remanence_tesla, magnet_thickness_mm, airgap_mm, recoil_mu_rel, magnet_area_mm2, gap_area_mm2, iron_mu_rel. " +
        "Add pole_width_mm (magnet pole face width across the fringing direction) to also apply the first-order " +
        "Carter-like fringing derate w/(w+2g) — the tool then reports raw AND derated B and uses the derated value for Kt. " +
        "SATURATION: the MEC iron is LINEAR unless you say otherwise, so it over-predicts Kt on a machine whose teeth " +
        "saturate. Pass slots + (tooth_width_mm or tooth_fraction) to make the teeth visible — the tool then reports " +
        "tooth_flux_density (B_gap · pitch/width, routinely 2x B_gap) and warns when it passes the ~1.5 T silicon-steel " +
        "knee. Add iron_js_t (saturation polarization, silicon steel ≈ 2.0 T) with iron_mu_rel to actually SOLVE the " +
        "saturating network instead of merely warning; tooth_path_mm puts the teeth in the reluctance loop.",
    },
    phase_current_a: { type: "number" as const, description: "Induction mode (required): phase current, A RMS (balanced 3-phase drive)." },
    electrical_freq_hz: { type: "number" as const, description: "Induction mode (required): electrical drive frequency, Hz." },
    effective_gap_mm: {
      type: "number" as const,
      description:
        "Induction mode (required): TOTAL non-ferromagnetic flux path, mm — all air gaps plus the rotor sheet " +
        "and any PCB substrate between back-irons (e.g. 4.7 for a PCB-stator sandwich).",
    },
    sheet_conductance_s: {
      type: "number" as const,
      description:
        "Induction mode (required): rotor sheet surface conductance σs = σ·thickness, siemens. " +
        "E.g. 2×2oz copper: 5.8e7 S/m × 0.14e-3 m ≈ 8120 S.",
    },
    end_effect_factor: {
      type: "number" as const,
      description:
        "Induction mode: Russell–Norsworthy end-effect factor (0..1] — fraction of ideal torque surviving the " +
        "eddy return paths outside the active annulus. Default 0.65.",
    },
  },
  required: ["pole_pairs", "turns_per_phase"],
};

/**
 * Flux density above which soft iron is flagged as saturating, tesla — the low
 * end of the M19-class silicon-steel knee. Mirrors
 * `vcad_ecad_sim::airgap::SILICON_STEEL_KNEE_T`; used only for the TS-side
 * fallback warning when the WASM build predates `ecadAirgapSolve`.
 */
const SILICON_STEEL_KNEE_T = 1.5;

/** Round to 6 significant digits — micro-newton-metre torques survive, noise doesn't. */
const sig6 = (v: number) => (v === 0 || !Number.isFinite(v) ? v : Number(v.toPrecision(6)));

/**
 * Thin-sheet axial induction branch of calc_motor. A TypeScript mirror of the
 * `vcad_ecad_sim::induction` closed form (same pattern as calc_coil /
 * calc_impedance mirroring their Rust twins):
 *
 *   F1 = (3/2)·(4/π)·(kw·N/(2p))·I_pk      rotating MMF fundamental
 *   B1 = μ0·F1/g                           g = total non-ferromagnetic path
 *   T(s) = k_ee·π·σs·s·(ωe/p)·B1²·(r2⁴−r1⁴)/4   linear-in-slip eddy torque
 *
 * k_ee is the Russell–Norsworthy end-effect factor (eddy return paths close
 * outside the active annulus). First-order: no breakdown peak, no magnetizing/
 * leakage reactance, no stator copper loss (no resistance input).
 */
function calcMotorInduction(
  args: Record<string, unknown>,
  polePairs: number,
  turnsPerPhase: number,
  windingFactor: number,
  innerR: number,
  outerR: number,
) {
  const fail = ecadError;
  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : NaN);
  const iRms = num(args.phase_current_a);
  const freqHz = num(args.electrical_freq_hz);
  const gapMm = num(args.effective_gap_mm);
  const sigmaS = num(args.sheet_conductance_s);
  const endEffect = Number.isFinite(num(args.end_effect_factor))
    ? num(args.end_effect_factor)
    : 0.65;

  if (!(iRms > 0)) return fail("induction mode: phase_current_a (A rms) must be > 0");
  if (!(freqHz > 0)) return fail("induction mode: electrical_freq_hz must be > 0");
  if (!(gapMm > 0)) return fail("induction mode: effective_gap_mm must be > 0");
  if (!(sigmaS > 0)) return fail("induction mode: sheet_conductance_s must be > 0");
  if (!(endEffect > 0 && endEffect <= 1)) {
    return fail("induction mode: end_effect_factor must be in (0, 1]");
  }

  // Rotating MMF fundamental of a balanced 3-phase winding.
  const iPk = iRms * Math.SQRT2;
  const f1 = 1.5 * (4 / Math.PI) * ((windingFactor * turnsPerPhase) / (2 * polePairs)) * iPk;
  const b1 = (MU0 * f1) / (gapMm * 1e-3);

  const omegaSync = (2 * Math.PI * freqHz) / polePairs; // mechanical rad/s
  const annulusM4 = (Math.pow(outerR * 1e-3, 4) - Math.pow(innerR * 1e-3, 4)) / 4;
  const torquePerSlipRaw = Math.PI * sigmaS * omegaSync * b1 * b1 * annulusM4;
  const torquePerSlip = endEffect * torquePerSlipRaw;
  const syncRpm = (60 * freqHz) / polePairs;
  // Locked rotor (s = 1): all air-gap power T·ωsync dissipates in the sheet.
  const copperLossW = torquePerSlip * omegaSync;

  const inductionInputs = {
    pole_pairs: polePairs,
    turns_per_phase: turnsPerPhase,
    winding_factor: windingFactor,
    phase_current_a: iRms,
    electrical_freq_hz: freqHz,
    effective_gap_mm: gapMm,
    sheet_conductance_s: sigmaS,
    inner_radius_mm: innerR,
    outer_radius_mm: outerR,
    end_effect_factor: endEffect,
  };
  const claim = (q: Parameters<typeof emClaim>[0], v: number, unit: string) =>
    emClaim(q, sig6(v), unit, "thin-sheet-induction", inductionInputs);
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          mode: "induction",
          b1_tesla: sig6(b1),
          torque_per_unit_slip_nm: sig6(torquePerSlip),
          locked_rotor_torque_nm: sig6(torquePerSlip),
          locked_rotor_torque_raw_nm: sig6(torquePerSlipRaw),
          sync_rpm: sig6(syncRpm),
          copper_loss_w: sig6(copperLossW),
          end_effect_factor: endEffect,
          note:
            "First-order thin-sheet induction model: torque linear in slip (T(s) = K·s, " +
            "no breakdown peak), B1 impressed by the winding MMF (no magnetizing/leakage " +
            "reactance, no slotting/saturation), Russell–Norsworthy end-effect factor " +
            "applied to torque. copper_loss_w is the ROTOR sheet dissipation at locked " +
            "rotor (T·ωsync); stator copper loss is not modeled (no resistance input). " +
            "Feed locked_rotor_torque_nm to check_self_start to answer 'will it spin?'.",
          claims: [
            claim("airgap_flux_density", b1, "T"),
            claim("torque_per_unit_slip", torquePerSlip, "N·m"),
            claim("locked_rotor_torque", torquePerSlip, "N·m"),
            claim("synchronous_speed", syncRpm, "rpm"),
            claim("rotor_copper_loss", copperLossW, "W"),
          ],
        }),
      },
    ],
  };
}

/**
 * Evaluate a motor's headline performance AS DATA — pure analysis, no board,
 * no mutation. Two machine models behind one tool:
 *
 * - `mode: "pm"` (default) — torque constant, back-EMF constant, no-load
 *   speed, stall torque, and a speed–torque curve from magnetics + electrical
 *   parameters. Air-gap flux is supplied directly or computed from magnet
 *   geometry via the first-order MEC reluctance model; pass
 *   `magnet.pole_width_mm` to additionally apply a Carter-like fringing
 *   derate (raw and derated B both reported).
 * - `mode: "induction"` — thin-sheet axial induction rotor (drag-cup / PCB
 *   cage): fundamental gap field B1, torque-per-unit-slip, locked-rotor
 *   torque, synchronous speed, rotor sheet loss.
 */
export async function calcMotor(args: Record<string, unknown>) {
  const fail = ecadError;

  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : NaN);
  const mode = typeof args.mode === "string" ? args.mode : "pm";
  if (mode !== "pm" && mode !== "induction") {
    return fail("mode must be 'pm' (default) or 'induction'");
  }
  const polePairs = num(args.pole_pairs);
  const turnsPerPhase = num(args.turns_per_phase);
  const windingFactor = Number.isFinite(num(args.winding_factor)) ? num(args.winding_factor) : 0.95;
  const innerR = num(args.inner_radius_mm);
  const outerR = num(args.outer_radius_mm);
  const phaseR = num(args.phase_resistance_ohm);
  const supplyV = num(args.supply_voltage_v);

  if (!(polePairs > 0)) return fail("pole_pairs must be > 0");
  if (!(turnsPerPhase > 0)) return fail("turns_per_phase must be > 0");
  if (!(outerR > innerR && innerR >= 0)) return fail("outer_radius_mm must be > inner_radius_mm >= 0");

  if (mode === "induction") {
    return calcMotorInduction(args, polePairs, turnsPerPhase, windingFactor, innerR, outerR);
  }

  if (!(phaseR > 0)) return fail("phase_resistance_ohm must be > 0");
  if (!(supplyV > 0)) return fail("supply_voltage_v must be > 0");

  // Resolve air-gap flux: explicit value, else compute from magnet geometry.
  let bGap = num(args.airgap_flux_tesla);
  let bGapSource: "supplied" | "computed" = "supplied";
  let magnetSpec: Parameters<typeof airgapFluxDensity>[0] | undefined;
  let solution: AirGapSolutionResult | null = null;
  // Optional first-order fringing derate on the MEC flux (see below).
  let fringing: { poleWidthMm: number; derate: number; bRawTesla: number } | undefined;
  if (!Number.isFinite(bGap)) {
    const m = (args.magnet ?? {}) as Record<string, unknown>;
    const mnum = (v: unknown, d: number) =>
      typeof v === "number" && Number.isFinite(v) ? (v as number) : d;
    magnetSpec = {
      remanenceTesla: mnum(m.remanence_tesla, 1.2),
      magnetThicknessMm: mnum(m.magnet_thickness_mm, 3),
      recoilMuRel: mnum(m.recoil_mu_rel, 1.05),
      airgapMm: mnum(m.airgap_mm, 1),
      magnetAreaMm2: mnum(m.magnet_area_mm2, 1),
      gapAreaMm2: mnum(m.gap_area_mm2, 1),
      ironMuRel: typeof m.iron_mu_rel === "number" ? (m.iron_mu_rel as number) : null,
      ironPathMm: mnum(m.iron_path_mm, 0),
      ironAreaMm2: mnum(m.iron_area_mm2, 1),
      ironJsT: typeof m.iron_js_t === "number" ? (m.iron_js_t as number) : null,
    };

    // Teeth. Without them the model has no tooth concept and structurally
    // cannot see the saturation that makes a linear-iron Kt optimistic.
    if (m.slots !== undefined) {
      const slots = num(m.slots);
      if (!(slots > 0)) return fail("magnet.slots must be > 0");
      const meanR = Number.isFinite(num(m.mean_radius_mm))
        ? num(m.mean_radius_mm)
        : (innerR + outerR) / 2;
      const pitch = (2 * Math.PI * meanR) / slots;
      let toothW = num(m.tooth_width_mm);
      if (!Number.isFinite(toothW)) {
        const frac = num(m.tooth_fraction);
        if (!Number.isFinite(frac)) {
          return fail("magnet.slots requires magnet.tooth_width_mm or magnet.tooth_fraction");
        }
        if (!(frac > 0 && frac <= 1)) return fail("magnet.tooth_fraction must be in (0, 1]");
        toothW = frac * pitch;
      }
      if (!(toothW > 0)) return fail("magnet.tooth_width_mm must be > 0");
      magnetSpec.teeth = {
        slots,
        toothWidthMm: toothW,
        meanRadiusMm: meanR,
        toothPathMm: mnum(m.tooth_path_mm, 0),
      };
    }

    // Prefer the full solve (tooth/yoke fields + saturation). Fall back to the
    // scalar binding on an older WASM build, mirroring the tooth geometry in TS
    // — it is pure geometry, so the warning survives even a stale kernel.
    solution = await airgapSolve(magnetSpec);
    if (solution == null) {
      const computed = await airgapFluxDensity(magnetSpec);
      if (computed == null) {
        return fail(
          "air-gap flux is required: pass airgap_flux_tesla, or `magnet` params (ECAD WASM must be available to compute B_gap).",
        );
      }
      const t = magnetSpec.teeth;
      const k = t ? Math.max(1, (2 * Math.PI * t.meanRadiusMm) / t.slots / t.toothWidthMm) : null;
      solution = {
        bGapTesla: computed,
        bToothTesla: k == null ? null : computed * k,
        bIronTesla: null,
        toothConcentration: k,
        nonlinear: false,
        iterations: 0,
        converged: true,
        warnings:
          k != null && computed * k > SILICON_STEEL_KNEE_T
            ? [
                `tooth flux density ${(computed * k).toFixed(2)} T exceeds the ` +
                  `${SILICON_STEEL_KNEE_T.toFixed(2)} T knee — the LINEAR iron model over-predicts ` +
                  `B_gap and Kt here; pass magnet.iron_js_t to solve the saturating network`,
              ]
            : [],
      };
    }
    bGap = solution.bGapTesla;
    bGapSource = "computed";

    // Carter-like pole-edge fringing derate (mirrors
    // vcad_ecad_sim::airgap::fringing_derate): flux fringes outward ~one gap
    // length per pole edge, so B under the pole drops by w/(w+2g). First-order,
    // honest only for pole width ≳ 2× gap. Opt-in via magnet.pole_width_mm.
    if (m.pole_width_mm !== undefined) {
      const poleW = num(m.pole_width_mm);
      if (!(poleW > 0)) return fail("magnet.pole_width_mm must be > 0");
      const derate = poleW / (poleW + 2 * magnetSpec.airgapMm);
      fringing = { poleWidthMm: poleW, derate, bRawTesla: bGap };
      bGap *= derate;
    }
  }

  const perf = await evaluateMotor({
    polePairs,
    turnsPerPhase,
    windingFactor,
    innerRMm: innerR,
    outerRMm: outerR,
    phaseResistanceOhm: phaseR,
    supplyVoltageV: supplyV,
    airgapFluxTesla: bGap,
  });
  if (perf == null) {
    return fail("motor evaluation unavailable — ECAD WASM is not loaded (rebuild vcad-kernel-wasm).");
  }

  const r4 = (n: number) => Math.round(n * 1e4) / 1e4;
  const motorInputs = {
    pole_pairs: polePairs,
    turns_per_phase: turnsPerPhase,
    winding_factor: windingFactor,
    inner_radius_mm: innerR,
    outer_radius_mm: outerR,
    phase_resistance_ohm: phaseR,
    supply_voltage_v: supplyV,
    airgap_flux_tesla: r4(bGap),
  };
  const claims = [
    emClaim("torque_constant", r4(perf.ktNmPerA), "N·m/A", "first-order-dc-motor", motorInputs),
    emClaim("back_emf_constant", r4(perf.keVSPerRad), "V·s/rad", "first-order-dc-motor", motorInputs),
    emClaim("no_load_speed", r4(perf.noLoadSpeedRadS), "rad/s", "first-order-dc-motor", motorInputs),
    emClaim("stall_torque", r4(perf.stallTorqueNm), "N·m", "first-order-dc-motor", motorInputs),
  ];
  if (bGapSource === "computed" && magnetSpec) {
    const mecInputs = {
      remanence_tesla: magnetSpec.remanenceTesla,
      magnet_thickness_mm: magnetSpec.magnetThicknessMm,
      recoil_mu_rel: magnetSpec.recoilMuRel,
      airgap_mm: magnetSpec.airgapMm,
      magnet_area_mm2: magnetSpec.magnetAreaMm2,
      gap_area_mm2: magnetSpec.gapAreaMm2,
      ...(magnetSpec.ironMuRel != null
        ? {
            iron_mu_rel: magnetSpec.ironMuRel,
            iron_path_mm: magnetSpec.ironPathMm,
            iron_area_mm2: magnetSpec.ironAreaMm2,
          }
        : {}),
    };
    // The raw MEC prediction stays a claim of its own even when derated —
    // a fringing-aware FEA pass grades the derate, not the network.
    claims.push(
      emClaim(
        "airgap_flux_density",
        r4(fringing ? fringing.bRawTesla : bGap),
        "T",
        "mec-reluctance",
        mecInputs,
      ),
    );
    if (fringing) {
      claims.push(
        emClaim("airgap_flux_density", r4(bGap), "T", "mec-fringing-derate", {
          ...mecInputs,
          pole_width_mm: fringing.poleWidthMm,
          fringing_derate: r4(fringing.derate),
        }),
      );
    }
    // Tooth field is the honest saturation indicator, and it is a claim in its
    // own right — an FEA/simulate_em pass grades it directly.
    if (solution?.bToothTesla != null && magnetSpec.teeth) {
      claims.push(
        emClaim(
          "tooth_flux_density",
          r4(solution.bToothTesla),
          "T",
          solution.nonlinear ? "mec-saturating-iron" : "mec-tooth-concentration",
          {
            ...mecInputs,
            slots: magnetSpec.teeth.slots,
            tooth_width_mm: r4(magnetSpec.teeth.toothWidthMm),
            mean_radius_mm: r4(magnetSpec.teeth.meanRadiusMm),
            tooth_concentration: r4(solution.toothConcentration ?? 1),
            ...(magnetSpec.ironJsT != null ? { iron_js_t: magnetSpec.ironJsT } : {}),
          },
        ),
      );
    }
  }
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          mode: "pm",
          airgap_flux_tesla: r4(bGap),
          airgap_flux_source: bGapSource,
          ...(fringing
            ? {
                airgap_flux_raw_tesla: r4(fringing.bRawTesla),
                fringing_derate: r4(fringing.derate),
                fringing_note:
                  "Carter-like first-order derate: B under the pole drops by " +
                  "w/(w+2g) as flux fringes ~one gap length past each pole edge. " +
                  "Kt/Ke and the curve use the DERATED flux. Honest for pole " +
                  "width ≳ 2× gap; below that treat it as a lower bound.",
              }
            : {}),
          ...(solution?.bToothTesla != null
            ? {
                tooth_flux_tesla: r4(solution.bToothTesla),
                tooth_concentration: r4(solution.toothConcentration ?? 1),
                // Fringing redistributes flux under the pole; it does not remove
                // it, so the tooth carries the RAW gap flux either way.
                tooth_note:
                  "B_tooth = raw B_gap · (tooth pitch / tooth width) — the gap flux of a whole " +
                  "tooth pitch funnels into one tooth body. Compare against your steel's knee " +
                  "(~1.5-1.7 T for M19-class silicon steel).",
              }
            : {}),
          ...(solution?.bIronTesla != null ? { yoke_flux_tesla: r4(solution.bIronTesla) } : {}),
          ...(solution?.nonlinear ? { iron_model: "saturating (arctangent B-H)" } : {}),
          ...(solution && solution.warnings.length > 0 ? { warnings: solution.warnings } : {}),
          winding_factor: windingFactor,
          kt_nm_per_a: r4(perf.ktNmPerA),
          ke_v_s_per_rad: r4(perf.keVSPerRad),
          no_load_speed_rad_s: r4(perf.noLoadSpeedRadS),
          no_load_rpm: r4((perf.noLoadSpeedRadS * 60) / (2 * Math.PI)),
          stall_torque_nm: r4(perf.stallTorqueNm),
          speed_torque_curve: perf.curve.map((p) => ({
            speed_rad_s: r4(p.speedRadS),
            torque_nm: r4(p.torqueNm),
          })),
          note:
            "First-order steady-state estimate (no slotting, no losses)" +
            (fringing
              ? "; pole-edge fringing derated via magnet.pole_width_mm"
              : "; pass magnet.pole_width_mm to derate for fringing") +
            (solution?.nonlinear
              ? "; iron solved with its saturating B-H law"
              : "; iron is LINEAR, so Kt is OPTIMISTIC on a machine whose teeth saturate — " +
                (solution?.bToothTesla != null
                  ? "see tooth_flux_tesla, and pass magnet.iron_js_t to solve the saturating network"
                  : "pass magnet.slots + tooth_width_mm to see the tooth field")) +
            ".",
          claims,
        }),
      },
    ],
  };
}

// ============================================================================
// check_self_start — will it spin? starting torque vs bearing friction
// ============================================================================

/**
 * Documented typical running-friction torque per bearing, mN·m, by preset and
 * preload class. Low-speed drag in small deep-groove bearings is dominated by
 * seal contact and grease churning, so seal type matters more than load:
 *
 * - `608-2RS` (8×22×7, contact rubber seals): the lip drag dominates —
 *   0.5–2 mN·m each at light preload (a pair lands at the oft-quoted
 *   1–4 mN·m), roughly double under medium preload.
 * - `608-ZZ` (8×22×7, non-contact metal shields): no lip contact, an order
 *   of magnitude freer.
 * - `625` (5×16×5) and `688` (8×16×5 thin-section): miniature bearings,
 *   figures for the common shielded (ZZ-type, non-contact) variants.
 *
 * Ranges are catalog-typical for greased bearings at room temperature; a
 * cold, over-greased, or axially pinched bearing can exceed the top end.
 */
const BEARING_FRICTION_MNM: Record<
  string,
  Record<"light" | "medium", { min: number; max: number }>
> = {
  "608-2RS": {
    light: { min: 0.5, max: 2.0 },
    medium: { min: 1.0, max: 4.0 },
  },
  "608-ZZ": {
    light: { min: 0.05, max: 0.3 },
    medium: { min: 0.15, max: 0.8 },
  },
  "625": {
    light: { min: 0.03, max: 0.2 },
    medium: { min: 0.1, max: 0.5 },
  },
  "688": {
    light: { min: 0.03, max: 0.25 },
    medium: { min: 0.1, max: 0.6 },
  },
};

export const checkSelfStartSchema = {
  type: "object" as const,
  properties: {
    available_torque_nm: {
      type: "number" as const,
      description:
        "Torque available at standstill, N·m — PM: Kt·I (or pass kt_nm_per_a + current_a " +
        "instead); induction: locked_rotor_torque_nm from calc_motor mode:'induction'.",
    },
    kt_nm_per_a: {
      type: "number" as const,
      description: "Alternative to available_torque_nm: torque constant Kt (N·m/A), multiplied by current_a.",
    },
    current_a: {
      type: "number" as const,
      description: "Alternative to available_torque_nm: standstill phase current (A), multiplied by kt_nm_per_a.",
    },
    friction_torque_nm: {
      type: "number" as const,
      description:
        "Direct friction estimate, N·m (single value or a measurement). Overrides `bearings` when given.",
    },
    bearings: {
      type: "object" as const,
      description:
        "Bearing-friction estimator (used when friction_torque_nm is omitted). Fields: " +
        "type ('608-2RS' | '608-ZZ' | '625' | '688'; 625/688 assume shielded non-contact variants), " +
        "preload ('light' default | 'medium'), count (number of bearings, default 2).",
    },
  },
};

/**
 * The single most important motor design question — will it spin? — as a pure
 * fail-closed check: starting torque (given directly, or Kt·I) against a
 * friction estimate (given directly, or from documented typical bearing-drag
 * ranges). `starts` is judged against the WORST-CASE (max) friction; a design
 * that only beats the optimistic end is reported `starts: false` with
 * `starts_best_case: true` so the margin story is visible.
 */
export function checkSelfStart(args: Record<string, unknown>) {
  const fail = ecadError;
  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : NaN);

  // Available starting torque: direct, or Kt·I.
  let available = num(args.available_torque_nm);
  let availableSource: "direct" | "kt_times_current" = "direct";
  if (!Number.isFinite(available)) {
    const kt = num(args.kt_nm_per_a);
    const i = num(args.current_a);
    if (!(kt > 0) || !(i > 0)) {
      return fail(
        "pass available_torque_nm (> 0), or both kt_nm_per_a and current_a (> 0) to compute Kt·I",
      );
    }
    available = kt * i;
    availableSource = "kt_times_current";
  }
  if (!(available > 0)) return fail("available_torque_nm must be > 0");

  // Friction estimate: direct value, or the bearing catalog.
  let frictionMinNm: number;
  let frictionMaxNm: number;
  let frictionSource: "direct" | "bearing-catalog" = "direct";
  let bearing:
    | { type: string; preload: "light" | "medium"; count: number; per_bearing_mnm: { min: number; max: number } }
    | undefined;
  const directFriction = num(args.friction_torque_nm);
  if (Number.isFinite(directFriction)) {
    if (!(directFriction > 0)) return fail("friction_torque_nm must be > 0");
    frictionMinNm = directFriction;
    frictionMaxNm = directFriction;
  } else {
    const b = (args.bearings ?? {}) as Record<string, unknown>;
    const type = typeof b.type === "string" ? b.type : "608-2RS";
    const table = BEARING_FRICTION_MNM[type];
    if (!table) {
      return fail(
        `unknown bearing type '${type}' — presets: ${Object.keys(BEARING_FRICTION_MNM).join(", ")} ` +
          "(or pass friction_torque_nm directly)",
      );
    }
    const preload = b.preload === "medium" ? "medium" : b.preload === "light" || b.preload === undefined ? "light" : null;
    if (preload === null) return fail("bearings.preload must be 'light' or 'medium'");
    const count = Number.isFinite(num(b.count)) ? num(b.count) : 2;
    if (!(count >= 1 && Number.isInteger(count))) return fail("bearings.count must be an integer >= 1");
    const range = table[preload];
    frictionMinNm = range.min * count * 1e-3;
    frictionMaxNm = range.max * count * 1e-3;
    frictionSource = "bearing-catalog";
    bearing = { type, preload, count, per_bearing_mnm: range };
  }

  const starts = available > frictionMaxNm;
  const startsBestCase = available > frictionMinNm;
  const margin = available / frictionMaxNm;

  const claimInputs: Record<string, number | string> = {
    available_torque_nm: sig6(available),
    available_torque_source: availableSource,
    friction_source: frictionSource,
    ...(bearing
      ? {
          bearing_type: bearing.type,
          bearing_preload: bearing.preload,
          bearing_count: bearing.count,
        }
      : { friction_torque_nm: sig6(frictionMaxNm) }),
  };
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          starts,
          starts_best_case: startsBestCase,
          margin: sig6(margin),
          available_torque_nm: sig6(available),
          available_torque_source: availableSource,
          friction_torque_mnm: { min: sig6(frictionMinNm * 1e3), max: sig6(frictionMaxNm * 1e3) },
          friction_source: frictionSource,
          ...(bearing ? { bearings: bearing } : {}),
          note:
            "`starts` is fail-closed: available torque vs the WORST-CASE (max) friction " +
            "estimate; `margin` = available / worst-case (aim for ≥ 2 to absorb cogging, " +
            "cold grease, and preload spread). Bearing ranges are documented catalog-" +
            "typical running drag for greased bearings, not measurements of yours.",
          claims: [
            // The friction estimate is only a claim when this tool predicted it
            // (catalog lookup); a passthrough of the caller's number is not.
            ...(frictionSource === "bearing-catalog"
              ? [
                  emClaim(
                    "friction_torque",
                    sig6(frictionMaxNm),
                    "N·m",
                    "bearing-friction-catalog",
                    claimInputs,
                  ),
                ]
              : []),
            emClaim("start_margin", sig6(margin), "dimensionless", "torque-friction-margin", claimInputs),
          ],
        }),
      },
    ],
  };
}

// ============================================================================
// board_from_solid — derive a board outline from solid-model geometry
// ============================================================================

/** Point-in-triangle test (2D, inclusive of edges). */
function pointInTriangle2D(
  px: number,
  py: number,
  ax: number,
  ay: number,
  bx: number,
  by: number,
  cx: number,
  cy: number,
): boolean {
  const d1 = (px - bx) * (ay - by) - (ax - bx) * (py - by);
  const d2 = (px - cx) * (by - cy) - (bx - cx) * (py - cy);
  const d3 = (px - ax) * (cy - ay) - (cx - ax) * (py - ay);
  const hasNeg = d1 < 0 || d2 < 0 || d3 < 0;
  const hasPos = d1 > 0 || d2 > 0 || d3 > 0;
  return !(hasNeg && hasPos);
}

/** Perpendicular distance from p to segment ab. */
function pointSegDist(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy;
  if (lenSq === 0) return Math.hypot(p.x - a.x, p.y - a.y);
  let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/** Ramer–Douglas–Peucker simplification of an open polyline. */
function rdpSimplify(points: Vec2[], eps: number): Vec2[] {
  if (points.length < 3) return points;
  const a = points[0];
  const b = points[points.length - 1];
  let maxD = 0;
  let idx = 0;
  for (let i = 1; i < points.length - 1; i++) {
    const d = pointSegDist(points[i], a, b);
    if (d > maxD) {
      maxD = d;
      idx = i;
    }
  }
  if (maxD <= eps) return [a, b];
  const left = rdpSimplify(points.slice(0, idx + 1), eps);
  const right = rdpSimplify(points.slice(idx), eps);
  return [...left.slice(0, -1), ...right];
}

/** Simplify a closed loop: split at the vertex farthest from v0, RDP both halves. */
function simplifyLoop(loop: Vec2[], eps: number): Vec2[] {
  if (loop.length < 8) return loop;
  let far = 1;
  let maxD = 0;
  for (let i = 1; i < loop.length; i++) {
    const d = Math.hypot(loop[i].x - loop[0].x, loop[i].y - loop[0].y);
    if (d > maxD) {
      maxD = d;
      far = i;
    }
  }
  const chain1 = rdpSimplify(loop.slice(0, far + 1), eps);
  const chain2 = rdpSimplify([...loop.slice(far), loop[0]], eps);
  // Drop the duplicated junction vertices when re-joining.
  return [...chain1.slice(0, -1), ...chain2.slice(0, -1)];
}

/** Shoelace signed area of a closed loop (CCW positive). */
function loopSignedArea(loop: Vec2[]): number {
  let area = 0;
  for (let i = 0; i < loop.length; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % loop.length];
    area += a.x * b.y - b.x * a.y;
  }
  return area / 2;
}

/** Even-odd point-in-polygon test. */
function pointInPolygon(p: Vec2, poly: Vec2[]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const a = poly[i];
    const b = poly[j];
    if (
      a.y > p.y !== b.y > p.y &&
      p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x
    ) {
      inside = !inside;
    }
  }
  return inside;
}

/** Proper interior crossing point of segments p1p2 and p3p4, or null. Parallel
 *  / collinear segments and endpoint-only touches return null — two pours that
 *  merely share a border are not an overlap. */
function segmentCrossing(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2): Vec2 | null {
  const d1x = p2.x - p1.x;
  const d1y = p2.y - p1.y;
  const d2x = p4.x - p3.x;
  const d2y = p4.y - p3.y;
  const denom = d1x * d2y - d1y * d2x;
  if (Math.abs(denom) < 1e-12) return null;
  const t = ((p3.x - p1.x) * d2y - (p3.y - p1.y) * d2x) / denom;
  const u = ((p3.x - p1.x) * d1y - (p3.y - p1.y) * d1x) / denom;
  const eps = 1e-9;
  if (t <= eps || t >= 1 - eps || u <= eps || u >= 1 - eps) return null;
  return { x: p1.x + t * d1x, y: p1.y + t * d1y };
}

/** Area-weighted centroid of a simple polygon — guaranteed interior for convex
 *  outlines (boards, plane islands); falls back to the vertex mean if the
 *  signed area is degenerate. */
function polygonCentroid(poly: Vec2[]): Vec2 {
  let a = 0;
  let cx = 0;
  let cy = 0;
  for (let i = 0; i < poly.length; i++) {
    const p = poly[i]!;
    const q = poly[(i + 1) % poly.length]!;
    const cross = p.x * q.y - q.x * p.y;
    a += cross;
    cx += (p.x + q.x) * cross;
    cy += (p.y + q.y) * cross;
  }
  if (Math.abs(a) < 1e-12) {
    const n = poly.length || 1;
    return {
      x: poly.reduce((s, p) => s + p.x, 0) / n,
      y: poly.reduce((s, p) => s + p.y, 0) / n,
    };
  }
  a *= 0.5;
  return { x: cx / (6 * a), y: cy / (6 * a) };
}

/** Intersection of the two polygons' axis-aligned bounding boxes, or null when
 *  they don't overlap. Used as the localized fallback bbox when two outlines
 *  coincide exactly (e.g. two `fill_board` planes) and produce no vertex or
 *  edge witnesses. */
function aabbIntersection(a: Vec2[], b: Vec2[]): { min: Vec2; max: Vec2 } | null {
  const box = (poly: Vec2[]) => ({
    minX: Math.min(...poly.map((p) => p.x)),
    minY: Math.min(...poly.map((p) => p.y)),
    maxX: Math.max(...poly.map((p) => p.x)),
    maxY: Math.max(...poly.map((p) => p.y)),
  });
  const A = box(a);
  const B = box(b);
  const minX = Math.max(A.minX, B.minX);
  const minY = Math.max(A.minY, B.minY);
  const maxX = Math.min(A.maxX, B.maxX);
  const maxY = Math.min(A.maxY, B.maxY);
  if (minX > maxX || minY > maxY) return null;
  return { min: { x: round3(minX), y: round3(minY) }, max: { x: round3(maxX), y: round3(maxY) } };
}

/** Bbox of the region where two simple polygons' interiors overlap, or null if
 *  they don't. The witnesses (vertices of one polygon inside the other, plus
 *  proper edge crossings) are exactly the vertices of the intersection region,
 *  so their bbox bounds it tightly. When two outlines coincide exactly there are
 *  no such witnesses, so a guaranteed-interior point + AABB overlap catches that
 *  case and the AABB intersection localizes it. */
function polygonOverlapBbox(a: Vec2[], b: Vec2[]): { min: Vec2; max: Vec2 } | null {
  const witnesses: Vec2[] = [];
  for (const p of a) if (pointInPolygon(p, b)) witnesses.push(p);
  for (const p of b) if (pointInPolygon(p, a)) witnesses.push(p);
  for (let i = 0; i < a.length; i++) {
    const a1 = a[i]!;
    const a2 = a[(i + 1) % a.length]!;
    for (let j = 0; j < b.length; j++) {
      const x = segmentCrossing(a1, a2, b[j]!, b[(j + 1) % b.length]!);
      if (x) witnesses.push(x);
    }
  }
  if (witnesses.length > 0) {
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const w of witnesses) {
      if (w.x < minX) minX = w.x;
      if (w.y < minY) minY = w.y;
      if (w.x > maxX) maxX = w.x;
      if (w.y > maxY) maxY = w.y;
    }
    return { min: { x: round3(minX), y: round3(minY) }, max: { x: round3(maxX), y: round3(maxY) } };
  }
  // No vertex/edge witnesses: coincident or one-contains-the-other-by-boundary.
  const inter = aabbIntersection(a, b);
  if (inter && (pointInPolygon(polygonCentroid(a), b) || pointInPolygon(polygonCentroid(b), a))) {
    return inter;
  }
  return null;
}

/**
 * Trace the boundary loops of a binary occupancy grid. Emits directed
 * segments along cell edges with the filled region on the left, then stitches
 * them into closed loops (outer boundaries CCW, holes CW). At saddle corners
 * the leftmost turn is taken; loops that still touch themselves there are
 * pinched apart afterwards by splitAtRepeatedVertices.
 */
function traceGridBoundaries(
  grid: Uint8Array,
  gw: number,
  gh: number,
): Array<Array<{ x: number; y: number }>> {
  const filled = (i: number, j: number) =>
    i >= 0 && j >= 0 && i < gw && j < gh && grid[j * gw + i] === 1;

  // Directed boundary segments keyed by start corner.
  const segs = new Map<string, Array<{ tx: number; ty: number; used: boolean }>>();
  const key = (x: number, y: number) => `${x},${y}`;
  const addSeg = (sx: number, sy: number, tx: number, ty: number) => {
    const k = key(sx, sy);
    const arr = segs.get(k) ?? [];
    arr.push({ tx, ty, used: false });
    segs.set(k, arr);
  };

  for (let j = 0; j < gh; j++) {
    for (let i = 0; i < gw; i++) {
      if (grid[j * gw + i] !== 1) continue;
      if (!filled(i, j - 1)) addSeg(i, j, i + 1, j); // bottom, heading +X
      if (!filled(i + 1, j)) addSeg(i + 1, j, i + 1, j + 1); // right, heading +Y
      if (!filled(i, j + 1)) addSeg(i + 1, j + 1, i, j + 1); // top, heading -X
      if (!filled(i - 1, j)) addSeg(i, j + 1, i, j); // left, heading -Y
    }
  }

  const loops: Array<Array<{ x: number; y: number }>> = [];
  for (const [startKey, startArr] of segs) {
    for (const startSeg of startArr) {
      if (startSeg.used) continue;
      const [sx, sy] = startKey.split(",").map(Number);
      const loop: Array<{ x: number; y: number }> = [{ x: sx, y: sy }];
      let cx = sx;
      let cy = sy;
      let seg = startSeg;
      for (;;) {
        seg.used = true;
        const dirX = seg.tx - cx;
        const dirY = seg.ty - cy;
        cx = seg.tx;
        cy = seg.ty;
        if (cx === sx && cy === sy) break;
        loop.push({ x: cx, y: cy });
        const candidates = (segs.get(key(cx, cy)) ?? []).filter((s) => !s.used);
        if (candidates.length === 0) break; // degenerate — shouldn't happen
        // Leftmost turn first: cross(dir, cand) descending, then dot descending.
        candidates.sort((a, b) => {
          const crossA = dirX * (a.ty - cy) - dirY * (a.tx - cx);
          const crossB = dirX * (b.ty - cy) - dirY * (b.tx - cx);
          if (crossA !== crossB) return crossB - crossA;
          const dotA = dirX * (a.tx - cx) + dirY * (a.ty - cy);
          const dotB = dirX * (b.tx - cx) + dirY * (b.ty - cy);
          return dotB - dotA;
        });
        seg = candidates[0];
      }
      if (loop.length >= 4) loops.push(loop);
    }
  }
  return loops;
}

/**
 * Split a traced loop at repeated vertices into simple loops. The boundary
 * walker can route two holes (or two lobes) that touch diagonally through
 * the same corner, producing one self-touching polygon — pinch it apart.
 */
function splitAtRepeatedVertices(
  loop: Array<{ x: number; y: number }>,
): Array<Array<{ x: number; y: number }>> {
  const out: Array<Array<{ x: number; y: number }>> = [];
  const stack: Array<Array<{ x: number; y: number }>> = [loop];
  while (stack.length > 0) {
    const cur = stack.pop()!;
    const seen = new Map<string, number>();
    let split = false;
    for (let i = 0; i < cur.length; i++) {
      const k = `${cur[i].x},${cur[i].y}`;
      const prev = seen.get(k);
      if (prev !== undefined) {
        const inner = cur.slice(prev, i);
        const rest = [...cur.slice(0, prev), ...cur.slice(i)];
        if (inner.length >= 4) stack.push(inner);
        if (rest.length >= 4) stack.push(rest);
        split = true;
        break;
      }
      seen.set(k, i);
    }
    if (!split && cur.length >= 4) out.push(cur);
  }
  return out;
}

/**
 * Derive a PCB board outline from a solid part's geometry: evaluate the CAD
 * session, project the part's mesh onto the XY plane, rasterize it to an
 * occupancy grid, and trace the boundary — outer outline plus interior
 * cutouts (e.g. a stator's center bore). The result plugs straight into
 * place_components' `outline` parameter.
 */
export function boardFromSolid(args: Record<string, unknown>, engine: Engine) {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);
  const thickness = (args.thickness as number) || 1.6;

  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);

  const candidates: Array<{ rootId: number; name?: string; mesh: TriangleMesh }> = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = doc.nodes[String(rootId)];
    const opType = (node?.op as { type?: string } | undefined)?.type;
    if (opType === "PcbBoard") continue;
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    candidates.push({ rootId, name: node?.name ?? undefined, mesh });
  }

  if (candidates.length === 0) {
    throw new Error("Document has no solid parts to trace (PcbBoard parts are excluded)");
  }

  const partId = args.part_id ? String(args.part_id) : undefined;
  let part: { rootId: number; name?: string; mesh: TriangleMesh };
  if (partId) {
    const found = candidates.find((c) => String(c.rootId) === partId);
    if (!found) {
      throw new Error(
        `No part with id "${partId}". Available: ${candidates
          .map((c) => `${c.rootId}${c.name ? ` (${c.name})` : ""}`)
          .join(", ")}`,
      );
    }
    part = found;
  } else if (candidates.length === 1) {
    part = candidates[0];
  } else {
    throw new Error(
      `Document has ${candidates.length} parts — pass part_id. Available: ${candidates
        .map((c) => `${c.rootId}${c.name ? ` (${c.name})` : ""}`)
        .join(", ")}`,
    );
  }

  // Project to XY (Z-up: the board plane) and rasterize.
  const pos = part.mesh.positions;
  const idx = part.mesh.indices;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (let i = 0; i < pos.length; i += 3) {
    if (pos[i] < minX) minX = pos[i];
    if (pos[i] > maxX) maxX = pos[i];
    if (pos[i + 1] < minY) minY = pos[i + 1];
    if (pos[i + 1] > maxY) maxY = pos[i + 1];
  }
  const extent = Math.max(maxX - minX, maxY - minY);
  if (!(extent > 0)) throw new Error("Part has no XY extent to trace");

  // Floor the cell size so the grid can never exceed ~4M cells, whatever
  // `resolution` the caller asks for on however large a part.
  const cell = Math.min(
    Math.max((args.resolution as number) || extent / 400, 0.02, extent / 2000),
    extent / 8,
  );
  // One padding cell on every side so the outer boundary always closes.
  const ox = minX - cell;
  const oy = minY - cell;
  const gw = Math.ceil((maxX - minX) / cell) + 2;
  const gh = Math.ceil((maxY - minY) / cell) + 2;
  const grid = new Uint8Array(gw * gh);

  const triCount = idx.length / 3;
  for (let t = 0; t < triCount; t++) {
    const i0 = idx[t * 3] * 3;
    const i1 = idx[t * 3 + 1] * 3;
    const i2 = idx[t * 3 + 2] * 3;
    const ax = pos[i0];
    const ay = pos[i0 + 1];
    const bx = pos[i1];
    const by = pos[i1 + 1];
    const cx = pos[i2];
    const cy = pos[i2 + 1];
    const tMinI = Math.max(0, Math.floor((Math.min(ax, bx, cx) - ox) / cell));
    const tMaxI = Math.min(gw - 1, Math.ceil((Math.max(ax, bx, cx) - ox) / cell));
    const tMinJ = Math.max(0, Math.floor((Math.min(ay, by, cy) - oy) / cell));
    const tMaxJ = Math.min(gh - 1, Math.ceil((Math.max(ay, by, cy) - oy) / cell));
    for (let j = tMinJ; j <= tMaxJ; j++) {
      for (let i = tMinI; i <= tMaxI; i++) {
        if (grid[j * gw + i] === 1) continue;
        const px = ox + (i + 0.5) * cell;
        const py = oy + (j + 0.5) * cell;
        if (pointInTriangle2D(px, py, ax, ay, bx, by, cx, cy)) {
          grid[j * gw + i] = 1;
        }
      }
    }
  }

  const rawLoops = traceGridBoundaries(grid, gw, gh).flatMap(splitAtRepeatedVertices);
  if (rawLoops.length === 0) {
    throw new Error("Projection produced no boundary — try a smaller `resolution`");
  }

  const eps = (args.simplify_tolerance as number) || cell * 1.5;
  const loops = rawLoops
    .map((loop) => {
      const mm = loop.map((p) => ({
        x: round3(ox + p.x * cell),
        y: round3(oy + p.y * cell),
      }));
      // A thin loop can collapse under simplification — keep the raw
      // polygon rather than emit a degenerate 2-point "outline".
      const simplified = simplifyLoop(mm, eps);
      return { points: simplified.length >= 3 ? simplified : mm, area: loopSignedArea(mm) };
    })
    .filter((l) => l.points.length >= 3 && Math.abs(l.area) > 1e-9);

  // Largest positive-area loop = the board outline; negative-area loops
  // inside it are cutouts; any other positive loops are disjoint islands.
  const outers = loops.filter((l) => l.area > 0).sort((a, b) => b.area - a.area);
  const outer = outers[0];
  if (!outer) throw new Error("No outer boundary found in projection");
  const warnings: string[] = [];
  if (outers.length > 1) {
    warnings.push(
      `Part projects to ${outers.length} disjoint regions — using the largest (${Math.round(outers[0].area)}mm²); a PCB outline must be one piece`,
    );
  }
  // Holes come out of the boundary trace wound CW (negative area); the
  // kernel extruder expects CCW polygons, so reverse them.
  const cutouts = loops
    .filter((l) => l.area < 0 && pointInPolygon(l.points[0], outer.points))
    .map((l) => [...l.points].reverse());

  const outline: BoardOutline = {
    vertices: outer.points,
    ...(cutouts.length > 0 ? { cutouts } : {}),
    thickness,
  };

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          part: { root_id: part.rootId, ...(part.name ? { name: part.name } : {}) },
          outline,
          outline_vertices: outer.points.length,
          cutouts: cutouts.length,
          area_mm2: Math.round(outer.area * 10) / 10,
          bbox: { min: { x: round3(minX), y: round3(minY) }, max: { x: round3(maxX), y: round3(maxY) } },
          cell_size_mm: round3(cell),
          ...(warnings.length > 0 ? { warnings } : {}),
          hint: "Pass `outline` to place_components to lay out a board with this shape",
        }),
      },
    ],
  };
}

// ============================================================================
// solid_from_board — materialize the session PCB as a solid CAD part
// (the inverse of board_from_solid)
// ============================================================================

/** JSON Schema for solid_from_board tool. */
export const solidFromBoardSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "PCB session id (from create_schematic / open_document) holding the " +
        "board to materialize as a solid.",
    },
    document_id_target: {
      type: "string" as const,
      description:
        "Existing CAD session to inject the part into (e.g. the enclosure " +
        "or motor-stack assembly it must fit). Omit to mint a fresh CAD " +
        "session; the response then carries the new document_id plus the " +
        "document IR (usable with open_document elsewhere).",
    },
    include_components: {
      type: "boolean" as const,
      description:
        "Also emit simplified component keep-out volumes — one box per " +
        "placed footprint, body extents from the kernel's 3D component " +
        "bodies where available, else courtyard × a per-package-class " +
        "default height. Default true.",
    },
    part_name: {
      type: "string" as const,
      description:
        "Name for the substrate part (default 'board'). Component keep-out " +
        "parts are named '<part_name>:<ref>'.",
    },
  },
  required: ["document_id"],
} as const;

/** FR4 substrate density, kg/m³ — matches the app's `__pcb_fr4__` preset. */
const FR4_DENSITY_KG_M3 = 1850;
/** Copper density, kg/m³ — matches the app's `copper` preset. */
const COPPER_DENSITY_KG_M3 = 8960;
/** 1 oz/ft² copper in mm, the fallback when a stackup layer omits thickness. */
const DEFAULT_COPPER_THICKNESS_MM = 0.035;

/** Absolute (sign-agnostic) shoelace area of a closed loop, mm². */
const loopArea = (loop: Vec2[]): number => Math.abs(loopSignedArea(loop));

/** Closed polygon → Line segments for a Sketch2D loop (zero-length runs from
 *  duplicated vertices are dropped). */
function loopToSegments(loop: Vec2[]): SketchSegment2D[] {
  const segs: SketchSegment2D[] = [];
  for (let i = 0; i < loop.length; i++) {
    const a = loop[i]!;
    const b = loop[(i + 1) % loop.length]!;
    if (Math.hypot(b.x - a.x, b.y - a.y) < 1e-9) continue;
    segs.push({ type: "Line", start: { x: a.x, y: a.y }, end: { x: b.x, y: b.y } });
  }
  return segs;
}

/** Copper area of one pad, mm² (Oval/RoundRect ≈ their bounding rect). */
function padAreaMm2(pad: Pad): number {
  const s = pad.shape;
  switch (s.type) {
    case "Circle":
      return (Math.PI / 4) * s.diameter * s.diameter;
    case "Rect":
    case "Oval":
    case "RoundRect":
      return s.width * s.height;
    case "Custom":
      return loopArea(s.vertices);
    default:
      return 0;
  }
}

/**
 * Estimated copper coverage per copper layer, mm²: zones (outline minus
 * holes; hatched pours count half), traces and arcs (length × width), pad
 * copper, and via annular rings on their span-end layers. Overlaps between
 * features are not deduplicated — a slight overestimate on dense boards —
 * and each layer is capped at the board area.
 */
function copperAreaByLayer(pcb: Pcb, boardArea: number): Record<string, number> {
  const area: Record<string, number> = {};
  const add = (layer: string, a: number) => {
    if (!/Cu$/.test(layer) || !(a > 0)) return;
    area[layer] = (area[layer] ?? 0) + a;
  };
  for (const z of pcb.zones) {
    const holes = (z.holes ?? []).reduce((s, h) => s + loopArea(h), 0);
    const fill = z.fillType === "Hatched" ? 0.5 : 1;
    add(z.layer, Math.max(0, loopArea(z.outline) - holes) * fill);
  }
  for (const t of pcb.traces) {
    add(t.layer, Math.hypot(t.end.x - t.start.x, t.end.y - t.start.y) * t.width);
  }
  for (const a of pcb.traceArcs ?? []) {
    const sweep = (Math.abs(a.endAngle - a.startAngle) * Math.PI) / 180;
    add(a.layer, sweep * a.radius * a.width);
  }
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      const padArea = padAreaMm2(pad);
      for (const layer of pad.layers) add(layer, padArea);
    }
  }
  for (const v of pcb.vias) {
    const ring = (Math.PI / 4) * Math.max(0, v.diameter * v.diameter - v.drill * v.drill);
    add(v.startLayer, ring);
    if (v.endLayer !== v.startLayer) add(v.endLayer, ring);
  }
  for (const k of Object.keys(area)) area[k] = Math.min(area[k]!, boardArea);
  return area;
}

/** Mass estimate for a bare board: FR4 slab + per-layer copper coverage. */
interface BoardMassEstimate {
  fr4VolumeMm3: number;
  copperVolumeMm3: number;
  copperAreaByLayer: Record<string, number>;
  fr4G: number;
  copperG: number;
  totalG: number;
  /** total mass / substrate solid volume — the density that makes the
   *  extruded slab weigh what the real FR4+copper board weighs. */
  homogenizedDensityKgM3: number;
}

/** Compute the board's FR4 + copper mass from outline area and stackup. */
function estimateBoardMass(pcb: Pcb, boardArea: number): BoardMassEstimate {
  const thickness = pcb.outline.thickness;
  const fr4VolumeMm3 = boardArea * thickness;
  const copperArea = copperAreaByLayer(pcb, boardArea);
  const thicknessByLayer = new Map<string, number | undefined>(
    pcb.stackup.layers.map((l) => [l.layer as string, l.copperThickness]),
  );
  let copperVolumeMm3 = 0;
  for (const [layer, a] of Object.entries(copperArea)) {
    copperVolumeMm3 += a * (thicknessByLayer.get(layer) ?? DEFAULT_COPPER_THICKNESS_MM);
  }
  // mass (g) = volume (mm³) × density (kg/m³) / 1e6
  const fr4G = (fr4VolumeMm3 * FR4_DENSITY_KG_M3) / 1e6;
  const copperG = (copperVolumeMm3 * COPPER_DENSITY_KG_M3) / 1e6;
  const totalG = fr4G + copperG;
  return {
    fr4VolumeMm3,
    copperVolumeMm3,
    copperAreaByLayer: copperArea,
    fr4G,
    copperG,
    totalG,
    homogenizedDensityKgM3:
      fr4VolumeMm3 > 0 ? (totalG / fr4VolumeMm3) * 1e6 : FR4_DENSITY_KG_M3,
  };
}

/** One simplified component keep-out volume, board-local mm (bottom of the
 *  substrate at z = 0, top at z = thickness). */
interface ComponentBoxOut {
  ref: string;
  footprint: string;
  /** "mesh" = kernel 3D body extents; "courtyard" = pad/courtyard bbox ×
   *  per-package-class default height. */
  source: "mesh" | "courtyard";
  min: Vec3;
  max: Vec3;
}

/** Footprints that are holes in the board, not bodies on it. */
const MOUNTING_FP_RE = /mount(ing)?[_-]?hole/i;

/** Default body height (mm) by package-class name — mirrors the kernel's
 *  component-mesh table (vcad-ecad-pcb::component_mesh::package_height). */
function packageHeightMm(footprintName: string): number {
  const n = footprintName.toUpperCase();
  if (n.includes("0402")) return 0.35;
  if (n.includes("0603")) return 0.45;
  if (n.includes("0805")) return 0.5;
  if (n.includes("1206")) return 0.55;
  if (n.includes("SOIC")) return 1.75;
  if (n.includes("QFP")) return 1.6;
  if (n.includes("DIP")) return 4.0;
  if (/SOT-?223/.test(n)) return 1.6;
  if (/SOT-?23/.test(n)) return 1.1;
  if (n.includes("HEADER")) return 8.5;
  return 1.0;
}

/**
 * Simplified keep-out volume per placed footprint. Preferred source is the
 * kernel's parametric 3D component bodies (exact XY + Z extents in board
 * coordinates); when the ECAD WASM is unavailable or a footprint has no
 * body, fall back to its courtyard/pad bbox extruded to a per-package-class
 * default height on the correct side of the board.
 */
async function componentKeepouts(
  pcb: Pcb,
): Promise<{ boxes: ComponentBoxOut[]; warnings: string[] }> {
  const thickness = pcb.outline.thickness;
  const warnings: string[] = [];
  const meshBoxes = new Map<string, { min: Vec3; max: Vec3 }>();
  try {
    for (const m of await componentMeshes(pcb)) {
      let minX = Infinity;
      let minY = Infinity;
      let minZ = Infinity;
      let maxX = -Infinity;
      let maxY = -Infinity;
      let maxZ = -Infinity;
      for (let i = 0; i + 2 < m.positions.length; i += 3) {
        if (m.positions[i] < minX) minX = m.positions[i];
        if (m.positions[i] > maxX) maxX = m.positions[i];
        if (m.positions[i + 1] < minY) minY = m.positions[i + 1];
        if (m.positions[i + 1] > maxY) maxY = m.positions[i + 1];
        if (m.positions[i + 2] < minZ) minZ = m.positions[i + 2];
        if (m.positions[i + 2] > maxZ) maxZ = m.positions[i + 2];
      }
      if (
        Number.isFinite(minX) &&
        maxX - minX > 1e-6 &&
        maxY - minY > 1e-6 &&
        maxZ - minZ > 1e-6
      ) {
        meshBoxes.set(m.footprint_ref, {
          min: { x: minX, y: minY, z: minZ },
          max: { x: maxX, y: maxY, z: maxZ },
        });
      }
    }
  } catch {
    // ECAD WASM unavailable — every footprint takes the courtyard fallback.
  }

  const boxes: ComponentBoxOut[] = [];
  for (const fp of pcb.footprints) {
    // Mounting holes / NPTH-only footprints are voids, not bodies.
    if (MOUNTING_FP_RE.test(fp.footprintName) || MOUNTING_FP_RE.test(fp.ref)) continue;
    if (fp.pads.length > 0 && fp.pads.every((p) => p.padType === "NPTH")) continue;

    const mesh = meshBoxes.get(fp.ref);
    if (mesh) {
      boxes.push({
        ref: fp.ref,
        footprint: fp.footprintName,
        source: "mesh",
        min: mesh.min,
        max: mesh.max,
      });
      continue;
    }
    const local = localCourtyardAabb(fp.pads, fp.graphics ?? []);
    const board = boardCourtyardAabb(local, fp.position, fp.rotation ?? 0, fp.front ?? true);
    if (!board) {
      warnings.push(`${fp.ref}: no 3D body, pads, or courtyard — keep-out skipped`);
      continue;
    }
    const h = packageHeightMm(fp.footprintName);
    const front = fp.front ?? true;
    boxes.push({
      ref: fp.ref,
      footprint: fp.footprintName,
      source: "courtyard",
      min: { x: board.min.x, y: board.min.y, z: front ? thickness : -h },
      max: { x: board.max.x, y: board.max.y, z: front ? thickness + h : 0 },
    });
  }
  return { boxes, warnings };
}

/** Cap on the per-component box list echoed in the response. */
const KEEPOUT_ECHO_CAP = 32;
/** Cap on the inline `document` IR echoed when a fresh session is minted. */
const INLINE_DOC_ECHO_CAP = 100_000;

/**
 * Materialize the session PCB as a solid CAD part — the inverse of
 * board_from_solid. The substrate is the board outline extruded to the board
 * thickness through the hole-aware sketch path, so bore/cutout polygons
 * survive as real holes (no boolean pass). Optional simplified component
 * keep-out volumes ride along as child parts. The substrate's material
 * carries a homogenized FR4+copper density so inspect_cad and physics see
 * the real board mass, not bare-FR4 mass.
 *
 * Injects into `document_id_target` when given (board bottom lands at
 * z = 0 in that session's frame), else mints a fresh CAD session.
 */
export async function solidFromBoard(args: Record<string, unknown>) {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);
  const pcb = getDocPcb(doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  const outline = pcb.outline;
  if (!outline.vertices || outline.vertices.length < 3) {
    return ecadError("Board outline needs >= 3 vertices — set_board_outline first");
  }
  const thickness = outline.thickness;
  if (!(thickness > 0)) return ecadError("Board thickness must be > 0");

  const partName =
    typeof args.part_name === "string" && args.part_name.trim()
      ? args.part_name.trim()
      : "board";
  const includeComponents = args.include_components !== false;

  const cutouts = outline.cutouts ?? [];
  const boardArea = Math.max(
    0,
    loopArea(outline.vertices) - cutouts.reduce((s, c) => s + loopArea(c), 0),
  );
  if (!(boardArea > 0)) return ecadError("Board outline has no area to extrude");
  const mass = estimateBoardMass(pcb, boardArea);

  const warnings: string[] = [];
  let boxes: ComponentBoxOut[] = [];
  if (includeComponents && pcb.footprints.length > 0) {
    const keepouts = await componentKeepouts(pcb);
    boxes = keepouts.boxes;
    warnings.push(...keepouts.warnings);
  }

  // Target: inject into an existing CAD session, or mint a fresh document.
  const targetId =
    typeof args.document_id_target === "string" && args.document_id_target
      ? args.document_id_target
      : undefined;
  let target: Document;
  if (targetId) {
    try {
      target = getSession(targetId);
    } catch (e) {
      return ecadError(e instanceof Error ? e.message : String(e));
    }
  } else {
    target = createDocument();
  }

  const existingIds = Object.keys(target.nodes)
    .map(Number)
    .filter(Number.isFinite);
  let nextId = (existingIds.length > 0 ? Math.max(...existingIds) : 0) + 1;
  const alloc = (name: string | null, op: CsgOp): number => {
    const id = nextId++;
    target.nodes[String(id)] = { id, name, op };
    return id;
  };

  // Substrate: hole-aware sketch + extrude (#396) — cutouts become interior
  // walls of one multi-loop solid, no Difference pass. Board bottom at z = 0.
  const sketchId = alloc(`${partName} profile`, {
    type: "Sketch2D",
    origin: { x: 0, y: 0, z: 0 },
    x_dir: { x: 1, y: 0, z: 0 },
    y_dir: { x: 0, y: 1, z: 0 },
    segments: loopToSegments(ensureCcw(outline.vertices)),
    ...(cutouts.length > 0 ? { holes: cutouts.map((c) => loopToSegments(c)) } : {}),
  });
  const substrateId = alloc(partName, {
    type: "Extrude",
    sketch: sketchId,
    direction: { x: 0, y: 0, z: thickness },
  });

  // Homogenized material: the extruded slab weighs what the real FR4+copper
  // board weighs, so inspect_cad / physics read a realistic mass off it.
  let materialKey = `pcb:${partName}`;
  for (let i = 2; target.materials[materialKey]; i++) materialKey = `pcb:${partName}-${i}`;
  target.materials[materialKey] = {
    name: materialKey,
    color: [0.05, 0.35, 0.18], // FR4 soldermask green
    metallic: 0.1,
    roughness: 0.7,
    density: Math.round(mass.homogenizedDensityKgM3 * 100) / 100,
  };
  target.roots.push({ root: substrateId, material: materialKey });
  target.part_materials[partName] = materialKey;

  const componentPartIds: string[] = [];
  for (const b of boxes) {
    const size = {
      x: b.max.x - b.min.x,
      y: b.max.y - b.min.y,
      z: b.max.z - b.min.z,
    };
    const cubeId = alloc(null, { type: "Cube", size });
    const moveId = alloc(`${partName}:${b.ref}`, {
      type: "Translate",
      child: cubeId,
      offset: { x: b.min.x, y: b.min.y, z: b.min.z },
    });
    target.roots.push({ root: moveId, material: "default" });
    componentPartIds.push(String(moveId));
  }

  const created = !targetId;
  const outId = targetId ?? registerSession(target);

  const roundVec = (v: Vec3) => ({ x: round3(v.x), y: round3(v.y), z: round3(v.z) });
  const copperAreaRounded = Object.fromEntries(
    Object.entries(mass.copperAreaByLayer).map(([k, v]) => [k, round3(v)]),
  );
  // Echo the minted IR so the part can travel to another server via
  // open_document — unless the outline is huge, in which case get_document
  // on the returned document_id serves it instead.
  const inlineDoc =
    created && JSON.stringify(target).length <= INLINE_DOC_ECHO_CAP ? target : undefined;

  const payload = {
    success: true,
    document_id: outId,
    created_session: created,
    source_document_id: documentId,
    part_id: String(substrateId),
    part_name: partName,
    substrate: {
      outline_vertices: outline.vertices.length,
      cutouts: cutouts.length,
      thickness,
      area_mm2: round3(boardArea),
      volume_mm3: round3(mass.fr4VolumeMm3),
    },
    components: {
      included: includeComponents,
      count: boxes.length,
      part_ids: componentPartIds,
      ...(boxes.length > 0
        ? {
            boxes: boxes.slice(0, KEEPOUT_ECHO_CAP).map((b) => ({
              ref: b.ref,
              footprint: b.footprint,
              source: b.source,
              min: roundVec(b.min),
              max: roundVec(b.max),
            })),
            ...(boxes.length > KEEPOUT_ECHO_CAP ? { boxes_truncated: true } : {}),
          }
        : {}),
    },
    mass: {
      fr4_g: round3(mass.fr4G),
      copper_g: round3(mass.copperG),
      total_g: round3(mass.totalG),
      fr4_volume_mm3: round3(mass.fr4VolumeMm3),
      copper_volume_mm3: round3(mass.copperVolumeMm3),
      copper_area_mm2_by_layer: copperAreaRounded,
      homogenized_density_kg_m3: Math.round(mass.homogenizedDensityKgM3 * 100) / 100,
      material: materialKey,
    },
    ...(inlineDoc ? { document: inlineDoc } : {}),
    ...(warnings.length > 0 ? { warnings } : {}),
    hint: created
      ? "Fresh CAD session minted — render_view / inspect_cad it, or feed `document` to open_document elsewhere"
      : "Part injected — render_view / inspect_cad the target session (e.g. run check_enclosure_fit against it)",
  };

  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: {
      solid_from_board: {
        part_id: String(substrateId),
        part_name: partName,
        components: componentPartIds.length,
        mass_g: round3(mass.totalG),
      },
      document_id: outId,
      source_document_id: documentId,
    },
  };
}

// ===========================================================================
// Generative parts catalog + verified substitution
// (vcad-ecad-parts / vcad-ecad-verify)
// ===========================================================================

export const searchElectronicPartsSchema = {
  type: "object",
  properties: {
    query: {
      type: "string",
      description:
        "Spec query, e.g. '10k 0603 1%', '100nF 0402', '4.7uH 0805'. Parses value, package, and tolerance.",
    },
    limit: {
      type: "integer",
      description: "Max candidates (E-series neighbours included). Default 5.",
    },
  },
  required: ["query"],
} as const;

export const resolvePartSchema = {
  type: "object",
  properties: {
    query: {
      type: "string",
      description:
        "Either a passive spec query (e.g. '10k 0603 1%') — E-series-snapped, " +
        "returns footprint + symbol + 3D body + MPN xrefs — or a jellybean part " +
        "name/alias (e.g. 'NE555', 'LM358', 'LM555'), which returns its universal " +
        "pin definitions (number, name, electrical type) plus datasheet and notes.",
    },
  },
  required: ["query"],
} as const;

export const findAlternativesSchema = {
  type: "object",
  properties: {
    query: {
      type: "string",
      description:
        "Spec query whose resolved part to find substitutes for. Alternatives keep the value and vary the package, each labelled identical / needs-reroute / incompatible.",
    },
  },
  required: ["query"],
} as const;

export const verifySubstitutionSchema = {
  type: "object",
  properties: {
    document_id: {
      type: "string",
      description: "Session id from create_schematic/open_document holding the PCB.",
    },
    reference: {
      type: "string",
      description: "Reference designator on the board to swap (e.g. 'R1').",
    },
    candidate: {
      type: "string",
      description: "Spec query for the replacement part (e.g. '10k 0805').",
    },
  },
  required: ["reference", "candidate"],
} as const;

export const buildReceiptSchema = {
  type: "object",
  properties: {
    document_id: {
      type: "string",
      description:
        "Session id holding the PCB and/or clearance-spec'd CAD parts to certify.",
    },
    enclosure_document_id: {
      type: "string",
      description:
        "Optional CAD session id holding the enclosure solid. When given, the " +
        "Receipt also carries a cross-domain enclosure-fit verdict (board fits, " +
        "components clear the lid, holes land on standoffs, connectors align) — " +
        "proof the board fits its case, not just that copper passes DRC.",
    },
    clearance: {
      type: "number",
      description: "Enclosure-fit clearance in mm (default 0.5); only used with enclosure_document_id.",
    },
  },
} as const;

/** Spec-search the generative catalog (offline). */
export async function searchElectronicParts(args: Record<string, unknown>) {
  const query = typeof args.query === "string" ? args.query : "";
  const limit = typeof args.limit === "number" ? Math.max(1, Math.round(args.limit)) : 5;
  const results = await kernelSearchPartsEcad(query, limit);
  return {
    content: [{ type: "text" as const, text: JSON.stringify({ query, count: results.length, results }) }],
  };
}

/**
 * Resolve a query into one fully-specified part. A passive spec
 * ('10k 0603 1%') resolves via the generative catalog (footprint + symbol +
 * 3D body); otherwise the query is tried as a jellybean part name/alias
 * ('NE555'), returning its universal pin definitions. This unifies the parts
 * search and schematic-capture pipelines behind one tool.
 */
export async function resolvePart(args: Record<string, unknown>) {
  const query = typeof args.query === "string" ? args.query : "";
  const part = await kernelResolvePart(query);
  if (part) {
    return { content: [{ type: "text" as const, text: JSON.stringify(part) }] };
  }
  // Fall back to the curated jellybean database (named ICs by part or alias).
  const def = await kernelResolvePartDef(query, undefined);
  if (def) {
    return {
      content: [{ type: "text" as const, text: JSON.stringify({ kind: "jellybean", ...def }) }],
    };
  }
  return {
    content: [
      {
        type: "text" as const,
        text: `No resolvable part for '${query}'. Provide a passive value with optional package + tolerance (e.g. '10k 0603 1%'), or a known part name/alias (e.g. 'NE555', 'LM358').`,
      },
    ],
    isError: true,
  };
}

/** Propose spec-compatible alternatives, each classified by footprint compat. */
export async function findAlternatives(args: Record<string, unknown>) {
  const query = typeof args.query === "string" ? args.query : "";
  const alts = await kernelFindAlternatives(query);
  return {
    content: [{ type: "text" as const, text: JSON.stringify({ query, count: alts.length, alternatives: alts }) }],
  };
}

/** PROVE a substitution: re-derive, re-place, re-run DRC, report the delta. */
export async function verifySubstitution(args: Record<string, unknown>) {
  const { doc } = resolveDocInput(args);
  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const reference = typeof args.reference === "string" ? args.reference : "";
  const candidate = typeof args.candidate === "string" ? args.candidate : "";
  const sub = await kernelVerifySubstitution(pcb, reference, candidate);
  if (!sub) {
    return {
      content: [
        {
          type: "text" as const,
          text: `Could not verify: no footprint '${reference}' on the board, or candidate '${candidate}' is unresolvable.`,
        },
      ],
      isError: true,
    };
  }
  return { content: [{ type: "text" as const, text: JSON.stringify(sub) }] };
}

/** Build a re-runnable verification Receipt for the session's PCB. When an
 *  `enclosure_document_id` is supplied (and `engine` is available), the Receipt
 *  also carries a cross-domain enclosure-fit verdict — proof the board fits the
 *  case it ships in, not just that copper passes DRC. */
export async function buildReceipt(args: Record<string, unknown>, engine?: Engine) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  const clearanceSpecs = ctx.doc.clearance_specs ?? [];
  if (!pcb) {
    if (clearanceSpecs.length === 0 && (ctx.doc.constraints ?? []).length === 0) {
      return {
        content: [
          {
            type: "text" as const,
            text: "Error: Document has no PCB, no clearance specs, and no design constraints — nothing to certify. (Persist assertions with check_clearance + label or add_constraint first.)",
          },
        ],
        isError: true,
      };
    }
    // Mechanical-only receipt: re-measure every persisted clearance spec
    // and design constraint.
    const unified: DesignReceipt = {
      schema: RECEIPT_SCHEMA,
      ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
      claims: [
        ...clearanceReceiptClaims(ctx.doc, engine),
        ...(await constraintReceiptClaims(ctx.doc)),
      ],
    };
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({ unified, unified_summary: summarize(unified) }),
        },
      ],
      structuredContent: {
        unified,
        ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
      },
    };
  }
  const validity = validatePcb(pcb);
  if (!validity.valid) {
    return pcbValidationError("build_receipt", validity, args.document_id ? String(args.document_id) : undefined);
  }
  const receipt = await kernelBuildReceipt(pcb);
  if (!receipt) {
    return {
      content: [{ type: "text" as const, text: "Error: ECAD engine unavailable" }],
      isError: true,
    };
  }
  // Surface realized-plane continuity at the top: a split power plane is an
  // open PDN even with a clean clearance/short DRC, so the receipt's verdict
  // must not read "clean" while a +3V3 plane sits in 15 islands.
  const power = receipt.power_integrity ?? [];
  const brokenPlanes = power.filter((p) => !p.continuous);
  const powerIntegrityOk = brokenPlanes.length === 0;
  const planeWarnings = brokenPlanes.map(
    (p) =>
      `net '${p.net}': ${p.islands} galvanic islands, ${Math.round(p.coverage * 1000) / 10}% pad coverage (${p.connected_pads}/${p.total_pads}), ${p.vias} stitching via(s)`,
  );

  // Optional cross-domain layer: cross-check the board against an enclosure.
  let enclosureFit: Awaited<ReturnType<typeof computeEnclosureFitForBoard>>["report"] | undefined;
  let enclosureFitError: string | undefined;
  const enclosureId = args.enclosure_document_id ? String(args.enclosure_document_id) : "";
  if (enclosureId) {
    if (!engine) {
      enclosureFitError = "enclosure-fit needs the kernel engine; unavailable in this context";
    } else {
      try {
        const res = await computeEnclosureFitForBoard(pcb, getSession(enclosureId), engine, {
          clearance: typeof args.clearance === "number" ? args.clearance : undefined,
        });
        if (res.report) enclosureFit = res.report;
        else enclosureFitError = res.error;
      } catch (e) {
        enclosureFitError = e instanceof Error ? e.message : String(e);
      }
    }
  }

  // The unified DesignReceipt (schema vcad.receipt/1) is the cross-domain
  // claim ledger — DRC, power continuity, provenance, and enclosure fit as
  // fail-closed claims. The legacy Receipt stays the re-runnable input to
  // verify_receipt. Emit both.
  const unified = unifiedFromPcbReceipt(receipt, ctx.documentId, {
    ...(enclosureFit ? { enclosureFit } : {}),
    ...(enclosureId && !enclosureFit && enclosureFitError
      ? { enclosureFitError }
      : {}),
  });
  // Persisted mechanical clearance assertions ride in the same ledger, so a
  // board+mechanics document certifies both domains in one receipt.
  if (clearanceSpecs.length > 0) {
    unified.claims.push(...clearanceReceiptClaims(ctx.doc, engine));
  }
  // Design constraints certify as constraint.* claims in the same ledger.
  if ((ctx.doc.constraints ?? []).length > 0) {
    unified.claims.push(...(await constraintReceiptClaims(ctx.doc)));
  }

  // The receipt rides in structuredContent so the inline viewer renders it
  // as an audit ledger (the only carrier ChatGPT's widget bridge exposes);
  // document_id lets the viewer also fetch the board GLB behind the ledger.
  const textPayload = enclosureFit
    ? { ...receipt, enclosure_fit: enclosureFit }
    : enclosureFitError
      ? { ...receipt, enclosure_fit_error: enclosureFitError }
      : receipt;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          ...textPayload,
          unified,
          unified_summary: summarize(unified),
        }),
      },
    ],
    structuredContent: {
      receipt,
      unified,
      power_integrity_ok: powerIntegrityOk,
      ...(planeWarnings.length ? { disconnected_planes: planeWarnings } : {}),
      ...(enclosureFit ? { enclosure_fit: enclosureFit } : {}),
      ...(enclosureFitError ? { enclosure_fit_error: enclosureFitError } : {}),
      ...(enclosureId ? { enclosure_document_id: enclosureId } : {}),
      ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
    },
  };
}

export const verifyReceiptSchema = {
  type: "object",
  properties: {
    document_id: {
      type: "string",
      description: "Session id holding the PCB to re-verify against.",
    },
    receipt: {
      type: "object",
      description: "A Receipt previously produced by build_receipt.",
    },
  },
  required: ["receipt"],
} as const;

/** Worst-wins rollup across verification domains. */
function worstStatus(statuses: ReceiptStatus[]): ReceiptStatus {
  if (statuses.includes("Violated")) return "Violated";
  if (statuses.includes("Stale")) return "Stale";
  return "Holds";
}

/** Re-run a prior Receipt against the session's current document. Returns the
 *  verdict: Holds (unchanged, clean), Stale (document changed but claims still
 *  hold), or Violated. Handles the re-runnable PCB Receipt (board hash + DRC
 *  diff), the unified DesignReceipt's mech.clearance claims (re-measured
 *  against current geometry), or both at once — worst verdict wins. */
export async function verifyReceipt(args: Record<string, unknown>, engine?: Engine) {
  const ctx = resolveDocInput(args);
  const raw = args.receipt as Record<string, unknown> | undefined;
  if (!raw || typeof raw !== "object") {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: missing `receipt` — pass a receipt produced by build_receipt.",
        },
      ],
      isError: true,
    };
  }

  // Legacy re-runnable PCB Receipt: the arg itself, or nested under
  // `.receipt` when the whole build_receipt payload is passed back verbatim.
  let legacy: Receipt | undefined;
  if (typeof raw.board_hash === "string" && raw.board_hash) {
    legacy = raw as unknown as Receipt;
  } else if (raw.receipt && typeof raw.receipt === "object") {
    const inner = raw.receipt as Record<string, unknown>;
    if (typeof inner.board_hash === "string" && inner.board_hash) {
      legacy = inner as unknown as Receipt;
    }
  }

  // Unified DesignReceipt: the arg itself, or nested under `.unified`.
  let unified: DesignReceipt | undefined;
  if (typeof raw.schema === "string" && Array.isArray(raw.claims)) {
    unified = raw as unknown as DesignReceipt;
  } else if (raw.unified && typeof raw.unified === "object") {
    const inner = raw.unified as Record<string, unknown>;
    if (typeof inner.schema === "string" && Array.isArray(inner.claims)) {
      unified = inner as unknown as DesignReceipt;
    }
  }
  const clearanceReceipt = unified && hasClearanceClaims(unified) ? unified : undefined;
  const constraintReceipt = unified && hasConstraintClaims(unified) ? unified : undefined;

  if (!legacy && !clearanceReceipt && !constraintReceipt) {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: `receipt` carries nothing re-verifiable — pass a PCB Receipt (board_hash) or a unified DesignReceipt with mech.clearance or constraint.* claims, as produced by build_receipt.",
        },
      ],
      isError: true,
    };
  }

  const statuses: ReceiptStatus[] = [];

  let boardHash: string | undefined;
  if (legacy) {
    const pcb = getDocPcb(ctx.doc);
    if (!pcb) {
      return {
        content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
        isError: true,
      };
    }
    const validity = validatePcb(pcb);
    if (!validity.valid) {
      return pcbValidationError("verify_receipt", validity, args.document_id ? String(args.document_id) : undefined);
    }
    const status = await kernelVerifyReceipt(pcb, legacy);
    if (!status) {
      return {
        content: [{ type: "text" as const, text: "Error: ECAD engine unavailable" }],
        isError: true,
      };
    }
    statuses.push(status);
    boardHash = legacy.board_hash;
  }

  let clearance: ReturnType<typeof verifyClearanceClaims> | undefined;
  if (clearanceReceipt) {
    if (!engine) {
      return {
        content: [
          {
            type: "text" as const,
            text: "Error: clearance re-verification needs the kernel engine; unavailable in this context",
          },
        ],
        isError: true,
      };
    }
    clearance = verifyClearanceClaims(ctx.doc, engine, clearanceReceipt);
    statuses.push(clearance.status);
  }

  let constraintChecks: Awaited<ReturnType<typeof verifyConstraintClaims>> | undefined;
  if (constraintReceipt) {
    constraintChecks = await verifyConstraintClaims(ctx.doc, constraintReceipt);
    statuses.push(constraintChecks.status);
  }

  const payload = {
    status: worstStatus(statuses),
    ...(boardHash ? { board_hash: boardHash } : {}),
    ...(clearance ? { clearance } : {}),
    ...(constraintChecks ? { constraints: constraintChecks } : {}),
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: {
      verify_receipt: payload,
      ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
    },
  };
}

// ============================================================================
// fix_drc — run DRC and auto-apply the mechanically-safe subset of fixes
// ============================================================================
//
// The "loop is the product" tool for boards: one call that runs DRC, applies
// only the fixes that are mechanically safe (no design intent required), and
// returns a fail-closed receipt of what it fixed, what it skipped and why,
// and the before/after violation counts. Every individual fix is verified
// with a drc_delta capture and REVERTED if it would introduce any new
// violation — a fix that can't be proven safe never lands. Never touched:
// different-net shorts/clearance, courtyard overlaps, keepouts, fab-rule
// scalars — anything whose resolution is a design decision.

/** Distance from a point to a segment. */
function distToSegment(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  let t = len2 > 0 ? ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2 : 0;
  t = Math.max(0, Math.min(1, t));
  const cx = a.x + t * dx;
  const cy = a.y + t * dy;
  return Math.hypot(p.x - cx, p.y - cy);
}

/** Closest point on a segment to `p`. */
function closestOnSegment(p: Vec2, a: Vec2, b: Vec2): Vec2 {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  let t = len2 > 0 ? ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2 : 0;
  t = Math.max(0, Math.min(1, t));
  return { x: a.x + t * dx, y: a.y + t * dy };
}

/** Every boundary loop of the board: the outer outline plus each cutout —
 *  edge clearance is measured to the nearest of any of them (kernel parity). */
function boardBoundaryLoops(outline: BoardOutline): Vec2[][] {
  const loops: Vec2[][] = [];
  if (outline.vertices.length >= 3) loops.push(outline.vertices);
  for (const c of outline.cutouts ?? []) if (c.length >= 3) loops.push(c);
  return loops;
}

/** Min distance from `p` to any board boundary edge, and the closest boundary
 *  point (for the inward-nudge direction). Null when the outline is degenerate. */
function nearestBoundary(
  p: Vec2,
  outline: BoardOutline,
): { dist: number; point: Vec2 } | null {
  let best: { dist: number; point: Vec2 } | null = null;
  for (const loop of boardBoundaryLoops(outline)) {
    for (let i = 0, j = loop.length - 1; i < loop.length; j = i++) {
      const d = distToSegment(p, loop[j]!, loop[i]!);
      if (!best || d < best.dist) {
        best = { dist: d, point: closestOnSegment(p, loop[j]!, loop[i]!) };
      }
    }
  }
  return best;
}

/** True when `p` is on the board: inside the outer outline and outside every
 *  cutout. */
function onBoard(p: Vec2, outline: BoardOutline): boolean {
  if (!pointInPolygon(p, outline.vertices)) return false;
  for (const c of outline.cutouts ?? []) {
    if (c.length >= 3 && pointInPolygon(p, c)) return false;
  }
  return true;
}

/** Coincidence tolerance for matching copper endpoints/vias to a DRC
 *  violation position (the kernel reports exact element coordinates). */
const FIX_EPS = 1e-3;

function samePoint(a: Vec2, b: Vec2, eps = FIX_EPS): boolean {
  return Math.abs(a.x - b.x) <= eps && Math.abs(a.y - b.y) <= eps;
}

/** One applied (or planned) fix in the fix_drc receipt. */
interface DrcFixAction {
  rule: string;
  action: string;
  detail: string;
  net?: string;
  position?: Vec2;
}

/** One skipped violation (or violation group) in the fix_drc receipt. */
interface DrcFixSkip {
  rule: string;
  reason: string;
  count: number;
  sample_message?: string;
}

/** Rules fix_drc will never touch, with the reason surfaced in the receipt. */
const FIX_DRC_NEVER: Record<string, string> = {
  Short:
    "different-net short — resolving requires design intent (re-route both nets or add a net-tie); never auto-safe",
  Clearance:
    "different-net clearance — pushing copper risks new shorts; re-route the involved nets deliberately",
  CourtyardOverlap: "placement decision — fix_drc never moves components (use set_placement)",
  Keepout: "copper inside a keepout — deliberate design change required",
  SilkscreenClearance: "silkscreen artwork — cosmetic, not auto-fixable",
  MinTraceWidth:
    "fab-rule break — widen the trace (or change design rules) deliberately",
  MinDrill: "fab-rule break — enlarge the drill (or change design rules) deliberately",
  AnnularRing:
    "fab-rule break — enlarge the via diameter (or change design rules) deliberately",
  AcidTrap: "acute copper junction — reshaping copper is a routing decision",
  SameNetBypass:
    "same-net bypass — whether the touch is intended is design intent, not auto-safe",
};

/** JSON Schema for fix_drc tool. */
export const fixDrcSchema = {
  type: "object" as const,
  properties: {
    ...docInputProperties,
    dry_run: {
      type: "boolean" as const,
      description:
        "Plan the fixes without mutating the board. Planned actions come back " +
        "in `planned` (NOT verified — only an applied fix is delta-verified).",
    },
    reroute_effort: {
      type: "number" as const,
      description:
        "route_nets effort used when re-routing UnconnectedNet/NetIslands nets " +
        "(default 4 — higher than a first-pass route).",
    },
    max_fixes: {
      type: "number" as const,
      description: "Cap on applied fixes in one call (default 50).",
    },
  },
  required: ["document_id"],
};

/** Outcome of one verified fix attempt. */
type FixAttempt = { ok: true; delta: DrcDelta } | { ok: false; reason: string };

/**
 * Run DRC and automatically apply the mechanically-safe subset of fixes:
 * stitching vias for UnstitchedPad, dedupe of overlapping same-net via drills
 * (HoleToHole), inward nudges for EdgeClearance when a legal corridor exists,
 * and per-net rip-up + re-route for UnconnectedNet/NetIslands. Each fix is
 * individually verified with a DRC delta and reverted if it introduces any
 * new violation. Fail-closed: refuses to run without the kernel DRC engine,
 * and an unverifiable board is never reported as fixed.
 */
export async function fixDrc(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return ecadError(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }
  // Fail closed: without the kernel every fix would be unverifiable guesswork.
  if (!(await isEcadAvailable())) {
    return ecadError(
      "fix_drc requires the kernel DRC engine (ECAD WASM unavailable) — refusing to apply unverifiable fixes",
    );
  }

  const dryRun = Boolean(args.dry_run);
  const effort = Math.min(100, Math.max(0.1, Number(args.reroute_effort) || 4));
  const maxFixes = Math.max(1, Math.round(Number(args.max_fixes as number) || 50));

  const before = await drcPcb(pcb, "full", 20);
  if (!before.success) return ecadUnverifiable("fix_drc", before);
  const viols = before.details ?? [];

  const fixed: DrcFixAction[] = [];
  const planned: DrcFixAction[] = [];
  const skipped: DrcFixSkip[] = [];
  let budget = maxFixes;

  const skip = (rule: string, reason: string, message?: string) => {
    const existing = skipped.find((s) => s.rule === rule && s.reason === reason);
    if (existing) existing.count++;
    else skipped.push({ rule, reason, count: 1, ...(message ? { sample_message: message } : {}) });
  };

  /** Apply `mutate` under a drc_delta capture; revert (restore the saved
   *  copper arrays) unless the delta proves no new violations. */
  const applyVerified = async (
    bounds: { min: Vec2; max: Vec2 } | "full",
    mutate: () => void,
  ): Promise<FixAttempt> => {
    const savedTraces = pcb.traces;
    const savedVias = pcb.vias;
    const cap = await beginDrcDelta(pcb, bounds);
    mutate();
    const delta = await cap.finish();
    if (delta.unverifiable) {
      pcb.traces = savedTraces;
      pcb.vias = savedVias;
      return { ok: false, reason: `verification failed: ${delta.unverifiable.reason}` };
    }
    if (delta.introduced > 0) {
      pcb.traces = savedTraces;
      pcb.vias = savedVias;
      const worst = delta.sample[0];
      return {
        ok: false,
        reason:
          `would introduce ${delta.introduced} new violation(s)` +
          (worst ? ` (worst: ${worst.rule})` : ""),
      };
    }
    return { ok: true, delta };
  };

  // -- 1) HoleToHole: dedupe overlapping same-net via drills ------------------
  for (const v of viols.filter((x) => x.rule === "HoleToHole")) {
    if (budget <= 0) break;
    if (!(typeof v.actual === "number" && v.actual < 0)) {
      skip(
        "HoleToHole",
        "holes are close but not overlapping — spacing needs a deliberate layout change",
        v.message,
      );
      continue;
    }
    if (!v.position) {
      skip("HoleToHole", "violation carries no position", v.message);
      continue;
    }
    // The kernel reports the midpoint between the two holes; re-derive the
    // pair from the live vias (nets are not in the message — recomputed here).
    const pos = v.position;
    let pair: [Via, Via] | null = null;
    outer: for (let i = 0; i < pcb.vias.length; i++) {
      for (let j = i + 1; j < pcb.vias.length; j++) {
        const a = pcb.vias[i]!;
        const b = pcb.vias[j]!;
        const mid = { x: (a.position.x + b.position.x) / 2, y: (a.position.y + b.position.y) / 2 };
        if (!samePoint(mid, pos, 0.05)) continue;
        const edge =
          Math.hypot(a.position.x - b.position.x, a.position.y - b.position.y) -
          a.drill / 2 -
          b.drill / 2;
        if (edge < 0) {
          pair = [a, b];
          break outer;
        }
      }
    }
    if (!pair) {
      skip(
        "HoleToHole",
        "overlapping pair is not two vias (a pad drill is involved, or it was already resolved earlier this pass) — component moves are out of scope",
        v.message,
      );
      continue;
    }
    const [a, b] = pair;
    if (a.net !== b.net) {
      skip("HoleToHole", "overlapping drills belong to different nets — not mechanically safe", v.message);
      continue;
    }
    // Prefer deleting autorouted copper; hand-placed vias survive when possible.
    const victim = b.source === "manual" && a.source !== "manual" ? a : b;
    if (dryRun) {
      planned.push({
        rule: "HoleToHole",
        action: "delete_via",
        detail: `dedupe overlapping same-net via at (${victim.position.x}, ${victim.position.y})`,
        net: victim.net,
        position: victim.position,
      });
      budget--;
      continue;
    }
    const boundsV = boundsOfPoints(
      [a.position, b.position],
      Math.max(a.diameter, b.diameter),
    );
    const res = await applyVerified(boundsV, () => {
      pcb.vias = pcb.vias.filter((x) => x !== victim);
    });
    if (res.ok) {
      fixed.push({
        rule: "HoleToHole",
        action: "delete_via",
        detail: `removed duplicate same-net via (net '${victim.net}', ${res.delta.resolved} violation(s) resolved)`,
        net: victim.net,
        position: victim.position,
      });
      budget--;
    } else {
      skip("HoleToHole", `dedupe reverted — ${res.reason}`, v.message);
    }
  }

  // -- 2) UnstitchedPad: drop a stitching via along the escape vector ---------
  for (const v of viols.filter((x) => x.rule === "UnstitchedPad")) {
    if (budget <= 0) break;
    const [net] = parseNetPair(v.message);
    if (!net || !v.position) {
      skip("UnstitchedPad", "violation carries no net/position", v.message);
      continue;
    }
    // Message format (drc.rs): "... escaping ~{mag}mm along ({ex}, {ey}) ..."
    const m = /escaping ~(-?[\d.]+)mm along \((-?[\d.]+), (-?[\d.]+)\)/.exec(v.message);
    const candidates: Vec2[] = [];
    if (m) {
      const mag = Number(m[1]);
      const escaped = {
        x: v.position.x + mag * Number(m[2]),
        y: v.position.y + mag * Number(m[3]),
      };
      if (onBoard(escaped, pcb.outline)) candidates.push(escaped);
    }
    // At-pad fallback: a via in the pad is legal on many stackups; the delta
    // check rejects it where it isn't.
    candidates.push(v.position);
    const diameter = pcb.rules.defaultRules.viaDiameter;
    const drill = pcb.rules.defaultRules.viaDrill;
    if (dryRun) {
      planned.push({
        rule: "UnstitchedPad",
        action: "add_via",
        detail: `stitching via for plane net '${net}' near (${round3(candidates[0]!.x)}, ${round3(candidates[0]!.y)})`,
        net,
        position: candidates[0],
      });
      budget--;
      continue;
    }
    let done = false;
    let lastReason = "";
    for (const cand of candidates) {
      const pos = { x: round3(cand.x), y: round3(cand.y) };
      const res = await applyVerified(boundsOfPoints([pos], diameter / 2), () => {
        // Tagged manual: a stitching via must survive route_nets rip-up.
        pcb.vias = [
          ...pcb.vias,
          { position: pos, diameter, drill, startLayer: "FCu", endLayer: "BCu", net, source: "manual" } as Via,
        ];
      });
      if (res.ok) {
        fixed.push({
          rule: "UnstitchedPad",
          action: "add_via",
          detail: `stitching via for plane net '${net}' (${res.delta.resolved} violation(s) resolved)`,
          net,
          position: pos,
        });
        budget--;
        done = true;
        break;
      }
      lastReason = res.reason;
    }
    if (!done) {
      skip("UnstitchedPad", `no legal via site at the pad or along the escape vector — ${lastReason}`, v.message);
    }
  }

  // -- 3) EdgeClearance: nudge copper inward when a legal corridor exists -----
  const NUDGE_MARGIN = 0.05;
  for (const v of viols.filter((x) => x.rule === "EdgeClearance")) {
    if (budget <= 0) break;
    const kindMatch = /^(Trace|Via) net '([^']+)'/.exec(v.message);
    if (!kindMatch || !v.position) {
      skip("EdgeClearance", "unrecognized edge-clearance subject", v.message);
      continue;
    }
    const kind = kindMatch[1]!;
    const net = kindMatch[2]!;
    const required = typeof v.required === "number" ? v.required : pcb.rules.edgeClearance;
    const deficit = required - (typeof v.actual === "number" ? v.actual : 0) + NUDGE_MARGIN;

    // Points to move: a via center, or the too-close endpoint(s) of the trace
    // whose midpoint the kernel reported.
    let halfWidth = 0;
    const movePoints: Vec2[] = [];
    if (kind === "Via") {
      const via = pcb.vias.find((x) => x.net === net && samePoint(x.position, v.position!));
      if (!via) {
        skip("EdgeClearance", "via no longer at the reported position (already fixed this pass?)", v.message);
        continue;
      }
      halfWidth = via.diameter / 2;
      movePoints.push(via.position);
    } else {
      const trace = pcb.traces.find(
        (t) =>
          t.net === net &&
          samePoint({ x: (t.start.x + t.end.x) / 2, y: (t.start.y + t.end.y) / 2 }, v.position!),
      );
      if (!trace) {
        skip("EdgeClearance", "trace no longer at the reported position (already fixed this pass?)", v.message);
        continue;
      }
      halfWidth = trace.width / 2;
      for (const pt of [trace.start, trace.end]) {
        const nb = nearestBoundary(pt, pcb.outline);
        if (nb && nb.dist - halfWidth < required) movePoints.push(pt);
      }
      if (movePoints.length === 0) {
        skip("EdgeClearance", "could not localize the offending trace endpoint", v.message);
        continue;
      }
    }

    // Compute the inward nudge per point; bail if any point has no legal spot.
    const moves: Array<{ from: Vec2; to: Vec2 }> = [];
    let corridorFail: string | null = null;
    for (const pt of movePoints) {
      // A pad at this point means the copper lands on a component — moving it
      // would disconnect the pad, and moving the component is out of scope.
      const onPad = pcb.footprints.some((fp) =>
        fp.pads.some((pad) => samePoint(padWorld(fp, pad), pt)),
      );
      if (onPad) {
        corridorFail = "endpoint lands on a component pad — fixing requires moving the component";
        break;
      }
      const nb = nearestBoundary(pt, pcb.outline);
      if (!nb || nb.dist < 1e-9) {
        corridorFail = "no inward direction from the board edge";
        break;
      }
      const dir = { x: (pt.x - nb.point.x) / nb.dist, y: (pt.y - nb.point.y) / nb.dist };
      const to = { x: round3(pt.x + dir.x * deficit), y: round3(pt.y + dir.y * deficit) };
      const after = nearestBoundary(to, pcb.outline);
      if (!onBoard(to, pcb.outline) || !after || after.dist - halfWidth < required) {
        corridorFail = "no legal corridor inward (target point still violates edge clearance)";
        break;
      }
      moves.push({ from: pt, to });
    }
    if (corridorFail) {
      skip("EdgeClearance", corridorFail, v.message);
      continue;
    }
    if (dryRun) {
      planned.push({
        rule: "EdgeClearance",
        action: kind === "Via" ? "move_via" : "nudge_trace",
        detail: `nudge ${moves.length} point(s) inward by ~${round3(deficit)}mm`,
        net,
        position: v.position,
      });
      budget--;
      continue;
    }
    // Move every same-net copper endpoint coincident with each point together,
    // so junctions (trace↔trace, trace↔via) stay connected.
    const allPts = moves.flatMap((mv) => [mv.from, mv.to]);
    const res = await applyVerified(
      boundsOfPoints(allPts, halfWidth) as { min: Vec2; max: Vec2 } | "full",
      () => {
        const remap = (p: Vec2, elNet: string): Vec2 => {
          if (elNet !== net) return p;
          const mv = moves.find((x) => samePoint(x.from, p));
          return mv ? mv.to : p;
        };
        pcb.traces = pcb.traces.map((t) => {
          const ns = remap(t.start, t.net);
          const ne = remap(t.end, t.net);
          return ns === t.start && ne === t.end ? t : { ...t, start: ns, end: ne };
        });
        pcb.vias = pcb.vias.map((via) => {
          const np = remap(via.position, via.net);
          return np === via.position ? via : { ...via, position: np };
        });
      },
    );
    if (res.ok) {
      fixed.push({
        rule: "EdgeClearance",
        action: kind === "Via" ? "move_via" : "nudge_trace",
        detail: `nudged net '${net}' inward by ~${round3(deficit)}mm (${res.delta.resolved} violation(s) resolved)`,
        net,
        position: v.position,
      });
      budget--;
    } else {
      skip("EdgeClearance", `nudge reverted — ${res.reason}`, v.message);
    }
  }

  // -- 4) UnconnectedNet / NetIslands: rip-up + re-route the net --------------
  const rerouteNets: string[] = [];
  for (const v of viols) {
    if (v.rule !== "UnconnectedNet" && v.rule !== "NetIslands") continue;
    const [net] = parseNetPair(v.message);
    if (net && !rerouteNets.includes(net)) rerouteNets.push(net);
  }
  for (const net of rerouteNets) {
    if (budget <= 0) break;
    if (pcb.traces.some((t) => t.net === net && t.source === "manual")) {
      skip(
        "UnconnectedNet",
        `net '${net}' carries hand-placed copper — route_nets preserves it wholesale; delete the manual copper first`,
      );
      continue;
    }
    if (dryRun) {
      planned.push({
        rule: "UnconnectedNet",
        action: "reroute_net",
        detail: `rip-up + re-route net '${net}' at effort ${effort}`,
        net,
      });
      budget--;
      continue;
    }
    const savedTraces = pcb.traces;
    const savedVias = pcb.vias;
    const cap = await beginDrcDelta(pcb, "full");
    const routeArgs: Record<string, unknown> = ctx.documentId
      ? { document_id: ctx.documentId, nets: [net], effort }
      : { document: ctx.doc, nets: [net], effort };
    const routeResult = (await routeNets(routeArgs)) as {
      content: Array<{ text?: string }>;
      isError?: boolean;
    };
    if (routeResult.isError) {
      pcb.traces = savedTraces;
      pcb.vias = savedVias;
      skip("UnconnectedNet", `re-route of net '${net}' failed — ${routeResult.content[0]?.text ?? "unknown error"}`);
      continue;
    }
    const delta = await cap.finish();
    if (delta.unverifiable || delta.introduced > 0 || delta.resolved === 0) {
      pcb.traces = savedTraces;
      pcb.vias = savedVias;
      const reason = delta.unverifiable
        ? `verification failed: ${delta.unverifiable.reason}`
        : delta.introduced > 0
          ? `re-route would introduce ${delta.introduced} new violation(s)`
          : "re-route did not resolve the connectivity fault";
      skip("UnconnectedNet", `re-route of net '${net}' reverted — ${reason}`);
      continue;
    }
    fixed.push({
      rule: "UnconnectedNet",
      action: "reroute_net",
      detail: `re-routed net '${net}' at effort ${effort} (${delta.resolved} violation(s) resolved)`,
      net,
    });
    budget--;
  }

  // -- 5) Everything else: never touched, with the reason on record -----------
  for (const v of viols) {
    const reason = FIX_DRC_NEVER[v.rule];
    if (reason) skip(v.rule, reason, v.message);
  }

  // Final fail-closed accounting: the after snapshot is the receipt's anchor.
  const after = dryRun ? before : await drcPcb(pcb, "full", 20);
  const brief = (s: DrcSummary) => ({
    violations: s.violations,
    errors: s.errors,
    warnings: s.warnings,
    by_rule: s.byRule,
    categories: s.categories,
  });
  const payload = {
    success: true,
    // Verified iff DRC ran before AND after — an unverifiable after-state
    // means the fixes cannot be certified, regardless of per-fix deltas.
    verified: after.success,
    ...(dryRun ? { dry_run: true } : {}),
    before: brief(before),
    after: after.success ? brief(after) : null,
    ...(after.success ? {} : { unverified_reason: (after as DrcUnverifiable).reason }),
    fixed,
    ...(dryRun ? { planned } : {}),
    skipped,
    fix_count: dryRun ? planned.length : fixed.length,
    skip_count: skipped.reduce((n, s) => n + s.count, 0),
    ...docResultPayload(ctx),
  };
  return { content: [{ type: "text" as const, text: JSON.stringify(payload) }] };
}

/**
 * The ECAD tool surface as a single `ToolDef[]`, assembled by `server.ts` into
 * the ListTools response and the name→def dispatch Map. Descriptions are
 * byte-identical to the former inline ListTools literals (a fixture test
 * asserts this).
 */
export const toolDefs: ToolDef[] = [
  {
    name: "create_schematic",
    pack: "ecad",
    description:
      "Create a schematic from components plus connectivity, and open it " +
      "as a server-side session. Declare connectivity as data with `nets` " +
      '({"PHA": ["L1.1", "J1.1"]}) — more reliable than wire/label ' +
      "coordinates. Returns a document_id for place_components / " +
      "route_nets / export_gerber, plus the resolved netlist so broken " +
      "connectivity is visible immediately.",
    inputSchema: createSchematicSchema,
    handler: (a) => createSchematic(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "place_components",
    pack: "ecad",
    description:
      "Create the board and place schematic components on it. Mutates the " +
      "session document (pass document_id). Outline: rectangle " +
      "(board_width/height), circle with optional center bore " +
      "(board_shape — e.g. a motor stator), or any polygon (outline, e.g. " +
      "from board_from_solid). strategy=radial rings components for " +
      "annular boards. Returns `placement_drc` — the pre-routing DRC subset " +
      "(shorts, pad clearance, courtyard overlaps, off-board parts); when " +
      "`placement_drc.clean` is false, fix the floorplan with set_placement " +
      "before route_nets instead of routing on top of the fault. " +
      "`placement_drc.layout_lint` adds heuristic EE warnings (crystal far " +
      "from its oscillator pins, decoupling cap far from its supply pin, " +
      "connector off the board edge, high-current pads crowding USB/analog) " +
      "with refs and distances. Also " +
      "returns a `utilization` report (board vs occupied area, % used, " +
      "component bounding box, and an advisory suggested_outline) so you can " +
      "right-size an over-large board in one step.",
    inputSchema: placeComponentsSchema,
    handler: (a) => placeComponents(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
  {
    name: "route_nets",
    pack: "ecad",
    description:
      "Route electrical nets on the PCB with copper traces. Connects pads " +
      "belonging to the same net. Idempotent: re-running rips up the " +
      "previously autorouted copper on the target nets before routing, so " +
      "a second call replaces the route instead of stacking shorts. " +
      "Hand-placed copper (add_trace / add_via / coils) is preserved " +
      "automatically; `locked_nets` additionally protects whole nets. A " +
      "net with a copper-pour zone (a plane) is connected by stitching " +
      "each pad to the plane with a via instead of tracing it — those " +
      "nets come back in `plane_stitched`. Mutates the session document " +
      "(pass document_id).",
    inputSchema: routeNetsSchema,
    handler: (a, c) => routeNets(a, c) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "add_coil",
    pack: "ecad",
    description:
      "Add a spiral copper coil (Archimedean) to the PCB \u2014 the primitive for PCB-motor stators and planar inductors. Generates the trace geometry on a layer, assigns it to a net, validates turn-to-turn clearance, and optionally drops a via at the (otherwise trapped) inner endpoint. Returns endpoints, copper length, and a DC resistance estimate. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addCoilSchema,
    handler: (a) => addCoil(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "add_coil_array",
    pack: "ecad",
    description:
      "Lay a ring of `count` spiral coils evenly around `center` at `pitch_radius` \u2014 the placement primitive for a PCB-motor stator. Net per coil comes from `net_sequence` (cycled); `chirality` sets winding sense. GEOMETRY ONLY: it has no notion of phases \u2014 derive correct per-coil phase/polarity with `winding_layout` first, then map it onto net_sequence/chirality. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addCoilArraySchema,
    handler: (a) => addCoilArray(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "winding_layout",
    pack: "ecad",
    description:
      "Plan a balanced polyphase motor winding (slots + poles → per-coil " +
      "phase, polarity, winding factor, feasibility) as DATA. Pure — it " +
      "does NOT take a board or modify anything; inspect the plan, then " +
      "realize it with add_coil_array/add_coil. Catches infeasible " +
      "slot/pole combos and wrong polarity before any copper is drawn.",
    inputSchema: windingLayoutSchema,
    handler: (a) => windingLayout(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "board_from_solid",
    pack: "ecad",
    description:
      "Derive a PCB outline polygon (with cutouts, e.g. a center bore) " +
      "from a solid part in a CAD session by projecting its geometry onto " +
      "the XY plane. Bridges solid modeling and PCB layout: feed the " +
      "returned `outline` to place_components.",
    inputSchema: boardFromSolidSchema,
    handler: (a, c) =>
      boardFromSolid(a, c.engine) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "solid_from_board",
    pack: "ecad",
    description:
      "Materialize the session PCB as a solid CAD part — the inverse of " +
      "board_from_solid. Extrudes the board outline to its thickness " +
      "through the hole-aware sketch path (bore/cutout polygons survive " +
      "as real holes), adds simplified per-component keep-out volumes " +
      "(kernel 3D body extents, else courtyard × package-class height), " +
      "and gives the substrate a homogenized FR4+copper density so " +
      "inspect_cad / physics see the real board mass. Inject into an " +
      "existing CAD session with `document_id_target` (enclosure fit, " +
      "motor stacks, clash checks) or omit it to mint a fresh session.",
    inputSchema: solidFromBoardSchema,
    // Cross-session writer: reads the PCB in `document_id`, writes the CAD
    // session in `document_id_target` (or mints one). The central
    // hydrate/snapshot/persist plumbing keys off args.document_id — here the
    // read-only PCB source — so the target session is hydrated, snapshotted,
    // persisted, and event-logged here instead.
    handler: async (args, ctx) => {
      const targetId =
        typeof args.document_id_target === "string" ? args.document_id_target : null;
      if (targetId) {
        try {
          await hydrateSession(ctx.sessionStore, targetId);
        } catch {
          // Durable load failed — fall back to cache.
        }
        if (documents.has(targetId)) recordHistorySnapshot(targetId);
      }
      const result = (await solidFromBoard(args)) as ToolResult;
      const writtenId = result.structuredContent?.document_id;
      if (!result.isError && typeof writtenId === "string" && documents.has(writtenId)) {
        try {
          await persistSession(ctx.sessionStore, writtenId);
        } catch {
          // best-effort durable write
        }
        try {
          await ctx.eventStore.append(writtenId, {
            author: ctx.user?.sub ?? "agent",
            kind: "kernel",
            type: "solid_from_board",
            payload: buildKernelEventPayload("solid_from_board", args, result),
          });
        } catch {
          // best-effort event append
        }
      }
      return result;
    },
    behavior: behavior({}),
  },
  {
    name: "list_footprints",
    pack: "ecad",
    description:
      "List the footprint families the parametric engine resolves, each " +
      "with a canonical example id to drop into create_schematic's " +
      "`footprint`. Optional `kind` filter (passive/ic/transistor/diode/" +
      "power/connector). Use this instead of guessing id spellings.",
    inputSchema: listFootprintsSchema,
    handler: (a) => listFootprints(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "search_footprints",
    pack: "ecad",
    description:
      "Fuzzy-search footprint families by name/alias (e.g. 'SOIC 8', " +
      "'jst', 'qfn') and get ranked matches with a canonical example id — " +
      "resolve a footprint id without a failed create_schematic round-trip.",
    inputSchema: searchFootprintsSchema,
    handler: (a) => searchFootprints(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "get_pad_positions",
    pack: "ecad",
    description:
      "Return every footprint pad's absolute board-frame (x, y), copper " +
      "layer, and net — the coordinates manual routing (add_trace / " +
      "add_via / add_via_array) needs so trace endpoints land exactly on " +
      "pads instead of being eyeballed from component centers. Read-only. " +
      "Optional `net` / `ref` filters narrow the result for targeted routing.",
    inputSchema: getPadPositionsSchema,
    handler: (a) => getPadPositions(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "get_footprint",
    pack: null,
    description:
      "Introspect ONE footprint's land pattern in BOTH the footprint-local " +
      "and board frames — origin, courtyard AABB, and every pad (with the " +
      "explicit rotation convention) — so connector/IC pad locations are " +
      "known exactly instead of render-and-guessed. Two modes: `ref` reads " +
      "a placed footprint (real transform + nets) from the session; " +
      "`footprint` resolves an id PRE-placement (pass `at`/`rotation`/" +
      "`side` to project a hypothetical placement). Read-only.",
    inputSchema: getFootprintSchema,
    handler: (a) => getFootprint(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "describe_pcb",
    pack: "ecad",
    description:
      "Inspect the session PCB as compact, structured data: board size + " +
      "outline, stackup (layer names + copper weights), net classes / " +
      "design rules, zones (net/layer/bbox/fill), trace & via counts by net " +
      "and layer, component count, the current DRC status, and an " +
      "exportability/renderability probe that actually serializes the board " +
      "for fab + 3D preview — surfacing the 'DRC-clean but unexportable' " +
      "state get_document/read can't see. Read-only.",
    inputSchema: describePcbSchema,
    handler: (a) => describePcb(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "add_trace",
    pack: "ecad",
    description:
      "Lay an explicit copper trace: a polyline of segments on a layer, assigned to a net. The general-purpose routing primitive \u2014 use it for coil interconnect, buses, and hand-routes that route_nets (pad-driven) won't make. Tagged as manual copper, so route_nets preserves it instead of ripping it up. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addTraceSchema,
    handler: (a) => addTrace(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "add_via",
    pack: "ecad",
    description:
      "Drop a via at a point connecting two layers on a net (defaults FCu\u2192BCu, diameter/drill from design rules). Pairs with add_trace for multi-layer routing. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addViaSchema,
    handler: (a) => addVia(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "set_stackup",
    pack: "ecad",
    description:
      "Set the board stackup copper weight (e.g. copper_oz: 2) and/or per-layer thickness/material, so DC-resistance and impedance estimates reflect the real fab stackup instead of a default 1 oz. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: setStackupSchema,
    handler: (a) => setStackup(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "set_placement",
    pack: "ecad",
    description:
      "Place footprints at explicit board-frame coordinates by ref — the " +
      "floorplan realizer the auto-placer (grid/force_directed/radial) can't " +
      "express: thermal rings, a quiet IMU corner, rim connectors. Batch; " +
      "sets position/rotation/side and warns on off-board, in-cutout, or " +
      "stacked landings. Mutates the session document. Returns the updated " +
      "`placement_drc` (same shape as place_components) so a move can be " +
      "re-checked in one call without running run_drc.",
    inputSchema: setPlacementSchema,
    handler: (a) => setPlacement(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "set_board_outline",
    pack: "ecad",
    description:
      "Resize or reshape the board outline in place \u2014 rectangle (board_width/height), circle/annulus (board_shape), or any polygon (outline) \u2014 WITHOUT re-placing components, traces, vias, or zones. Unlike re-running place_components, the floorplan is preserved; any footprint whose origin ends up off the new board is reported in `off_board` rather than silently relocated. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: setBoardOutlineSchema,
    handler: (a) => setBoardOutline(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "add_zone",
    pack: "ecad",
    description:
      "Add a copper pour (ground/power plane) on a net+layer — fills are not " +
      "traces. `fill_board:true` pours the whole outline (cutouts become " +
      "voids); or give an explicit polygon for a partial plane. Mutates the " +
      "session document.",
    inputSchema: addZoneSchema,
    handler: (a) => addZone(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "delete_zone",
    pack: "ecad",
    description:
      "Remove a copper pour from the board \u2014 the take-back for a bad add_zone, without rebuilding the session. Target by `index` (0-based, the add order) or by `net`/`layer` when exactly one zone matches. Returns a `changed` diff of what was removed. To undo the very last mutation of any kind, use `undo` instead. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: deleteZoneSchema,
    handler: (a) => deleteZone(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "delete_trace",
    pack: "ecad",
    description:
      "Remove a single routed trace segment by `index` (0-based, the add order) or by an unambiguous `net`/`layer` match. The take-back for a stray add_trace. Returns a `changed` diff. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: deleteTraceSchema,
    handler: (a) => deleteTrace(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "delete_via",
    pack: "ecad",
    description:
      "Remove a single via by `index` (0-based, the add order) or by an unambiguous `net` match. The take-back for a stray add_via. Returns a `changed` diff. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: deleteViaSchema,
    handler: (a) => deleteVia(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "get_copper",
    pack: "ecad",
    description:
      "Query the board's routed copper — traces, trace arcs, vias, zones — " +
      "with optional `layer`/`net`/`bbox`/`kind` filters. Each element comes " +
      "back with its `kind` + `index`: exactly the addressing delete_trace / " +
      "delete_via / delete_zone accept, so a query can drive a surgical " +
      "delete without exporting the document. describe_pcb aggregates " +
      "counts; this returns the elements themselves (geometry, width, net, " +
      "layer), capped at 200 per page with `offset` pagination and a `total`. " +
      "Read-only.",
    inputSchema: getCopperSchema,
    handler: (a) => getCopper(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "add_net_tie",
    pack: "ecad",
    description:
      "Declare an intentional junction between >= 2 nets (a net-tie) so DRC treats them as one node where they meet \u2014 required for wye/star motor neutrals, split grounds (GND+AGND), and current-sense shunt taps, which are otherwise reported as shorts. With `position`+`radius` the tie is region-scoped: clearance/short checks are exempt only for contacts inside the region, and connectivity accepts nets joined through copper when each has a tie-covered contact there \u2014 a stray crossing of the same nets elsewhere still fires. Without them the exemption is board-wide (prefer scoped: it keeps DRC honest away from the junction). Nets must exist on the board. Returns the updated tie list with indices. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced or resolved (a tie edit changes short/clearance exemptions) with `clean` to branch on in one step.",
    inputSchema: addNetTieSchema,
    handler: (a) => addNetTie(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "delete_net_tie",
    pack: "ecad",
    description:
      "Remove a net tie by `index`, or by matching `nets` (set equality, order-insensitive) and/or `position` \u2014 the take-back for a bad add_net_tie. Any junction copper stays on the board; DRC will report it as a short again. Returns the deleted tie and the updated tie list. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced or resolved (a tie edit changes short/clearance exemptions) with `clean` to branch on in one step.",
    inputSchema: deleteNetTieSchema,
    handler: (a) => deleteNetTie(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "undo",
    // Always-on core (#442): undo serves every session type, so it must not
    // disappear when the ecad pack is disabled.
    pack: null,
    description:
      "Rewind the most recent mutation on a session — the snapshot taken " +
      "before the last add_zone / add_trace / add_via / delete_* / route_nets " +
      "/ place_components (or a CAD create/update/delete) is restored, without " +
      "re-sending the document. Repeated calls walk further back. Returns a " +
      "`changed` diff of the board elements the rewind moved.",
    inputSchema: undoSchema,
    handler: (a) => undo(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "set_design_rules",
    pack: "ecad",
    description:
      "Set the board design rules run_drc enforces (clearance, track width, " +
      "via, edge/hole/annular) and net classes — the way to give a power or " +
      "high-voltage class wider clearance than signal nets. run_drc already " +
      "reads pcb.rules; this writes them. Mutates the session document.",
    inputSchema: setDesignRulesSchema,
    handler: (a) => setDesignRules(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "size_trace_for_current",
    pack: "ecad",
    description:
      "IPC-2221 conductor ampacity solved for trace width: given current, " +
      "copper weight, allowed temp rise, and layer (outer/inner), returns the " +
      "minimum width. The ampacity sibling of size_impedance/size_pdn — pure " +
      "calc, no document.",
    inputSchema: sizeTraceForCurrentSchema,
    handler: (a) =>
      sizeTraceForCurrent(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "add_via_array",
    pack: "ecad",
    description:
      "Place many vias at once \u2014 a grid over a rectangular `region` (thermal vias under FETs, GND-plane stitching) or an explicit `points` list. Grid vias are clipped to the board outline by default. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addViaArraySchema,
    handler: (a) => addViaArray(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "add_motor_winding",
    pack: "ecad",
    description:
      "One-shot motor winding realizer: plans a balanced slots/poles/phases winding, drops a spiral coil per tooth with correct phase + polarity, series-connects each phase with planar staggered-radius arcs in the coil-free bore (never crossing), ties the wye/delta termination with a region-scoped net-tie on real board material, and routes phase feeds to same-net pads when present \u2014 closing the winding_layout plan into DRC-clean copper. Mutates the session document. Verify-on-write: the result carries `drc_delta` \u2014 the DRC violations this call introduced (shorts/clearance/connectivity/manufacturing, capped sample with positions) with `clean` to branch on in one step.",
    inputSchema: addMotorWindingSchema,
    handler: (a) => addMotorWinding(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "calc_motor",
    pack: "ecad",
    description:
      "Evaluate motor performance AS DATA. mode:'pm' (default): torque " +
      "constant Kt, back-EMF constant Ke, no-load speed, stall torque, " +
      "and a speed–torque curve; supply air-gap flux directly or compute " +
      "it from magnet geometry via the first-order MEC field model, with " +
      "an optional Carter-like fringing derate (magnet.pole_width_mm). " +
      "mode:'induction': thin-sheet axial induction rotor (drag-cup / " +
      "PCB cage) — gap field B1, torque-per-unit-slip, locked-rotor " +
      "torque, sync speed, rotor sheet loss. Pure: no board, no " +
      "mutation. First-order steady state.",
    inputSchema: calcMotorSchema,
    handler: (a) => calcMotor(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "check_self_start",
    pack: "ecad",
    description:
      "Will it spin? Starting torque (direct, or Kt·I; induction: the " +
      "locked-rotor torque from calc_motor) vs a friction estimate " +
      "(direct, or the built-in bearing catalog: 608-2RS/608-ZZ/625/688 " +
      "× light/medium preload × count). Returns starts (fail-closed vs " +
      "worst-case friction), best-case verdict, and the margin. Pure " +
      "calc, no document.",
    inputSchema: checkSelfStartSchema,
    handler: (a) => checkSelfStart(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "run_drc",
    pack: "ecad",
    description:
      "Run Design Rule Check (DRC) on a PCB. Checks clearance, trace width, drill size, annular ring, hole-to-hole, edge clearance, and connectivity \u2014 including SameNetBypass, a warning when same-net copper touches far from any intended junction (e.g. a trace over a coil's inner via, short-circuiting the spiral). Every violation is tagged with `provenance` (intra_footprint / inter_component / routing) and `generated` (involves a synthesized footprint land pattern); the summary adds `byProvenance`, `generatedArtifacts`, and `realViolations` so the headline count excludes footprint artifacts without hand-triage.",
    inputSchema: runDrcSchema,
    handler: (a) => runDrc(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "fix_drc",
    pack: "ecad",
    description:
      "Run DRC and automatically apply the mechanically-safe subset of fixes: " +
      "stitching vias for UnstitchedPad, dedupe of overlapping same-net via " +
      "drills (HoleToHole), inward nudges for EdgeClearance when a legal " +
      "corridor exists, and per-net rip-up + re-route (at higher effort) for " +
      "UnconnectedNet/NetIslands. Every fix is individually verified with a " +
      "DRC delta and REVERTED if it would introduce any new violation. Never " +
      "touches design decisions: different-net shorts/clearance, courtyard " +
      "overlaps, keepouts, or anything requiring a component move. Returns a " +
      "fail-closed receipt: before/after violation counts, what was fixed, " +
      "and what was skipped with the reason. `dry_run` plans without " +
      "mutating. Mutates the session document (pass document_id).",
    inputSchema: fixDrcSchema,
    handler: (a) => fixDrc(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "search_electronic_parts",
    pack: "ecad",
    description:
      "Spec-search the generative parts catalog (offline). A query like " +
      "'10k 0603 1%' parses to value+package+tolerance and returns the best " +
      "match plus E-series neighbours, each with a generated footprint, symbol, " +
      "and 3D body. A part is family+value+package, not a scraped row.",
    inputSchema: searchElectronicPartsSchema,
    handler: (a) =>
      searchElectronicParts(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "resolve_part",
    pack: "ecad",
    description:
      "Resolve a spec query (e.g. '10k 0603 1%') into ONE fully-specified part: " +
      "E-series-snapped value plus a generated footprint + schematic symbol + 3D " +
      "body (one parametric source of truth) and any MPN cross-references.",
    inputSchema: resolvePartSchema,
    handler: (a) => resolvePart(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "find_alternatives",
    pack: "ecad",
    description:
      "Propose spec-compatible substitutes for the part a query resolves to. " +
      "Each alternative keeps the value, varies the package, and is labelled " +
      "identical / needs-reroute / incompatible by re-deriving its footprint.",
    inputSchema: findAlternativesSchema,
    handler: (a) => findAlternatives(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "verify_substitution",
    pack: "ecad",
    description:
      "PROVE a part swap on the session PCB: replace `reference` with the part " +
      "`candidate` resolves to, re-derive its footprint, re-place at the same " +
      "anchor, re-run DRC (incl. connectivity), and return the before/after " +
      "violation delta with a `drop_in` verdict. An alternative is only drop-in " +
      "when it adds no new violations and preserves pin numbering.",
    inputSchema: verifySubstitutionSchema,
    handler: (a) =>
      verifySubstitution(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "build_receipt",
    pack: "ecad",
    description:
      "Build a re-runnable verification Receipt for the session PCB: a content hash, the DRC backend, a canonicalized DRC summary, and per-part provenance \u2014 a durable proof that round-trips and re-verifies later as Holds / Stale / Violated. Persisted clearance specs (check_clearance with a label) join the unified ledger as mech.clearance claims \u2014 a CAD-only session with specs gets a mechanical receipt, no PCB needed. Renders as an audit ledger in the inline viewer.",
    inputSchema: buildReceiptSchema,
    handler: (a, c) =>
      buildReceipt(a, c.engine) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ mount: true }),
  },
  {
    name: "verify_receipt",
    pack: "ecad",
    description:
      "Re-run a prior receipt (from build_receipt) against the session's current document and return the verdict \u2014 Holds (unchanged, clean), Stale (changed but claims still hold), or Violated. Covers the PCB Receipt (board hash + DRC diff) and mech.clearance claims (re-measured against current geometry); worst verdict wins. Powers the ledger's Re-run button.",
    inputSchema: verifyReceiptSchema,
    handler: (a) => verifyReceipt(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ widgetCallable: true }),
  },
  {
    name: "route_diff_pair",
    pack: null,
    description:
      "Route a declared differential pair (net_p/net_n) coupled and length-matched, " +
      "using the pair's diff-pair net-class gap and width. Routes straight (best on a " +
      "clear channel); verify with run_drc / critique_route afterwards.",
    inputSchema: routeDiffPairSchema,
    handler: (a) => routeDiffPair(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "length_match_traces",
    pack: null,
    description:
      "Length-match a group of routed nets (a DDR lane, clock tree, SPI bus): " +
      "measures each net's copper and grows the shorter ones with clearance-" +
      "checked trombone/sawtooth meanders until all reach the longest net (or " +
      "target_length) within tolerance. Nets it can't tune (branching, multi-" +
      "layer, arcs, no room) are reported with a reason, never guessed at. " +
      "check_only:true measures and verdicts without touching copper. Mutating " +
      "runs carry drc_delta.",
    inputSchema: lengthMatchTracesSchema,
    handler: (a) => lengthMatchTraces(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "critique_route",
    pack: null,
    description:
      "Audit one net's routing without changing anything: total length, via/" +
      "layer-change count, the closest approach to other-net copper, and any " +
      "clearance/short/unconnected DRC issues it's in. Inspect a route before trusting it.",
    inputSchema: critiqueRouteSchema,
    handler: (a) => critiqueRoute(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "run_erc",
    pack: "ecad",
    description:
      "Run Electrical Rule Check (ERC) on a schematic. " +
      "Checks for duplicate references, unconnected pins, and pin type conflicts.",
    inputSchema: runErcSchema,
    handler: (a) => runErc(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "export_gerber",
    pack: "ecad",
    description:
      "Export Gerber RS-274X fabrication files from a PCB design. " +
      "Generates copper layer files, drill file, pick-and-place CSV, and BOM. " +
      "Gated on a clean DRC by default (require_clean_drc) — a dirty or " +
      "unverifiable board is BLOCKED with its DRC summary instead of emitting " +
      "an invalid bundle. Run validate_for_fab first for the full readiness verdict.",
    inputSchema: exportGerberSchema,
    handler: (a) => exportGerber(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "export_kicad",
    pack: null,
    description:
      "Export the session as a native, editable KiCad 9 file. " +
      "filename ending in .kicad_pcb writes the board (footprints, pads, nets, " +
      "traces, vias, zones, layers, outline); .kicad_sch writes the schematic; " +
      ".kicad_pro (or a bare name) writes a linked project bundle (.kicad_pro + " +
      ".kicad_sch + .kicad_pcb) with footprints tied to their schematic symbols " +
      "for cross-probing. " +
      "Unlike export_gerber (fab-only output), this round-trips: a human can open " +
      "it in KiCad to finish routing nets the autorouter couldn't close, then " +
      "re-import. Large files respect the inline byte cap (use output_dir for those).",
    inputSchema: exportKicadSchema,
    handler: (a) => exportKicad(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "fab_prep",
    pack: "ecad",
    description:
      "Take a routed board to fab-ready in one call, and return the DRC-delta " +
      "receipt that says what was actually achieved. Runs the whole pipeline: " +
      "optional rule calibration (opt-in, every change logged with its " +
      "derivation), the verdict ladder over unrouted connections (each ends " +
      "Routed / ProvedInfeasible / honest-unknown), then a strip-and-re-route " +
      "fix loop until the violations the ROUTING is answerable for reach zero, " +
      "then a dangling-copper prune. Mutates the session document. " +
      "THE RECEIPT IS THE POINT: on an imported fixture absolute zero is not " +
      "achievable — the same board stripped of all routing already violates its " +
      "own rules — so the report always gives BOTH numbers (stripped-board " +
      "baseline and finished board) and charges only the difference to the " +
      "router. Fails closed: a loop that does not converge returns " +
      "`converged:false` with the remaining offenders and does not pretend " +
      "otherwise, and `export_gerber`'s clean-DRC gate still stands — this is " +
      "the supported way to GET clean, not a way around it.",
    inputSchema: fabPrepSchema,
    handler: (a) => fabPrep(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
  {
    name: "validate_for_fab",
    pack: "ecad",
    description:
      "The single 'is this board ready to fabricate?' oracle. Runs the whole " +
      "readiness gate in one call and returns ONE structured verdict: DRC " +
      "(fail-closed — a board that won't parse is 'unverifiable', never clean), " +
      "renderability, Gerber-exportability (attempts serialization; names the " +
      "exact failing field when it can't), unsupported features, the precise " +
      "blockers, and suggested fixes. Read-only. Use before export_gerber / " +
      "quote_manufacturing to know — not guess — whether the board is shippable.",
    inputSchema: validateForFabSchema,
    handler: (a) => validateForFab(a) as ToolResult | Promise<ToolResult>,
    // mount: the readiness verdict is a moment-of-truth — put a live canvas
    // next to it instead of making the user scroll back up.
    behavior: behavior({ mount: true }),
  },
  {
    name: "calc_impedance",
    pack: "ecad",
    description:
      "Calculate trace impedance using IPC-2141 formulas. " +
      "Supports microstrip, stripline, and differential pair configurations. " +
      "Returns Z0, effective Er, and propagation delay. Pass document_id + " +
      "net to gate the number on realized copper: an impedance for a trace " +
      "that isn't actually routed/continuous is blocked, not reported.",
    inputSchema: calcImpedanceSchema,
    handler: (a) => calcImpedance(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "size_impedance",
    pack: "ecad",
    description:
      "Inverse of calc_impedance: solve trace geometry for a TARGET impedance. " +
      "Given a target Z0 (and diff Z0 for pairs) + stackup, returns the trace " +
      "width (and spacing) AS DATA, snapped to the fab grid and re-verified " +
      "against the same model. Reports a binding DFM min-width/spacing bound " +
      "and whether the target is reachable — it will not silently hand back a " +
      "width that misses spec. Pure: no board, no mutation.",
    inputSchema: sizeImpedanceSchema,
    handler: (a) => sizeImpedance(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "size_pdn",
    pack: "ecad",
    description:
      "Size copper-segment widths across a power-distribution resistor mesh " +
      "so each load node's IR-drop meets its budget with minimal copper. " +
      "Solves G·V=I for node voltages and drives drop→budget with a bounded " +
      "gradient tuner; returns per-segment widths AS DATA with drops " +
      "recomputed from a forward solve, and flags any node it can't meet " +
      "within the width bounds. Pure by default; pass document_id + net to " +
      "REFUSE a PASS when that power plane isn't galvanically continuous " +
      "(returns coverage %, stitching-via count, and the worst island).",
    inputSchema: sizePdnSchema,
    handler: (a) => sizePdn(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "calc_coil",
    pack: "ecad",
    description:
      "Analyze a planar spiral coil: inductance (modified Wheeler), DC " +
      "resistance, copper length, and L/R time constant. The analyzer for " +
      "the planar-magnetics archetype (inductors, sensor coils, motor " +
      "stators). Pure.",
    inputSchema: calcCoilSchema,
    handler: (a) => calcCoil(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "size_coil",
    pack: "ecad",
    description:
      "Inverse of calc_coil: solve the turn count for a target inductance " +
      "in a given annulus (Wheeler L ∝ turns², so it's closed-form). Reports " +
      "continuous + integer turns, the inductance achieved, and whether that " +
      "many turns fit the radial band (else fit-limited). Pure.",
    inputSchema: sizeCoilSchema,
    handler: (a) => sizeCoil(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
  {
    name: "calc_rf",
    pack: "ecad",
    description:
      "Frequency-domain (AC) analysis of an RLC resonator: sweeps complex " +
      "impedance over frequency and reports |Z|, phase, and S11/return-loss " +
      "vs a reference Z0, plus resonance, Q, and the best match in the band. " +
      "The RF/AC analyzer (calc_impedance is geometry-only). Pure.",
    inputSchema: calcRfSchema,
    handler: (a) => calcRf(a) as ToolResult | Promise<ToolResult>,
    behavior: behavior({}),
  },
];
