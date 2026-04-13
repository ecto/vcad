import { useState, useEffect, useCallback } from "react";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { useSlicerStore } from "@/stores/slicer-store";
import { useEngineStore } from "@vcad/core";
import { usePrinterStore } from "@/stores/printer-store";

interface DfmWarning {
  severity: "Error" | "Warning" | "Info";
  kind: string;
  message: string;
  face_indices: number[];
  suggestion: string | null;
}

interface DfmResult {
  warnings: DfmWarning[];
  score: number;
}

let wasmModule: typeof import("@vcad/kernel-wasm") | null = null;

async function loadWasm(): Promise<typeof import("@vcad/kernel-wasm") | null> {
  if (wasmModule) return wasmModule;
  try {
    const wasm = await import("@vcad/kernel-wasm");
    wasmModule = wasm;
    return wasm;
  } catch {
    return null;
  }
}

export function DfmOverlay() {
  const [dfmResult, setDfmResult] = useState<DfmResult | null>(null);
  const [, setLoading] = useState(false);
  const printPanelOpen = useSlicerStore((s) => s.printPanelOpen);
  const scene = useEngineStore((s) => s.scene);
  const selectedPrinter = usePrinterStore((s) => s.selectedPrinter);

  const runDfmCheck = useCallback(async () => {
    if (!scene?.parts?.length) return;

    setLoading(true);
    try {
      const wasm = await loadWasm();
      if (!wasm) return;

      const checkFn = (wasm as Record<string, unknown>).checkPrintability as
        | ((solid: unknown, profile: string) => unknown)
        | undefined;
      if (!checkFn) return;

      // Try to get the first part's solid for BRep analysis
      const firstPart = scene.parts[0];
      if (!firstPart?.solid) {
        setDfmResult(null);
        return;
      }

      const profileId = selectedPrinter?.id || "generic";
      const result = checkFn(firstPart.solid, profileId) as DfmResult;
      setDfmResult(result);
    } catch {
      setDfmResult(null);
    } finally {
      setLoading(false);
    }
  }, [scene, selectedPrinter]);

  // Run DFM check when print panel opens or scene changes
  useEffect(() => {
    if (printPanelOpen) {
      runDfmCheck();
    } else {
      setDfmResult(null);
    }
  }, [printPanelOpen, runDfmCheck]);

  if (!printPanelOpen || !dfmResult || dfmResult.warnings.length === 0) {
    return null;
  }

  const severityIcon = (severity: string) => {
    switch (severity) {
      case "Error":
        return <XCircle size={14} className="text-red-400 flex-shrink-0" />;
      case "Warning":
        return <Warning size={14} className="text-orange-400 flex-shrink-0" />;
      default:
        return <Info size={14} className="text-blue-400 flex-shrink-0" />;
    }
  };

  const scoreColor =
    dfmResult.score >= 80
      ? "text-green-400"
      : dfmResult.score >= 50
        ? "text-orange-400"
        : "text-red-400";

  return (
    <div className="fixed top-16 left-4 w-72 bg-surface border border-border rounded-lg shadow-lg z-30 max-h-64 overflow-y-auto">
      <div className="flex items-center justify-between p-2 border-b border-border">
        <div className="flex items-center gap-1.5">
          <CheckCircle size={16} className={scoreColor} />
          <span className="text-xs font-medium">
            Printability: <span className={scoreColor}>{dfmResult.score}/100</span>
          </span>
        </div>
        <span className="text-xs text-text-muted">
          {dfmResult.warnings.length} issue{dfmResult.warnings.length !== 1 ? "s" : ""}
        </span>
      </div>
      <div className="p-1.5 space-y-1">
        {dfmResult.warnings.map((w, i) => (
          <div
            key={i}
            className="flex items-start gap-1.5 p-1.5 rounded hover:bg-hover text-xs"
          >
            {severityIcon(w.severity)}
            <div>
              <div className="text-text">{w.message}</div>
              {w.suggestion && (
                <div className="text-text-muted mt-0.5">{w.suggestion}</div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
