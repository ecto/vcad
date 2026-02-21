/**
 * Top-left floating status panel for electronics mode.
 * Shows mode name, current tool, DRC/ERC counts, and active layer.
 * Mirrors the pattern of SketchStatusPanel.
 */

import { cn } from "@/lib/utils";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useUiStore } from "@vcad/core";

const TOOL_HINTS: Record<string, string> = {
  select: "Click to select",
  move: "Drag to move",
  place: "Click to place component",
  wire: "Click start pin, then end pin",
  label: "Click to place label",
  delete: "Click to delete",
  route: "Click start pad, then route",
};

export function ElectronicsStatusPanel() {
  const focusedPane = useElectronicsStore((s) => s.focusedPane);
  const schTool = useElectronicsStore((s) => s.schTool);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const ercViolations = useElectronicsStore((s) => s.ercViolations);
  const placingSymbol = useElectronicsStore((s) => s.schPlacingSymbol);
  const isOrbiting = useUiStore((s) => s.isOrbiting);

  const currentTool = focusedPane === "pcb" ? pcbTool : schTool;
  const toolLabel = currentTool.charAt(0).toUpperCase() + currentTool.slice(1);
  const hint = placingSymbol
    ? `Placing ${placingSymbol} — click to place, R to rotate`
    : TOOL_HINTS[currentTool] ?? "";

  const drcErrors = drcViolations.filter((v) => v.severity === "Error").length;
  const drcWarnings = drcViolations.length - drcErrors;
  const ercCount = ercViolations.length;

  const activeLayerConfig = pcbLayers.find((l) => l.layer === pcbActiveLayer);

  return (
    <div
      className={cn(
        "fixed left-2 sm:left-4 top-2 sm:top-4 z-30",
        "bg-surface/95 backdrop-blur-sm border border-border",
        "px-3 py-2 shadow-lg",
        "transition-opacity duration-200",
        isOrbiting && "opacity-0 pointer-events-none",
      )}
    >
      <div className="flex flex-col gap-1.5">
        {/* Header */}
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-text">Electronics</span>
          <span className="text-xs text-text-muted capitalize">{focusedPane}</span>
        </div>

        {/* Current tool + hint */}
        <div className="flex items-center gap-2 text-xs">
          <span className="text-text font-medium">{toolLabel}</span>
          <span className="text-text-muted">{hint}</span>
        </div>

        {/* DRC/ERC + Layer */}
        <div className="flex items-center gap-3 text-xs">
          {/* DRC/ERC counts */}
          <div className="flex items-center gap-1.5">
            <span className="text-text-muted">DRC:</span>
            <span className={drcErrors > 0 ? "text-danger" : "text-text-muted"}>
              {drcErrors}
            </span>
            <span className={drcWarnings > 0 ? "text-warning" : "text-text-muted"}>
              {drcWarnings}
            </span>
            <span className="text-text-muted ml-1">ERC:</span>
            <span className={ercCount > 0 ? "text-warning" : "text-text-muted"}>
              {ercCount}
            </span>
          </div>

          {/* Active layer indicator */}
          {focusedPane === "pcb" && activeLayerConfig && (
            <div className="flex items-center gap-1">
              <span
                className="w-2 h-2 rounded-full"
                style={{ backgroundColor: activeLayerConfig.color }}
              />
              <span className="text-text-muted">{pcbActiveLayer}</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default ElectronicsStatusPanel;
