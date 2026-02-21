import { useEffect, useRef } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { cn } from "@/lib/utils";
import type { LogEntry, LogLevelName } from "@vcad/core";
import { useLogStore, getFilteredEntries } from "@/stores/log-store";
import { LogFilterBar } from "@/components/LogFilterBar";
import { useNotificationStore } from "@/stores/notification-store";

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
        "flex items-start gap-2 px-3 py-1.5 border-b border-border/50 font-mono text-[11px]",
        LEVEL_BG[entry.level],
      )}
    >
      {/* Timestamp */}
      <span className="text-text-muted shrink-0">
        {formatTimestamp(entry.timestamp)}
      </span>

      {/* Level badge */}
      <span
        className={cn(
          "shrink-0 w-12 text-center font-bold uppercase text-[9px]",
          LEVEL_COLORS[entry.level],
        )}
      >
        {entry.level}
      </span>

      {/* Source badge */}
      <span className="shrink-0 w-14 text-center text-[9px] font-medium bg-hover px-1 py-0.5 text-text-muted">
        {entry.source}
      </span>

      {/* Message */}
      <span className="text-text break-all">{entry.message}</span>
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
  const enabledSources = useLogStore((s) => s.enabledSources);

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
    <>
      {/* Quake-style dropdown console */}
      <div
        ref={panelRef}
        className={cn(
          "fixed z-50 bg-surface/95 backdrop-blur-sm border-b border-border shadow-2xl flex flex-col",
          "animate-in slide-in-from-top duration-150",
          "top-0 left-0 right-0 h-[50vh]",
        )}
      >
        {/* Header */}
        <div className="flex items-center justify-between border-b border-border px-3 py-2 shrink-0">
          <h3 className="text-xs font-bold">Logs</h3>
          <div className="flex items-center gap-1">
            <button
              onClick={handleCopy}
              className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
              title="Copy logs"
            >
              <Copy size={14} />
            </button>
            <button
              onClick={handleClear}
              className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
              title="Clear logs"
            >
              <Trash size={14} />
            </button>
            <button
              onClick={closePanel}
              className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
              title="Close (~)"
            >
              <X size={14} />
            </button>
          </div>
        </div>

        {/* Filter bar */}
        <LogFilterBar />

        {/* Log entries */}
        <div
          ref={listRef}
          onScroll={handleScroll}
          className="flex-1 overflow-y-auto min-h-0"
        >
          {filteredEntries.length === 0 ? (
            <div className="flex items-center justify-center h-32 text-text-muted text-xs">
              No logs to display
            </div>
          ) : (
            filteredEntries.map((entry) => (
              <LogEntryRow key={entry.id} entry={entry} />
            ))
          )}
        </div>

        {/* Footer with count */}
        <div className="border-t border-border px-3 py-1.5 text-[10px] text-text-muted shrink-0">
          {filteredEntries.length} of {entries.length} entries
        </div>
      </div>
    </>
  );
}
