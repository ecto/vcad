/**
 * Electronics workspace: split-pane canvas container.
 *
 * Toolbar and status panel are rendered as siblings in App.tsx.
 * This component handles only the split-pane layout, canvases,
 * property panel, and resize handle.
 */

import { useCallback, useRef, Suspense, lazy } from "react";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useElectronicsSync } from "@/hooks/useElectronicsSync";
import { useTheme } from "@/hooks/useTheme";
import { useCoreElectronicsStore, useDocumentStore } from "@vcad/core";
import { ElectronicsEmptyState } from "./ElectronicsEmptyState";

const SchematicCanvas = lazy(() =>
  import("./SchematicCanvas").then((m) => ({ default: m.SchematicCanvas })),
);
const PcbCanvas = lazy(() =>
  import("./PcbCanvas").then((m) => ({ default: m.PcbCanvas })),
);
const ElectronicsPropertyPanel = lazy(() =>
  import("./ElectronicsPropertyPanel").then((m) => ({
    default: m.ElectronicsPropertyPanel,
  })),
);
const LengthTunePanel = lazy(() =>
  import("./LengthTunePanel").then((m) => ({
    default: m.LengthTunePanel,
  })),
);

export function ElectronicsWorkspace() {
  useElectronicsSync();
  const { isDark } = useTheme();

  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const hasSchematic = useDocumentStore((s) => !!s.document.schematic);

  const layout = useElectronicsStore((s) => s.layout);
  const splitRatio = useElectronicsStore((s) => s.splitRatio);

  const dragRef = useRef<{ startX: number; startRatio: number } | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  // Drag handle for split resize
  const onDragStart = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      dragRef.current = { startX: e.clientX, startRatio: splitRatio };
      const onMove = (ev: PointerEvent) => {
        if (!dragRef.current || !containerRef.current) return;
        const rect = containerRef.current.getBoundingClientRect();
        const dx = ev.clientX - dragRef.current.startX;
        const ratio = dragRef.current.startRatio + dx / rect.width;
        useElectronicsStore.setState({
          splitRatio: Math.max(0.2, Math.min(0.8, ratio)),
        });
      };
      const onUp = () => {
        dragRef.current = null;
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [splitRatio],
  );

  const showSch = layout === "split" || layout === "schematic-only";
  const showPcb = layout === "split" || layout === "pcb-only";

  const bgColor = isDark ? "#0a0a0a" : "#ffffff";

  if (activeBoardNodeId == null && !hasSchematic) {
    return (
      <div
        className="flex flex-col h-full w-full select-none"
        style={{ backgroundColor: bgColor }}
      >
        <ElectronicsEmptyState />
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className="flex flex-col h-full w-full select-none"
      style={{ backgroundColor: bgColor }}
      tabIndex={0}
    >
      {/* Canvas area */}
      <div className="flex-1 relative overflow-hidden">
        <div
          className="absolute inset-0"
          style={
            layout === "split"
              ? {
                  display: "grid",
                  gridTemplateColumns: `${splitRatio}fr 4px ${1 - splitRatio}fr`,
                }
              : undefined
          }
        >
          {showSch && (
            <div
              className="overflow-hidden relative"
              onClick={() =>
                useElectronicsStore.setState({ focusedPane: "schematic" })
              }
              style={
                layout !== "split"
                  ? { position: "absolute", inset: 0 }
                  : undefined
              }
            >
              <Suspense fallback={null}>
                <SchematicCanvas />
              </Suspense>
            </div>
          )}
          {layout === "split" && (
            <div
              className="cursor-col-resize bg-border hover:bg-accent transition-colors"
              onPointerDown={onDragStart}
            />
          )}
          {showPcb && (
            <div
              className="overflow-hidden relative"
              onClick={() =>
                useElectronicsStore.setState({ focusedPane: "pcb" })
              }
              style={
                layout !== "split"
                  ? { position: "absolute", inset: 0 }
                  : undefined
              }
            >
              <Suspense fallback={null}>
                <PcbCanvas />
              </Suspense>
            </div>
          )}
        </div>

        {/* Property panel */}
        <Suspense fallback={null}>
          <ElectronicsPropertyPanel />
        </Suspense>

        {/* Length tuning panel */}
        <Suspense fallback={null}>
          <LengthTunePanel />
        </Suspense>
      </div>
    </div>
  );
}
