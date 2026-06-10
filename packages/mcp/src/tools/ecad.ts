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
  SchematicJunction,
  SchematicPin,
  Pcb,
  BoardOutline,
  LayerStackup,
  StackupLayer,
  Net,
  DesignRules,
  NetClassRules,
  Footprint,
  Pad,
  PadShape,
  PadType,
  PcbLayer,
  Trace,
  Via,
  Zone,
  Vec2,
} from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import {
  computeRatsnest,
  exportFabFiles,
  footprintForName,
  generateNetlist,
  isEcadAvailable,
  routeNet,
  routeNetShove,
  runDrc as kernelRunDrc,
} from "@vcad/engine";
import type { NetlistResult } from "@vcad/engine";

/** Get PCB data from a document — checks PcbBoard nodes first, falls back to legacy doc.pcb */
function getDocPcb(doc: Document): Pcb | null {
  const nodeIds = getPcbNodeIds(doc);
  if (nodeIds.length > 0) return getNodePcb(doc, nodeIds[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
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
          value: { type: "string" as const, description: 'Component value (e.g. "10k", "ATmega328P")' },
          footprint: { type: "string" as const, description: 'Footprint ID (e.g. "Resistor_SMD:R_0805")' },
          x: { type: "number" as const, description: "X position on sheet" },
          y: { type: "number" as const, description: "Y position on sheet" },
          rotation: { type: "number" as const, description: "Rotation in degrees (default 0)" },
          pins: {
            type: "array" as const,
            description: "Component pins",
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
        },
        required: ["ref", "value", "footprint", "x", "y", "pins"],
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
  },
  required: ["components"],
};

/** JSON Schema for place_components tool. */
export const placeComponentsSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document with schematic",
    },
    board_width: {
      type: "number" as const,
      description: "Board width in mm",
    },
    board_height: {
      type: "number" as const,
      description: "Board height in mm",
    },
    board_thickness: {
      type: "number" as const,
      description: "Board thickness in mm (default 1.6)",
    },
    strategy: {
      type: "string" as const,
      description: "Placement strategy: grid, force_directed (default: grid)",
    },
  },
  required: ["document", "board_width", "board_height"],
};

/** JSON Schema for route_nets tool. */
export const routeNetsSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document with PCB and placed footprints",
    },
    nets: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Net IDs to route (empty = route all)",
    },
    trace_width: {
      type: "number" as const,
      description: "Trace width in mm (default from design rules)",
    },
  },
  required: ["document"],
};

/** JSON Schema for run_drc tool. */
export const runDrcSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document with PCB",
    },
  },
  required: ["document"],
};

/** JSON Schema for run_erc tool. */
export const runErcSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document with schematic",
    },
  },
  required: ["document"],
};

/** JSON Schema for export_gerber tool. */
export const exportGerberSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "object" as const,
      description: "vcad IR Document with PCB",
    },
    output_dir: {
      type: "string" as const,
      description:
        "Directory to write the fabrication files to (created if missing). " +
        "When omitted, file contents are returned inline instead.",
    },
  },
  required: ["document"],
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

// ============================================================================
// Tool implementations
// ============================================================================

/** Create a schematic from component and wire definitions. */
export function createSchematic(args: Record<string, unknown>) {
  const title = (args.title as string) || undefined;
  const componentsInput = (args.components as Array<Record<string, unknown>>) || [];
  const wiresInput = (args.wires as Array<Record<string, unknown>>) || [];
  const labelsInput = (args.labels as Array<Record<string, unknown>>) || [];

  const components: SchematicComponent[] = componentsInput.map((c) => ({
    ref: c.ref as string,
    value: c.value as string,
    footprintId: c.footprint as string,
    position: { x: c.x as number, y: c.y as number },
    rotation: (c.rotation as number) || 0,
    pins: ((c.pins as Array<Record<string, unknown>>) || []).map((p) => ({
      number: p.number as string,
      name: p.name as string,
      pin_type: (p.type as SchematicPin["pin_type"]) || "Passive",
      position: { x: (p.x as number) || 0, y: (p.y as number) || 0 },
    })),
  }));

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
  };

  const doc = createDocument();
  doc.schematic = schematic;

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          components: components.length,
          wires: wires.length,
          labels: labels.length,
          document: doc,
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
  bounds: { minX: number; minY: number; maxX: number; maxY: number; minSep: number },
): void {
  // net id → component indices on that net
  const netMembers = new Map<string, number[]>();
  components.forEach((comp, i) => {
    const seen = new Set<string>();
    for (const pin of comp.pins) {
      const netId = useConnectivity
        ? netByPin.get(`${comp.ref}\u0000${pin.number}`)
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
  const minSep = bounds.minSep;

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

    // Repulsion when components get closer than the minimum separation.
    for (let i = 0; i < positions.length; i++) {
      for (let j = i + 1; j < positions.length; j++) {
        const dx = positions[j].x - positions[i].x;
        const dy = positions[j].y - positions[i].y;
        const dist = Math.max(Math.hypot(dx, dy), 0.1);
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

/** Place components on a PCB from schematic data. */
export async function placeComponents(args: Record<string, unknown>) {
  const doc = args.document as Document;
  const boardWidth = args.board_width as number;
  const boardHeight = args.board_height as number;
  const boardThickness = (args.board_thickness as number) || 1.6;

  if (!doc.schematic) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no schematic" }],
      isError: true,
    };
  }

  // Derive pad nets from schematic connectivity (wires + junctions + labels)
  // using the kernel netlist extractor. Pin names only serve as net ids as a
  // fallback when the schematic has no wires or the kernel is unavailable.
  const netByPin = new Map<string, string>();
  if (doc.schematic.wires.length > 0) {
    const netlist = await generateNetlist(doc.schematic);
    for (const net of netlist.nets) {
      // Single-connection groups are unconnected pins, not real nets.
      if (net.connections.length < 2) continue;
      for (const conn of net.connections) {
        netByPin.set(`${conn.component_ref}\u0000${conn.pin_number}`, net.name);
      }
    }
  }
  const useConnectivity = netByPin.size > 0;

  // Create PCB structure
  const outline: BoardOutline = {
    vertices: [
      { x: 0, y: 0 },
      { x: boardWidth, y: 0 },
      { x: boardWidth, y: boardHeight },
      { x: 0, y: boardHeight },
    ],
    thickness: boardThickness,
  };

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

  // Grid placement sized to the board: split the usable area into cells so
  // every component lands inside the outline regardless of board size.
  const components = doc.schematic.components;
  const strategy = (args.strategy as string) || "grid";
  const margin = Math.min(5, boardWidth / 8, boardHeight / 8);
  const usableW = boardWidth - 2 * margin;
  const usableH = boardHeight - 2 * margin;
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

  const warnings: string[] = [];
  if (n > 1 && Math.min(cellW, cellH) < 4) {
    warnings.push(
      `Placement cells are ${Math.min(cellW, cellH).toFixed(1)}mm — components may overlap; consider a larger board`,
    );
  }

  const positions: Vec2[] = components.map((_, i) => ({
    x: margin + ((i % cols) + 0.5) * cellW,
    y: margin + (Math.floor(i / cols) + 0.5) * cellH,
  }));

  if (strategy === "force_directed") {
    forceDirectedRefine(components, positions, netByPin, useConnectivity, {
      minX: margin,
      minY: margin,
      maxX: boardWidth - margin,
      maxY: boardHeight - margin,
      minSep: Math.min(cellW, cellH),
    });
  }

  const nets: Net[] = [];
  const netSet = new Set<string>();

  const netIdForPin = (comp: SchematicComponent, pin: SchematicPin): string | undefined => {
    // Net from schematic connectivity; pin-name fallback for wireless docs.
    const netId = useConnectivity
      ? netByPin.get(`${comp.ref}\u0000${pin.number}`)
      : pin.name && pin.name !== "~"
        ? pin.name
        : undefined;
    if (netId && !netSet.has(netId)) {
      netSet.add(netId);
      nets.push({ id: netId, name: netId });
    }
    return netId;
  };

  // Resolve real pad geometry from the kernel footprint library (SOIC, DIP,
  // QFP, headers, chip sizes, ...). Null only when the kernel is unavailable.
  const templates = await Promise.all(
    components.map((c) => footprintForName(c.footprintId, c.pins.length)),
  );

  const footprints: Footprint[] = components.map((comp, i) => {
    const x = Math.round(positions[i].x * 100) / 100;
    const y = Math.round(positions[i].y * 100) / 100;
    const template = templates[i];

    let pads: Pad[];
    if (template) {
      pads = template.pads.map((pad) => {
        const pin = comp.pins.find((p) => p.number === pad.number);
        const netId = pin ? netIdForPin(comp, pin) : undefined;
        const layers: PcbLayer[] =
          pad.padType === "THT"
            ? ["FCu", "BCu", "FMask", "BMask"]
            : ["FCu", "FPaste", "FMask"];
        return { ...pad, net: netId, layers };
      });
    } else {
      // No kernel: spread generic SMD pads in a row so they never stack.
      pads = comp.pins.map((pin, pi) => ({
        number: pin.number,
        padType: "SMD" as PadType,
        shape: { type: "Rect" as const, width: 1.0, height: 1.2 },
        position: { x: (pi - (comp.pins.length - 1) / 2) * 2.54, y: 0 },
        net: netIdForPin(comp, pin),
        layers: ["FCu", "FPaste", "FMask"] as PcbLayer[],
      }));
    }

    return {
      ref: comp.ref,
      value: comp.value,
      footprintName: comp.footprintId,
      position: { x, y },
      rotation: 0,
      front: true,
      pads,
      ...(template && template.graphics.length > 0 ? { graphics: template.graphics } : {}),
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

  // Create a PcbBoard DAG node instead of legacy doc.pcb
  const existingIds = Object.keys(doc.nodes).map(Number);
  const nid = existingIds.length > 0 ? Math.max(...existingIds) + 1 : 1;
  doc.nodes[String(nid)] = {
    id: nid,
    name: "PCB Board",
    op: { type: "PcbBoard", board: pcb } as any,
  };
  doc.roots.push({ root: nid, material: "__pcb_fr4__" });
  if (!doc.materials["__pcb_fr4__"]) {
    doc.materials["__pcb_fr4__"] = {
      color: [0.05, 0.35, 0.15],
      roughness: 0.6,
      metallic: 0.0,
    } as any;
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          footprints_placed: footprints.length,
          strategy,
          ...(warnings.length > 0 ? { warnings } : {}),
          board: { width: boardWidth, height: boardHeight, thickness: boardThickness },
          document: doc,
        }),
      },
    ],
  };
}

/** Route nets on a PCB with the kernel autorouter (obstacle-avoiding). */
export async function routeNets(args: Record<string, unknown>) {
  const doc = args.document as Document;
  const traceWidth = (args.trace_width as number) || undefined;
  const netsFilter = (args.nets as string[]) || [];

  const pcb = getDocPcb(doc);
  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  const width = traceWidth || pcb.rules.defaultRules.traceWidth;

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

  const rats = await computeRatsnest(pcb, netlist);

  const routedNets = new Set<string>();
  const fallbackNets = new Set<string>();
  let tracesAdded = 0;

  for (const line of rats) {
    if (netsFilter.length > 0 && !netsFilter.includes(line.net)) continue;

    // Push-and-shove first (continuous-space, detours around copper), grid
    // router as fallback. Each committed route becomes an obstacle for the
    // next because we append to pcb.traces between calls.
    let res = await routeNetShove(pcb, line.net, line.from, line.to, width);
    if (!res.success || res.segments.length === 0) {
      res = await routeNet(pcb, line.net, line.from, line.to, width);
    }

    if (res.success && res.segments.length > 0) {
      for (const [start, end] of res.segments) {
        pcb.traces.push({
          start: { x: start.x, y: start.y },
          end: { x: end.x, y: end.y },
          width,
          layer: "FCu",
          net: line.net,
        });
        tracesAdded++;
      }
      for (const via of res.vias) {
        pcb.vias.push({
          position: { x: via.x, y: via.y },
          diameter: pcb.rules.defaultRules.viaDiameter,
          drill: pcb.rules.defaultRules.viaDrill,
          startLayer: "FCu",
          endLayer: "BCu",
          net: line.net,
        });
      }
      routedNets.add(line.net);
    } else {
      // Last resort (kernel unavailable or no path found): direct segment.
      // Flagged so callers know this connection may cross other copper.
      pcb.traces.push({
        start: { x: line.from.x, y: line.from.y },
        end: { x: line.to.x, y: line.to.y },
        width,
        layer: "FCu",
        net: line.net,
      });
      tracesAdded++;
      routedNets.add(line.net);
      fallbackNets.add(line.net);
    }
  }

  // No kernel at all: computeRatsnest returns [] — chain pads directly so
  // the tool still produces connectivity (legacy behavior).
  if (rats.length === 0) {
    for (const [netId, conns] of netConnections) {
      if (conns.length < 2) continue;
      if (netsFilter.length > 0 && !netsFilter.includes(netId)) continue;
      if (routedNets.has(netId)) continue;
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

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          nets_routed: routedNets.size,
          traces_added: tracesAdded,
          ...(fallbackNets.size > 0
            ? {
                fallback_nets: [...fallbackNets],
                warnings: [
                  `${fallbackNets.size} net(s) used direct fallback segments that may cross other copper — run run_drc to verify`,
                ],
              }
            : {}),
          document: doc,
        }),
      },
    ],
  };
}

/** Run DRC checks on a PCB. */
export async function runDrc(args: Record<string, unknown>) {
  const doc = args.document as Document;
  const pcb = getDocPcb(doc);

  if (!pcb) {
    return {
      content: [{ type: "text" as const, text: "Error: Document has no PCB" }],
      isError: true,
    };
  }

  // Kernel DRC: copper clearance (trace↔copper and pad↔pad shorts), trace
  // width, drill, annular ring, edge clearance, hole-to-hole.
  if (await isEcadAvailable()) {
    const kernelViolations = await kernelRunDrc(pcb);
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify({
            success: true,
            violations: kernelViolations.length,
            errors: kernelViolations.filter((v) => v.severity === "Error").length,
            warnings: kernelViolations.filter((v) => v.severity === "Warning").length,
            details: kernelViolations,
          }),
        },
      ],
    };
  }

  // Fallback when the kernel WASM is unavailable: basic scalar checks only.
  const violations: Array<{
    rule: string;
    severity: string;
    message: string;
    position?: Vec2;
  }> = [];

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
  const boardMinX = Math.min(...pcb.outline.vertices.map(v => v.x));
  const boardMaxX = Math.max(...pcb.outline.vertices.map(v => v.x));
  const boardMinY = Math.min(...pcb.outline.vertices.map(v => v.y));
  const boardMaxY = Math.max(...pcb.outline.vertices.map(v => v.y));
  const edgeClr = pcb.rules.edgeClearance;

  for (const trace of pcb.traces) {
    for (const pt of [trace.start, trace.end]) {
      const hw = trace.width / 2;
      if (pt.x - hw < boardMinX + edgeClr || pt.x + hw > boardMaxX - edgeClr ||
          pt.y - hw < boardMinY + edgeClr || pt.y + hw > boardMaxY - edgeClr) {
        violations.push({
          rule: "EdgeClearance",
          severity: "Error",
          message: `Trace too close to board edge (min ${edgeClr}mm)`,
          position: pt,
        });
      }
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

/** Run ERC checks on a schematic. */
export function runErc(args: Record<string, unknown>) {
  const doc = args.document as Document;

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

  // Check for unconnected pins (pins at positions with no wires)
  for (const comp of sheet.components) {
    for (const pin of comp.pins) {
      if (pin.pin_type === "NotConnected") continue;
      const pinX = comp.position.x + pin.position.x;
      const pinY = comp.position.y + pin.position.y;

      const connected = sheet.wires.some(w =>
        (Math.abs(w.start.x - pinX) < 0.01 && Math.abs(w.start.y - pinY) < 0.01) ||
        (Math.abs(w.end.x - pinX) < 0.01 && Math.abs(w.end.y - pinY) < 0.01)
      );

      if (!connected) {
        violations.push({
          severity: pin.pin_type === "PowerInput" ? "Error" : "Warning",
          message: `Unconnected pin: ${comp.ref} pin ${pin.number} (${pin.name})`,
          position: { x: pinX, y: pinY },
        });
      }
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
  const doc = args.document as Document;
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
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    await fs.mkdir(outputDir, { recursive: true });
    for (const f of files) {
      await fs.writeFile(path.join(outputDir, f.name), f.content, "utf8");
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
export function calcImpedance(args: Record<string, unknown>) {
  const traceWidth = args.trace_width as number;
  const copperThickness = (args.copper_thickness as number) || 0.035;
  const dielectricHeight = args.dielectric_height as number;
  const er = (args.dielectric_er as number) || 4.5;
  const traceType = (args.trace_type as string) || "microstrip";
  const spacing = (args.spacing as number) || 0;

  let z0: number;
  let erEff: number;
  let delayPsPerMm: number;

  if (traceType === "stripline") {
    // Stripline impedance
    const h = dielectricHeight;
    const w = traceWidth;
    const t = copperThickness;
    z0 = (60 / Math.sqrt(er)) * Math.log(4 * h / (0.67 * Math.PI * (0.8 * w + t)));
    erEff = er;
    delayPsPerMm = 3.336 * Math.sqrt(erEff);
  } else {
    // Microstrip impedance
    const h = dielectricHeight;
    const w = traceWidth;
    const t = copperThickness;

    // Effective width adjustment for copper thickness
    const we = w + (t / Math.PI) * Math.log(
      4 * Math.E / Math.sqrt(
        Math.pow(t / h, 2) + Math.pow(t / (w * Math.PI + 1.1 * t * Math.PI), 2)
      )
    );

    z0 = (87 / Math.sqrt(er + 1.41)) * Math.log(5.98 * h / (0.8 * we + t));
    erEff = (er + 1) / 2 + ((er - 1) / 2) * Math.pow(1 + 12 * h / we, -0.5);
    delayPsPerMm = 3.336 * Math.sqrt(erEff);
  }

  // Differential pair calculations
  let zDiff: number | undefined;
  if (spacing > 0 && (traceType === "diff_microstrip" || traceType === "diff_stripline")) {
    // Approximate differential impedance
    const k = 1 - 0.48 * Math.exp(-0.96 * spacing / dielectricHeight);
    zDiff = 2 * z0 * k;
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
