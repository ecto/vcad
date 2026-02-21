/**
 * Continuous sync hook for electronics workspace.
 *
 * Implements:
 * - Principle 7: Continuous netlist regeneration on schematic change
 * - Principle 3: Real-time DRC/ERC validation
 */

import { useEffect } from "react";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { generateNetlist, runDrc, runErc } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";

export function useElectronicsSync() {
  const active = useElectronicsStore((s) => s.active);
  const schematic = useDocumentStore((s) => s.document.schematic);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
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
}
