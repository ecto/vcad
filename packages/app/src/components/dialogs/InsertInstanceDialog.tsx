import { useState } from "react";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDocumentStore, useUiStore } from "@vcad/core";
import { cn } from "@/lib/utils";

interface InsertInstanceDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function InsertInstanceDialog({
  open,
  onOpenChange,
}: InsertInstanceDialogProps) {
  const document = useDocumentStore((s) => s.document);
  const createInstance = useDocumentStore((s) => s.createInstance);
  const select = useUiStore((s) => s.select);

  const [selectedPartDefId, setSelectedPartDefId] = useState<string | null>(
    null,
  );

  const partDefs = document.partDefs
    ? Object.values(document.partDefs)
    : [];

  function handleInsert() {
    if (!selectedPartDefId) return;

    const instanceId = createInstance(selectedPartDefId);
    select(instanceId);
    onOpenChange(false);
    setSelectedPartDefId(null);
  }

  function handleCancel() {
    onOpenChange(false);
    setSelectedPartDefId(null);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Insert Instance">
        <p className="text-xs text-text-muted mb-3">
          Select a part definition to create a new instance:
        </p>

        <div className="max-h-48 overflow-y-auto border border-border">
          {partDefs.length === 0 ? (
            <div className="p-4 text-center text-xs text-text-muted">
              No part definitions available.
              <br />
              Create one first by selecting a part and running "Create Part
              Definition".
            </div>
          ) : (
            partDefs.map((partDef) => (
              <button
                key={partDef.id}
                className={cn(
                  "w-full flex items-center gap-2 px-3 py-2 text-xs text-left",
                  "hover:bg-hover",
                  selectedPartDefId === partDef.id &&
                    "bg-brand/20 text-brand",
                )}
                onClick={() => setSelectedPartDefId(partDef.id)}
              >
                <Package size={14} className="shrink-0" />
                <span className="flex-1 truncate">
                  {partDef.name ?? partDef.id}
                </span>
                {selectedPartDefId === partDef.id && (
                  <Check size={14} className="shrink-0" />
                )}
              </button>
            ))
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={handleCancel}>
            Cancel
          </Button>
          <Button
            variant="default"
            size="sm"
            disabled={!selectedPartDefId}
            onClick={handleInsert}
          >
            Insert
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
