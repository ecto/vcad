/**
 * Layer visibility popover for the PCB canvas.
 * Click layer name to set active, eye toggle for visibility.
 */

import { useState } from "react";
import { useElectronicsStore } from "@/stores/electronics-store";

export function LayerPanel() {
  const [open, setOpen] = useState(false);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const setPcbActiveLayer = useElectronicsStore((s) => s.setPcbActiveLayer);
  const setLayerVisible = useElectronicsStore((s) => s.setLayerVisible);

  return (
    <div className="relative">
      <button
        className="px-2 py-0.5 text-[11px] text-text-muted hover:text-text rounded hover:bg-surface-hover transition-colors"
        onClick={() => setOpen(!open)}
      >
        Layers
      </button>

      {open && (
        <>
          {/* Backdrop */}
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />

          {/* Popover */}
          <div className="absolute right-0 top-full mt-1 z-50 w-48 rounded-lg border border-border bg-surface shadow-lg p-1.5">
            <div className="text-[10px] text-text-muted px-1.5 pb-1 font-medium uppercase tracking-wider">
              PCB Layers
            </div>
            {pcbLayers.map((cfg) => (
              <div
                key={cfg.layer}
                className={`flex items-center gap-1.5 px-1.5 py-0.5 rounded text-[11px] cursor-pointer transition-colors ${
                  cfg.layer === pcbActiveLayer
                    ? "bg-accent/15 text-accent"
                    : "text-text hover:bg-surface-hover"
                }`}
                onClick={() => setPcbActiveLayer(cfg.layer)}
              >
                {/* Color dot */}
                <span
                  className="w-2.5 h-2.5 rounded-full shrink-0"
                  style={{ backgroundColor: cfg.color, opacity: cfg.visible ? 1 : 0.3 }}
                />
                {/* Name */}
                <span className="flex-1 truncate">{cfg.layer}</span>
                {/* Visibility toggle */}
                <button
                  className={`text-[10px] px-1 rounded ${
                    cfg.visible ? "text-text-muted" : "text-text-muted/40"
                  } hover:bg-surface-hover`}
                  onClick={(e) => {
                    e.stopPropagation();
                    setLayerVisible(cfg.layer, !cfg.visible);
                  }}
                  title={cfg.visible ? "Hide layer" : "Show layer"}
                >
                  {cfg.visible ? "eye" : "off"}
                </button>
              </div>
            ))}
            <div className="text-[9px] text-text-muted px-1.5 pt-1.5 border-t border-border mt-1">
              F = Front Cu, B = Back Cu
            </div>
          </div>
        </>
      )}
    </div>
  );
}
