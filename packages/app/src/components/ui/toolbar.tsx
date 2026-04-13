import { useState, useEffect, useCallback, useRef } from "react";
import { DotsThree } from "@phosphor-icons/react/dist/ssr/DotsThree";
import * as Popover from "@radix-ui/react-popover";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { MOBILE_BREAKPOINT } from "./toolbar-constants";

export interface ToolbarButtonProps {
  children: React.ReactNode;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  tooltip: string;
  pulse?: boolean;
  expanded?: boolean;
  label?: string;
  shortcut?: string;
  iconColor?: string;
  className?: string;
  labelClassName?: string;
}

export function ToolbarButton({
  children,
  active,
  disabled,
  onClick,
  tooltip,
  pulse,
  expanded,
  label,
  shortcut,
  iconColor,
  className,
  labelClassName,
}: ToolbarButtonProps) {
  return (
    <Tooltip content={tooltip} side="top">
      <button
        className={cn(
          "flex items-center justify-center relative gap-1",
          "h-10 min-w-[40px] px-1.5",
          "md:h-6 md:min-w-0",
          expanded ? "md:px-1.5" : "md:px-1",
          "disabled:opacity-30 disabled:cursor-not-allowed",
          pulse && "animate-pulse",
          className,
        )}
        disabled={disabled}
        onClick={onClick}
      >
        <span className={cn(
          iconColor,
          "transition-transform",
          active && "scale-110",
          !disabled && "hover:scale-110"
        )}>
          {children}
        </span>
        {expanded && label && (
          <span className={cn(
            "hidden md:inline text-[10px] whitespace-nowrap",
            active ? "text-text" : "text-text-muted",
            labelClassName
          )}>
            {label}
            {shortcut && <span className="ml-1 opacity-60">{shortcut}</span>}
          </span>
        )}
      </button>
    </Tooltip>
  );
}

export interface TabDropdownProps<T extends string> {
  id: T;
  label: string;
  icon: React.ComponentType<{ size?: number; weight?: "regular" | "fill"; className?: string }>;
  index?: number;
  children: React.ReactNode;
  onSelect?: () => void;
  colors?: Record<T, string>;
  shortcutKey?: string;
}

export function TabDropdown<T extends string>({
  id,
  label,
  icon: Icon,
  index,
  children,
  onSelect,
  colors,
  shortcutKey,
}: TabDropdownProps<T>) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [isMobile, setIsMobile] = useState(false);
  const hoverTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // Get color from colors map or default to text-text-muted
  const color = colors?.[id] ?? "text-text-muted";

  const handleMouseEnter = useCallback(() => {
    clearTimeout(hoverTimeoutRef.current);
    setOpen(true);
  }, []);

  const handleMouseLeave = useCallback(() => {
    if (!pinned) {
      hoverTimeoutRef.current = setTimeout(() => setOpen(false), 100);
    }
  }, [pinned]);

  const handleClick = useCallback(() => {
    setPinned((p) => !p);
    setOpen(true);
    onSelect?.();
  }, [onSelect]);

  const handleOpenChange = useCallback((newOpen: boolean) => {
    setOpen(newOpen);
    if (!newOpen) setPinned(false);
  }, []);

  // Track mobile state for tooltip visibility
  useEffect(() => {
    const mq = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`);
    setIsMobile(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches);
    mq.addEventListener("change", handler);
    return () => {
      mq.removeEventListener("change", handler);
      clearTimeout(hoverTimeoutRef.current);
    };
  }, []);

  const tooltipContent = index !== undefined
    ? `${index + 1}. ${label}`
    : shortcutKey
      ? `${label} (${shortcutKey})`
      : label;

  const triggerButton = (
    <button
      className={cn(
        "relative flex items-center justify-center gap-1 text-xs",
        "w-9 h-9 sm:w-auto sm:h-auto sm:px-3 sm:py-2",
        "hover:bg-hover/50 transition-all",
        pinned && "bg-hover/50",
      )}
      onClick={handleClick}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      <Icon
        size={18}
        weight={pinned ? "fill" : "regular"}
        className={cn(color, "transition-transform")}
      />
      <span className={cn(
        "hidden sm:inline font-medium transition-colors",
        pinned ? "text-text" : "text-text-muted"
      )}>
        {label}
      </span>
      {shortcutKey && (
        <span className="hidden sm:inline text-[10px] text-text-muted/50 absolute bottom-0.5 right-1 font-mono">
          {shortcutKey}
        </span>
      )}
    </button>
  );

  return (
    <Popover.Root open={open} onOpenChange={handleOpenChange}>
      {isMobile ? (
        <Tooltip content={tooltipContent} side="top">
          <Popover.Trigger asChild>{triggerButton}</Popover.Trigger>
        </Tooltip>
      ) : (
        <Popover.Trigger asChild>{triggerButton}</Popover.Trigger>
      )}
      <Popover.Portal>
        <Popover.Content
          className="bottom-toolbar-menu z-50 bg-surface p-2"
          sideOffset={4}
          side="top"
          align="center"
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          <div className="flex items-center gap-1">
            {children}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

export interface MoreDropdownProps<T extends string> {
  tabs: { id: T; label: string; icon: React.ComponentType<{ size?: number; weight?: "regular" | "fill"; className?: string }> }[];
  activeTab: T;
  onSelect: (tab: T) => void;
  children: (tab: T) => React.ReactNode;
  colors?: Record<T, string>;
}

export function MoreDropdown<T extends string>({
  tabs,
  activeTab,
  onSelect,
  children,
  colors,
}: MoreDropdownProps<T>) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [selectedSubTab, setSelectedSubTab] = useState<T | null>(null);
  const [isMobile, setIsMobile] = useState(false);
  const hoverTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const handleMouseEnter = useCallback(() => {
    clearTimeout(hoverTimeoutRef.current);
    setOpen(true);
  }, []);

  const handleMouseLeave = useCallback(() => {
    if (!pinned) {
      hoverTimeoutRef.current = setTimeout(() => setOpen(false), 100);
    }
  }, [pinned]);

  const handleClick = useCallback(() => {
    setPinned((p) => !p);
    setOpen(true);
  }, []);

  const handleOpenChange = useCallback((newOpen: boolean) => {
    setOpen(newOpen);
    if (!newOpen) setPinned(false);
  }, []);

  // Track mobile state for tooltip visibility
  useEffect(() => {
    const mq = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`);
    setIsMobile(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches);
    mq.addEventListener("change", handler);
    return () => {
      mq.removeEventListener("change", handler);
      clearTimeout(hoverTimeoutRef.current);
    };
  }, []);

  // Check if the active tab is in the "More" section
  const isMoreTabActive = tabs.some(t => t.id === activeTab);
  const activeMoreTab = tabs.find(t => t.id === activeTab);

  const triggerButton = (
    <button
      className={cn(
        "relative flex items-center justify-center gap-1 text-xs",
        "w-9 h-9 sm:w-auto sm:h-auto sm:px-3 sm:py-2",
        "hover:bg-hover/50 transition-all",
        pinned && "bg-hover/50",
      )}
      onClick={handleClick}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      {pinned && activeMoreTab ? (
        <activeMoreTab.icon
          size={18}
          weight="fill"
          className={colors?.[activeMoreTab.id]}
        />
      ) : (
        <DotsThree size={18} weight={pinned ? "fill" : "bold"} className="text-text-muted" />
      )}
      <span className={cn(
        "hidden sm:inline font-medium transition-colors",
        pinned ? "text-text" : "text-text-muted"
      )}>
        {pinned && activeMoreTab ? activeMoreTab.label : "More"}
      </span>
    </button>
  );

  return (
    <Popover.Root open={open} onOpenChange={handleOpenChange}>
      {isMobile ? (
        <Tooltip content="More" side="top">
          <Popover.Trigger asChild>{triggerButton}</Popover.Trigger>
        </Tooltip>
      ) : (
        <Popover.Trigger asChild>{triggerButton}</Popover.Trigger>
      )}
      <Popover.Portal>
        <Popover.Content
          className="bottom-toolbar-menu z-50 bg-surface p-2"
          sideOffset={4}
          side="top"
          align="center"
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {/* Tab selector row */}
          <div className="flex items-center gap-1 border-b border-border pb-2 mb-2">
            {tabs.map((tab) => {
              const isActive = selectedSubTab === tab.id || (!selectedSubTab && activeTab === tab.id);
              const tabButton = (
                <button
                  className={cn(
                    "flex items-center gap-1.5 px-2 py-1.5 text-xs",
                    "hover:bg-hover transition-colors",
                    isActive && "bg-hover/50",
                  )}
                  onClick={() => {
                    setSelectedSubTab(tab.id);
                    onSelect(tab.id);
                  }}
                >
                  <tab.icon
                    size={14}
                    weight={isActive ? "fill" : "regular"}
                    className={colors?.[tab.id]}
                  />
                  <span className={cn(
                    "hidden sm:inline",
                    isActive ? "text-text" : "text-text-muted"
                  )}>
                    {tab.label}
                  </span>
                </button>
              );
              return isMobile ? (
                <Tooltip key={tab.id} content={tab.label} side="top">
                  {tabButton}
                </Tooltip>
              ) : (
                <span key={tab.id}>{tabButton}</span>
              );
            })}
          </div>
          {/* Tools for selected tab */}
          <div className="flex items-center gap-1">
            {children(selectedSubTab || (isMoreTabActive ? activeTab : tabs[0]!.id))}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
