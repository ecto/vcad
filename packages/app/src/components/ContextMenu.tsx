import * as RadixContextMenu from "@radix-ui/react-context-menu";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Unite } from "@phosphor-icons/react/dist/ssr/Unite";
import { Subtract } from "@phosphor-icons/react/dist/ssr/Subtract";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { useDocumentStore, useUiStore, useEngineStore } from "@vcad/core";
import type { ReactNode } from "react";

function MenuItem({
  icon: Icon,
  label,
  shortcut,
  disabled,
  onClick,
}: {
  icon: typeof Copy;
  label: string;
  shortcut?: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <RadixContextMenu.Item
      className="group flex items-center gap-2  px-2 py-1.5 text-xs text-text outline-none cursor-pointer data-[disabled]:opacity-40 data-[disabled]:cursor-default data-[highlighted]:bg-accent/20 data-[highlighted]:text-accent"
      disabled={disabled}
      onClick={onClick}
    >
      <Icon size={14} className="shrink-0" />
      <span className="flex-1">{label}</span>
      {shortcut && (
        <span className="ml-4 text-[10px] text-text-muted">{shortcut}</span>
      )}
    </RadixContextMenu.Item>
  );
}

export function ContextMenu({ children }: { children: ReactNode }) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const select = useUiStore((s) => s.select);
  const removePart = useDocumentStore((s) => s.removePart);
  const duplicateParts = useDocumentStore((s) => s.duplicateParts);
  const applyBoolean = useDocumentStore((s) => s.applyBoolean);

  const hasSelection = selectedPartIds.size > 0;
  const hasTwoSelected = selectedPartIds.size === 2;

  function handleDelete() {
    for (const id of selectedPartIds) {
      removePart(id);
    }
    clearSelection();
  }

  function handleDuplicate() {
    const ids = Array.from(selectedPartIds);
    const newIds = duplicateParts(ids);
    useUiStore.getState().selectMultiple(newIds);
  }

  function handleBoolean(type: "union" | "difference" | "intersection") {
    if (!hasTwoSelected) return;
    const ids = Array.from(selectedPartIds);
    const newId = applyBoolean(type, ids[0]!, ids[1]!);
    if (newId) select(newId);
  }

  return (
    <RadixContextMenu.Root>
      <RadixContextMenu.Trigger asChild>{children}</RadixContextMenu.Trigger>
      <RadixContextMenu.Portal>
        <RadixContextMenu.Content className="z-50 min-w-[180px]  border border-border bg-card p-1 shadow-xl">
          <MenuItem
            icon={Copy}
            label="Duplicate"
            shortcut="Ctrl+D"
            disabled={!hasSelection}
            onClick={handleDuplicate}
          />
          <MenuItem
            icon={PencilSimple}
            label="Rename"
            disabled={selectedPartIds.size !== 1}
            onClick={() => {
              // Dispatch custom event for inline rename
              window.dispatchEvent(new CustomEvent("vcad:rename-part"));
            }}
          />
          <MenuItem
            icon={Trash}
            label="Delete"
            shortcut="Del"
            disabled={!hasSelection}
            onClick={handleDelete}
          />

          <RadixContextMenu.Separator className="my-1 h-px bg-border" />

          <MenuItem
            icon={Unite}
            label="Union"
            shortcut="Ctrl+Shift+U"
            disabled={!hasTwoSelected}
            onClick={() => handleBoolean("union")}
          />
          <MenuItem
            icon={Subtract}
            label="Difference"
            shortcut="Ctrl+Shift+D"
            disabled={!hasTwoSelected}
            onClick={() => handleBoolean("difference")}
          />
          <MenuItem
            icon={Intersect}
            label="Intersection"
            shortcut="Ctrl+Shift+I"
            disabled={!hasTwoSelected}
            onClick={() => handleBoolean("intersection")}
          />

          <RadixContextMenu.Separator className="my-1 h-px bg-border" />

          <MenuItem
            icon={Circuitry}
            label="Add PCB Board"
            onClick={() => {
              window.dispatchEvent(new CustomEvent("vcad:open-pcb-dialog"));
            }}
          />
          <MenuItem
            icon={Circuitry}
            label="Design PCB to fit"
            disabled={selectedPartIds.size !== 1}
            onClick={() => {
              const partId = Array.from(selectedPartIds)[0]!;
              const scene = useEngineStore.getState().scene;
              const parts = useDocumentStore.getState().parts;
              const partIdx = parts.findIndex((p) => p.id === partId);
              const evalPart = partIdx >= 0 ? scene?.parts?.[partIdx] : null;
              if (evalPart?.mesh?.positions && evalPart.mesh.positions.length >= 3) {
                const pos = evalPart.mesh.positions;
                let minX = Infinity, maxX = -Infinity;
                let minY = Infinity, maxY = -Infinity;
                for (let i = 0; i < pos.length; i += 3) {
                  const x = pos[i]!, y = pos[i + 1]!;
                  if (x < minX) minX = x;
                  if (x > maxX) maxX = x;
                  if (y < minY) minY = y;
                  if (y > maxY) maxY = y;
                }
                const w = Math.ceil((maxX - minX) * 10) / 10;
                const h = Math.ceil((maxY - minY) * 10) / 10;
                window.dispatchEvent(new CustomEvent("vcad:fit-pcb-dialog", { detail: { width: w, height: h } }));
              } else {
                window.dispatchEvent(new CustomEvent("vcad:open-pcb-dialog"));
              }
            }}
          />
        </RadixContextMenu.Content>
      </RadixContextMenu.Portal>
    </RadixContextMenu.Root>
  );
}
