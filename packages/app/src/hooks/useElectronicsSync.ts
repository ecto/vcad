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
import * as THREE from "three";
import {
  useDocumentStore,
  useCoreElectronicsStore,
  useEngineStore,
  getNodePcb,
  isPcbBoardPart,
  findPcbBoardPart,
  getPcbBoardTransform,
} from "@vcad/core";
import { generateNetlist, runDrc, runErc, componentMeshes } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";
import {
  aabbOfPositions,
  interferingRefs,
  type Aabb,
  type PointTransform,
} from "@/lib/pcb-interference";

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

      // Keep the board's nets in sync with the schematic continuously: assign
      // each pad's net from the netlist and register the net names on the PCB.
      // `placeUnplaced: false` means this never auto-drops footprints (only the
      // explicit "place unplaced" action does that); it's idempotent, so it
      // no-ops once pad.net + pcb.nets already match. Without it, footprints
      // auto-created on component-add stay net-less and routing/DRC are blind.
      if (activeBoardNodeId != null) {
        useDocumentStore
          .getState()
          .syncSchematicToPcb(activeBoardNodeId, netlist, { placeUnplaced: false });
      }
    }, 200);
    return () => clearTimeout(timer);
  }, [schematic, pcb, active, activeBoardNodeId]);

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

      // Component bodies are board-local; the mechanical AABBs are world. Map
      // the components into world via the focused board's transform so a board
      // moved/rotated as a part still clashes correctly.
      const boardPart =
        activeBoardNodeId != null ? findPcbBoardPart(parts, activeBoardNodeId) : null;
      let boardToWorld: PointTransform | undefined;
      if (boardPart) {
        const xf = getPcbBoardTransform(document, boardPart);
        const mat = new THREE.Matrix4().compose(
          new THREE.Vector3(xf.position.x, xf.position.y, xf.position.z),
          new THREE.Quaternion().setFromEuler(
            new THREE.Euler(
              (xf.rotationDeg.x * Math.PI) / 180,
              (xf.rotationDeg.y * Math.PI) / 180,
              (xf.rotationDeg.z * Math.PI) / 180,
              "XYZ",
            ),
          ),
          new THREE.Vector3(xf.scale.x, xf.scale.y, xf.scale.z),
        );
        const isIdentity = mat.equals(new THREE.Matrix4());
        if (!isIdentity) {
          const v = new THREE.Vector3();
          boardToWorld = (x, y, z) => {
            v.set(x, y, z).applyMatrix4(mat);
            return [v.x, v.y, v.z];
          };
        }
      }

      useElectronicsStore
        .getState()
        .setInterferingFootprints(interferingRefs(comps, mech, 0, boardToWorld));
    }, 400);
    return () => clearTimeout(timer);
  }, [pcb, active, scene, parts]);
}
