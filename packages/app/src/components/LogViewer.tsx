import { useEffect, useRef } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Prohibit } from "@phosphor-icons/react/dist/ssr/Prohibit";
import { cn } from "@/lib/utils";
import type { LogEntry, LogLevelName, LogSourceName } from "@vcad/core";
import { useLogStore, getFilteredEntries } from "@/stores/log-store";
import { useNotificationStore } from "@/stores/notification-store";

const TABS = [{ id: "console", label: "Console" }] as const;
type TabId = (typeof TABS)[number]["id"];

const LEVELS: { value: LogLevelName; label: string }[] = [
  { value: "DEBUG", label: "Debug" },
  { value: "INFO", label: "Info" },
  { value: "WARN", label: "Warn" },
  { value: "ERROR", label: "Error" },
];

const SOURCES: { value: LogSourceName; label: string }[] = [
  { value: "kernel", label: "kernel" },
  { value: "engine", label: "engine" },
  { value: "app", label: "app" },
  { value: "gpu", label: "gpu" },
  { value: "step", label: "step" },
  { value: "mesh", label: "mesh" },
];

const LEVEL_COLORS: Record<LogLevelName, string> = {
  DEBUG: "text-text-muted",
  INFO: "text-blue-400",
  WARN: "text-yellow-400",
  ERROR: "text-red-400",
};

const LEVEL_BG: Record<LogLevelName, string> = {
  DEBUG: "bg-text-muted/10",
  INFO: "bg-blue-400/10",
  WARN: "bg-yellow-400/10",
  ERROR: "bg-red-400/10",
};

function formatTimestamp(ts: number): string {
  const date = new Date(ts);
  return date.toLocaleTimeString("en-US", {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function LogEntryRow({ entry }: { entry: LogEntry }) {
  return (
    <div
      className={cn(
        "flex items-baseline gap-2 px-3 py-0.5 border-b border-border/40 font-mono text-[11px]",
        LEVEL_BG[entry.level],
      )}
    >
      <span className="text-text-muted/60 shrink-0 text-[10px]">
        {formatTimestamp(entry.timestamp)}
      </span>
      <span className={cn("shrink-0 text-[10px] uppercase font-semibold w-10", LEVEL_COLORS[entry.level])}>
        {entry.level}
      </span>
      <span className="shrink-0 text-text-muted/70 text-[10px]">
        {entry.source}
      </span>
      <span className="text-text break-all min-w-0 flex-1">{entry.message}</span>
    </div>
  );
}

export function LogViewer() {
  const panelRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const shouldAutoScroll = useRef(true);

  const panelOpen = useLogStore((s) => s.panelOpen);
  const closePanel = useLogStore((s) => s.closePanel);
  const clearLogs = useLogStore((s) => s.clearLogs);
  const entries = useLogStore((s) => s.entries);
  const minLevel = useLogStore((s) => s.minLevel);
  const setMinLevel = useLogStore((s) => s.setMinLevel);
  const enabledSources = useLogStore((s) => s.enabledSources);
  const toggleSource = useLogStore((s) => s.toggleSource);
  const activeTab: TabId = "console";

  const filteredEntries = getFilteredEntries({
    entries,
    panelOpen,
    minLevel,
    enabledSources,
    togglePanel: () => {},
    openPanel: () => {},
    closePanel: () => {},
    setMinLevel: () => {},
    toggleSource: () => {},
    clearLogs: () => {},
  });

  // Auto-scroll to bottom when new entries arrive
  useEffect(() => {
    if (listRef.current && shouldAutoScroll.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [filteredEntries.length]);

  // Track scroll position to disable auto-scroll when user scrolls up
  const handleScroll = () => {
    if (!listRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = listRef.current;
    shouldAutoScroll.current = scrollHeight - scrollTop - clientHeight < 50;
  };

  // Close on escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && panelOpen) {
        closePanel();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [panelOpen, closePanel]);

  const handleCopy = () => {
    const text = filteredEntries
      .map(
        (e) =>
          `[${formatTimestamp(e.timestamp)}] [${e.level}] [${e.source}] ${e.message}`,
      )
      .join("\n");
    navigator.clipboard.writeText(text);
    useNotificationStore.getState().addToast("Logs copied to clipboard", "success");
  };

  const handleClear = () => {
    clearLogs();
    useNotificationStore.getState().addToast("Logs cleared", "info");
  };

  if (!panelOpen) return null;

  return (
    <div
      ref={panelRef}
      className={cn(
        "w-full bg-surface flex flex-col h-[40vh] min-h-0 border-t border-border/40",
        "animate-in slide-in-from-bottom duration-150",
      )}
    >
      {/* Tab strip — DevTools-style. One tab today; structure ready for more. */}
      <div className="flex h-7 shrink-0 items-stretch border-b border-border/40">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            className={cn(
              "px-3 text-[11px] border-r border-border/40 transition-colors",
              activeTab === tab.id
                ? "bg-bg text-text border-b-0"
                : "text-text-muted hover:text-text hover:bg-hover",
            )}
          >
            {tab.label}
          </button>
        ))}
        <div className="flex-1" />
        <button
          onClick={closePanel}
          className="flex h-full w-7 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          title="Close DevTools (~)"
        >
          <X size={13} />
        </button>
      </div>

      {/* Toolbar — clear / copy / level filter / source filter / count */}
      <div className="flex h-7 shrink-0 items-center gap-1 px-2 border-b border-border/40 text-[10px]">
        <button
          onClick={handleClear}
          className="flex h-5 w-5 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          title="Clear console"
        >
          <Prohibit size={12} />
        </button>
        <button
          onClick={handleCopy}
          className="flex h-5 w-5 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          title="Copy filtered entries"
        >
          <Copy size={12} />
        </button>

        <div className="mx-1 h-4 w-px bg-border" />

        {/* Level pills */}
        <div className="flex items-center gap-0.5">
          {LEVELS.map((opt) => (
            <button
              key={opt.value}
              onClick={() => setMinLevel(opt.value)}
              className={cn(
                "px-1.5 h-5 leading-none transition-colors",
                minLevel === opt.value
                  ? "bg-brand/15 text-brand"
                  : "text-text-muted hover:text-text hover:bg-hover",
              )}
              title={`Show ${opt.label} and above`}
            >
              {opt.label}
            </button>
          ))}
        </div>

        <div className="mx-1 h-4 w-px bg-border" />

        {/* Source toggles */}
        <div className="flex items-center gap-0.5 flex-wrap">
          {SOURCES.map((opt) => {
            const enabled = enabledSources.has(opt.value);
            return (
              <button
                key={opt.value}
                onClick={() => toggleSource(opt.value)}
                className={cn(
                  "px-1.5 h-5 leading-none font-mono transition-colors",
                  enabled
                    ? "text-text"
                    : "text-text-muted/50 line-through hover:text-text-muted",
                )}
                title={enabled ? `Hide ${opt.label}` : `Show ${opt.label}`}
              >
                {opt.label}
              </button>
            );
          })}
        </div>

        <div className="flex-1" />

        <span className="text-text-muted">
          {filteredEntries.length}
          {filteredEntries.length !== entries.length && ` / ${entries.length}`}
        </span>
      </div>

      {/* Log entries */}
      <div
        ref={listRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto min-h-0 bg-bg"
      >
        {filteredEntries.length === 0 ? (
          <div className="flex items-center justify-center h-32 text-text-muted text-xs">
            No messages
          </div>
        ) : (
          filteredEntries.map((entry) => (
            <LogEntryRow key={entry.id} entry={entry} />
          ))
        )}
      </div>
    </div>
  );
}
