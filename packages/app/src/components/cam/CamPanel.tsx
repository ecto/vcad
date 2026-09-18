/**
 * The web app's CAM panel, on the same verified job API the native app and the
 * MCP tools use.
 *
 * It used to drive the granular WASM bindings one operation at a time —
 * `camGenerateFace`, `camGeneratePocket`, `camGenerateContour`,
 * `camExportGcode` — and hand over whatever came back. Nothing replayed the
 * program against the part, so nothing could say no. Now the whole job goes to
 * `cam_job` and comes back verified or refused, and a refused job carries no
 * G-code at all: there is nothing here to export by accident.
 */
import { useCallback, useState } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Gear } from "@phosphor-icons/react/dist/ssr/Gear";
import { List } from "@phosphor-icons/react/dist/ssr/List";
import { SealCheck } from "@phosphor-icons/react/dist/ssr/SealCheck";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { useEngineStore } from "@vcad/core";
import { useCamStore, formatMachiningTime } from "@/stores/cam-store";
import {
  canExport,
  exportBlocker,
  useCamJobStore,
} from "@/stores/cam-job-store";
import { useNotificationStore } from "@/stores/notification-store";
import { downloadBlob } from "@/lib/download";
import { JobSetup } from "./JobSetup";
import { OperationList } from "./OperationList";
import { VerificationPanel } from "./VerificationPanel";
import { ToolpathPreview } from "./ToolpathPreview";

type Tab = "setup" | "operations" | "verify";

export function CamPanel() {
  const [activeTab, setActiveTab] = useState<Tab>("setup");

  const closeCamPanel = useCamStore((s) => s.closeCamPanel);
  const engine = useEngineStore((s) => s.engine);
  const addToast = useNotificationStore((s) => s.addToast);

  const operations = useCamJobStore((s) => s.operations);
  const building = useCamJobStore((s) => s.building);
  const result = useCamJobStore((s) => s.result);
  const build = useCamJobStore((s) => s.build);
  const exportable = useCamJobStore(canExport);
  const blocker = useCamJobStore(exportBlocker);

  const hasOperations = operations.some((op) => op.enabled);

  const handleBuild = useCallback(() => {
    if (!engine) {
      addToast("The kernel is still loading", "error");
      return;
    }
    build(engine);
    // The verdict is the point of pressing the button, so go and show it.
    setActiveTab("verify");
  }, [engine, build, addToast]);

  const handleExport = useCallback(() => {
    // The gate is asked again here rather than trusted from the disabled
    // attribute: a disabled button is a hint, and this is the boundary where
    // G-code leaves the app.
    const state = useCamJobStore.getState();
    if (!canExport(state)) {
      addToast(exportBlocker(state) ?? "This job cannot be exported.", "error");
      return;
    }
    const job = state.result;
    if (!job || job.blocked) return;
    downloadBlob(new Blob([job.gcode], { type: "text/plain" }), "job.nc");
    addToast("Exported job.nc", "success");
  }, [addToast]);

  const duration = result?.duration?.accel_aware_s;

  return (
    <div className="fixed right-0 top-0 bottom-0 w-80 bg-surface border-l border-border z-50 flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between p-3 border-b border-border">
        <h2 className="font-medium">CAM</h2>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted"
          onClick={closeCamPanel}
          aria-label="Close the CAM panel"
        >
          <X size={16} />
        </button>
      </div>

      {/* Tab bar */}
      <div className="flex border-b border-border">
        {(
          [
            ["setup", "Setup", Gear],
            ["operations", "Ops", List],
            ["verify", "Verify", SealCheck],
          ] as const
        ).map(([tab, label, Icon]) => (
          <button
            key={tab}
            className={`flex-1 flex items-center justify-center gap-1 py-2 text-sm ${
              activeTab === tab ? "border-b-2 border-brand text-text" : "text-text-muted"
            }`}
            onClick={() => setActiveTab(tab)}
          >
            <Icon size={14} />
            {label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {activeTab === "setup" && <JobSetup />}
        {activeTab === "operations" && (
          <>
            <OperationList />
            <ToolpathPreview />
          </>
        )}
        {activeTab === "verify" && (
          <>
            <VerificationPanel />
            <ToolpathPreview />
          </>
        )}
      </div>

      {/* Cycle time */}
      {duration !== undefined && (
        <div className="px-3 py-2 border-t border-border bg-surface-secondary text-xs">
          <span className="text-text-muted">Est. cycle time: </span>
          <span>{formatMachiningTime(duration)}</span>
          <span className="text-text-muted"> (predicted, not measured)</span>
        </div>
      )}

      {/* Footer actions */}
      <div className="p-3 border-t border-border space-y-2">
        <button
          className="w-full flex items-center justify-center gap-2 py-2 bg-brand text-white rounded hover:bg-brand/90 disabled:opacity-50"
          onClick={handleBuild}
          disabled={building || !hasOperations || !engine}
        >
          {building ? (
            <>
              <Spinner size={16} className="animate-spin" />
              Building and checking…
            </>
          ) : (
            <>
              <Play size={16} />
              Build and verify job
            </>
          )}
        </button>

        <button
          className="w-full flex items-center justify-center gap-2 py-2 border border-border rounded hover:bg-hover disabled:opacity-50"
          onClick={handleExport}
          disabled={!exportable}
          title={blocker ?? "Save the verified program"}
        >
          <Export size={16} />
          Export G-code
        </button>
        {blocker && <p className="text-xs text-text-muted">{blocker}</p>}
      </div>
    </div>
  );
}
