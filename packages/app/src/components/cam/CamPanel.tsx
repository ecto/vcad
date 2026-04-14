import { useState, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Gear } from "@phosphor-icons/react/dist/ssr/Gear";
import { List } from "@phosphor-icons/react/dist/ssr/List";
import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { useCamStore, formatMachiningTime, toolToJson } from "@/stores/cam-store";
import { useNotificationStore } from "@/stores/notification-store";
import { downloadBlob } from "@/lib/download";
import { ToolLibrary } from "./ToolLibrary";
import { OperationList } from "./OperationList";
import { CamSettings } from "./CamSettings";

type Tab = "operations" | "tools" | "settings";

// Cache for WASM module
let wasmModule: typeof import("@vcad/kernel-wasm") | null = null;

async function loadCamWasm(): Promise<typeof import("@vcad/kernel-wasm") | null> {
  if (wasmModule) return wasmModule;
  try {
    const wasm = await import("@vcad/kernel-wasm");
    // Check if CAM functions are available
    if (typeof (wasm as Record<string, unknown>).isCamAvailable !== "function") {
      return null;
    }
    wasmModule = wasm;
    return wasmModule;
  } catch {
    return null;
  }
}

export function CamPanel() {
  const [activeTab, setActiveTab] = useState<Tab>("operations");

  const closeCamPanel = useCamStore((s) => s.closeCamPanel);
  const operations = useCamStore((s) => s.operations);
  const tools = useCamStore((s) => s.tools);
  const settings = useCamStore((s) => s.settings);

  const isGenerating = useCamStore((s) => s.isGenerating);
  const setGenerating = useCamStore((s) => s.setGenerating);
  const generateError = useCamStore((s) => s.generateError);
  const setGenerateError = useCamStore((s) => s.setGenerateError);
  const setToolpathJson = useCamStore((s) => s.setToolpathJson);
  const setGcodeOutput = useCamStore((s) => s.setGcodeOutput);
  const stats = useCamStore((s) => s.stats);
  const setStats = useCamStore((s) => s.setStats);
  const gcodeOutput = useCamStore((s) => s.gcodeOutput);

  const addToast = useNotificationStore((s) => s.addToast);

  const enabledOperations = operations.filter((op) => op.enabled);
  const hasOperations = enabledOperations.length > 0;

  const handleGenerate = useCallback(async () => {
    if (!hasOperations) {
      addToast("No operations to generate", "error");
      return;
    }

    setGenerating(true);
    setGenerateError(null);
    setStats(null);
    setGcodeOutput(null);

    try {
      const wasm = await loadCamWasm();
      if (!wasm) {
        throw new Error("CAM not available in this WASM build");
      }

      // Type assertion for CAM-specific functions
      const camWasm = wasm as typeof wasm & {
        isCamAvailable: () => boolean;
        WasmCamSettings: new () => {
          stepover: number;
          stepdown: number;
          feed_rate: number;
          plunge_rate: number;
          spindle_rpm: number;
          safe_z: number;
          retract_z: number;
        };
        camGenerateFace: (minX: number, minY: number, maxX: number, maxY: number, depth: number, toolJson: string, settings: unknown) => string;
        camGeneratePocket: (x: number, y: number, width: number, height: number, depth: number, toolJson: string, settings: unknown) => string;
        camGenerateCircularPocket: (centerX: number, centerY: number, radius: number, depth: number, toolJson: string, settings: unknown) => string;
        camGenerateContour: (x: number, y: number, width: number, height: number, depth: number, offset: number, tabCount: number, tabWidth: number, tabHeight: number, toolJson: string, settings: unknown) => string;
        camToolpathStats: (toolpathJson: string) => { cutting_length?: number; estimated_time?: number; segment_count?: number };
        camExportGcode: (toolpathJson: string, name: string, toolJson: string, settings: unknown) => string;
      };

      // Check if CAM is available
      if (!camWasm.isCamAvailable()) {
        throw new Error("CAM not available in this WASM build");
      }

      // Create WASM settings
      const wasmSettings = new camWasm.WasmCamSettings();
      wasmSettings.stepover = settings.stepover;
      wasmSettings.stepdown = settings.stepdown;
      wasmSettings.feed_rate = settings.feedRate;
      wasmSettings.plunge_rate = settings.plungeRate;
      wasmSettings.spindle_rpm = settings.spindleRpm;
      wasmSettings.safe_z = settings.safeZ;
      wasmSettings.retract_z = settings.retractZ;

      // Generate toolpath for each operation and collect G-code
      let allGcode = "";
      const totalStats = {
        cuttingLength: 0,
        estimatedTime: 0,
        segmentCount: 0,
      };

      for (const op of enabledOperations) {
        const tool = tools.find((t) => t.id === op.toolId);
        if (!tool) {
          throw new Error(`Tool not found for operation "${op.name}"`);
        }

        const toolJson = toolToJson(tool);
        let toolpathJson: string;

        switch (op.type) {
          case "face":
            toolpathJson = camWasm.camGenerateFace(
              op.minX,
              op.minY,
              op.maxX,
              op.maxY,
              op.depth,
              toolJson,
              wasmSettings
            );
            break;

          case "pocket":
            toolpathJson = camWasm.camGeneratePocket(
              op.x,
              op.y,
              op.width,
              op.height,
              op.depth,
              toolJson,
              wasmSettings
            );
            break;

          case "pocket_circle":
            toolpathJson = camWasm.camGenerateCircularPocket(
              op.centerX,
              op.centerY,
              op.radius,
              op.depth,
              toolJson,
              wasmSettings
            );
            break;

          case "contour":
            toolpathJson = camWasm.camGenerateContour(
              op.x,
              op.y,
              op.width,
              op.height,
              op.depth,
              op.offset,
              op.tabCount,
              op.tabWidth,
              op.tabHeight,
              toolJson,
              wasmSettings
            );
            break;

          case "roughing3d":
            // 3D roughing requires a height field from drop-cutter analysis
            // For now, skip operations without a mesh
            throw new Error(
              `3D roughing operation "${op.name}" requires a part mesh. Select a part first.`
            );
        }

        // Get stats for this operation
        const opStats = camWasm.camToolpathStats(toolpathJson);
        totalStats.cuttingLength += opStats.cutting_length || 0;
        totalStats.estimatedTime += opStats.estimated_time || 0;
        totalStats.segmentCount += opStats.segment_count || 0;

        // Generate G-code
        const gcode = camWasm.camExportGcode(
          toolpathJson,
          op.name,
          toolJson,
          wasmSettings
        );

        allGcode += `\n; === ${op.name} ===\n${gcode}\n`;

        setToolpathJson(toolpathJson);
      }

      setStats({
        cuttingLength: totalStats.cuttingLength,
        estimatedTime: totalStats.estimatedTime,
        segmentCount: totalStats.segmentCount,
        boundingBox: null,
      });

      setGcodeOutput(allGcode);
      addToast("Toolpath generated successfully", "success");
    } catch (err) {
      console.error("CAM generation failed:", err);
      const message = err instanceof Error ? err.message : "Generation failed";
      setGenerateError(message);
      addToast(message, "error");
    } finally {
      setGenerating(false);
    }
  }, [
    hasOperations,
    enabledOperations,
    tools,
    settings,
    addToast,
    setGenerating,
    setGenerateError,
    setStats,
    setGcodeOutput,
    setToolpathJson,
  ]);

  const handleExportGcode = useCallback(() => {
    if (!gcodeOutput) return;

    const blob = new Blob([gcodeOutput], { type: "text/plain" });
    downloadBlob(blob, "toolpath.nc");
    addToast("Exported toolpath.nc", "success");
  }, [gcodeOutput, addToast]);

  return (
    <div className="fixed right-0 top-0 bottom-0 w-80 bg-surface border-l border-border z-50 flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between p-3 border-b border-border">
        <h2 className="font-medium">CAM</h2>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted"
          onClick={closeCamPanel}
        >
          <X size={16} />
        </button>
      </div>

      {/* Tab bar */}
      <div className="flex border-b border-border">
        <button
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "operations"
              ? "border-b-2 border-brand text-text"
              : "text-text-muted"
          }`}
          onClick={() => setActiveTab("operations")}
        >
          <List size={14} />
          Ops
        </button>
        <button
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "tools"
              ? "border-b-2 border-brand text-text"
              : "text-text-muted"
          }`}
          onClick={() => setActiveTab("tools")}
        >
          <Wrench size={14} />
          Tools
        </button>
        <button
          className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
            activeTab === "settings"
              ? "border-b-2 border-brand text-text"
              : "text-text-muted"
          }`}
          onClick={() => setActiveTab("settings")}
        >
          <Gear size={14} />
          Settings
        </button>
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-y-auto p-3">
        {activeTab === "operations" && <OperationList />}
        {activeTab === "tools" && <ToolLibrary />}
        {activeTab === "settings" && <CamSettings />}
      </div>

      {/* Stats */}
      {stats && (
        <div className="px-3 py-2 border-t border-border bg-surface-secondary text-xs">
          <div className="grid grid-cols-2 gap-2">
            <div>
              <span className="text-text-muted">Cutting length: </span>
              <span>{stats.cuttingLength.toFixed(1)} mm</span>
            </div>
            <div>
              <span className="text-text-muted">Est. time: </span>
              <span>{formatMachiningTime(stats.estimatedTime)}</span>
            </div>
          </div>
        </div>
      )}

      {/* Error */}
      {generateError && (
        <div className="px-3 py-2 bg-error/10 text-error text-xs">
          {generateError}
        </div>
      )}

      {/* Footer actions */}
      <div className="p-3 border-t border-border space-y-2">
        <button
          className="w-full flex items-center justify-center gap-2 py-2 bg-brand text-white rounded hover:bg-brand/90 disabled:opacity-50"
          onClick={handleGenerate}
          disabled={isGenerating || !hasOperations}
        >
          {isGenerating ? (
            <>
              <Spinner size={16} className="animate-spin" />
              Generating...
            </>
          ) : (
            <>
              <Play size={16} />
              Generate Toolpath
            </>
          )}
        </button>

        {gcodeOutput && (
          <button
            className="w-full flex items-center justify-center gap-2 py-2 border border-border rounded hover:bg-hover"
            onClick={handleExportGcode}
          >
            <Export size={16} />
            Export G-Code
          </button>
        )}
      </div>
    </div>
  );
}
