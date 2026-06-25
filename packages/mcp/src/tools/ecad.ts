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
  Via,
  Zone,
  Vec2,
  Receipt,
} from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getNodePcb, getPcbNodeIds, buildEntry, agentView } from "@vcad/core";
import {
  computeRatsnest,
  exportFabFiles,
  resolveFootprint,
  generateNetlist,
  isEcadAvailable,
  routeAll,
  routeDiffPair as kernelRouteDiffPair,
  critiqueRoute as kernelCritiqueRoute,
  runDrc as kernelRunDrc,
  evaluateMotor,
  airgapFluxDensity,
  resolvePart as kernelResolvePart,
  resolvePartDef as kernelResolvePartDef,
  searchEcadParts as kernelSearchPartsEcad,
  findAlternatives as kernelFindAlternatives,
  verifySubstitution as kernelVerifySubstitution,
  buildReceipt as kernelBuildReceipt,
  verifyReceipt as kernelVerifyReceipt,
} from "@vcad/engine";
import type { Engine, NetlistResult, TriangleMesh } from "@vcad/engine";
import { registerSession, getSession } from "./session.js";
import { sizePdnExact, ecadDiffEngineAvailable } from "../wasm/ecad-diff.js";

/** Get PCB data from a document — checks PcbBoard nodes first, falls back to legacy doc.pcb */
function getDocPcb(doc: Document): Pcb | null {
  const nodeIds = getPcbNodeIds(doc);
  if (nodeIds.length > 0) return getNodePcb(doc, nodeIds[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
}

/** Standard `{ content, isError }` failure result for ECAD tools. */
function ecadError(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

// ============================================================================
// Document resolution — session-based (document_id) with inline fallback
// ============================================================================

/** A resolved document input: session-backed (preferred) or inline (legacy). */
interface EcadDocCtx {
  doc: Document;
  /** Set when the doc came from (or was registered as) a server session. */
  documentId?: string;
}

/**
 * Resolve the document argument for ECAD tools. `document_id` (a session
 * from create_schematic / open_document) is preferred; an inline `document`
 * object is still accepted for backward compatibility and is mutated and
 * echoed back like the pre-session API did.
 */
function resolveDocInput(args: Record<string, unknown>): EcadDocCtx {
  const id = args.document_id ? String(args.document_id) : "";
  if (id) return { doc: getSession(id), documentId: id };
  const doc = args.document as Document | undefined;
  if (doc && typeof doc === "object") return { doc };
  throw new Error(
    "Pass `document_id` (from create_schematic or open_document) — or an inline `document` for the legacy stateless flow.",
  );
}

/**
 * The document part of a mutating tool's response. Session docs are mutated
 * server-side, so only the id is echoed; inline docs get the full mutated
 * document back (the caller has no other way to retrieve it).
 */
function docResultPayload(ctx: EcadDocCtx): Record<string, unknown> {
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
        "self-cleaning: routing rips up the prior copper on each net first and " +
        "lays a complete fresh route, so trace counts don't grow across " +
        "iterations. After a set_placement move, nets whose pads no longer sit " +
        "under their copper are detected as stale and re-routed too (even if not " +
        "listed here); the result reports `traces_removed`/`vias_removed` and " +
        "`stale_nets_cleared` so the cleanup is visible.",
    },
    locked_nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Nets whose existing copper is preserved — never ripped up or " +
        "re-routed by this call. Use for hand-routed traces/vias (e.g. a " +
        "manual via bridge, or a stitched plane net) that the autorouter's " +
        "self-cleaning would otherwise delete. Copper on these nets survives " +
        "across route_nets passes; the kernel still routes every other net.",
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
        "When true, wrap the route in a before/after DRC and return a `receipt` verdict (what it fixed, what it introduced incl. shorts, with each violation attributed to footprint vs routing) — instead of just a document_id. Routing is not idempotent; this surfaces a re-route that silently shorts the board.",
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
        "the write may fail, in which case file contents are returned inline " +
        "instead. When omitted, file contents are always returned inline.",
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

export async function createSchematic(args: Record<string, unknown>) {
  const title = (args.title as string) || undefined;
  const componentsInput = (args.components as Array<Record<string, unknown>>) || [];
  const wiresInput = (args.wires as Array<Record<string, unknown>>) || [];
  const labelsInput = (args.labels as Array<Record<string, unknown>>) || [];
  const netsInput = (args.nets as Record<string, string[]>) || undefined;

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
        ? explicitPins.map((p) => ({
            number: p.number as string,
            name: p.name as string,
            pin_type: validatePinType(
              p.type,
              `${(c.ref as string) ?? "?"}.${(p.number as string) ?? "?"}`,
            ),
            position: { x: (p.x as number) || 0, y: (p.y as number) || 0 },
          }))
        : def
          ? def.pins.map((p) => ({
              number: p.number,
              name: p.name,
              pin_type: p.pin_type as SchematicPin["pin_type"],
              position: { x: p.x, y: p.y },
            }))
          : [];

    // Carry the resolved part identity + datasheet for traceability.
    const properties: Record<string, string> = {};
    if (c.part) properties.part = c.part as string;
    if (def?.datasheet_url) properties.datasheet = def.datasheet_url;

    return {
      ref: c.ref as string,
      value: (c.value as string) || (c.part as string) || def?.name || "",
      footprintId: c.footprint as string,
      position: { x: c.x as number, y: c.y as number },
      rotation: (c.rotation as number) || 0,
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

  const wires: SchematicWire[] = wiresInput.map((w) => ({
    start: { x: w.x1 as number, y: w.y1 as number },
    end: { x: w.x2 as number, y: w.y2 as number },
  }));

  const labels: SchematicLabel[] = labelsInput.map((l) => ({
    name: l.name as string,
    position: { x: l.x as number, y: l.y as number },
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
      positions[i].x = Math.min(bounds.maxX, Math.max(bounds.minX, positions[i].x + forces[i].x));
      positions[i].y = Math.min(bounds.maxY, Math.max(bounds.minY, positions[i].y + forces[i].y));
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

  const defaultRules: NetClassRules = {
    name: "Default",
    traceWidth: 0.25,
    clearance: 0.2,
    viaDiameter: 0.8,
    viaDrill: 0.4,
  };

  const rules: DesignRules = {
    defaultRules,
    edgeClearance: 0.5,
    holeToHole: 0.5,
    minAnnularRing: 0.15,
    minDrill: 0.2,
  };

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

    if (comp.pads && comp.pads.length > 0) {
      // (1) Inline override — author-supplied geometry.
      pads = comp.pads.map((pad) => applyNet(comp, pad));
    } else if (resolution?.template) {
      // (2) Engine result — real family match or compact placeholder.
      pads = resolution.template.pads.map((pad) => applyNet(comp, pad));
      graphics = resolution.template.graphics;
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
    };
  });

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
  if (!placementDrc.clean) {
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

  const padsByNet = new Map<string, { pos: Vec2; layers: PcbLayer[] }[]>();
  for (const fp of pcb.footprints) {
    for (const pad of fp.pads) {
      if (!pad.net) continue;
      const arr = padsByNet.get(pad.net) ?? [];
      arr.push({ pos: padWorld(fp, pad), layers: pad.layers });
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

    // (1) Any loose trace endpoint?
    const anchored = (p: Vec2, selfIdx: number): boolean => {
      if (pads.some((q) => near(p, q.pos))) return true;
      if (vias.some((q) => near(p, q))) return true;
      for (let j = 0; j < traces.length; j++) {
        if (j === selfIdx) continue;
        if (near(p, traces[j].start) || near(p, traces[j].end)) return true;
      }
      return false;
    };
    let isStale = traces.some((t, i) => !anchored(t.start, i) || !anchored(t.end, i));

    // (2) Any current pad uncovered by copper (and not on a same-net pour)?
    if (!isStale) {
      const zoneLayers = zoneLayersByNet.get(net);
      isStale = pads.some((pad) => {
        if (zoneLayers && pad.layers.some((l) => zoneLayers.has(l))) return false;
        const onTrace = traces.some((t) => near(pad.pos, t.start) || near(pad.pos, t.end));
        const onVia = vias.some((v) => near(pad.pos, v));
        return !onTrace && !onVia;
      });
    }

    if (isStale) stale.add(net);
  }
  return stale;
}

/** Route nets on a PCB with the kernel autorouter (obstacle-avoiding). */
export async function routeNets(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const doc = ctx.doc;
  const traceWidth = (args.trace_width as number) || undefined;
  const netsFilter = (args.nets as string[]) || [];
  const lockedNets = new Set<string>(
    ((args.locked_nets as string[]) || []).map(String),
  );

  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  const width = traceWidth || pcb.rules.defaultRules.traceWidth;

  // Receipt: snapshot DRC before the (non-idempotent) route so the after-diff
  // can attribute exactly what this call fixed and what it introduced.
  const wantReceipt = args.receipt === true;
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

  let tracesRemoved = 0;
  let viasRemoved = 0;
  if (targetNets.size > 0) {
    const beforeT = pcb.traces.length;
    const beforeV = pcb.vias.length;
    pcb.traces = pcb.traces.filter((t) => !targetNets.has(t.net));
    pcb.vias = pcb.vias.filter((v) => !targetNets.has(v.net));
    tracesRemoved = beforeT - pcb.traces.length;
    viasRemoved = beforeV - pcb.vias.length;
  }

  const rats = await computeRatsnest(pcb, netlist);

  const routedNets = new Set<string>();
  const fallbackNets = new Set<string>();
  const unroutedNets = new Set<string>();
  let tracesAdded = 0;

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
  // locked. A locked net is neither ripped up (above) nor re-routed here, so
  // its hand-placed copper stays exactly as authored. An empty effectiveFilter
  // means "route everything", so to subtract locked nets we make the all-set
  // explicit; with no locked nets it stays empty (behavior unchanged).
  let routeFilter = effectiveFilter;
  if (lockedNets.size > 0) {
    if (effectiveFilter.length > 0) {
      routeFilter = effectiveFilter.filter((n) => !lockedNets.has(n));
    } else {
      const allRoutable = new Set<string>();
      for (const [net, conns] of netConnections) {
        if (conns.length >= 2 && !lockedNets.has(net)) allRoutable.add(net);
      }
      routeFilter = [...allRoutable];
    }
  }
  const result = await routeAll(pcb, width, routeFilter);
  const routedSomething =
    result.traces.length > 0 || result.vias.length > 0 || result.unrouted_nets.length > 0;

  if (routedSomething) {
    for (const t of result.traces) {
      pcb.traces.push({
        start: { x: t.start.x, y: t.start.y },
        end: { x: t.end.x, y: t.end.y },
        width: t.width,
        // The kernel returns the layer as a string ("FCu"/"BCu"); it is always
        // a valid PcbLayer value.
        layer: t.layer as PcbLayer,
        net: t.net,
      });
      realizedWidths[t.net] = Math.max(realizedWidths[t.net] ?? 0, t.width);
      tracesAdded++;
    }
    for (const v of result.vias) {
      pcb.vias.push({
        position: { x: v.position.x, y: v.position.y },
        diameter: pcb.rules.defaultRules.viaDiameter,
        drill: pcb.rules.defaultRules.viaDrill,
        startLayer: "FCu",
        endLayer: "BCu",
        net: v.net,
      });
    }
    for (const n of result.routed_nets) routedNets.add(n);
    for (const n of result.unrouted_nets) unroutedNets.add(n);
  } else if (rats.length === 0) {
    // No kernel at all: computeRatsnest returns [] and the auto-router is empty
    // — chain pads directly so the tool still produces connectivity (legacy
    // behavior; flagged because it may cross copper).
    for (const [netId, conns] of netConnections) {
      if (conns.length < 2) continue;
      if (lockedNets.has(netId)) continue; // never reroute a locked net
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
        });
        tracesAdded++;
      }
      routedNets.add(netId);
      fallbackNets.add(netId);
    }
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
  // is honest about the extra cleanup it did.
  const staleCleared = [...staleNets].filter((n) => !netsFilter.includes(n));
  if (netsFilter.length > 0 && staleCleared.length > 0) {
    warnings.push(
      `ripped up orphaned copper on ${staleCleared.length} net(s) whose pads moved after the last route and re-routed them: ${staleCleared.join(", ")}`,
    );
  }

  const receiptField: Record<string, unknown> = {};
  if (wantReceipt && beforeSnap) {
    const after = await drcPcb(pcb, "full", 500);
    const entry = buildEntry(
      { tool: "route_nets", args: { nets: netsFilter, trace_width: traceWidth }, before: beforeSnap, after },
      0,
    );
    receiptField.receipt = agentView(entry, ctx.documentId ?? "");
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          nets_routed: routedNets.size,
          traces_added: tracesAdded,
          // Copper hygiene: re-routing rips the prior route up first, so a
          // re-route reports both what it removed and what it laid — `added`
          // alone reads like monotonic growth even when copper is being
          // replaced, not stacked.
          ...(tracesRemoved > 0 ? { traces_removed: tracesRemoved } : {}),
          ...(viasRemoved > 0 ? { vias_removed: viasRemoved } : {}),
          ...(staleCleared.length > 0 ? { stale_nets_cleared: staleCleared } : {}),
          ...(lockedNets.size > 0 ? { locked_nets: [...lockedNets] } : {}),
          ...(Object.keys(realizedWidths).length > 0
            ? { track_widths_mm: realizedWidths }
            : {}),
          ...(unroutedNets.size > 0 ? { unrouted_nets: [...unroutedNets] } : {}),
          ...(fallbackNets.size > 0 ? { fallback_nets: [...fallbackNets] } : {}),
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
interface DrcViol {
  rule: string;
  severity: string;
  message: string;
  position?: Vec2;
  actual?: number;
  required?: number;
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
 *  conflicts / fab-rule breaks). UnconnectedNet is the only "incomplete" rule —
 *  it's a to-do, not a defect. */
const DRC_CATEGORY: Record<string, "connectivity" | "clearance" | "manufacturing"> = {
  UnconnectedNet: "connectivity",
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

/** Summary-first DRC payload: counts + worst-case + a capped representative
 *  sample by default; the full violation array only when detail==="full". */
interface DrcSummary {
  success: true;
  violations: number;
  errors: number;
  warnings: number;
  /** Counts split into connectivity (incomplete) vs clearance/manufacturing (illegal). */
  categories: DrcCategories;
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

  const summary: DrcSummary = {
    success: true,
    violations: violations.length,
    errors: violations.filter((v) => v.severity === "Error").length,
    warnings: violations.filter((v) => v.severity === "Warning").length,
    categories,
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
  const critique = await kernelCritiqueRoute(pcb, net);
  if (!critique) {
    return {
      content: [
        { type: "text" as const, text: "critique_route unavailable: kernel WASM not loaded" },
      ],
      isError: true,
    };
  }
  return { content: [{ type: "text" as const, text: JSON.stringify(critique) }] };
}

/** Run DRC on a board and return the summary-first payload. Shared by the
 *  run_drc tool and the inline receipt wrap in the mutators. */
export async function drcPcb(
  pcb: Pcb,
  detail: "summary" | "full" = "summary",
  sampleSize = 20,
): Promise<DrcSummary> {
  // Kernel DRC: copper clearance (trace↔copper and pad↔pad shorts), trace
  // width, drill, annular ring, edge clearance, hole-to-hole. Falls back to
  // basic scalar checks when the kernel WASM is unavailable.
  let violations: DrcViol[];
  if (await isEcadAvailable()) {
    violations = (await kernelRunDrc(pcb)) as unknown as DrcViol[];
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
  return a <= b ? `${a} ${b}` : `${b} ${a}`;
}

/** Run the lightweight pre-routing DRC subset against a freshly-placed board.
 *  Reuses the kernel DRC (single source of truth for pad geometry, net-ties,
 *  diff-pairs) and keeps only the rules that are meaningful with no copper —
 *  so it agrees with what `run_drc` will later report. Shared by
 *  `place_components` and `set_placement` so the move→re-check loop never has
 *  to fall through to a full route→DRC pass. */
export async function summarizePlacementDrc(pcb: Pcb): Promise<PlacementDrc> {
  // `full` so we get every violation, not a capped sample.
  const viols = (await drcPcb(pcb, "full")).details ?? [];

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
    const [a, b] = key.split(" ") as [string, string];
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

  return {
    clean:
      shorts.length === 0 &&
      clearanceViolations === 0 &&
      courtyardOverlaps === 0 &&
      offBoard.length === 0,
    shorts,
    clearance_violations: clearanceViolations,
    courtyard_overlaps: courtyardOverlaps,
    off_board: offBoard,
  };
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
  return {
    content: [{ type: "text" as const, text: JSON.stringify(await drcPcb(pcb, detail, sampleSize)) }],
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

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          violations: violations.length,
          errors: violations.filter(v => v.severity === "Error").length,
          warnings: violations.filter(v => v.severity === "Warning").length,
          details: violations,
        }),
      },
    ],
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
      // Sandboxed/hosted servers can't write arbitrary paths — fall back to
      // inline content so the caller still gets the files.
      const reason = e instanceof Error ? e.message : String(e);
      return {
        content: [
          {
            type: "text" as const,
            text: JSON.stringify({
              success: true,
              message: `Generated ${files.length} fabrication files (could not write to '${outputDir}': ${reason}; returning contents inline)`,
              files,
            }),
          },
        ],
      };
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          message: `Generated ${files.length} fabrication files`,
          files,
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

/** Edge-coupling factor for a differential pair; zDiff = 2·z0·k. ↑ in spacing. */
function diffCouplingK(spacing: number, h: number): number {
  return 1 - 0.48 * Math.exp((-0.96 * spacing) / h);
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

export function calcImpedance(args: Record<string, unknown>) {
  const traceWidth = args.trace_width as number;
  const copperThickness = (args.copper_thickness as number) || 0.035;
  const dielectricHeight = args.dielectric_height as number;
  const er = (args.dielectric_er as number) || 4.5;
  const traceType = (args.trace_type as string) || "microstrip";
  const spacing = (args.spacing as number) || 0;

  const h = dielectricHeight;
  const w = traceWidth;
  const t = copperThickness;

  let z0: number;
  let erEff: number;

  if (traceType === "stripline") {
    z0 = striplineZ0(w, t, h, er);
    erEff = er;
  } else {
    z0 = microstripZ0(w, t, h, er);
    erEff = microstripErEff(w, t, h, er);
  }
  const delayPsPerMm = 3.336 * Math.sqrt(erEff);

  // Differential pair calculations
  let zDiff: number | undefined;
  if (spacing > 0 && (traceType === "diff_microstrip" || traceType === "diff_stripline")) {
    zDiff = 2 * z0 * diffCouplingK(spacing, h);
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
        2 * seZ0(x[0]!) * diffCouplingK(x[1]!, h) - (targetDiff as number),
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
  const diffMeas = isDiff ? 2 * z0Meas * diffCouplingK(sSnap as number, h) : undefined;

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
export function sizePdn(args: Record<string, unknown>) {
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
      return {
        content: [
          {
            type: "text" as const,
            text: JSON.stringify({
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
            }),
          },
        ],
      };
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

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
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
        }),
      },
    ],
  };
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
export function addCoil(args: Record<string, unknown>) {
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
  const layer = ((args.layer as string) || "FCu") as PcbLayer;
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
  if (!/Cu$/.test(layer)) {
    return fail(`layer "${layer}" is not a copper layer (use FCu, BCu, In1Cu, ...)`);
  }
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

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  // ---- Multilayer stacked coil ------------------------------------------
  const layersIn = Array.isArray(args.layers) ? (args.layers as string[]) : undefined;
  if (layersIn && layersIn.length >= 2) {
    for (const l of layersIn) {
      if (!/Cu$/.test(l)) {
        return fail(`layers entry "${l}" is not a copper layer (use FCu, BCu, In1Cu, ...)`);
      }
    }
    const stitchVias: Array<{ position: Vec2; startLayer: PcbLayer; endLayer: PcbLayer }> = [];
    const perLayerLen = segLength(pts);
    let totalLengthMm = 0;
    let totalTraces = 0;
    let totalResistance = 0;
    for (let li = 0; li < layersIn.length; li++) {
      const lyr = layersIn[li] as PcbLayer;
      for (let i = 0; i + 1 < pts.length; i++) {
        pcb.traces.push({ start: pts[i], end: pts[i + 1], width: traceWidth, layer: lyr, net });
        totalTraces++;
      }
      totalLengthMm += perLayerLen;
      const cuT =
        pcb.stackup.layers.find((s) => s.layer === lyr)?.copperThickness ?? 0.035;
      totalResistance += (1.68e-5 * perLayerLen) / (traceWidth * cuT);
      // Stitch to the next layer at an alternating terminal.
      if (li + 1 < layersIn.length) {
        const atInner = li % 2 === 0;
        const stitchPt = atInner ? pts[0] : pts[pts.length - 1];
        pcb.vias.push({
          position: stitchPt,
          diameter: pcb.rules.defaultRules.viaDiameter,
          drill: pcb.rules.defaultRules.viaDrill,
          startLayer: lyr,
          endLayer: layersIn[li + 1] as PcbLayer,
          net,
        });
        stitchVias.push({ position: stitchPt, startLayer: lyr, endLayer: layersIn[li + 1] as PcbLayer });
      }
    }
    // External terminals: layer[0] outer, and the last layer's free terminal
    // (inner if an even number of stitches landed on the inner side). The last
    // stitch (index n-2) is at inner when (n-2)%2===0 → last free end is outer;
    // otherwise inner. Equivalently the last layer's free end is inner when
    // (layersIn.length-1) is even.
    const lastFreeInner = (layersIn.length - 1) % 2 === 0;
    const terminalA = pts[pts.length - 1]; // layer[0] outer
    const terminalB = lastFreeInner ? pts[0] : pts[pts.length - 1];
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
              "Multilayer stacked coil: `layer` and inner_via/via_to_layer ignored; " +
              "layers stitched with alternating inner/outer vias.",
            total_traces: totalTraces,
            total_length_mm: Math.round(totalLengthMm * 100) / 100,
            total_resistance_ohms: Math.round(totalResistance * 1000) / 1000,
            stitch_vias: stitchVias,
            terminals: { a: terminalA, b: terminalB },
            inner_endpoint: pts[0],
            outer_endpoint: pts[pts.length - 1],
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
    });
    tracesAdded++;
    lengthMm += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
  }

  let via: { position: Vec2; startLayer: PcbLayer; endLayer: PcbLayer } | undefined;
  if (args.inner_via) {
    const viaTo = ((args.via_to_layer as string) || "BCu") as PcbLayer;
    const v = {
      position: pts[0],
      diameter: pcb.rules.defaultRules.viaDiameter,
      drill: pcb.rules.defaultRules.viaDrill,
      startLayer: layer,
      endLayer: viaTo,
      net,
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
export function addCoilArray(args: Record<string, unknown>) {
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
    const res = addCoil({
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
    });
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

  return { content: [{ type: "text" as const, text: JSON.stringify(plan) }] };
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
  return pad.layers.filter((l) => /Cu$/.test(l));
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
// add_trace — push straight copper segments between consecutive points
// ============================================================================

/** Shared {x, y} JSON schema fragment. */
const vec2Schema = {
  type: "object" as const,
  properties: { x: { type: "number" as const }, y: { type: "number" as const } },
  required: ["x", "y"],
};

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
export function addTrace(args: Record<string, unknown>) {
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
  const layer = ((args.layer as string) || "FCu") as PcbLayer;
  const width = (args.width as number) ?? pcb.rules.defaultRules.traceWidth;

  if (!points || points.length < 2) return fail("points must be an array of >= 2 {x, y}");
  for (const p of points) {
    if (!p || typeof p.x !== "number" || typeof p.y !== "number") {
      return fail("every point must be {x, y} in mm");
    }
  }
  if (!net) return fail("net is required — copper must belong to a net");
  if (!/Cu$/.test(layer)) {
    return fail(`layer "${layer}" is not a copper layer (use FCu, BCu, In1Cu, ...)`);
  }
  if (!(width > 0)) return fail("width must be > 0");

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  let tracesAdded = 0;
  let lengthMm = 0;
  for (let i = 0; i + 1 < points.length; i++) {
    const start = { x: round3(points[i].x), y: round3(points[i].y) };
    const end = { x: round3(points[i + 1].x), y: round3(points[i + 1].y) };
    const trace: Trace = { start, end, width, layer, net };
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
export function addVia(args: Record<string, unknown>) {
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
  const startLayer = ((args.start_layer as string) || "FCu") as PcbLayer;
  const endLayer = ((args.end_layer as string) || "BCu") as PcbLayer;
  const diameter = (args.diameter as number) ?? pcb.rules.defaultRules.viaDiameter;
  const drill = (args.drill as number) ?? pcb.rules.defaultRules.viaDrill;

  if (!position || typeof position.x !== "number" || typeof position.y !== "number") {
    return fail("position must be {x, y} in mm");
  }
  if (!net) return fail("net is required — a via must belong to a net");
  if (!/Cu$/.test(startLayer)) return fail(`start_layer "${startLayer}" is not a copper layer`);
  if (!/Cu$/.test(endLayer)) return fail(`end_layer "${endLayer}" is not a copper layer`);

  if (!pcb.nets.some((n) => n.id === net)) {
    pcb.nets.push({ id: net, name: net });
  }

  const pos = { x: round3(position.x), y: round3(position.y) };
  const via: Via = { position: pos, diameter, drill, startLayer, endLayer, net };
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
export function setStackup(args: Record<string, unknown>) {
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

  const stackup = pcb.stackup.layers;

  // 1) Uniform copper weight across all copper layers.
  if (copperOz != null) {
    const t = round3(copperOz * OZ_TO_MM);
    for (const l of stackup) {
      if (/Cu$/.test(l.layer)) l.copperThickness = t;
    }
  }

  // 2) Per-layer overrides; create copper-layer entries that are missing.
  for (const ov of perLayer ?? []) {
    const layerName = String(ov.layer ?? "");
    if (!layerName) return fail("each layers entry needs a `layer`");
    let entry = stackup.find((l) => l.layer === layerName);
    if (!entry) {
      if (!/Cu$/.test(layerName)) {
        return fail(`cannot create non-copper layer "${layerName}" — only copper layers are auto-added`);
      }
      entry = { layer: layerName as PcbLayer } as StackupLayer;
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
  const layer = ((args.layer as string) || "FCu") as PcbLayer;
  if (!net) return fail("net is required — a pour must belong to a net");
  if (!/Cu$/.test(layer)) {
    return fail(`layer "${layer}" is not a copper layer (use FCu, BCu, In1Cu, ...)`);
  }

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
          ...docResultPayload(ctx),
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
  if (!pcb) {
    return fail(
      "Document has no PCB — run place_components first (or open a document that has a board)",
    );
  }

  const rules = pcb.rules;
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
    const knownNets = new Set(pcb.nets.map((n) => n.id));
    for (const c of classesIn) {
      const name = String(c.name ?? "");
      const nets = Array.isArray(c.nets) ? (c.nets as unknown[]).map(String) : [];
      if (!name) return fail("each class needs a `name`");
      if (nets.length === 0) return fail(`class "${name}" needs a non-empty nets array`);
      const unknown = nets.filter((id) => !knownNets.has(id));
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

  if (!touched) {
    return fail("provide at least one rule field (clearance, track_width, …) or `classes`");
  }

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
          ...(classNames ? { classes: classNames } : {}),
          ...(warnings.length > 0 ? { warnings } : {}),
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
export function addViaArray(args: Record<string, unknown>) {
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
  const startLayer = ((args.start_layer as string) || "FCu") as PcbLayer;
  const endLayer = ((args.end_layer as string) || "BCu") as PcbLayer;
  const diameter = (args.diameter as number) ?? pcb.rules.defaultRules.viaDiameter;
  const drill = (args.drill as number) ?? pcb.rules.defaultRules.viaDrill;
  if (!net) return fail("net is required — a via must belong to a net");
  if (!/Cu$/.test(startLayer)) return fail(`start_layer "${startLayer}" is not a copper layer`);
  if (!/Cu$/.test(endLayer)) return fail(`end_layer "${endLayer}" is not a copper layer`);

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

  if (!pcb.nets.some((n) => n.id === net)) pcb.nets.push({ id: net, name: net });

  for (const p of kept) {
    pcb.vias.push({
      position: { x: round3(p.x), y: round3(p.y) },
      diameter,
      drill,
      startLayer,
      endLayer,
      net,
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
          ...docResultPayload(ctx),
        }),
      },
    ],
  };
}

// ============================================================================
// add_motor_winding — plan → copper realizer (closes the winding loop)
// ============================================================================

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
      description: "Layer for interconnect + neutral/loop (default 'BCu')",
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
 * winding_layout, drops a spiral coil per tooth (each escaping to the return
 * layer via inner+outer vias), series-connects coils within each phase on the
 * return layer, and terminates the phases (wye star or delta loop) — recording
 * the join as a NetTie so DRC treats it as intentional, not a short. The
 * interconnect is a baseline straight-line realizer; the new DRC honestly flags
 * any crossings it introduces for follow-up routing.
 */
export function addMotorWinding(args: Record<string, unknown>) {
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
  const copperLayer = ((args.copper_layer as string) || "FCu") as PcbLayer;
  const returnLayer = ((args.return_layer as string) || "BCu") as PcbLayer;

  if (!center || typeof center.x !== "number" || typeof center.y !== "number") {
    return fail("center must be {x, y} in mm");
  }
  if (!(pitchRadius >= 0)) return fail("pitch_radius must be >= 0");
  if (!(outerR > innerR)) return fail("outer_radius must be > inner_radius");
  if (!(traceWidth > 0)) return fail("trace_width must be > 0");
  if (!/Cu$/.test(copperLayer)) return fail(`copper_layer "${copperLayer}" is not a copper layer`);
  if (!/Cu$/.test(returnLayer)) return fail(`return_layer "${returnLayer}" is not a copper layer`);

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

  // b. One spiral per tooth; both terminals reachable on the return layer.
  //    Keyed by slot so the series walk can find each coil's endpoints.
  const coilTerminals = new Map<number, { inner: Vec2; outer: Vec2 }>();
  for (const coil of plan.coils) {
    const angle = (coil.angleDeg * Math.PI) / 180;
    const coilCenter = {
      x: round3(center.x + pitchRadius * Math.cos(angle)),
      y: round3(center.y + pitchRadius * Math.sin(angle)),
    };
    const dir = coil.polarity === 1 ? "ccw" : "cw";
    const res = addCoil({
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
      inner_via: true,
      via_to_layer: returnLayer,
    });
    if (res.isError) {
      errors.push(`coil slot ${coil.slot} (net ${coil.net}): ${res.content[0]!.text}`);
      continue;
    }
    const payload = JSON.parse(res.content[0]!.text) as {
      inner_endpoint: Vec2;
      outer_endpoint: Vec2;
    };
    coilsPlaced++;
    vias++; // the inner via add_coil placed
    // Add an outer via so the outer terminal is also reachable on returnLayer.
    pcb.vias.push({
      position: payload.outer_endpoint,
      diameter: pcb.rules.defaultRules.viaDiameter,
      drill: pcb.rules.defaultRules.viaDrill,
      startLayer: copperLayer,
      endLayer: returnLayer,
      net: coil.net,
    });
    vias++;
    coilTerminals.set(coil.slot, {
      inner: payload.inner_endpoint,
      outer: payload.outer_endpoint,
    });
  }

  // c. Series interconnect on the return layer, per phase, in plan order.
  //    Connect coil k's outer terminal to coil k+1's inner terminal.
  const phaseEnds: Record<string, { start: Vec2; end: Vec2 }> = {};
  for (const [netName, slotSeq] of Object.entries(plan.phaseSeries)) {
    const present = slotSeq.filter((s) => coilTerminals.has(s));
    if (present.length === 0) continue;
    const first = coilTerminals.get(present[0]!)!;
    let prevEnd = first.outer;
    phaseEnds[netName] = { start: first.inner, end: first.outer };
    for (let i = 1; i < present.length; i++) {
      const term = coilTerminals.get(present[i]!)!;
      pcb.traces.push({
        start: prevEnd,
        end: term.inner,
        width: traceWidth,
        layer: returnLayer,
        net: netName,
      });
      interconnectTraces++;
      prevEnd = term.outer;
    }
    phaseEnds[netName]!.end = prevEnd;
  }

  // d. Termination + net-tie so DRC sees an intentional join, not a short.
  const phaseNetNames = Object.keys(phaseEnds);
  pcb.netTies = pcb.netTies ?? [];
  let netTiesAdded = 0;
  if (connection === "wye") {
    const neutral = plan.neutralNet ?? (args.neutral_net ? String(args.neutral_net) : "WIND_N");
    for (const netName of phaseNetNames) {
      pcb.traces.push({
        start: phaseEnds[netName]!.end,
        end: center,
        width: traceWidth,
        layer: returnLayer,
        net: netName,
      });
      interconnectTraces++;
    }
    pcb.netTies.push({
      nets: [...phaseNetNames, neutral],
      position: center,
      radius: Math.max(2, innerR),
    });
    netTiesAdded++;
  } else {
    // delta: phase[i].end → phase[(i+1)%n].start, one tie per junction.
    const n = phaseNetNames.length;
    for (let i = 0; i < n; i++) {
      const a = phaseNetNames[i]!;
      const b = phaseNetNames[(i + 1) % n]!;
      pcb.traces.push({
        start: phaseEnds[a]!.end,
        end: phaseEnds[b]!.start,
        width: traceWidth,
        layer: returnLayer,
        net: a,
      });
      interconnectTraces++;
      pcb.netTies.push({
        nets: [a, b],
        position: phaseEnds[b]!.start,
        radius: Math.max(2, innerR),
      });
      netTiesAdded++;
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: errors.length === 0,
          coils_placed: coilsPlaced,
          coils_failed: errors.length,
          interconnect_traces: interconnectTraces,
          vias_added: vias,
          net_ties_added: netTiesAdded,
          connection,
          winding_factor: plan.windingFactor,
          interconnect_note:
            "Series interconnect and termination are straight-line on the return " +
            "layer and may cross; run_drc will flag any crossings for routing cleanup.",
          ...(errors.length ? { errors } : {}),
          ...docResultPayload(ctx),
        }),
      },
    ],
    ...(errors.length && coilsPlaced === 0 ? { isError: true as const } : {}),
  };
}

// ============================================================================
// calc_motor — first-order analytical motor performance (Kt/Ke/speed/torque)
// ============================================================================

export const calcMotorSchema = {
  type: "object" as const,
  properties: {
    pole_pairs: { type: "number" as const, description: "Pole pairs p (electrical periods per mechanical rev)." },
    turns_per_phase: { type: "number" as const, description: "Series turns per phase N." },
    winding_factor: { type: "number" as const, description: "Winding factor kw (default 0.95). Use the value from winding_layout for accuracy." },
    inner_radius_mm: { type: "number" as const, description: "Inner (bore) stator radius, mm." },
    outer_radius_mm: { type: "number" as const, description: "Outer stator radius, mm." },
    phase_resistance_ohm: { type: "number" as const, description: "Per-phase resistance, ohms (e.g. the add_coil DC estimate)." },
    supply_voltage_v: { type: "number" as const, description: "DC bus / supply voltage, volts." },
    airgap_flux_tesla: {
      type: "number" as const,
      description: "Air-gap flux density B_gap (T). Omit to COMPUTE it from `magnet` via the MEC model.",
    },
    magnet: {
      type: "object" as const,
      description:
        "Optional magnet/geometry to compute B_gap when airgap_flux_tesla is omitted (NdFeB defaults). Fields: remanence_tesla, magnet_thickness_mm, airgap_mm, recoil_mu_rel, magnet_area_mm2, gap_area_mm2, iron_mu_rel.",
    },
  },
  required: [
    "pole_pairs",
    "turns_per_phase",
    "inner_radius_mm",
    "outer_radius_mm",
    "phase_resistance_ohm",
    "supply_voltage_v",
  ],
};

/**
 * Evaluate a motor's headline performance — torque constant, back-EMF constant,
 * no-load speed, stall torque, and a speed–torque curve — from its magnetics +
 * electrical parameters. Pure analysis (no board, no mutation). The air-gap flux
 * is either supplied directly or computed from magnet geometry via the
 * first-order MEC reluctance model. First-order steady state: no slotting,
 * fringing, saturation, or losses.
 */
export async function calcMotor(args: Record<string, unknown>) {
  const fail = ecadError;

  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : NaN);
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
  if (!(phaseR > 0)) return fail("phase_resistance_ohm must be > 0");
  if (!(supplyV > 0)) return fail("supply_voltage_v must be > 0");

  // Resolve air-gap flux: explicit value, else compute from magnet geometry.
  let bGap = num(args.airgap_flux_tesla);
  let bGapSource: "supplied" | "computed" = "supplied";
  if (!Number.isFinite(bGap)) {
    const m = (args.magnet ?? {}) as Record<string, unknown>;
    const mnum = (v: unknown, d: number) =>
      typeof v === "number" && Number.isFinite(v) ? (v as number) : d;
    const computed = await airgapFluxDensity({
      remanenceTesla: mnum(m.remanence_tesla, 1.2),
      magnetThicknessMm: mnum(m.magnet_thickness_mm, 3),
      recoilMuRel: mnum(m.recoil_mu_rel, 1.05),
      airgapMm: mnum(m.airgap_mm, 1),
      magnetAreaMm2: mnum(m.magnet_area_mm2, 1),
      gapAreaMm2: mnum(m.gap_area_mm2, 1),
      ironMuRel: typeof m.iron_mu_rel === "number" ? (m.iron_mu_rel as number) : null,
      ironPathMm: mnum(m.iron_path_mm, 0),
      ironAreaMm2: mnum(m.iron_area_mm2, 1),
    });
    if (computed == null) {
      return fail(
        "air-gap flux is required: pass airgap_flux_tesla, or `magnet` params (ECAD WASM must be available to compute B_gap).",
      );
    }
    bGap = computed;
    bGapSource = "computed";
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
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          airgap_flux_tesla: r4(bGap),
          airgap_flux_source: bGapSource,
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
          note: "First-order steady-state estimate (no slotting/fringing/saturation/losses).",
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
      description: "Session id holding the PCB to certify.",
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

/** Build a re-runnable verification Receipt for the session's PCB. */
export async function buildReceipt(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const receipt = await kernelBuildReceipt(pcb);
  if (!receipt) {
    return {
      content: [{ type: "text" as const, text: "Error: ECAD engine unavailable" }],
      isError: true,
    };
  }
  // The receipt rides in structuredContent so the inline viewer renders it
  // as an audit ledger (the only carrier ChatGPT's widget bridge exposes);
  // document_id lets the viewer also fetch the board GLB behind the ledger.
  return {
    content: [{ type: "text" as const, text: JSON.stringify(receipt) }],
    structuredContent: {
      receipt,
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

/** Re-run a prior Receipt against the session's current board. Returns the
 *  verdict: Holds (same board, clean), Stale (board changed), or Violated. */
export async function verifyReceipt(args: Record<string, unknown>) {
  const ctx = resolveDocInput(args);
  const pcb = getDocPcb(ctx.doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }
  const receipt = args.receipt as Receipt | undefined;
  if (!receipt || typeof receipt !== "object" || !receipt.board_hash) {
    return {
      content: [
        {
          type: "text" as const,
          text: "Error: missing `receipt` — pass a Receipt produced by build_receipt.",
        },
      ],
      isError: true,
    };
  }
  const status = await kernelVerifyReceipt(pcb, receipt);
  if (!status) {
    return {
      content: [{ type: "text" as const, text: "Error: ECAD engine unavailable" }],
      isError: true,
    };
  }
  const payload = { status, board_hash: receipt.board_hash };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: {
      verify_receipt: payload,
      ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
    },
  };
}
