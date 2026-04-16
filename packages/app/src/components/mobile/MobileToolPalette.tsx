import { useState, type ReactNode } from "react";
import { useUiStore, type ToolbarTab } from "@vcad/core";
import { BottomSheet } from "./BottomSheet";
import { TAB_COLORS } from "@/components/ui/toolbar-constants";
import { cn } from "@/lib/utils";
import {
  ALL_TABS,
  useToolDefinitions,
  type ToolDef,
} from "@/hooks/useToolDefinitions";

/**
 * Mobile tool palette. A horizontally-scrolling tab bar across the bottom of
 * the screen — tapping a tab opens a bottom sheet whose grid mirrors the
 * desktop ToolPalette's active-tab content. Tool definitions come from
 * `useToolDefinitions`, the same hook that powers desktop, so the two stay
 * at parity automatically.
 */
export function MobileToolPalette() {
  const toolbarTab = useUiStore((s) => s.toolbarTab);
  const setToolbarTab = useUiStore((s) => s.setToolbarTab);
  const { byTab, renderSimulateExtras } = useToolDefinitions();
  const [sheetTab, setSheetTab] = useState<ToolbarTab | null>(null);

  const handleTap = (tab: ToolbarTab) => {
    setToolbarTab(tab);
    setSheetTab(tab);
  };

  const activeTab = sheetTab;
  const activeMeta = activeTab ? ALL_TABS.find((t) => t.id === activeTab) : null;
  const activeDefs = activeTab ? byTab[activeTab] : [];

  return (
    <>
      <div
        className={cn(
          "flex h-14 shrink-0 items-stretch border-t border-border/40 bg-surface",
          "overflow-x-auto no-scrollbar",
          "pb-[env(safe-area-inset-bottom)]",
        )}
      >
        {ALL_TABS.map(({ id, label, icon: Icon }) => {
          const isActive = toolbarTab === id;
          return (
            <button
              key={id}
              onClick={() => handleTap(id)}
              className={cn(
                "flex min-w-[72px] flex-1 flex-col items-center justify-center gap-0.5 min-h-11 px-2",
                "text-text-muted active:bg-hover",
                isActive && "text-text",
              )}
              aria-label={label}
            >
              <Icon
                size={22}
                weight={isActive ? "fill" : "regular"}
                className={cn(isActive && TAB_COLORS[id])}
              />
              <span className="text-[10px] leading-none">{label}</span>
            </button>
          );
        })}
      </div>

      <BottomSheet
        open={sheetTab !== null}
        onOpenChange={(open) => !open && setSheetTab(null)}
        title={activeMeta?.label}
      >
        <div className="p-3">
          <ToolGrid defs={activeDefs} onRun={() => setSheetTab(null)} />
          {activeTab === "simulate" && (
            <div className="mt-3 border-t border-border/40 pt-3">
              {renderSimulateExtras({ compact: true })}
            </div>
          )}
          {activeDefs.length === 0 && (
            <div className="py-8 text-center text-sm text-text-muted">
              No tools on this tab yet.
            </div>
          )}
        </div>
      </BottomSheet>
    </>
  );
}

function ToolGrid({
  defs,
  onRun,
}: {
  defs: ToolDef[];
  onRun: () => void;
}): ReactNode {
  return (
    <div className="grid grid-cols-3 gap-2">
      {defs.map((def) => (
        <ToolTile
          key={def.id}
          def={def}
          onClick={() => {
            def.onClick();
            onRun();
          }}
        />
      ))}
    </div>
  );
}

function ToolTile({ def, onClick }: { def: ToolDef; onClick: () => void }) {
  const Icon = def.icon;
  return (
    <button
      onClick={onClick}
      disabled={!def.enabled}
      className={cn(
        "flex aspect-square flex-col items-center justify-center gap-1.5 rounded",
        "border border-border bg-card active:bg-hover",
        "disabled:opacity-30 disabled:active:bg-card",
        def.active && "border-brand bg-brand/10",
        def.pulse && "animate-pulse",
      )}
    >
      <Icon size={28} className={def.iconColor} />
      <span className="text-[11px] text-text leading-none text-center px-1">
        {def.label}
      </span>
      {def.shortcut && (
        <span className="text-[9px] text-text-muted/60 font-mono leading-none">
          {def.shortcut}
        </span>
      )}
    </button>
  );
}
