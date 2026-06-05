import { useState, useEffect, useCallback, useRef } from "react";
import { ToolbarButton, MoreDropdown } from "@/components/ui/toolbar";
import { RichTooltip } from "@/components/ui/tooltip";
import {
  TAB_COLORS,
  TAB_THEMES,
  TAB_DESCRIPTIONS,
  MOBILE_BREAKPOINT,
} from "@/components/ui/toolbar-constants";
import { useDocumentStore, useUiStore, useSketchStore, type ToolbarTab } from "@vcad/core";
import { useDrawingStore } from "@/stores/drawing-store";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { cn } from "@/lib/utils";
import { useToolDefinitions, getAllTabs, type ToolDef } from "@/hooks/useToolDefinitions";
import { useLocaleStore } from "@/stores/locale-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { CircuitTabTools } from "@/components/electronics/CircuitTabTools";

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
  useLocaleStore((s) => s.locale);

  const { byTab, renderSimulateExtras } = useToolDefinitions();
  const ALL_TABS = getAllTabs();
  // The Circuit tab is the home for the electronics workspace: being on it ⟺
  // editing a circuit (see handleTabClick + autoSwitchTab below).
  const electronicsActive = useElectronicsStore((s) => s.active);

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

  // Sketch tab is permanent in ALL_TABS. When sketch becomes active,
  // autoSwitchTab pins toolbarTab to "sketch" so the relevant tools are
  // in front; users can still click any other tab.
  const sketchActive = useSketchStore((s) => s.active);
  const visibleTabs = ALL_TABS.slice(0, visibleTabCount);
  const overflowTabs = ALL_TABS.slice(visibleTabCount);
  const displayedTab: ToolbarTab = toolbarTab;

  // Track manual tab clicks to temporarily disable auto-switch
  const manualOverrideRef = useRef(false);
  const manualOverrideTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleTabClick = useCallback(
    (tab: ToolbarTab) => {
      // Circuit tab ⟺ electronics workspace: entering the tab opens the circuit
      // (or the New-PCB entry when none exists); leaving it for any other tab
      // exits back to the mechanical workspace.
      const elx = useElectronicsStore.getState();
      if (tab === "circuit" && !elx.active) {
        // Instant start: enters the circuit, scaffolding a default board +
        // schematic first if the document has none, so you land in a working
        // schematic instead of an empty "no data" screen.
        elx.startCircuit();
      } else if (tab !== "circuit" && elx.active) {
        elx.exit();
      }
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

  // Keyboard shortcuts: 1-7 to switch tabs. Stay enabled during sketch so the
  // user can still browse Create/Transform/etc. while drawing — the sketch
  // tab itself is reachable via the tab strip or by re-entering sketch mode.
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
    // Editing a circuit pins the toolbar to the Circuit tab (highest priority,
    // like sketch) so the electronics tools are always the ones showing.
    if (electronicsActive) {
      setToolbarTab("circuit");
      return;
    }
    // Sketch wins over selection-driven defaults — once sketch is open we
    // pin the toolbar to the sketch tab regardless of what's selected.
    if (sketchActive) {
      setToolbarTab("sketch");
      return;
    }

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
      toolbarTab !== "build" &&
      toolbarTab !== "circuit"
    ) {
      setToolbarTab("create");
    }
  }, [
    guidedFlowActive,
    electronicsActive,
    sketchActive,
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
    // The Circuit tab's tools are electronics-specific and context-aware
    // (schematic vs board), rendered by CircuitTabTools rather than ToolDefs.
    if (targetTab === "circuit") return <CircuitTabTools />;
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
      <div className="flex h-8 items-stretch border-b border-border/30 px-1">
        {visibleTabs.map(({ id, label, icon: Icon }, index) => {
          const isActive = displayedTab === id;
          const theme = TAB_THEMES[id];
          const tabTools = byTab[id];
          return (
            <RichTooltip
              key={id}
              title={label}
              description={TAB_DESCRIPTIONS[id]}
              shortcut={String(index + 1)}
              accent={theme.accent}
              icon={<Icon size={18} className={theme.text} />}
              preview={
                tabTools.length > 0 ? (
                  <div className="flex flex-wrap gap-x-2 gap-y-1">
                    {tabTools.map((t) => {
                      const TIcon = t.icon;
                      return (
                        <span
                          key={t.id}
                          className={cn(
                            "inline-flex items-center gap-1 text-[10px] leading-none",
                            t.enabled ? "text-text-muted" : "text-text-muted/40",
                          )}
                        >
                          <TIcon
                            size={11}
                            className={cn(
                              t.iconColor,
                              !t.enabled && "opacity-40",
                            )}
                          />
                          <span>{t.label}</span>
                        </span>
                      );
                    })}
                  </div>
                ) : undefined
              }
              tip={isActive ? undefined : `Press ${index + 1} or click to switch`}
              side="top"
            >
              <button
                onClick={() => handleTabClick(id)}
                className={cn(
                  "group flex items-center gap-1.5 px-2 text-[11px] font-medium border-b-2 -mb-px",
                  "transition-colors",
                  isActive
                    ? cn("border-brand", theme.bg)
                    : cn("border-transparent", theme.hoverBg),
                )}
              >
                <Icon
                  size={14}
                  className={cn(
                    "transition-colors",
                    isActive ? theme.text : "text-text-muted",
                    !isActive && theme.groupHoverText,
                  )}
                />
                <span
                  className={cn(
                    "transition-colors",
                    isActive ? "text-text" : "text-text-muted group-hover:text-text",
                  )}
                >
                  {label}
                </span>
                <span className="ml-0.5 text-[10px] font-mono text-text-muted/40 hidden sm:inline">
                  {index + 1}
                </span>
              </button>
            </RichTooltip>
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

      {/* Row 2: active tab content — same horizontal rhythm as the tab strip,
          with horizontal scroll + soft right-edge fade when tools overflow. */}
      <div
        className={cn(
          "flex items-center h-8 px-1",
          "overflow-x-auto no-scrollbar",
          "[mask-image:linear-gradient(to_right,black_calc(100%-24px),transparent_100%)]",
        )}
      >
        {renderTabContent(displayedTab)}
      </div>
    </div>
  );
}

/**
 * Split a tooltip string like "Move (M)" or "Move (select a part)" into a
 * clean title and a status/description tail. Keeps the existing tooltip
 * data in useToolDefinitions usable as a richer two-line layout without
 * having to thread a new `description` field through every tool.
 *
 * Trailing single-token parens (e.g. "(M)") are shortcut hints that the
 * dedicated shortcut chip already shows — stripped from the title.
 * Multi-word parens (e.g. "select a part") and explicit " — " separators
 * become the description line.
 */
function splitTooltip(s: string): { title: string; description?: string } {
  const dashIdx = s.indexOf(" — ");
  if (dashIdx > 0) {
    return { title: s.slice(0, dashIdx), description: s.slice(dashIdx + 3) };
  }
  const parenMatch = s.match(/^(.*?)\s*\(([^)]+)\)\s*$/);
  if (parenMatch) {
    const head = parenMatch[1]!;
    const inner = parenMatch[2]!;
    if (inner.includes(" ")) return { title: head, description: inner };
    return { title: head };
  }
  return { title: s };
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
  const accent = TAB_THEMES[def.tab]?.accent;
  const tooltipText = readOnlyShare
    ? "Sign in to fork — this doc is read-only"
    : def.tooltip;
  const { title, description } = splitTooltip(tooltipText);
  return (
    <ToolbarButton
      tooltip={title}
      tooltipDescription={description}
      tooltipAccent={accent}
      tooltipIcon={<Icon size={20} className={def.iconColor} />}
      tooltipSide="bottom"
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
