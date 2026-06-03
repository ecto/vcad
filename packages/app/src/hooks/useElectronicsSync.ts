/**
 * Continuous sync hook for electronics workspace.
 *
 * Implements:
 * - Principle 7: Continuous netlist regeneration on schematic change
 * - Principle 3: Real-time DRC/ERC validation
 * - Phase 3: Live route-vs-enclosure interference (component bodies vs the
 *   mechanical parts sharing the canvas)
 */

import { useEffect } from "react";
import {
  useDocumentStore,
  useCoreElectronicsStore,
  useEngineStore,
  getNodePcb,
  isPcbBoardPart,
} from "@vcad/core";
import { generateNetlist, runDrc, runErc, componentMeshes } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";
import { aabbOfPositions, interferingRefs, type Aabb } from "@/lib/pcb-interference";

export function useElectronicsSync() {
  const active = useElectronicsStore((s) => s.active);
  const schematic = useDocumentStore((s) => s.document.schematic);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const scene = useEngineStore((s) => s.scene);
  const parts = useDocumentStore((s) => s.parts);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;

  // Principle 7: Continuous netlist sync
  useEffect(() => {
    if (!active || !schematic) return;
    const timer = setTimeout(async () => {
      const netlist = await generateNetlist(schematic);
      const store = useElectronicsStore.getState();
      store.setNetlist(netlist);

      // Compute sync status
      const schRefs = new Set(schematic.components.map((c) => c.ref));
      const pcbRefs = new Set((pcb?.footprints ?? []).map((f) => f.ref));
      store.setOrphanFootprints([...pcbRefs].filter((r) => !schRefs.has(r)));
      store.setUnplacedComponents([...schRefs].filter((r) => !pcbRefs.has(r)));
    }, 200);
    return () => clearTimeout(timer);
  }, [schematic, pcb, active]);

  // Principle 3: Real-time DRC
  useEffect(() => {
    if (!active || !pcb) return;
    const timer = setTimeout(async () => {
      const violations = await runDrc(pcb);
      useElectronicsStore.getState().setDrcViolations(violations);
    }, 300);
    return () => clearTimeout(timer);
  }, [pcb, active]);

  // Real-time ERC
  useEffect(() => {
    if (!active || !schematic) return;
    const timer = setTimeout(async () => {
      const violations = await runErc(schematic);
      useElectronicsStore.getState().setErcViolations(violations);
    }, 300);
    return () => clearTimeout(timer);
  }, [schematic, active]);

  // Phase 3: route-vs-enclosure interference. AABB overlap between each
  // component's 3D body and the surrounding mechanical parts (the board body
  // itself is excluded). Debounced; runs alongside DRC.
  useEffect(() => {
    if (!active || !pcb) {
      useElectronicsStore.getState().setInterferingFootprints([]);
      return;
    }
    const timer = setTimeout(async () => {
      const comps = await componentMeshes(pcb);
      const mech: Aabb[] = [];
      scene?.parts.forEach((ep, idx) => {
        const pi = parts[idx];
        if (pi && isPcbBoardPart(pi)) return; // don't clash with the board itself
        const bb = aabbOfPositions(ep.mesh?.positions);
        if (bb) mech.push(bb);
      });
      useElectronicsStore
        .getState()
        .setInterferingFootprints(interferingRefs(comps, mech));
    }, 400);
    return () => clearTimeout(timer);
  }, [pcb, active, scene, parts]);
}
