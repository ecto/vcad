import { useState } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDocumentStore, useUiStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { cn } from "@/lib/utils";

interface MirrorDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  partId: string;
}

type MirrorPlane = "XY" | "XZ" | "YZ";

export function MirrorDialog({
  open,
  onOpenChange,
  partId,
}: MirrorDialogProps) {
  const [plane, setPlane] = useState<MirrorPlane>("XZ");
  const addMirror = useDocumentStore((s) => s.addMirror);
  const select = useUiStore((s) => s.select);
  const addToast = useNotificationStore((s) => s.addToast);

  const planeDescriptions = {
    XY: "Mirror across the XY plane (flip Z)",
    XZ: "Mirror across the XZ plane (flip Y)",
    YZ: "Mirror across the YZ plane (flip X)",
  };

  function handleApply() {
    const newPartId = addMirror(partId, plane);

    if (newPartId) {
      select(newPartId);
      addToast("Created Mirror", "success");
    } else {
      addToast("Failed to create mirror", "error");
    }
    onOpenChange(false);
  }

  function handleCancel() {
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Mirror">
        <p className="text-xs text-text-muted mb-3">
          Create a mirrored copy of the selected part.
        </p>

        <div className="flex flex-col gap-4 py-2">
          {/* Plane selection */}
          <div className="flex flex-col gap-2">
            <span className="text-xs text-text-muted">Mirror Plane</span>
            <div className="flex gap-1">
              {(["XY", "XZ", "YZ"] as MirrorPlane[]).map((p) => (
                <button
                  key={p}
                  className={cn(
                    "flex-1 px-3 py-2 text-xs font-medium",
                    plane === p
                      ? "bg-brand text-white"
                      : "bg-surface text-text hover:bg-hover border border-border"
                  )}
                  onClick={() => setPlane(p)}
                >
                  {p}
                </button>
              ))}
            </div>
            <span className="text-xs text-text-muted/70">
              {planeDescriptions[plane]}
            </span>
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={handleCancel}>
            Cancel
          </Button>
          <Button variant="default" size="sm" onClick={handleApply}>
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
