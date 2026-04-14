import { useState } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDocumentStore, useUiStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import type { TextAlignment } from "@vcad/ir";

interface TextDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function TextDialog({ open, onOpenChange }: TextDialogProps) {
  const [text, setText] = useState("Text");
  const [height, setHeight] = useState(10);
  const [depth, setDepth] = useState(2);
  const [alignment, setAlignment] = useState<TextAlignment>("left");

  const addText = useDocumentStore((s) => s.addText);
  const select = useUiStore((s) => s.select);
  const setTransformMode = useUiStore((s) => s.setTransformMode);
  const addToast = useNotificationStore((s) => s.addToast);

  function handleApply() {
    if (!text.trim()) {
      addToast("Enter some text", "error");
      return;
    }

    const partId = addText({
      text: text.trim(),
      height,
      depth,
      alignment,
    });

    if (partId) {
      select(partId);
      setTransformMode("translate");
      addToast("Created text", "success");
    } else {
      addToast("Failed to create text", "error");
    }
    onOpenChange(false);
  }

  function handleCancel() {
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Add Text">
        <div className="flex flex-col gap-4 py-2">
          {/* Text input */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs text-text-muted">Text</label>
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="Enter text..."
              rows={3}
              className="w-full rounded border border-border bg-card px-3 py-2 text-sm text-text outline-none focus:border-brand resize-none"
              autoFocus
            />
          </div>

          {/* Height input */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs text-text-muted">Height</label>
            <div className="flex items-center gap-2">
              <input
                type="number"
                value={height}
                onChange={(e) => setHeight(Math.max(0.1, parseFloat(e.target.value) || 0.1))}
                min={0.1}
                step={1}
                className="flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-brand"
              />
              <span className="text-xs text-text-muted">mm</span>
            </div>
          </div>

          {/* Depth input */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs text-text-muted">Depth</label>
            <div className="flex items-center gap-2">
              <input
                type="number"
                value={depth}
                onChange={(e) => setDepth(Math.max(0.1, parseFloat(e.target.value) || 0.1))}
                min={0.1}
                step={0.5}
                className="flex-1 rounded border border-border bg-card px-3 py-1.5 text-sm text-text outline-none focus:border-brand"
              />
              <span className="text-xs text-text-muted">mm</span>
            </div>
          </div>

          {/* Alignment */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs text-text-muted">Alignment</label>
            <div className="flex gap-1">
              {(["left", "center", "right"] as const).map((align) => (
                <button
                  key={align}
                  type="button"
                  onClick={() => setAlignment(align)}
                  className={`flex-1 px-3 py-1.5 text-xs capitalize rounded border transition-colors ${
                    alignment === align
                      ? "border-brand bg-brand/10 text-brand"
                      : "border-border text-text-muted hover:border-text-muted"
                  }`}
                >
                  {align}
                </button>
              ))}
            </div>
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
