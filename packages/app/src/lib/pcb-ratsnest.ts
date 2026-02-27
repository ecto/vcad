/**
 * Shared ratsnest computation utility.
 *
 * Computes minimum-spanning-tree ratsnest for unrouted net connections.
 */

import type { Footprint, Vec2 } from "@vcad/ir";

export interface RatsnestLine {
  net: string;
  from: Vec2;
  to: Vec2;
  fpRef: string;
  padNum: string;
}

interface NetlistInfo {
  nets: {
    name: string;
    connections: { component_ref: string; pin_number: string }[];
  }[];
}

/** Compute ratsnest lines: unrouted connections between same-net pads. */
export function computeRatsnest(
  footprints: Footprint[],
  netlist: NetlistInfo | null,
  traces: { net: string; start: Vec2; end: Vec2 }[],
): RatsnestLine[] {
  if (!netlist) return [];
  const lines: RatsnestLine[] = [];

  // Build pad position lookup
  const padPositions = new Map<string, Vec2>();
  for (const fp of footprints) {
    for (const pad of fp.pads) {
      const key = `${fp.ref}:${pad.number}`;
      padPositions.set(key, {
        x: fp.position.x + pad.position.x,
        y: fp.position.y + pad.position.y,
      });
    }
  }

  // For each net with >1 connection, create ratsnest between sequential pads
  for (const net of netlist.nets) {
    if (net.connections.length < 2) continue;

    // Check which pairs are already routed (simplified: any trace with matching net)
    const hasTrace = traces.some((t) => t.net === net.name);
    if (hasTrace) continue;

    for (let i = 0; i < net.connections.length - 1; i++) {
      const a = net.connections[i]!;
      const b = net.connections[i + 1]!;
      const posA = padPositions.get(`${a.component_ref}:${a.pin_number}`);
      const posB = padPositions.get(`${b.component_ref}:${b.pin_number}`);
      if (posA && posB) {
        lines.push({
          net: net.name,
          from: posA,
          to: posB,
          fpRef: a.component_ref,
          padNum: a.pin_number,
        });
      }
    }
  }

  return lines;
}
