import { useSketchStore, useDocumentStore } from "@vcad/core";
import { Crosshair } from "@phosphor-icons/react/dist/ssr/Crosshair";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Button } from "@/components/ui/button";

export function FaceSelectionOverlay() {
  const faceSelectionMode = useSketchStore((s) => s.faceSelectionMode);
  const cancelFaceSelection = useSketchStore((s) => s.cancelFaceSelection);
  const enterSketchMode = useSketchStore((s) => s.enterSketchMode);
  const parts = useDocumentStore((s) => s.parts);

  if (!faceSelectionMode) return null;

  const hasParts = parts.length > 0;

  return (
    <div className="fixed inset-x-0 top-4 z-30 flex justify-center pointer-events-none">
      <div className="bg-surface border border-brand/50 shadow-lg px-4 py-3 flex items-center gap-4 pointer-events-auto">
        <div className="flex items-center gap-2 text-brand">
          <Crosshair size={20} weight="bold" />
          <span className="text-sm font-medium">
            {hasParts ? "Click a face to start sketching" : "No parts to select from"}
          </span>
        </div>

        <div className="h-4 w-px bg-border" />

        <div className="flex items-center gap-2">
          {/* Fallback to XY plane */}
          <Button
            variant="ghost"
            size="sm"
            className="text-xs"
            onClick={() => enterSketchMode("XY")}
          >
            Use XY Plane
          </Button>

          {/* Cancel */}
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={cancelFaceSelection}
          >
            <X size={16} />
          </Button>
        </div>

        <span className="text-xs text-text-muted">ESC to cancel</span>
      </div>
    </div>
  );
}
