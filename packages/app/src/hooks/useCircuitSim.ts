/**
 * Live circuit simulation driver — the "come alive" loop.
 *
 * When simulation is on, builds a circuit from the active schematic + netlist,
 * then steps it on every animation frame and publishes the observation (node
 * voltages + device currents) to the electronics store, where the schematic
 * renderer reads it to colour wires by voltage and glow LEDs by current.
 *
 * The sim is rebuilt only when the circuit *structurally* changes (a component,
 * value, or connection edit) — not on every netlist re-gen — so editing a value
 * live re-solves without thrashing.
 */

import { useEffect, useMemo, useRef } from "react";
import { useDocumentStore } from "@vcad/core";
import { createCircuitSim, type CircuitSimHandle } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";
import { buildCircuitSpec } from "@/lib/circuit-build";

const STEPS_PER_FRAME = 80;

export function useCircuitSim() {
  const active = useElectronicsStore((s) => s.active);
  const simulating = useElectronicsStore((s) => s.simulating);
  const netlist = useElectronicsStore((s) => s.netlist);
  const components = useDocumentStore((s) => s.document.schematic?.components);

  // A structural signature: rebuild the sim only when this changes, so a live
  // netlist re-gen with identical topology doesn't reset the running solve.
  const signature = useMemo(() => {
    if (!simulating || !components || !netlist) return "";
    return JSON.stringify({
      c: components.map((c) => [c.ref, c.value, c.properties?.symbolId]),
      n: netlist.nets.map((n) => [n.name, n.connections.length]),
    });
  }, [simulating, components, netlist]);

  const simRef = useRef<CircuitSimHandle | null>(null);
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    if (!active || !simulating || !signature) return;
    const comps = useDocumentStore.getState().document.schematic?.components;
    const nl = useElectronicsStore.getState().netlist;
    if (!comps || !nl) return;

    let cancelled = false;
    const built = buildCircuitSpec(comps, nl);
    const store = useElectronicsStore.getState();
    if (built.spec.devices.length === 0) {
      store.clearSim();
      return;
    }
    store.setSimMaps(
      Object.fromEntries(built.netToNode),
      Object.fromEntries(built.refToDevice),
    );

    createCircuitSim(JSON.stringify(built.spec)).then((sim) => {
      if (cancelled || !sim) return;
      simRef.current = sim;
      const loop = () => {
        if (cancelled || !simRef.current) return;
        try {
          const obs = simRef.current.step(STEPS_PER_FRAME);
          useElectronicsStore.getState().setSimObservation(obs.nodeVoltages, obs.deviceCurrents);
        } catch (e) {
          console.warn("[circuit-sim] step failed", e);
          return;
        }
        rafRef.current = requestAnimationFrame(loop);
      };
      rafRef.current = requestAnimationFrame(loop);
    });

    return () => {
      cancelled = true;
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
      if (simRef.current) {
        try {
          simRef.current.free();
        } catch {
          /* already freed */
        }
        simRef.current = null;
      }
    };
  }, [active, simulating, signature]);
}
