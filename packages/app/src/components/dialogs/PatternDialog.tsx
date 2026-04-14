import { useState } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { ScrubInput } from "@/components/ui/scrub-input";
import { useDocumentStore, useUiStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { cn } from "@/lib/utils";

interface PatternDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  partId: string;
}

type PatternType = "linear" | "circular";
type Axis = "X" | "Y" | "Z";

export function PatternDialog({
  open,
  onOpenChange,
  partId,
}: PatternDialogProps) {
  const [patternType, setPatternType] = useState<PatternType>("linear");
  const [axis, setAxis] = useState<Axis>("X");
  const [count, setCount] = useState(3);
  const [spacing, setSpacing] = useState(20);
  const [angle, setAngle] = useState(360);

  const addLinearPattern = useDocumentStore((s) => s.addLinearPattern);
  const addCircularPattern = useDocumentStore((s) => s.addCircularPattern);
  const select = useUiStore((s) => s.select);
  const addToast = useNotificationStore((s) => s.addToast);

  const axisDirections = {
    X: { x: 1, y: 0, z: 0 },
    Y: { x: 0, y: 1, z: 0 },
    Z: { x: 0, y: 0, z: 1 },
  };

  function handleApply() {
    let newPartId: string | null = null;

    if (patternType === "linear") {
      newPartId = addLinearPattern(partId, axisDirections[axis], count, spacing);
    } else {
      // For circular pattern, use origin at 0,0,0 and axis direction
      newPartId = addCircularPattern(
        partId,
        { x: 0, y: 0, z: 0 },
        axisDirections[axis],
        count,
        angle
      );
    }

    if (newPartId) {
      select(newPartId);
      addToast(`Created ${patternType === "linear" ? "Linear" : "Circular"} Pattern`, "success");
    } else {
      addToast("Failed to create pattern", "error");
    }
    onOpenChange(false);
  }

  function handleCancel() {
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Pattern">
        <div className="flex flex-col gap-4 py-2">
          {/* Pattern type tabs */}
          <div className="flex gap-1 border-b border-border pb-2">
            <button
              className={cn(
                "px-3 py-1.5 text-xs",
                patternType === "linear"
                  ? "bg-brand text-white"
                  : "bg-surface text-text hover:bg-hover"
              )}
              onClick={() => setPatternType("linear")}
            >
              Linear
            </button>
            <button
              className={cn(
                "px-3 py-1.5 text-xs",
                patternType === "circular"
                  ? "bg-brand text-white"
                  : "bg-surface text-text hover:bg-hover"
              )}
              onClick={() => setPatternType("circular")}
            >
              Circular
            </button>
          </div>

          {/* Axis selection */}
          <div className="flex items-center gap-3">
            <span className="text-xs text-text-muted w-24">
              {patternType === "linear" ? "Direction" : "Axis"}
            </span>
            <div className="flex gap-1">
              {(["X", "Y", "Z"] as Axis[]).map((a) => (
                <button
                  key={a}
                  className={cn(
                    "w-8 h-8 text-xs font-medium",
                    axis === a
                      ? "bg-brand text-white"
                      : "bg-surface text-text hover:bg-hover border border-border"
                  )}
                  onClick={() => setAxis(a)}
                >
                  {a}
                </button>
              ))}
            </div>
          </div>

          {/* Count */}
          <ScrubInput
            label="Count"
            value={count}
            onChange={setCount}
            min={2}
            max={50}
            step={1}
          />

          {/* Spacing (linear) or Angle (circular) */}
          {patternType === "linear" ? (
            <ScrubInput
              label="Spacing"
              value={spacing}
              onChange={setSpacing}
              min={1}
              max={500}
              step={5}
              unit="mm"
            />
          ) : (
            <ScrubInput
              label="Angle"
              value={angle}
              onChange={setAngle}
              min={1}
              max={360}
              step={15}
              unit="deg"
            />
          )}
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
