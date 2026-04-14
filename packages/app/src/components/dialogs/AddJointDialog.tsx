import { useState } from "react";
import { Anchor } from "@phosphor-icons/react/dist/ssr/Anchor";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { Spiral } from "@phosphor-icons/react/dist/ssr/Spiral";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useDocumentStore, useUiStore } from "@vcad/core";
import type { JointKind } from "@vcad/ir";
import { cn } from "@/lib/utils";

interface AddJointDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const JOINT_TYPES: {
  kind: JointKind;
  label: string;
  description: string;
  icon: typeof Anchor;
}[] = [
  {
    kind: { type: "Fixed" },
    label: "Fixed",
    description: "No relative motion",
    icon: Anchor,
  },
  {
    kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 } },
    label: "Revolute",
    description: "Rotates around Z axis",
    icon: ArrowsClockwise,
  },
  {
    kind: { type: "Slider", axis: { x: 0, y: 0, z: 1 } },
    label: "Slider",
    description: "Slides along Z axis",
    icon: ArrowUp,
  },
  {
    kind: { type: "Cylindrical", axis: { x: 0, y: 0, z: 1 } },
    label: "Cylindrical",
    description: "Rotates and slides on Z",
    icon: Spiral,
  },
  {
    kind: { type: "Ball" },
    label: "Ball",
    description: "Free rotation in all axes",
    icon: Globe,
  },
];

export function AddJointDialog({ open, onOpenChange }: AddJointDialogProps) {
  const document = useDocumentStore((s) => s.document);
  const addJoint = useDocumentStore((s) => s.addJoint);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const select = useUiStore((s) => s.select);

  const [selectedKind, setSelectedKind] = useState<JointKind>(
    JOINT_TYPES[0]!.kind,
  );

  // Get selected instances
  const selectedInstanceIds = Array.from(selectedPartIds).filter((id) =>
    document.instances?.some((i) => i.id === id),
  );

  const parentId =
    selectedInstanceIds.length >= 1 ? selectedInstanceIds[0] : null;
  const childId =
    selectedInstanceIds.length >= 2 ? selectedInstanceIds[1] : null;

  const parentInstance = document.instances?.find((i) => i.id === parentId);
  const childInstance = document.instances?.find((i) => i.id === childId);

  function handleAdd() {
    if (!childId) return;

    const jointId = addJoint({
      parentInstanceId: parentId ?? null,
      childInstanceId: childId,
      parentAnchor: { x: 0, y: 0, z: 0 },
      childAnchor: { x: 0, y: 0, z: 0 },
      kind: selectedKind,
    });

    select(`joint:${jointId}`);
    onOpenChange(false);
  }

  function handleCancel() {
    onOpenChange(false);
  }

  const canAdd = childId !== null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Add Joint">
        <div className="space-y-4">
          {/* Connection info */}
          <div className="text-xs text-text-muted">
            <p className="mb-2">Connecting:</p>
            <div className="flex items-center gap-2 pl-2">
              <span className="font-medium text-text">
                {parentInstance?.name ?? "Ground"}
              </span>
              <span className="text-text-muted">→</span>
              <span className="font-medium text-text">
                {childInstance?.name ?? "(select second instance)"}
              </span>
            </div>
          </div>

          {/* Joint type selection */}
          <div>
            <p className="text-xs text-text-muted mb-2">Joint type:</p>
            <div className="space-y-1">
              {JOINT_TYPES.map(({ kind, label, description, icon: Icon }) => (
                <button
                  key={kind.type}
                  className={cn(
                    "w-full flex items-center gap-3 px-3 py-2 text-xs text-left",
                    "border border-transparent",
                    "hover:bg-hover",
                    selectedKind.type === kind.type &&
                      "bg-brand/20 text-brand border-brand/30",
                  )}
                  onClick={() => setSelectedKind(kind)}
                >
                  <Icon size={16} className="shrink-0" />
                  <div className="flex-1 min-w-0">
                    <div className="font-medium">{label}</div>
                    <div className="text-[10px] text-text-muted truncate">
                      {description}
                    </div>
                  </div>
                  {selectedKind.type === kind.type && (
                    <Check size={14} className="shrink-0" />
                  )}
                </button>
              ))}
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={handleCancel}>
            Cancel
          </Button>
          <Button
            variant="default"
            size="sm"
            disabled={!canAdd}
            onClick={handleAdd}
          >
            Add Joint
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
