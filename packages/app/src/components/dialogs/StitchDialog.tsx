import { useState } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { ScrubInput } from "@/components/ui/scrub-input";
import { useDocumentStore, useUiStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";

type StitchType = "running" | "satin" | "fill";

interface StitchDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  partId: string;
}

export function StitchDialog({
  open,
  onOpenChange,
  partId,
}: StitchDialogProps) {
  const [stitchType, setStitchType] = useState<StitchType>("running");
  const [color, setColor] = useState("#ffffff");
  const [stitchLength, setStitchLength] = useState(2.5);
  const [density, setDensity] = useState(4.0);
  const [satinWidth, setSatinWidth] = useState(3.0);
  const [fillAngle, setFillAngle] = useState(0);
  const [applying, setApplying] = useState(false);

  const addStitch = useDocumentStore((s) => s.addStitch);
  const select = useUiStore((s) => s.select);
  const addToast = useNotificationStore((s) => s.addToast);

  function hexToRgb(hex: string): [number, number, number] {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return [r, g, b];
  }

  async function handleApply() {
    setApplying(true);
    try {
      const newPartId = await addStitch(partId, {
        stitchType,
        color: hexToRgb(color),
        stitchLength,
        density,
        satinWidth,
        fillAngle,
      });

      if (newPartId) {
        select(newPartId);
        addToast("Created Stitch embroidery", "success");
        useEmbroideryStore.getState().openPanel();
      } else {
        addToast("Failed to create Stitch", "error");
      }
      onOpenChange(false);
    } finally {
      setApplying(false);
    }
  }

  function handleCancel() {
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Stitch">
        <p className="text-xs text-text-muted mb-3">
          Convert the selected part to embroidery stitches.
        </p>

        <div className="flex flex-col gap-4 py-2">
          {/* Stitch type selector */}
          <div>
            <label className="text-xs font-medium text-text-muted mb-1 block">Stitch Type</label>
            <div className="flex gap-2">
              {(["running", "satin", "fill"] as const).map((st) => (
                <button
                  key={st}
                  onClick={() => setStitchType(st)}
                  className={`flex-1 rounded px-3 py-2 text-xs font-medium transition-colors ${
                    stitchType === st
                      ? "bg-brand text-white"
                      : "bg-hover text-text-muted hover:text-text"
                  }`}
                >
                  {st.charAt(0).toUpperCase() + st.slice(1)}
                </button>
              ))}
            </div>
          </div>

          {/* Thread color */}
          <div>
            <label className="text-xs font-medium text-text-muted mb-1 block">Thread Color</label>
            <input
              type="color"
              value={color}
              onChange={(e) => setColor(e.target.value)}
              className="w-full h-8 rounded cursor-pointer border border-border"
            />
          </div>

          {/* Common: stitch length */}
          <ScrubInput
            label="Stitch Length"
            value={stitchLength}
            onChange={setStitchLength}
            min={0.5}
            max={10}
            step={0.1}
            unit="mm"
          />

          {/* Satin-specific: width */}
          {stitchType === "satin" && (
            <ScrubInput
              label="Satin Width"
              value={satinWidth}
              onChange={setSatinWidth}
              min={0.5}
              max={20}
              step={0.5}
              unit="mm"
            />
          )}

          {/* Satin/Fill: density */}
          {(stitchType === "satin" || stitchType === "fill") && (
            <ScrubInput
              label="Density"
              value={density}
              onChange={setDensity}
              min={0.5}
              max={20}
              step={0.5}
              unit="lines/mm"
            />
          )}

          {/* Fill-specific: angle */}
          {stitchType === "fill" && (
            <ScrubInput
              label="Fill Angle"
              value={fillAngle}
              onChange={setFillAngle}
              min={0}
              max={360}
              step={5}
              unit="deg"
            />
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={handleCancel}>
            Cancel
          </Button>
          <Button variant="default" size="sm" onClick={handleApply} disabled={applying}>
            {applying ? "Applying..." : "Apply"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
