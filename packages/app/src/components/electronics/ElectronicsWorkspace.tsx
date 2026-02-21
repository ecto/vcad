/**
 * Electronics workspace: split layout with schematic + PCB canvases.
 *
 * Layout:
 * ┌─────────────────────────────────────────────────────────┐
 * │  [◧ Split][□ Sch][□ PCB]        [Layers] [DRC: 0✗ 2⚠] │
 * ├───────────────────────┬─────────────────────────────────┤
 * │   SchematicCanvas     ┃     PcbCanvas                   │
 * ├───────────────────────┻─────────────────────────────────┤
 * │ [Select][Move][Route] | [Grid▾][Snap] | [Layer ▾] [←3D]│
 * └─────────────────────────────────────────────────────────┘
 */

import { useCallback, useRef, Suspense, lazy } from "react";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useElectronicsSync } from "@/hooks/useElectronicsSync";
import { useTheme } from "@/hooks/useTheme";

const SchematicCanvas = lazy(() =>
  import("./SchematicCanvas").then((m) => ({ default: m.SchematicCanvas })),
);
const PcbCanvas = lazy(() =>
  import("./PcbCanvas").then((m) => ({ default: m.PcbCanvas })),
);
const LayerPanel = lazy(() =>
  import("./LayerPanel").then((m) => ({ default: m.LayerPanel })),
);
const ElectronicsPropertyPanel = lazy(() =>
  import("./ElectronicsPropertyPanel").then((m) => ({
    default: m.ElectronicsPropertyPanel,
  })),
);

// ---------------------------------------------------------------------------
// Layout toggle buttons
// ---------------------------------------------------------------------------

function LayoutButton({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`px-2 py-0.5 text-[11px] rounded transition-colors ${
        active
          ? "bg-accent text-white"
          : "text-text-muted hover:text-text hover:bg-surface-hover"
      }`}
      onClick={onClick}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Tool button
// ---------------------------------------------------------------------------

function ToolButton({
  label,
  active,
  onClick,
  shortcut,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  shortcut?: string;
}) {
  return (
    <button
      className={`px-2 py-1 text-[11px] rounded transition-colors ${
        active
          ? "bg-accent text-white"
          : "text-text-muted hover:text-text hover:bg-surface-hover"
      }`}
      onClick={onClick}
      title={shortcut ? `${label} (${shortcut})` : label}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Main workspace
// ---------------------------------------------------------------------------

export function ElectronicsWorkspace() {
  useElectronicsSync();
  const { isDark } = useTheme();

  const layout = useElectronicsStore((s) => s.layout);
  const splitRatio = useElectronicsStore((s) => s.splitRatio);
  const focusedPane = useElectronicsStore((s) => s.focusedPane);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbGridSize = useElectronicsStore((s) => s.pcbGridSize);
  const pcbSnapToGrid = useElectronicsStore((s) => s.pcbSnapToGrid);
  const drcCount = useElectronicsStore((s) => s.drcViolations.length);
  const ercCount = useElectronicsStore((s) => s.ercViolations.length);
  const drcErrors = useElectronicsStore(
    (s) => s.drcViolations.filter((v) => v.severity === "Error").length,
  );
  const drcWarnings = drcCount - drcErrors;

  const setLayout = useElectronicsStore((s) => s.setLayout);
  const setPcbTool = useElectronicsStore((s) => s.setPcbTool);
  const setPcbActiveLayer = useElectronicsStore((s) => s.setPcbActiveLayer);
  const setPcbGridSize = useElectronicsStore((s) => s.setPcbGridSize);
  const setPcbSnapToGrid = useElectronicsStore((s) => s.setPcbSnapToGrid);
  const exit = useElectronicsStore((s) => s.exit);

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

  // Keyboard shortcuts
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.target !== e.currentTarget && (e.target as HTMLElement).tagName === "INPUT") return;
      switch (e.key) {
        case "1":
          setLayout("split");
          break;
        case "2":
          setLayout("schematic-only");
          break;
        case "3":
          setLayout("pcb-only");
          break;
        case "Escape":
          exit();
          break;
        case "v":
        case "V":
          setPcbTool("select");
          break;
        case "m":
        case "M":
          setPcbTool("move");
          break;
        case "x":
        case "X":
          if (focusedPane === "pcb") setPcbTool("route");
          break;
        case "f":
        case "F":
          setPcbActiveLayer("FCu");
          break;
        case "b":
        case "B":
          setPcbActiveLayer("BCu");
          break;
      }
    },
    [setLayout, exit, setPcbTool, setPcbActiveLayer, focusedPane],
  );

  const showSch = layout === "split" || layout === "schematic-only";
  const showPcb = layout === "split" || layout === "pcb-only";

  const bgColor = isDark ? "#0a0a0a" : "#ffffff";

  return (
    <div
      ref={containerRef}
      className="flex flex-col h-full w-full select-none"
      style={{ backgroundColor: bgColor }}
      tabIndex={0}
      onKeyDown={onKeyDown}
    >
      {/* Header bar */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border shrink-0">
        <div className="flex items-center gap-1">
          <LayoutButton
            label="Split"
            active={layout === "split"}
            onClick={() => setLayout("split")}
          />
          <LayoutButton
            label="Sch"
            active={layout === "schematic-only"}
            onClick={() => setLayout("schematic-only")}
          />
          <LayoutButton
            label="PCB"
            active={layout === "pcb-only"}
            onClick={() => setLayout("pcb-only")}
          />
        </div>
        <div className="flex-1" />
        <Suspense fallback={null}>
          <LayerPanel />
        </Suspense>
        <span className="text-[11px] text-text-muted">
          DRC:{" "}
          <span className={drcErrors > 0 ? "text-danger" : "text-text-muted"}>
            {drcErrors}
          </span>
          {" / "}
          <span className={drcWarnings > 0 ? "text-warning" : "text-text-muted"}>
            {drcWarnings}
          </span>
          {" "}
          ERC: {ercCount}
        </span>
      </div>

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
      </div>

      {/* Bottom toolbar */}
      <div className="flex items-center gap-2 px-3 py-1 border-t border-border shrink-0">
        {focusedPane === "pcb" ? (
          <>
            <ToolButton label="Select" active={pcbTool === "select"} onClick={() => setPcbTool("select")} shortcut="V" />
            <ToolButton label="Move" active={pcbTool === "move"} onClick={() => setPcbTool("move")} shortcut="M" />
            <ToolButton label="Route" active={pcbTool === "route"} onClick={() => setPcbTool("route")} shortcut="X" />
            <div className="w-px h-4 bg-border mx-1" />
            <span className="text-[11px] text-text-muted">Grid:</span>
            <select
              className="text-[11px] bg-transparent text-text border border-border rounded px-1 py-0.5"
              value={pcbGridSize}
              onChange={(e) => setPcbGridSize(Number(e.target.value))}
            >
              <option value={0.1}>0.1mm</option>
              <option value={0.25}>0.25mm</option>
              <option value={0.5}>0.5mm</option>
              <option value={1.0}>1mm</option>
              <option value={2.54}>2.54mm</option>
            </select>
            <label className="flex items-center gap-1 text-[11px] text-text-muted">
              <input
                type="checkbox"
                checked={pcbSnapToGrid}
                onChange={(e) => setPcbSnapToGrid(e.target.checked)}
                className="w-3 h-3"
              />
              Snap
            </label>
            <div className="w-px h-4 bg-border mx-1" />
            <span className="text-[11px] text-text-muted">Layer:</span>
            <select
              className="text-[11px] bg-transparent text-text border border-border rounded px-1 py-0.5"
              value={pcbActiveLayer}
              onChange={(e) => setPcbActiveLayer(e.target.value as any)}
            >
              <option value="FCu">FCu</option>
              <option value="BCu">BCu</option>
              <option value="In1Cu">In1Cu</option>
              <option value="In2Cu">In2Cu</option>
            </select>
          </>
        ) : (
          <>
            <ToolButton
              label="Select"
              active={useElectronicsStore.getState().schTool === "select"}
              onClick={() => useElectronicsStore.getState().setSchTool("select")}
              shortcut="V"
            />
            <ToolButton
              label="Move"
              active={useElectronicsStore.getState().schTool === "move"}
              onClick={() => useElectronicsStore.getState().setSchTool("move")}
              shortcut="M"
            />
          </>
        )}
        <div className="flex-1" />
        <button
          className="px-2 py-0.5 text-[11px] text-text-muted hover:text-text rounded hover:bg-surface-hover transition-colors"
          onClick={exit}
        >
          Back to 3D
        </button>
      </div>
    </div>
  );
}
