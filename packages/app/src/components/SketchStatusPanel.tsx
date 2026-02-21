import { useState } from "react";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { Crosshair } from "@phosphor-icons/react/dist/ssr/Crosshair";
import { GridFour } from "@phosphor-icons/react/dist/ssr/GridFour";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useSketchStore, useUiStore, getSketchPlaneName } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";

export function SketchStatusPanel() {
  const [showChangePlaneDialog, setShowChangePlaneDialog] = useState(false);

  const active = useSketchStore((s) => s.active);
  const plane = useSketchStore((s) => s.plane);
  const cursorSketchPos = useSketchStore((s) => s.cursorSketchPos);
  const constraintStatus = useSketchStore((s) => s.constraintStatus);
  const constraints = useSketchStore((s) => s.constraints);
  const segments = useSketchStore((s) => s.segments);
  const enterFaceSelectionMode = useSketchStore((s) => s.enterFaceSelectionMode);
  const exitSketchMode = useSketchStore((s) => s.exitSketchMode);

  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);
  const toggleGridSnap = useUiStore((s) => s.toggleGridSnap);
  const togglePointSnap = useUiStore((s) => s.togglePointSnap);
  const isOrbiting = useUiStore((s) => s.isOrbiting);

  const addToast = useNotificationStore((s) => s.addToast);

  if (!active) return null;

  const hasSegments = segments.length > 0;

  function handleChangePlane() {
    if (hasSegments) {
      setShowChangePlaneDialog(true);
    } else {
      exitSketchMode();
      enterFaceSelectionMode();
      addToast("Select a new face", "info");
    }
  }

  function confirmChangePlane() {
    setShowChangePlaneDialog(false);
    exitSketchMode();
    enterFaceSelectionMode();
    addToast("Select a new face", "info");
  }

  // Get constraint status color
  const statusColor = constraintStatus === "under"
    ? "text-yellow-500"
    : constraintStatus === "solved"
      ? "text-green-500"
      : constraintStatus === "over"
        ? "text-orange-500"
        : "text-red-500";

  const statusDotColor = constraintStatus === "under"
    ? "bg-yellow-500"
    : constraintStatus === "solved"
      ? "bg-green-500"
      : constraintStatus === "over"
        ? "bg-orange-500"
        : "bg-red-500";

  return (
    <div className={cn(
      "fixed left-2 sm:left-4 top-2 sm:top-4 z-30",
      "bg-surface/95 backdrop-blur-sm border border-border",
      "px-3 py-2 shadow-lg",
      "transition-opacity duration-200",
      isOrbiting && "opacity-0 pointer-events-none"
    )}>
      <div className="flex flex-col gap-1.5">
        {/* Header: Sketch + Plane */}
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-text">Sketch</span>
          <span className="text-xs text-text-muted">{getSketchPlaneName(plane)}</span>
          <Tooltip content="Change sketch plane">
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={handleChangePlane}
              className="h-5 w-5 ml-1"
            >
              <ArrowsClockwise size={12} />
            </Button>
          </Tooltip>
        </div>

        {/* Coordinates */}
        <div className="flex items-center gap-2 font-mono text-xs text-text-muted">
          <span>X:</span>
          <span className="w-12 text-right text-text">
            {cursorSketchPos ? cursorSketchPos.x.toFixed(1) : "-"}
          </span>
          <span className="ml-2">Y:</span>
          <span className="w-12 text-right text-text">
            {cursorSketchPos ? cursorSketchPos.y.toFixed(1) : "-"}
          </span>
        </div>

        {/* Snap toggles + Constraint count */}
        <div className="flex items-center gap-2">
          {/* Point snap */}
          <Tooltip content="Toggle point snap (P)">
            <button
              onClick={togglePointSnap}
              className={cn(
                "flex items-center gap-1 px-1.5 py-0.5 text-xs transition-colors",
                pointSnap
                  ? "text-green-400 bg-green-400/10"
                  : "text-text-muted hover:text-text"
              )}
            >
              <Crosshair size={12} weight={pointSnap ? "fill" : "regular"} />
              <span className="hidden sm:inline">Point</span>
            </button>
          </Tooltip>

          {/* Grid snap */}
          <Tooltip content="Toggle grid snap (G)">
            <button
              onClick={toggleGridSnap}
              className={cn(
                "flex items-center gap-1 px-1.5 py-0.5 text-xs transition-colors",
                gridSnap
                  ? "text-cyan-400 bg-cyan-400/10"
                  : "text-text-muted hover:text-text"
              )}
            >
              <GridFour size={12} weight={gridSnap ? "fill" : "regular"} />
              <span className="hidden sm:inline">Grid</span>
            </button>
          </Tooltip>

          {/* Constraint status */}
          {hasSegments && (
            <Tooltip
              content={
                constraintStatus === "under"
                  ? "Under-constrained"
                  : constraintStatus === "solved"
                    ? "Fully constrained"
                    : constraintStatus === "over"
                      ? "Over-constrained"
                      : "Constraints conflict"
              }
            >
              <div className={cn("flex items-center gap-1 px-1.5 py-0.5 text-xs", statusColor)}>
                <div className={cn("w-1.5 h-1.5 rounded-full", statusDotColor)} />
                <span>{constraints.length} constraint{constraints.length !== 1 ? "s" : ""}</span>
              </div>
            </Tooltip>
          )}
        </div>
      </div>

      {/* Change plane confirmation dialog */}
      {showChangePlaneDialog && (
        <div className="absolute left-0 top-full mt-2 border border-border bg-card p-4 shadow-2xl min-w-[220px] z-50">
          <div className="flex flex-col gap-3">
            <div className="flex items-center gap-2 text-amber-500">
              <Warning size={18} weight="fill" />
              <span className="text-sm font-medium">Change sketch plane?</span>
            </div>
            <p className="text-xs text-text-muted">
              This will clear your current sketch geometry.
            </p>
            <div className="flex gap-2">
              <Button
                variant="default"
                size="sm"
                className="flex-1"
                onClick={confirmChangePlane}
              >
                Change Plane
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="flex-1"
                onClick={() => setShowChangePlaneDialog(false)}
              >
                Cancel
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
