import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { EyeSlash } from "@phosphor-icons/react/dist/ssr/EyeSlash";
import { Ruler } from "@phosphor-icons/react/dist/ssr/Ruler";
import { MagnifyingGlassPlus } from "@phosphor-icons/react/dist/ssr/MagnifyingGlassPlus";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Download } from "@phosphor-icons/react/dist/ssr/Download";
import { CaretDown } from "@phosphor-icons/react/dist/ssr/CaretDown";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import { useEngineStore } from "@vcad/core";
import { useDrawingStore, type ViewDirection } from "@/stores/drawing-store";
import { useNotificationStore } from "@/stores/notification-store";
import { useUiStore } from "@vcad/core";
import { downloadDxf } from "@/lib/save-load";

const VIEW_DIRECTIONS: { value: ViewDirection; label: string }[] = [
  { value: "front", label: "Front" },
  { value: "back", label: "Back" },
  { value: "top", label: "Top" },
  { value: "bottom", label: "Bottom" },
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
  { value: "isometric", label: "Isometric" },
];

export function DrawingToolbar() {
  const viewMode = useDrawingStore((s) => s.viewMode);
  const viewDirection = useDrawingStore((s) => s.viewDirection);
  const setViewDirection = useDrawingStore((s) => s.setViewDirection);
  const showHiddenLines = useDrawingStore((s) => s.showHiddenLines);
  const toggleHiddenLines = useDrawingStore((s) => s.toggleHiddenLines);
  const showDimensions = useDrawingStore((s) => s.showDimensions);
  const toggleDimensions = useDrawingStore((s) => s.toggleDimensions);
  const detailViews = useDrawingStore((s) => s.detailViews);
  const clearDetailViews = useDrawingStore((s) => s.clearDetailViews);

  const engine = useEngineStore((s) => s.engine);
  const scene = useEngineStore((s) => s.scene);
  const isOrbiting = useUiStore((s) => s.isOrbiting);

  if (viewMode !== "2d") return null;

  const hasParts = scene?.parts?.length ?? 0 > 0;

  function handleExportDxf() {
    if (!scene?.parts?.length || !engine) return;
    try {
      const mesh = scene.parts[0]!.mesh;
      const projectedView = engine.projectMesh(mesh, viewDirection);
      if (!projectedView) {
        useNotificationStore.getState().addToast("Failed to project view", "error");
        return;
      }
      const dxfData = engine.exportDrawingToDxf(projectedView);
      downloadDxf(dxfData, `drawing-${viewDirection}.dxf`);
      useNotificationStore.getState().addToast("DXF exported", "success");
    } catch (err) {
      console.error("DXF export failed:", err);
      useNotificationStore.getState().addToast("DXF export failed", "error");
    }
  }

  function handleStartDetailView() {
    window.dispatchEvent(new CustomEvent("vcad:start-detail-view"));
  }

  return (
    <>
      {/* Top-left status indicator */}
      <div
        className={cn(
          "fixed left-4 top-4 z-30 bg-surface border border-border px-3 py-2 text-xs text-text shadow-lg flex items-center gap-2",
          "transition-opacity duration-200",
          isOrbiting && "opacity-0 pointer-events-none"
        )}
      >
        <span className="font-medium">Drawing Mode</span>
        <span className="text-text-muted">
          View: {VIEW_DIRECTIONS.find((v) => v.value === viewDirection)?.label}
        </span>
      </div>

      {/* Bottom toolbar */}
      <div
        className={cn(
          "fixed left-1/2 bottom-20 z-30 -translate-x-1/2",
          "transition-opacity duration-200",
          isOrbiting && "opacity-0 pointer-events-none"
        )}
      >
        <div className="relative flex items-center gap-1 rounded-full border border-border bg-card px-2 py-1.5 shadow-md">
          {/* View direction dropdown */}
          <div className="relative">
            <select
              value={viewDirection}
              onChange={(e) => setViewDirection(e.target.value as ViewDirection)}
              className="h-7 px-2 pr-6 text-xs bg-surface border border-border text-text appearance-none cursor-pointer hover:bg-hover transition-colors"
            >
              {VIEW_DIRECTIONS.map(({ value, label }) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <CaretDown
              size={12}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 text-text-muted pointer-events-none"
            />
          </div>

          <Separator orientation="vertical" className="mx-1 h-5" />

          {/* Display options */}
          <Tooltip content={showHiddenLines ? "Hide Hidden Lines" : "Show Hidden Lines"}>
            <Button
              variant={showHiddenLines ? "default" : "ghost"}
              size="icon-sm"
              onClick={toggleHiddenLines}
            >
              {showHiddenLines ? <Eye size={16} /> : <EyeSlash size={16} />}
            </Button>
          </Tooltip>

          <Tooltip content={showDimensions ? "Hide Dimensions" : "Show Dimensions"}>
            <Button
              variant={showDimensions ? "default" : "ghost"}
              size="icon-sm"
              onClick={toggleDimensions}
            >
              <Ruler size={16} />
            </Button>
          </Tooltip>

          <Separator orientation="vertical" className="mx-1 h-5" />

          {/* Detail views */}
          <Tooltip content="Add Detail View (drag to select region)">
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={handleStartDetailView}
              disabled={!hasParts}
            >
              <MagnifyingGlassPlus size={16} />
            </Button>
          </Tooltip>

          {detailViews.length > 0 && (
            <Tooltip content="Clear Detail Views">
              <Button variant="ghost" size="icon-sm" onClick={clearDetailViews}>
                <X size={16} />
              </Button>
            </Tooltip>
          )}

          <Separator orientation="vertical" className="mx-1 h-5" />

          {/* Export */}
          <Tooltip content="Export DXF">
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={handleExportDxf}
              disabled={!hasParts || !engine}
            >
              <Download size={16} />
            </Button>
          </Tooltip>
        </div>
      </div>
    </>
  );
}
