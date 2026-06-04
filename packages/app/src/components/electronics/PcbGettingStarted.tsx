/**
 * Getting-started overlay shown on the PCB viewport when the board has 0 components.
 * Auto-dismisses when the first component is placed, or via the dismiss button.
 */

import { useState, useCallback } from "react";
import { Upload } from "@phosphor-icons/react/dist/ssr/Upload";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useNotificationStore } from "@/stores/notification-store";
import { useTheme } from "@/hooks/useTheme";
import { useSymbolLibrary } from "./symbol-library";
import type { Pcb } from "@vcad/ir";

function boardSummary(pcb: Pcb): string {
  const verts = pcb.outline.vertices;
  if (verts.length < 3) return "Empty board";

  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (const v of verts) {
    if (v.x < minX) minX = v.x;
    if (v.x > maxX) maxX = v.x;
    if (v.y < minY) minY = v.y;
    if (v.y > maxY) maxY = v.y;
  }
  const w = Math.round(maxX - minX);
  const h = Math.round(maxY - minY);

  const copperLayers = pcb.stackup.layers.filter(
    (l) => l.layer.endsWith("Cu"),
  ).length;
  const thickness = pcb.outline.thickness;

  return `${w} \u00d7 ${h}mm \u00b7 ${copperLayers}-layer \u00b7 ${thickness}mm`;
}

export function PcbGettingStarted() {
  const [dismissed, setDismissed] = useState(false);
  const { isDark } = useTheme();
  const addToast = useNotificationStore((s) => s.addToast);

  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const doc = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(doc, activeBoardNodeId) : null;
  const symbols = useSymbolLibrary();

  const hasComponents = pcb ? pcb.footprints.length > 0 : false;

  const placeComponent = useCallback((symbolId: string) => {
    useElectronicsStore.setState({
      focusedPane: "schematic",
      schTool: "place",
      schPlacingSymbol: symbolId,
      schPlacingRotation: 0,
    });
    setDismissed(true);
  }, []);

  const handleImportKicad = useCallback(() => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".kicad_pcb";
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      try {
        const text = await file.text();
        const { parseKicadPcb } = await import("@vcad/engine");
        const result = await parseKicadPcb(text);
        if (result) {
          useDocumentStore.getState().importPcb(result, file.name);
          useElectronicsStore.getState().enter();
          addToast(`Imported ${file.name}`, "success");
        } else {
          addToast("Failed to parse KiCad PCB", "error");
        }
      } catch {
        addToast("KiCad import not available", "error");
      }
    };
    input.click();
  }, [addToast]);

  // Don't show if dismissed, no PCB, or has components already
  if (dismissed || !pcb || hasComponents) return null;

  const bg = isDark ? "rgba(20, 20, 20, 0.92)" : "rgba(255, 255, 255, 0.92)";
  const border = isDark ? "#333" : "#ddd";
  const cardBg = isDark ? "rgba(40, 40, 40, 0.8)" : "rgba(245, 245, 245, 0.8)";
  const cardHover = isDark ? "rgba(60, 60, 60, 0.9)" : "rgba(235, 235, 235, 0.9)";

  return (
    <div
      className="absolute inset-0 z-30 flex items-center justify-center pointer-events-none"
    >
      <div
        className="relative flex flex-col items-center gap-5 p-6 rounded-lg pointer-events-auto"
        style={{
          backgroundColor: bg,
          border: `1px solid ${border}`,
          backdropFilter: "blur(12px)",
          maxWidth: 480,
          width: "90%",
        }}
      >
        {/* Dismiss button */}
        <button
          onClick={() => setDismissed(true)}
          className="absolute top-2 right-2 text-text-muted hover:text-text transition-colors p-1"
          title="Dismiss"
        >
          <X size={14} />
        </button>

        {/* Board summary */}
        <div className="flex flex-col items-center gap-1">
          <span className="text-xs font-medium text-text-muted uppercase tracking-wider">
            Board Created
          </span>
          <span className="text-sm text-text">
            {boardSummary(pcb)}
          </span>
        </div>

        {/* Component grid — click to place */}
        <div className="w-full">
          <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
            Add a component
          </span>
          <div className="grid grid-cols-4 gap-1.5 mt-1.5">
            {symbols.map((sym) => (
              <button
                key={sym.id}
                onClick={() => placeComponent(sym.id)}
                className="flex flex-col items-center gap-1 p-2 rounded border transition-colors"
                style={{ backgroundColor: cardBg, borderColor: border }}
                onMouseEnter={(e) => { e.currentTarget.style.backgroundColor = cardHover; }}
                onMouseLeave={(e) => { e.currentTarget.style.backgroundColor = cardBg; }}
              >
                <span className="text-xs font-medium text-text">{sym.name}</span>
                <span className="text-[10px] text-text-muted">{sym.defaultValue}</span>
              </button>
            ))}
          </div>
        </div>

        {/* Import alternative */}
        <button
          onClick={handleImportKicad}
          className="flex items-center gap-1.5 text-xs text-text-muted hover:text-text transition-colors"
        >
          <Upload size={14} />
          or import a .kicad_pcb file
        </button>

        {/* Tip */}
        <p className="text-[11px] text-text-muted text-center leading-relaxed">
          Click a component to start placing it on the schematic.
        </p>
      </div>
    </div>
  );
}
