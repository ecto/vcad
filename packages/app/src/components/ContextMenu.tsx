import * as RadixContextMenu from "@radix-ui/react-context-menu";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Unite } from "@phosphor-icons/react/dist/ssr/Unite";
import { Subtract } from "@phosphor-icons/react/dist/ssr/Subtract";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { CrosshairSimple } from "@phosphor-icons/react/dist/ssr/CrosshairSimple";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import {
  useDocumentStore,
  useUiStore,
  useEngineStore,
  SELECTION_FILTER_OPTIONS,
  type SelectionFilter,
} from "@vcad/core";
import type { ReactNode } from "react";
import { useCallback } from "react";
import {
  nativeMenuAvailable,
  popupNativeContextMenu,
  type NativeMenuItem,
} from "@/lib/native-context-menu";

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
      className="group flex items-center gap-2  px-2 py-1.5 text-xs text-text outline-none cursor-pointer data-[disabled]:opacity-40 data-[disabled]:cursor-default data-[highlighted]:bg-brand/20 data-[highlighted]:text-brand"
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

/**
 * Compute the dimensions of the design-PCB-to-fit feature for the
 * selected part — pulled into a helper so both the native and Radix
 * paths share the bounding-box logic.
 */
function dispatchDesignPcbForSelection() {
  const selectedIds = useUiStore.getState().selectedPartIds;
  if (selectedIds.size !== 1) return;
  const partId = Array.from(selectedIds)[0]!;
  const scene = useEngineStore.getState().scene;
  const parts = useDocumentStore.getState().parts;
  const partIdx = parts.findIndex((p) => p.id === partId);
  const evalPart = partIdx >= 0 ? scene?.parts?.[partIdx] : null;
  if (
    evalPart?.mesh?.positions &&
    evalPart.mesh.positions.length >= 3
  ) {
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
    window.dispatchEvent(
      new CustomEvent("vcad:fit-pcb-dialog", { detail: { width: w, height: h } }),
    );
  } else {
    window.dispatchEvent(new CustomEvent("vcad:open-pcb-dialog"));
  }
}

export function ContextMenu({ children }: { children: ReactNode }) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const select = useUiStore((s) => s.select);
  const selectionFilter = useUiStore((s) => s.selectionFilter);
  const setSelectionFilter = useUiStore((s) => s.setSelectionFilter);
  const removePart = useDocumentStore((s) => s.removePart);
  const duplicateParts = useDocumentStore((s) => s.duplicateParts);
  const applyBoolean = useDocumentStore((s) => s.applyBoolean);

  const hasSelection = selectedPartIds.size > 0;
  const hasTwoSelected = selectedPartIds.size === 2;
  const hasOneSelected = selectedPartIds.size === 1;

  const handleDelete = useCallback(() => {
    const ids = useUiStore.getState().selectedPartIds;
    for (const id of ids) {
      removePart(id);
    }
    clearSelection();
  }, [removePart, clearSelection]);

  const handleDuplicate = useCallback(() => {
    const ids = Array.from(useUiStore.getState().selectedPartIds);
    const newIds = duplicateParts(ids);
    useUiStore.getState().selectMultiple(newIds);
  }, [duplicateParts]);

  const handleBoolean = useCallback(
    (type: "union" | "difference" | "intersection") => {
      const ids = Array.from(useUiStore.getState().selectedPartIds);
      if (ids.length !== 2) return;
      const newId = applyBoolean(type, ids[0]!, ids[1]!);
      if (newId) select(newId);
    },
    [applyBoolean, select],
  );

  // Native popup path — composed at click time so disabled/checked state
  // reflects the latest store snapshot, and the radio submenu reads off
  // SELECTION_FILTER_OPTIONS without re-encoding it. Action ids match the
  // dispatch table at the bottom; rename `vcad:` events for clarity.
  const buildNativeMenu = (): NativeMenuItem[] => {
    const filter = useUiStore.getState().selectionFilter;
    const sel = useUiStore.getState().selectedPartIds;
    const has = sel.size > 0;
    const oneSel = sel.size === 1;
    const twoSel = sel.size === 2;
    return [
      { kind: "item", id: "duplicate", label: "Duplicate", accelerator: "CmdOrCtrl+D", disabled: !has },
      { kind: "item", id: "rename", label: "Rename", disabled: !oneSel },
      { kind: "item", id: "delete", label: "Delete", accelerator: "Delete", disabled: !has },
      { kind: "separator" },
      {
        kind: "submenu",
        label: "Selection priority",
        items: SELECTION_FILTER_OPTIONS.map((o) => ({
          kind: "item" as const,
          id: `selfilter:${o.value}`,
          label: o.hotkey ? `${o.label}  (${o.hotkey})` : o.label,
          checked: filter === o.value,
        })),
      },
      { kind: "separator" },
      { kind: "item", id: "boolean:union", label: "Union", accelerator: "CmdOrCtrl+Shift+U", disabled: !twoSel },
      { kind: "item", id: "boolean:difference", label: "Difference", accelerator: "CmdOrCtrl+Shift+D", disabled: !twoSel },
      { kind: "item", id: "boolean:intersection", label: "Intersection", accelerator: "CmdOrCtrl+Shift+I", disabled: !twoSel },
      { kind: "separator" },
      { kind: "item", id: "pcb:add", label: "Add PCB Board" },
      { kind: "item", id: "pcb:fit", label: "Design PCB to fit", disabled: !oneSel },
    ];
  };

  const dispatchById = (id: string) => {
    if (id === "duplicate") return handleDuplicate();
    if (id === "rename")
      return window.dispatchEvent(new CustomEvent("vcad:rename-part"));
    if (id === "delete") return handleDelete();
    if (id.startsWith("selfilter:")) {
      const v = id.slice("selfilter:".length) as SelectionFilter;
      return setSelectionFilter(v);
    }
    if (id.startsWith("boolean:")) {
      const op = id.slice("boolean:".length) as
        | "union"
        | "difference"
        | "intersection";
      return handleBoolean(op);
    }
    if (id === "pcb:add")
      return window.dispatchEvent(new CustomEvent("vcad:open-pcb-dialog"));
    if (id === "pcb:fit") return dispatchDesignPcbForSelection();
  };

  // Native path — a single right-click handler swaps the Radix root for
  // a real OS menu. We still render Radix's <Trigger> so `children` keeps
  // its tabindex/aria, but we intercept contextmenu before Radix sees it.
  const onNativeContext = async (e: React.MouseEvent) => {
    if (!nativeMenuAvailable()) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      const id = await popupNativeContextMenu(buildNativeMenu());
      if (id) dispatchById(id);
    } catch {
      // Native popup failed — Radix handles the fallback on the next click.
    }
  };

  if (nativeMenuAvailable()) {
    return (
      <div onContextMenu={onNativeContext} className="contents">
        {children}
      </div>
    );
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
            disabled={!hasOneSelected}
            onClick={() => {
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

          <RadixContextMenu.Sub>
            <RadixContextMenu.SubTrigger className="group flex items-center gap-2  px-2 py-1.5 text-xs text-text outline-none cursor-pointer data-[highlighted]:bg-brand/20 data-[highlighted]:text-brand data-[state=open]:bg-brand/20 data-[state=open]:text-brand">
              <CrosshairSimple size={14} className="shrink-0" />
              <span className="flex-1">Selection priority</span>
              <span className="ml-4 text-[10px] text-text-muted uppercase tracking-wide">
                {SELECTION_FILTER_OPTIONS.find((o) => o.value === selectionFilter)?.label ?? "Auto"}
              </span>
            </RadixContextMenu.SubTrigger>
            <RadixContextMenu.Portal>
              <RadixContextMenu.SubContent
                className="z-50 min-w-[200px] border border-border bg-card p-1 shadow-xl"
                sideOffset={2}
                alignOffset={-4}
              >
                {SELECTION_FILTER_OPTIONS.map(({ value, label, hotkey }) => {
                  const active = selectionFilter === value;
                  return (
                    <RadixContextMenu.Item
                      key={value}
                      className="group flex items-center gap-2 px-2 py-1.5 text-xs text-text outline-none cursor-pointer data-[highlighted]:bg-brand/20 data-[highlighted]:text-brand"
                      onClick={() => setSelectionFilter(value)}
                    >
                      {active ? (
                        <Check size={14} weight="bold" className="shrink-0 text-brand" />
                      ) : (
                        <span className="w-[14px] shrink-0" />
                      )}
                      <span className="flex-1">{label}</span>
                      {hotkey && (
                        <span className="ml-4 text-[10px] text-text-muted">{hotkey}</span>
                      )}
                    </RadixContextMenu.Item>
                  );
                })}
              </RadixContextMenu.SubContent>
            </RadixContextMenu.Portal>
          </RadixContextMenu.Sub>

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
            disabled={!hasOneSelected}
            onClick={dispatchDesignPcbForSelection}
          />
        </RadixContextMenu.Content>
      </RadixContextMenu.Portal>
    </RadixContextMenu.Root>
  );
}
