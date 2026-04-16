import { useState, useEffect, useCallback, useRef } from "react";
import { ToolbarButton, MoreDropdown } from "@/components/ui/toolbar";
import { TAB_COLORS, MOBILE_BREAKPOINT } from "@/components/ui/toolbar-constants";
import { useDocumentStore, useUiStore, type ToolbarTab } from "@vcad/core";
import { useDrawingStore } from "@/stores/drawing-store";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { cn } from "@/lib/utils";
import { useToolDefinitions, ALL_TABS, type ToolDef } from "@/hooks/useToolDefinitions";

// Responsive breakpoints and widths
const TAB_WIDTH_DESKTOP = 95;
const TAB_WIDTH_MOBILE = 44;
const CHAT_WIDTH = 70;
const MORE_WIDTH = 44;
const MIN_VISIBLE_TABS = 0;

export function ToolPalette() {
  const toolbarExpanded = useUiStore((s) => s.toolbarExpanded);
  const toolbarTab = useUiStore((s) => s.toolbarTab);
  const setToolbarTab = useUiStore((s) => s.setToolbarTab);

  const { byTab, renderSimulateExtras } = useToolDefinitions();

  // Responsive toolbar — track how many tabs fit
  const [visibleTabCount, setVisibleTabCount] = useState(ALL_TABS.length);
  const toolbarRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function calculateVisibleTabs() {
      const viewportWidth = window.innerWidth;
      const isMobile = viewportWidth < MOBILE_BREAKPOINT;
      const tabWidth = isMobile ? TAB_WIDTH_MOBILE : TAB_WIDTH_DESKTOP;
      const padding = isMobile ? 24 : 40;
      const availableWidth = viewportWidth - padding;
      const fixedWidth = CHAT_WIDTH + MORE_WIDTH;
      const tabsWidth = availableWidth - fixedWidth;
      const maxTabs = Math.max(MIN_VISIBLE_TABS, Math.floor(tabsWidth / tabWidth));
      setVisibleTabCount(Math.min(maxTabs, ALL_TABS.length));
    }

    calculateVisibleTabs();
    window.addEventListener("resize", calculateVisibleTabs);
    return () => window.removeEventListener("resize", calculateVisibleTabs);
  }, []);

  const overflowTabs = ALL_TABS.slice(visibleTabCount);
  const displayedTab = toolbarTab;

  // Track manual tab clicks to temporarily disable auto-switch
  const manualOverrideRef = useRef(false);
  const manualOverrideTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleTabClick = useCallback(
    (tab: ToolbarTab) => {
      manualOverrideRef.current = true;
      if (manualOverrideTimeout.current) {
        clearTimeout(manualOverrideTimeout.current);
      }
      manualOverrideTimeout.current = setTimeout(() => {
        manualOverrideRef.current = false;
      }, 2000);
      setToolbarTab(tab);
    },
    [setToolbarTab],
  );

  // Keyboard shortcuts: 1-7 to switch tabs
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)
        return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      const tabIndex = parseInt(e.key) - 1;
      if (tabIndex >= 0 && tabIndex < ALL_TABS.length) {
        handleTabClick(ALL_TABS[tabIndex]!.id);
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleTabClick]);

  // Auto-switch tabs based on context
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const viewMode = useDrawingStore((s) => s.viewMode);
  const docInstances = useDocumentStore((s) => s.document.instances);
  const docPartDefs = useDocumentStore((s) => s.document.partDefs);
  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);

  const hasSelection = selectedPartIds.size > 0;
  const hasTwoSelected = selectedPartIds.size === 2;
  const isAssemblyMode =
    (docPartDefs && Object.keys(docPartDefs).length > 0) ||
    (docInstances && docInstances.length > 0);
  const hasInstanceSelected = Array.from(selectedPartIds).some((id) =>
    docInstances?.some((i) => i.id === id),
  );

  const autoSwitchTab = useCallback(() => {
    if (guidedFlowActive || manualOverrideRef.current) return;

    if (viewMode === "2d") {
      setToolbarTab("build");
      return;
    }
    if (hasInstanceSelected && isAssemblyMode) {
      setToolbarTab("assembly");
      return;
    }
    if (hasTwoSelected) {
      setToolbarTab("combine");
      return;
    }
    if (hasSelection) {
      setToolbarTab("transform");
      return;
    }
    if (
      !hasSelection &&
      toolbarTab !== "modify" &&
      toolbarTab !== "simulate" &&
      toolbarTab !== "build"
    ) {
      setToolbarTab("create");
    }
  }, [
    guidedFlowActive,
    viewMode,
    hasInstanceSelected,
    isAssemblyMode,
    hasTwoSelected,
    hasSelection,
    toolbarTab,
    setToolbarTab,
  ]);

  useEffect(() => {
    autoSwitchTab();
  }, [selectedPartIds.size, viewMode, hasInstanceSelected, autoSwitchTab]);

  // Render tab content — ToolDefs first, then any tab-specific extras
  const renderTabContent = (tab?: ToolbarTab) => {
    const targetTab = tab ?? displayedTab;
    const defs = byTab[targetTab];
    return (
      <>
        {defs.map((def) => (
          <ToolPaletteButton
            key={def.id}
            def={def}
            expanded={toolbarExpanded}
          />
        ))}
        {targetTab === "simulate" && renderSimulateExtras()}
      </>
    );
  };

  return (
    <div
      ref={toolbarRef}
      className={cn("tool-palette flex flex-col", "bg-surface")}
    >
      {/* Row 1: tab strip */}
      <div className="flex h-7 items-stretch border-b border-border/40">
        {ALL_TABS.slice(0, visibleTabCount).map(({ id, label, icon: Icon }, index) => {
          const isActive = displayedTab === id;
          return (
            <button
              key={id}
              onClick={() => handleTabClick(id)}
              className={cn(
                "flex items-center gap-1.5 px-3 text-[11px] font-medium border-b-2",
                "transition-colors",
                isActive
                  ? cn("border-brand text-text bg-hover/30")
                  : "border-transparent text-text-muted hover:text-text hover:bg-hover/20",
              )}
              title={`${index + 1}. ${label}`}
            >
              <Icon size={13} className={cn(isActive && TAB_COLORS[id])} />
              <span>{label}</span>
              <span className="ml-1 text-[9px] text-text-muted/60 font-mono hidden sm:inline">
                {index + 1}
              </span>
            </button>
          );
        })}
        {overflowTabs.length > 0 && (
          <MoreDropdown
            tabs={overflowTabs}
            activeTab={toolbarTab}
            onSelect={handleTabClick}
            colors={TAB_COLORS}
          >
            {(tab) => renderTabContent(tab)}
          </MoreDropdown>
        )}
        <div className="flex-1" />
      </div>

      {/* Row 2: active tab content — rendered inline, not in a popover */}
      <div className="flex items-center gap-0.5 px-2 h-7">
        {renderTabContent(displayedTab)}
      </div>
    </div>
  );
}

function ToolPaletteButton({ def, expanded }: { def: ToolDef; expanded: boolean }) {
  const Icon = def.icon;
  const readOnlyShare = useUiStore((s) => s.readOnlyShare);
  const handleClick = () => {
    if (readOnlyShare) {
      window.dispatchEvent(
        new CustomEvent("vcad:fork-prompt", { detail: readOnlyShare }),
      );
      return;
    }
    def.onClick();
  };
  return (
    <ToolbarButton
      tooltip={readOnlyShare ? "Sign in to fork — this doc is read-only" : def.tooltip}
      active={def.active}
      disabled={!def.enabled}
      onClick={handleClick}
      pulse={def.pulse}
      expanded={expanded}
      label={def.label}
      shortcut={def.shortcut}
      iconColor={def.iconColor}
      className={cn(def.className, readOnlyShare && "opacity-50")}
      labelClassName={def.labelClassName}
    >
      <Icon size={15} />
    </ToolbarButton>
  );
}
