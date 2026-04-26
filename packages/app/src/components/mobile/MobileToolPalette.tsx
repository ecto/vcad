import { useEffect } from "react";
import { useSketchStore, useUiStore } from "@vcad/core";
import { TAB_COLORS } from "@/components/ui/toolbar-constants";
import { cn } from "@/lib/utils";
import {
  ALL_TABS,
  useToolDefinitions,
  type ToolDef,
} from "@/hooks/useToolDefinitions";

/**
 * Mobile tool palette. Two rows mirroring the desktop layout:
 *   secondary tool row — horizontal-scroll list of the active tab's tools
 *   tab bar            — primary categories (Create / Sketch / Transform / …)
 *
 * Tool definitions come from `useToolDefinitions`, the same hook that powers
 * desktop, so the two stay at parity automatically.
 */
export function MobileToolPalette() {
  const toolbarTab = useUiStore((s) => s.toolbarTab);
  const setToolbarTab = useUiStore((s) => s.setToolbarTab);
  const sketchActive = useSketchStore((s) => s.active);
  const { byTab, renderSimulateExtras } = useToolDefinitions();

  // Sketch tab is permanent in ALL_TABS. Auto-select it whenever sketch
  // becomes active so the relevant tools are at hand.
  useEffect(() => {
    if (sketchActive) setToolbarTab("sketch");
  }, [sketchActive, setToolbarTab]);

  const activeDefs = byTab[toolbarTab] ?? [];

  return (
    <>
      {/* Secondary tool row — active tab's tools, scrollable horizontally. */}
      <div
        className={cn(
          "flex h-12 shrink-0 items-stretch border-t border-border/40 bg-surface",
          "overflow-x-auto no-scrollbar gap-1 px-2",
        )}
      >
        {activeDefs.length === 0 ? (
          <div className="flex items-center text-text-muted/60 text-xs px-2">
            No tools on this tab yet.
          </div>
        ) : (
          activeDefs.map((def) => <ToolTile key={def.id} def={def} />)
        )}
        {toolbarTab === "simulate" && (
          <div className="flex items-center pl-2 border-l border-border/40 ml-1">
            {renderSimulateExtras({ compact: true })}
          </div>
        )}
      </div>

      {/* Primary tab bar. */}
      <div
        className={cn(
          "flex h-12 shrink-0 items-stretch border-t border-border/40 bg-surface",
          "overflow-x-auto no-scrollbar",
          "pb-[env(safe-area-inset-bottom)]",
        )}
      >
        {ALL_TABS.map(({ id, label, icon: Icon }) => {
          const isActive = toolbarTab === id;
          return (
            <button
              key={id}
              onClick={() => setToolbarTab(id)}
              className={cn(
                "flex min-w-[64px] flex-1 flex-col items-center justify-center gap-0.5 min-h-11 px-2",
                "text-text-muted active:bg-hover",
                isActive && "text-text",
              )}
              aria-label={label}
              aria-pressed={isActive}
            >
              <Icon
                size={18}
                weight={isActive ? "fill" : "regular"}
                className={cn(isActive && TAB_COLORS[id])}
              />
              <span className="text-[10px] leading-none">{label}</span>
            </button>
          );
        })}
      </div>
    </>
  );
}

function ToolTile({ def }: { def: ToolDef }) {
  const Icon = def.icon;
  return (
    <button
      onClick={def.onClick}
      disabled={!def.enabled}
      title={def.label}
      className={cn(
        "flex shrink-0 items-center gap-1.5 px-3 rounded",
        "text-xs text-text active:bg-hover",
        "disabled:opacity-30",
        def.active && "bg-brand/10 text-brand",
        def.pulse && "animate-pulse",
      )}
    >
      <Icon size={16} className={def.iconColor} />
      <span className="leading-none whitespace-nowrap">{def.label}</span>
    </button>
  );
}
