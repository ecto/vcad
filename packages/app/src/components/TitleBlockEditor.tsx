import { useState } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Button } from "@/components/ui/button";
import { useDocumentStore } from "@vcad/core";
import type { DrawingTitleBlock } from "@vcad/ir";

const FIELDS: Array<{ key: keyof DrawingTitleBlock; label: string; placeholder: string }> = [
  { key: "partName", label: "Part name", placeholder: "Bracket, rev A" },
  { key: "author", label: "Drawn by", placeholder: "Your name" },
  { key: "date", label: "Date", placeholder: "2026-07-18" },
  { key: "scale", label: "Scale", placeholder: "1:1" },
  { key: "material", label: "Material", placeholder: "6061-T6 AL" },
  { key: "revision", label: "Revision", placeholder: "A" },
];

/**
 * Title block editor for the 2D drawing sheet. Fields persist on the
 * document (`document.drawing.titleBlock`) and render on both the on-screen
 * sheet and the exported PDF.
 */
export function TitleBlockEditor({ onClose }: { onClose: () => void }) {
  const persisted = useDocumentStore((s) => s.document.drawing?.titleBlock);
  const setDrawingSettings = useDocumentStore((s) => s.setDrawingSettings);

  const [fields, setFields] = useState<DrawingTitleBlock>({
    partName: persisted?.partName ?? "",
    author: persisted?.author ?? "",
    date: persisted?.date ?? new Date().toISOString().slice(0, 10),
    scale: persisted?.scale ?? "",
    material: persisted?.material ?? "",
    revision: persisted?.revision ?? "A",
  });

  function handleApply() {
    setDrawingSettings({ titleBlock: fields });
    onClose();
  }

  return (
    <div className="fixed right-4 bottom-20 z-40 w-64 border border-border bg-card p-3 shadow-lg">
      <div className="mb-2 flex items-center justify-between">
        <span className="text-xs font-medium text-text">Title Block</span>
        <Button variant="ghost" size="icon-sm" onClick={onClose}>
          <X size={14} />
        </Button>
      </div>
      <div className="flex flex-col gap-2">
        {FIELDS.map(({ key, label, placeholder }) => (
          <label key={key} className="flex flex-col gap-1 text-xs text-text-muted">
            {label}
            <input
              value={fields[key] ?? ""}
              placeholder={placeholder}
              onChange={(e) => setFields((f) => ({ ...f, [key]: e.target.value }))}
              className="h-7 border border-border bg-surface px-2 text-xs text-text"
            />
          </label>
        ))}
      </div>
      <div className="mt-3 flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="default" size="sm" onClick={handleApply}>
          Apply
        </Button>
      </div>
    </div>
  );
}
