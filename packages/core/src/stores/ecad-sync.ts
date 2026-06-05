/**
 * Pure schematic→PCB reconciliation.
 *
 * Splitting this out of the document store keeps it free of the CRDT engine so
 * it can be unit-tested on plain `Pcb`/`SchematicSheet` values. The store action
 * `syncSchematicToPcb` is a thin wrapper that runs this and commits the result.
 */

import type { Pcb, Footprint, SchematicSheet, PcbLayer } from "@vcad/ir";

/** Minimal netlist shape consumed by the sync (matches the engine's
 *  `NetlistResult`): named nets, each a set of pin connections. */
export interface SyncNetlist {
  nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[];
}

const FALLBACK_PAD_LAYERS: PcbLayer[] = ["FCu", "FPaste", "FMask"];

/**
 * Derive a generic single-row pad layout from a component's schematic pins,
 * used when the component carries no explicit `footprintTemplate`. One pad per
 * pin (numbered to match the schematic) so every placed part is visible and
 * routable instead of landing as an empty, padless footprint.
 */
export function padsFromPins(pins: { number: string }[]): Footprint["pads"] {
  const n = pins.length;
  const pitch = 2.54;
  return pins.map((pin, i) => ({
    number: pin.number,
    padType: "SMD",
    shape: { type: "RoundRect", width: 1.5, height: 1.5, cornerRatio: 0.25 },
    position: { x: (i - (n - 1) / 2) * pitch, y: 0 },
    layers: [...FALLBACK_PAD_LAYERS],
  }));
}

/**
 * Reconcile a PCB against its schematic:
 *  1. Place any schematic component that has no footprint yet. Pads come from
 *     the component's `footprintTemplate` when present, otherwise from its pins.
 *     New footprints are staggered on a coarse grid.
 *  2. Map schematic nets onto the board: assign `pad.net` for every pad the
 *     netlist connects, and union new net names into `pcb.nets`. The schematic
 *     is the source of truth, so a pad no longer on any net has its stale
 *     assignment cleared; net entries are only added (never removed) so
 *     already-routed nets keep their identity. Runs over all footprints, not
 *     just newly placed ones, so re-syncing repairs drift.
 *
 * Returns a fresh `Pcb` (the input is not mutated) and whether anything changed.
 */
export function syncSchematicToPcbData(
  pcb: Pcb,
  schematic: SchematicSheet,
  netlist?: SyncNetlist,
  opts?: { placeUnplaced?: boolean },
): { pcb: Pcb; changed: boolean } {
  // Placement is opt-out so the continuous sync can keep nets current without
  // ever yanking a not-yet-placed component onto the board (only the explicit
  // "place unplaced" action should do that).
  const placeUnplaced = opts?.placeUnplaced ?? true;
  const next = structuredClone(pcb);
  let changed = false;

  // 1. Place missing footprints.
  const existingRefs = new Set(next.footprints.map((fp) => fp.ref));
  let added = 0;
  for (const comp of placeUnplaced ? schematic.components : []) {
    if (existingRefs.has(comp.ref)) continue;
    let pads: Footprint["pads"] = [];
    let graphics: Footprint["graphics"] = [];
    if (comp.properties?.footprintTemplate) {
      try {
        const template = JSON.parse(comp.properties.footprintTemplate);
        pads = template.pads ?? [];
        graphics = template.graphics ?? [];
      } catch {
        /* malformed template — fall through to pin-derived pads */
      }
    }
    if (pads.length === 0) pads = padsFromPins(comp.pins);
    const fpCount = next.footprints.length;
    const staggerX = 10 + ((fpCount + added) % 5) * 10;
    const staggerY = 10 + Math.floor((fpCount + added) / 5) * 10;
    next.footprints.push({
      ref: comp.ref,
      value: comp.value,
      footprintName: comp.footprintId ?? comp.ref,
      position: { x: staggerX, y: staggerY },
      pads,
      graphics,
    });
    added++;
    changed = true;
  }

  // 2. Map nets onto pads + pcb.nets.
  if (netlist) {
    const padNet = new Map<string, string>();
    const netNames = new Set<string>();
    for (const net of netlist.nets) {
      netNames.add(net.name);
      for (const c of net.connections) {
        padNet.set(`${c.component_ref}:${c.pin_number}`, net.name);
      }
    }
    for (const fp of next.footprints) {
      for (const pad of fp.pads) {
        const wanted = padNet.get(`${fp.ref}:${pad.number}`);
        if (wanted !== pad.net) {
          if (wanted) pad.net = wanted;
          else delete pad.net;
          changed = true;
        }
      }
    }
    const known = new Set(next.nets.map((n) => n.name));
    for (const name of netNames) {
      if (!known.has(name)) {
        next.nets.push({ id: name, name });
        changed = true;
      }
    }
  }

  return { pcb: next, changed };
}
