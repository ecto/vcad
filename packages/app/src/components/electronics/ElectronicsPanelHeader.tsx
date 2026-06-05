/**
 * Persistent header for the electronics property panel.
 *
 * Consolidates what used to be two floating HUDs — the top-left status overlay
 * (mode/tool, DRC/ERC, active layer) and the top-right "Board Overview" card
 * (footprint/trace/via/net/layer/component counts) — into a single strip that
 * sits above the contextual panel body (a selected ECAD item, or the board
 * transform when nothing is selected). Shown whenever electronics is active.
 */

import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";

function Stat({
  label,
  value,
  danger,
}: {
  label: string;
  value: number;
  danger?: boolean;
}) {
  return (
    <span className="inline-flex items-baseline gap-1">
      <span className="text-text-muted">{label}</span>
      <span className={danger && value > 0 ? "text-danger font-medium" : "text-text font-medium"}>
        {value}
      </span>
    </span>
  );
}

export function ElectronicsPanelHeader() {
  const focusedPane = useElectronicsStore((s) => s.focusedPane);
  const schTool = useElectronicsStore((s) => s.schTool);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const ercViolations = useElectronicsStore((s) => s.ercViolations);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;
  const schematic = document.schematic;

  const currentTool = focusedPane === "pcb" ? pcbTool : schTool;
  const toolLabel = currentTool.charAt(0).toUpperCase() + currentTool.slice(1);
  const drcErrors = drcViolations.filter((v) => v.severity === "Error").length;
  const drcWarnings = drcViolations.length - drcErrors;
  const ercCount = ercViolations.length;
  const activeLayerConfig = pcbLayers.find((l) => l.layer === pcbActiveLayer);

  return (
    <div className="shrink-0 border-b border-border/40 bg-surface/60 px-3 py-2 text-[11px]">
      {/* Title + health */}
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-text">Circuit</span>
        <div className="flex items-center gap-2 shrink-0">
          <span className="inline-flex items-center gap-1" title="DRC errors / warnings">
            <span className="text-text-muted">DRC</span>
            <span className={drcErrors > 0 ? "text-danger font-medium" : "text-text-muted"}>
              {drcErrors}
            </span>
            <span className={drcWarnings > 0 ? "text-warning" : "text-text-muted"}>
              ⚠{drcWarnings}
            </span>
          </span>
          <span className="inline-flex items-center gap-1" title="ERC issues">
            <span className="text-text-muted">ERC</span>
            <span className={ercCount > 0 ? "text-warning font-medium" : "text-text-muted"}>
              {ercCount}
            </span>
          </span>
        </div>
      </div>

      {/* Mode · tool · active layer */}
      <div className="mt-0.5 flex items-center gap-1.5 text-text-muted">
        <span className="capitalize">{focusedPane}</span>
        <span>·</span>
        <span className="text-text">{toolLabel}</span>
        {focusedPane === "pcb" && activeLayerConfig && (
          <span className="ml-auto inline-flex items-center gap-1" title="Active layer">
            <span
              className="w-2 h-2 rounded-full"
              style={{ backgroundColor: activeLayerConfig.color }}
            />
            <span>{pcbActiveLayer}</span>
          </span>
        )}
      </div>

      {/* Board overview counts */}
      <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5">
        {pcb && (
          <>
            <Stat label="Footprints" value={pcb.footprints.length} />
            <Stat label="Traces" value={pcb.traces.length} />
            <Stat label="Vias" value={pcb.vias.length} />
            <Stat label="Nets" value={pcb.nets.length} />
            <Stat
              label="Layers"
              value={pcb.stackup.layers.filter((l) => l.copperThickness).length}
            />
          </>
        )}
        {schematic && <Stat label="Components" value={schematic.components.length} />}
      </div>
    </div>
  );
}

export default ElectronicsPanelHeader;
