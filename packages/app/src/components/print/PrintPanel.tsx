import { useState, useRef, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Printer } from "@phosphor-icons/react/dist/ssr/Printer";
import { Gear } from "@phosphor-icons/react/dist/ssr/Gear";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { useSlicerStore, formatDuration, type SliceResult } from "@/stores/slicer-store";
import { usePrinterStore } from "@/stores/printer-store";
import { useEngineStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { SlicerSettings } from "./SlicerSettings";
import { PrinterSelect } from "./PrinterSelect";
import { PrinterStatus } from "./PrinterStatus";
import { PrintPreview } from "./PrintPreview";

type Tab = "settings" | "preview" | "printer";

export function PrintPanel() {
  const [activeTab, setActiveTab] = useState<Tab>("settings");
  const sliceResultRef = useRef<SliceResult | null>(null);

  const closePrintPanel = useSlicerStore((s) => s.closePrintPanel);
  const isSlicing = useSlicerStore((s) => s.isSlicing);
  const setSlicing = useSlicerStore((s) => s.setSlicing);
  const stats = useSlicerStore((s) => s.stats);
  const setStats = useSlicerStore((s) => s.setStats);
  const sliceError = useSlicerStore((s) => s.sliceError);
  const setSliceError = useSlicerStore((s) => s.setSliceError);
  const setCurrentLayerPreview = useSlicerStore((s) => s.setCurrentLayerPreview);
  const setSliceResult = useSlicerStore((s) => s.setSliceResult);
  const slicerSettings = useSlicerStore((s) => s.settings);

  const selectedPrinter = usePrinterStore((s) => s.selectedPrinter);
  const connectionState = usePrinterStore((s) => s.connectionState);
  const printTemp = usePrinterStore((s) => s.printTemp);
  const bedTemp = usePrinterStore((s) => s.bedTemp);
  const setPrintTemp = usePrinterStore((s) => s.setPrintTemp);
  const setBedTemp = usePrinterStore((s) => s.setBedTemp);

  const scene = useEngineStore((s) => s.scene);
  const engine = useEngineStore((s) => s.engine);

  const addToast = useNotificationStore((s) => s.addToast);

  const hasMesh = scene?.parts && scene.parts.length > 0;

  const handleSlice = useCallback(async () => {
    if (!engine || !scene?.parts?.length) {
      addToast("No model to slice", "error");
      return;
    }

    setSlicing(true);
    setSliceError(null);
    setStats(null);
    setSliceResult(null);
    sliceResultRef.current = null;

    try {
      // Slicer is not yet implemented in kernel-wasm
      // This will be enabled once vcad-kernel-slicer is complete
      throw new Error("Slicer not yet available. Coming soon!");
    } catch (err) {
      const message = err instanceof Error ? err.message : "Slicing failed";
      setSliceError(message);
      addToast(message, "error");
    } finally {
      setSlicing(false);
    }
  }, [engine, scene, slicerSettings, addToast, setSlicing, setSliceError, setStats, setSliceResult, setCurrentLayerPreview]);

  const handleExportGcode = useCallback(async () => {
    const result = sliceResultRef.current;
    if (!result || !stats) return;

    try {
      // G-code export not yet implemented
      addToast("G-code export coming soon", "info");
    } catch (err) {
      console.error("Export failed:", err);
      addToast("Export failed", "error");
    }
  }, [stats, addToast]);

  async function handleStartPrint() {
    if (!selectedPrinter || connectionState !== "connected") {
      addToast("No printer connected", "error");
      return;
    }

    // In production, this would send the print job to the printer
    addToast("Print job sent to printer", "success");
  }

  return (
    <div className="fixed top-16 right-4 w-80 bg-surface border border-border rounded-lg shadow-lg z-30 flex flex-col max-h-[calc(100vh-5rem)]">
      {/* Header */}
      <div className="flex items-center justify-between p-3 border-b border-border">
        <div className="flex items-center gap-2">
          <Printer size={20} className="text-accent" />
          <span className="font-medium">Print</span>
        </div>
        <button
          onClick={closePrintPanel}
          className="p-1 hover:bg-hover rounded"
        >
          <X size={18} />
        </button>
      </div>

      {/* Tabs */}
      <div className="flex border-b border-border">
        <button
          onClick={() => setActiveTab("settings")}
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "settings"
              ? "text-accent border-b-2 border-accent"
              : "text-text-muted hover:text-text"
          }`}
        >
          <Gear size={16} />
          Settings
        </button>
        <button
          onClick={() => setActiveTab("preview")}
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "preview"
              ? "text-accent border-b-2 border-accent"
              : "text-text-muted hover:text-text"
          }`}
        >
          <Eye size={16} />
          Preview
        </button>
        <button
          onClick={() => setActiveTab("printer")}
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "printer"
              ? "text-accent border-b-2 border-accent"
              : "text-text-muted hover:text-text"
          }`}
        >
          <Printer size={16} />
          Printer
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-3">
        {activeTab === "settings" && (
          <div className="space-y-4">
            <SlicerSettings />

            {/* Temperature settings */}
            <div className="border-t border-border pt-3">
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-xs text-text-muted mb-1">Nozzle °C</label>
                  <input
                    type="number"
                    value={printTemp}
                    onChange={(e) => setPrintTemp(parseInt(e.target.value) || 0)}
                    className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text"
                  />
                </div>
                <div>
                  <label className="block text-xs text-text-muted mb-1">Bed °C</label>
                  <input
                    type="number"
                    value={bedTemp}
                    onChange={(e) => setBedTemp(parseInt(e.target.value) || 0)}
                    className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text"
                  />
                </div>
              </div>
            </div>
          </div>
        )}

        {activeTab === "preview" && (
          <div className="space-y-3">
            {stats ? (
              <>
                <PrintPreview />

                {/* Stats */}
                <div className="grid grid-cols-2 gap-2 text-sm">
                  <div className="bg-hover p-2 rounded">
                    <div className="text-text-muted text-xs">Layers</div>
                    <div className="font-mono">{stats.layerCount}</div>
                  </div>
                  <div className="bg-hover p-2 rounded">
                    <div className="text-text-muted text-xs">Time</div>
                    <div className="font-mono">{formatDuration(stats.printTimeSeconds)}</div>
                  </div>
                  <div className="bg-hover p-2 rounded">
                    <div className="text-text-muted text-xs">Filament</div>
                    <div className="font-mono">{stats.filamentGrams.toFixed(1)}g</div>
                  </div>
                  <div className="bg-hover p-2 rounded">
                    <div className="text-text-muted text-xs">Length</div>
                    <div className="font-mono">{(stats.filamentMm / 1000).toFixed(2)}m</div>
                  </div>
                </div>
              </>
            ) : (
              <div className="text-center py-8 text-text-muted">
                <Eye size={32} className="mx-auto mb-2 opacity-50" />
                <p className="text-sm">Click "Slice" to preview layers</p>
              </div>
            )}
          </div>
        )}

        {activeTab === "printer" && (
          <div className="space-y-3">
            <PrinterSelect />
            <PrinterStatus />
          </div>
        )}
      </div>

      {/* Footer actions */}
      <div className="p-3 border-t border-border space-y-2">
        {sliceError && (
          <div className="text-xs text-red-400 mb-2">{sliceError}</div>
        )}

        {/* Slice button */}
        <button
          onClick={handleSlice}
          disabled={!hasMesh || isSlicing}
          className="w-full flex items-center justify-center gap-2 py-2 bg-accent hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium"
        >
          {isSlicing ? (
            <>
              <Spinner className="animate-spin" size={18} />
              Slicing...
            </>
          ) : (
            <>
              <Eye size={18} />
              Slice
            </>
          )}
        </button>

        {/* Export / Print buttons */}
        {stats && (
          <div className="flex gap-2">
            <button
              onClick={handleExportGcode}
              className="flex-1 flex items-center justify-center gap-1 py-2 bg-hover hover:bg-border rounded text-sm"
            >
              <Export size={16} />
              Export G-code
            </button>
            <button
              onClick={handleStartPrint}
              disabled={!selectedPrinter || connectionState !== "connected"}
              className="flex-1 flex items-center justify-center gap-1 py-2 bg-green-600 hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-sm"
            >
              <Printer size={16} />
              Print
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
