import { useState, useRef, useCallback, useEffect } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Printer } from "@phosphor-icons/react/dist/ssr/Printer";
import { Gear } from "@phosphor-icons/react/dist/ssr/Gear";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { useSlicerStore, formatDuration, type SliceResult } from "@/stores/slicer-store";
import { usePrinterStore, type PrinterStatus as PrinterStatusData } from "@/stores/printer-store";
import { useEngineStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { SlicerSettings } from "./SlicerSettings";
import { PrinterSelect } from "./PrinterSelect";
import { PrinterStatus } from "./PrinterStatus";
import { PrintPreview } from "./PrintPreview";
import { downloadBlob } from "@/lib/download";
import { sendPrint, getPrinterStatus } from "@/lib/print-relay";
import {
  sliceMesh as sliceMeshOnWorker,
  generateGcode as generateGcodeOnWorker,
  generate3mf as generate3mfOnWorker,
  generate3mfWithGcode as generate3mfWithGcodeOnWorker,
  releaseSlice,
  cancelSlicing,
} from "@/lib/slicer-client";

type Tab = "settings" | "preview" | "printer";

/** Collect all mesh data from scene parts into flat vertex/index arrays. */
function collectMeshData(scene: { parts: Array<{ mesh: { positions: Float32Array; indices: Uint32Array } }> }) {
  const allVertices: number[] = [];
  const allIndices: number[] = [];

  for (const part of scene.parts) {
    const baseIdx = allVertices.length / 3;
    for (let i = 0; i < part.mesh.positions.length; i++) {
      allVertices.push(part.mesh.positions[i]!);
    }
    for (let i = 0; i < part.mesh.indices.length; i++) {
      allIndices.push(part.mesh.indices[i]! + baseIdx);
    }
  }

  return {
    vertices: new Float32Array(allVertices),
    indices: new Uint32Array(allIndices),
  };
}

export function PrintPanel() {
  const [activeTab, setActiveTab] = useState<Tab>("settings");
  const [progress, setProgress] = useState<{ stage: string; percent: number } | null>(null);
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
    setProgress({ stage: "Starting", percent: 0 });
    // Free the previous slice result on the worker side before kicking off a new one.
    const previous = sliceResultRef.current;
    setSliceResult(null);
    sliceResultRef.current = null;
    if (previous) {
      releaseSlice(previous).catch(() => { /* worker may be dead, ignore */ });
    }

    try {
      const { vertices, indices } = collectMeshData(scene);
      const outcome = await sliceMeshOnWorker(
        vertices,
        indices,
        slicerSettings,
        (stage, current, total) => {
          const percent = total > 0 ? Math.min(100, Math.round((current / total) * 100)) : 0;
          setProgress({ stage, percent });
        },
      );

      sliceResultRef.current = outcome.handle;
      setSliceResult(outcome.handle);

      const parsedStats = JSON.parse(outcome.statsJson);
      setStats({
        layerCount: parsedStats.layer_count,
        printTimeSeconds: parsedStats.print_time_seconds,
        filamentMm: parsedStats.filament_mm,
        filamentGrams: parsedStats.filament_grams,
        boundsMin: parsedStats.bounds_min,
        boundsMax: parsedStats.bounds_max,
      });

      if (outcome.firstPreview) {
        setCurrentLayerPreview(outcome.firstPreview);
      }

      addToast(`Sliced: ${outcome.handle.layerCount} layers`, "success");
      setActiveTab("preview");
    } catch (err) {
      const message = err instanceof Error ? err.message : "Slicing failed";
      setSliceError(message);
      addToast(message, "error");
    } finally {
      setSlicing(false);
      setProgress(null);
    }
  }, [engine, scene, slicerSettings, addToast, setSlicing, setSliceError, setStats, setSliceResult, setCurrentLayerPreview]);

  const handleCancelSlice = useCallback(() => {
    cancelSlicing();
    sliceResultRef.current = null;
    setSliceResult(null);
    setStats(null);
    setSlicing(false);
    setSliceError("Slicing cancelled");
    setProgress(null);
  }, [setSliceResult, setStats, setSlicing, setSliceError]);

  const handleExportGcode = useCallback(async () => {
    const result = sliceResultRef.current;
    if (!result || !stats) return;

    try {
      const profileId = selectedPrinter?.id || "generic";
      const gcode = await generateGcodeOnWorker(result, profileId, printTemp, bedTemp);
      const blob = new Blob([gcode], { type: "text/plain" });
      downloadBlob(blob, "model.gcode");
      addToast("G-code exported", "success");
    } catch (err) {
      console.error("G-code export failed:", err);
      addToast("G-code export failed", "error");
    }
  }, [stats, selectedPrinter, printTemp, bedTemp, addToast]);

  const handleExport3mf = useCallback(async () => {
    if (!scene?.parts?.length) return;

    try {
      const { vertices, indices } = collectMeshData(scene);
      const settingsJson = JSON.stringify({
        layer_height: slicerSettings.layerHeight,
        first_layer_height: slicerSettings.firstLayerHeight,
        wall_count: slicerSettings.wallCount,
        infill_density: slicerSettings.infillDensity,
        print_temp: printTemp,
        bed_temp: bedTemp,
      });

      const bytes = await generate3mfOnWorker("model", vertices, indices, settingsJson);
      const blob = new Blob([bytes as unknown as BlobPart], { type: "application/vnd.ms-package.3dmanufacturing-3dmodel+xml" });
      downloadBlob(blob, "model.3mf");
      addToast("3MF exported", "success");
    } catch (err) {
      console.error("3MF export failed:", err);
      addToast("3MF export failed", "error");
    }
  }, [scene, slicerSettings, printTemp, bedTemp, addToast]);

  const selectedProfile = usePrinterStore((s) => s.selectedProfile);
  const setStatus = usePrinterStore((s) => s.setStatus);
  const [isPrinting, setIsPrinting] = useState(false);

  async function handleStartPrint() {
    if (!selectedPrinter || connectionState !== "connected") {
      addToast("No printer connected", "error");
      return;
    }
    const result = sliceResultRef.current;
    if (!result || !scene?.parts?.length) {
      addToast("Slice the model first", "error");
      return;
    }

    setIsPrinting(true);
    try {
      const gcodeText = await generateGcodeOnWorker(result, selectedProfile, printTemp, bedTemp);
      const gcodeBytes = new TextEncoder().encode(gcodeText);

      const { vertices, indices } = collectMeshData(scene);
      const settingsJson = JSON.stringify({
        layer_height: slicerSettings.layerHeight,
        first_layer_height: slicerSettings.firstLayerHeight,
        wall_count: slicerSettings.wallCount,
        infill_density: slicerSettings.infillDensity,
        print_temp: printTemp,
        bed_temp: bedTemp,
      });

      const threemfBytes = await generate3mfWithGcodeOnWorker(
        result,
        "vcad_print",
        vertices,
        indices,
        gcodeBytes,
        settingsJson,
      );

      await sendPrint(threemfBytes, "vcad_print.gcode.3mf");
      addToast("Print job sent to printer", "success");
      setActiveTab("printer");
    } catch (err) {
      const message = err instanceof Error ? err.message : "Print failed";
      console.error("Print failed:", err);
      addToast(message, "error");
    } finally {
      setIsPrinting(false);
    }
  }

  // Poll printer status while connected so PrinterStatus stays live.
  useEffect(() => {
    if (connectionState !== "connected") return;
    let cancelled = false;
    const tick = async () => {
      try {
        const s = await getPrinterStatus();
        if (cancelled) return;
        setStatus({
          state: s.state as PrinterStatusData["state"],
          progressPercent: s.progress_percent,
          layerCurrent: s.layer_current,
          layerTotal: s.layer_total,
          timeRemainingMin: s.time_remaining_min,
          timeElapsedMin: 0,
          nozzleTemp: s.nozzle_temp,
          nozzleTarget: s.nozzle_target,
          bedTemp: s.bed_temp,
          bedTarget: s.bed_target,
          filename: s.filename,
        });
      } catch {
        // Ignore transient errors; the next tick will retry.
      }
    };
    tick();
    const id = window.setInterval(tick, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [connectionState, setStatus]);

  return (
    <div className="fixed top-16 right-4 w-80 bg-surface border border-border rounded-lg shadow-lg z-30 flex flex-col max-h-[calc(100vh-5rem)]">
      {/* Header */}
      <div className="flex items-center justify-between p-3 border-b border-border">
        <div className="flex items-center gap-2">
          <Printer size={20} className="text-brand" />
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
              ? "text-brand border-b-2 border-brand"
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
              ? "text-brand border-b-2 border-brand"
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
              ? "text-brand border-b-2 border-brand"
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

        {/* Slice button (turns into Cancel + progress bar while in flight) */}
        {isSlicing ? (
          <div className="space-y-2">
            <div className="flex gap-2">
              <button
                disabled
                className="flex-1 flex items-center justify-center gap-2 py-2 bg-brand/60 text-white rounded font-medium cursor-not-allowed"
              >
                <Spinner className="animate-spin" size={18} />
                {progress?.stage ?? "Slicing"}
                {progress ? ` ${progress.percent}%` : "…"}
              </button>
              <button
                onClick={handleCancelSlice}
                className="px-3 py-2 bg-hover hover:bg-border rounded text-sm"
              >
                Cancel
              </button>
            </div>
            <div className="h-1 w-full bg-hover rounded overflow-hidden">
              <div
                className="h-full bg-brand transition-[width] duration-150"
                style={{ width: `${progress?.percent ?? 0}%` }}
              />
            </div>
          </div>
        ) : (
          <button
            onClick={handleSlice}
            disabled={!hasMesh}
            className="w-full flex items-center justify-center gap-2 py-2 bg-brand hover:bg-brand/90 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium"
          >
            <Eye size={18} />
            Slice
          </button>
        )}

        {/* Export / Print buttons */}
        {stats && (
          <div className="flex gap-2">
            <button
              onClick={handleExportGcode}
              className="flex-1 flex items-center justify-center gap-1 py-2 bg-hover hover:bg-border rounded text-sm"
            >
              <Export size={16} />
              G-code
            </button>
            <button
              onClick={handleExport3mf}
              className="flex-1 flex items-center justify-center gap-1 py-2 bg-hover hover:bg-border rounded text-sm"
            >
              <Export size={16} />
              3MF
            </button>
            <button
              onClick={handleStartPrint}
              disabled={!selectedPrinter || connectionState !== "connected" || isPrinting}
              className="flex-1 flex items-center justify-center gap-1 py-2 bg-green-600 hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-sm"
            >
              {isPrinting ? (
                <>
                  <Spinner className="animate-spin" size={16} />
                  Sending
                </>
              ) : (
                <>
                  <Printer size={16} />
                  Print
                </>
              )}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
