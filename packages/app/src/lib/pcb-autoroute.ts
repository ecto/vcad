/**
 * Best-effort autoroute of unrouted ratsnest connections on the focused board
 * (Phase 2/3). Routes each ratsnest line with the grid router (engine
 * `routeNet`) and commits successful routes as FCu traces.
 *
 * This is the in-app twin of the `route_nets` MCP tool the AI calls — same
 * engine, same live focused board — so "route the power nets" from chat and
 * the toolbar button drive identical paths.
 */

import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { computeRatsnest, routeNet } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";

export async function autorouteRatsnest(): Promise<{ routed: number; failed: number }> {
  const boardNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
  if (boardNodeId == null) return { routed: 0, failed: 0 };
  const netlist = useElectronicsStore.getState().netlist;
  let pcb = getNodePcb(useDocumentStore.getState().document, boardNodeId);
  if (!pcb || !netlist) return { routed: 0, failed: 0 };

  const width = pcb.rules?.defaultRules?.traceWidth ?? 0.15;
  const rats = await computeRatsnest(pcb, netlist);

  let routed = 0;
  let failed = 0;
  for (const line of rats) {
    // Re-read the board each pass so freshly committed traces are seen as
    // obstacles by the router (avoids stacking routes on top of each other).
    pcb = getNodePcb(useDocumentStore.getState().document, boardNodeId);
    if (!pcb) break;
    const res = await routeNet(pcb, line.net, line.from, line.to, width);
    if (res.success && res.segments.length > 0) {
      for (const [start, end] of res.segments) {
        useDocumentStore.getState().addTrace(boardNodeId, {
          start: { x: start.x, y: start.y, z: 0 },
          end: { x: end.x, y: end.y, z: 0 },
          width,
          layer: "FCu",
          net: line.net,
        });
      }
      routed++;
    } else {
      failed++;
    }
  }
  return { routed, failed };
}
