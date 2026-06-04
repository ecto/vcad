/**
 * Floating board-overview HUD for the electronics workspace.
 *
 * Per-item inspectors (component / footprint / net / trace / via / pad) now
 * render in the main contextual PropertyPanel via `EcadFeatureInspector`
 * (Task 3, modeless PCB editing). This overlay is the always-on board summary:
 * counts + DRC/ERC health, so the board's state stays glanceable while you
 * inspect an individual feature on the left.
 */

import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { Row } from "./EcadFeatureInspector";

export function ElectronicsPropertyPanel() {
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const ercViolations = useElectronicsStore((s) => s.ercViolations);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;
  const schematic = document.schematic;

  return (
    <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
      <div className="font-medium text-text mb-2">Board Overview</div>
      <div className="space-y-1 text-text-muted">
        {pcb && (
          <>
            <Row label="Footprints" value={String(pcb.footprints.length)} />
            <Row label="Traces" value={String(pcb.traces.length)} />
            <Row label="Vias" value={String(pcb.vias.length)} />
            <Row label="Nets" value={String(pcb.nets.length)} />
            <Row
              label="Layers"
              value={String(pcb.stackup.layers.filter((l) => l.copperThickness).length)}
            />
          </>
        )}
        {schematic && (
          <Row label="Components" value={String(schematic.components.length)} />
        )}
        <div className="pt-1 border-t border-border mt-1">
          <Row
            label="DRC Errors"
            value={String(drcViolations.filter((v) => v.severity === "Error").length)}
            danger={drcViolations.some((v) => v.severity === "Error")}
          />
          <Row
            label="DRC Warnings"
            value={String(drcViolations.filter((v) => v.severity === "Warning").length)}
          />
          <Row
            label="ERC Issues"
            value={String(ercViolations.length)}
            danger={ercViolations.length > 0}
          />
        </div>
      </div>
    </div>
  );
}
