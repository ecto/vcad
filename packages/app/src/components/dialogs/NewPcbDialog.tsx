import { useState, useEffect, useCallback } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDocumentStore } from "@vcad/core";
import type { PcbCreateOptions } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useNotificationStore } from "@/stores/notification-store";

interface NewPcbDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialWidth?: number;
  initialHeight?: number;
}

type PresetId = "arduino" | "rpi" | "breakout" | "custom";

interface Preset {
  id: PresetId;
  label: string;
  width: number;
  height: number;
}

const PRESETS: Preset[] = [
  { id: "arduino", label: "Arduino Shield", width: 68.6, height: 53.3 },
  { id: "rpi", label: "RPi HAT", width: 65, height: 56.5 },
  { id: "breakout", label: "Breakout", width: 25, height: 25 },
  { id: "custom", label: "Custom", width: 50, height: 30 },
];

export function NewPcbDialog({ open, onOpenChange, initialWidth, initialHeight }: NewPcbDialogProps) {
  const [preset, setPreset] = useState<PresetId>("custom");
  const [width, setWidth] = useState(initialWidth ?? 50);
  const [height, setHeight] = useState(initialHeight ?? 30);
  const [layers, setLayers] = useState<2 | 4 | 6>(2);
  const [thickness, setThickness] = useState(1.6);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [traceWidth, setTraceWidth] = useState(0.15);
  const [clearance, setClearance] = useState(0.15);

  const addToast = useNotificationStore((s) => s.addToast);

  // Reset fields when dialog opens
  useEffect(() => {
    if (open) {
      if (initialWidth != null || initialHeight != null) {
        setPreset("custom");
        setWidth(initialWidth ?? 50);
        setHeight(initialHeight ?? 30);
      } else {
        setPreset("custom");
        setWidth(50);
        setHeight(30);
      }
      setLayers(2);
      setThickness(1.6);
      setShowAdvanced(false);
      setTraceWidth(0.15);
      setClearance(0.15);
    }
  }, [open, initialWidth, initialHeight]);

  const selectPreset = useCallback((p: Preset) => {
    setPreset(p.id);
    setWidth(p.width);
    setHeight(p.height);
  }, []);

  function handleDimensionChange(setter: (v: number) => void, value: number) {
    setter(value);
    setPreset("custom");
  }

  function handleCreate() {
    const options: PcbCreateOptions = {
      width,
      height,
      layers,
      thickness,
      traceWidth,
      clearance,
    };

    const boardNodeId = useDocumentStore.getState().initPcb(options);
    if (!boardNodeId) {
      addToast("Failed to create PCB — engine not ready, try again", "error");
      onOpenChange(false);
      return;
    }

    // Init schematic if none exists
    const doc = useDocumentStore.getState().document;
    if (!doc.schematic) {
      useDocumentStore.getState().initSchematic();
    }

    // Enter electronics workspace
    useElectronicsStore.getState().enter();

    addToast(`Created PCB Board (${width}x${height}mm, ${layers}-layer)`, "success");
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="New PCB Board">
        <div className="flex flex-col gap-4 py-2">
          {/* Presets */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs text-text-muted">Preset</label>
            <div className="grid grid-cols-4 gap-1">
              {PRESETS.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => selectPreset(p)}
                  className={`px-2 py-1.5 text-[11px] rounded border transition-colors ${
                    preset === p.id
                      ? "border-accent bg-accent/10 text-accent"
                      : "border-border text-text-muted hover:border-text-muted"
                  }`}
                >
                  {p.label}
                </button>
              ))}
            </div>
          </div>

          {/* Dimensions */}
          <div className="grid grid-cols-2 gap-2">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs text-text-muted">Width</label>
              <div className="flex items-center gap-1.5">
                <input
                  type="number"
                  value={width}
                  onChange={(e) => handleDimensionChange(setWidth, Math.max(1, parseFloat(e.target.value) || 1))}
                  min={1}
                  step={0.1}
                  className="min-w-0 flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-accent"
                />
                <span className="shrink-0 text-xs text-text-muted">mm</span>
              </div>
            </div>
            <div className="flex flex-col gap-1.5">
              <label className="text-xs text-text-muted">Height</label>
              <div className="flex items-center gap-1.5">
                <input
                  type="number"
                  value={height}
                  onChange={(e) => handleDimensionChange(setHeight, Math.max(1, parseFloat(e.target.value) || 1))}
                  min={1}
                  step={0.1}
                  className="min-w-0 flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-accent"
                />
                <span className="shrink-0 text-xs text-text-muted">mm</span>
              </div>
            </div>
          </div>

          {/* Layers + Thickness */}
          <div className="grid grid-cols-2 gap-2">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs text-text-muted">Layers</label>
              <div className="grid grid-cols-3 gap-1">
                {([2, 4, 6] as const).map((n) => (
                  <button
                    key={n}
                    type="button"
                    onClick={() => setLayers(n)}
                    className={`px-2 py-1.5 text-xs rounded border transition-colors ${
                      layers === n
                        ? "border-accent bg-accent/10 text-accent"
                        : "border-border text-text-muted hover:border-text-muted"
                    }`}
                  >
                    {n}-layer
                  </button>
                ))}
              </div>
            </div>
            <div className="flex flex-col gap-1.5">
              <label className="text-xs text-text-muted">Thickness</label>
              <div className="flex items-center gap-1.5">
                <input
                  type="number"
                  value={thickness}
                  onChange={(e) => setThickness(Math.max(0.2, parseFloat(e.target.value) || 0.2))}
                  min={0.2}
                  step={0.1}
                  className="min-w-0 flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-accent"
                />
                <span className="shrink-0 text-xs text-text-muted">mm</span>
              </div>
            </div>
          </div>

          {/* Advanced design rules (collapsed by default) */}
          <button
            type="button"
            onClick={() => setShowAdvanced(!showAdvanced)}
            className="flex items-center gap-1 text-xs text-text-muted hover:text-text transition-colors"
          >
            <span className={`transition-transform ${showAdvanced ? "rotate-90" : ""}`}>&#9654;</span>
            Design Rules
          </button>
          {showAdvanced && (
            <div className="grid grid-cols-2 gap-2 pl-3">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs text-text-muted">Trace Width</label>
                <div className="flex items-center gap-1.5">
                  <input
                    type="number"
                    value={traceWidth}
                    onChange={(e) => setTraceWidth(Math.max(0.05, parseFloat(e.target.value) || 0.05))}
                    min={0.05}
                    step={0.01}
                    className="min-w-0 flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-accent"
                  />
                  <span className="shrink-0 text-xs text-text-muted">mm</span>
                </div>
              </div>
              <div className="flex flex-col gap-1.5">
                <label className="text-xs text-text-muted">Clearance</label>
                <div className="flex items-center gap-1.5">
                  <input
                    type="number"
                    value={clearance}
                    onChange={(e) => setClearance(Math.max(0.05, parseFloat(e.target.value) || 0.05))}
                    min={0.05}
                    step={0.01}
                    className="min-w-0 flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-accent"
                  />
                  <span className="shrink-0 text-xs text-text-muted">mm</span>
                </div>
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="default" size="sm" onClick={handleCreate}>
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
