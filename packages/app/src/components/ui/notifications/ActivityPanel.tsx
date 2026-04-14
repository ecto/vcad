import { useCallback, useState, useMemo } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CaretDown } from "@phosphor-icons/react/dist/ssr/CaretDown";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { Gear } from "@phosphor-icons/react/dist/ssr/Gear";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Funnel } from "@phosphor-icons/react/dist/ssr/Funnel";
import { cn } from "@/lib/utils";
import {
  useNotificationStore,
  type ActivityEntry,
  type ActivityType,
} from "@/stores/notification-store";

const TYPE_ICONS: Record<ActivityType, typeof Sparkle> = {
  "ai-generation": Sparkle,
  operation: Gear,
  export: Export,
  error: XCircle,
  warning: Warning,
};

const TYPE_COLORS: Record<ActivityType, string> = {
  "ai-generation": "text-brand",
  operation: "text-text-muted",
  export: "text-blue-400",
  error: "text-red-400",
  warning: "text-yellow-400",
};

interface ActivityEntryItemProps {
  entry: ActivityEntry;
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);

  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;

  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;

  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function ActivityEntryItem({ entry }: ActivityEntryItemProps) {
  const [expanded, setExpanded] = useState(false);

  const Icon = TYPE_ICONS[entry.type];
  const iconColor = TYPE_COLORS[entry.type];

  const hasDetails = Object.keys(entry.details).length > 0;
  const hasActions = entry.undoable || entry.onRegenerate;

  return (
    <div
      className={cn(
        "border-b border-border/30 last:border-b-0",
        "hover:bg-border/10 transition-colors"
      )}
    >
      {/* Entry header */}
      <button
        onClick={() => hasDetails && setExpanded(!expanded)}
        disabled={!hasDetails}
        className={cn(
          "w-full flex items-center gap-2 px-3 py-2 text-left",
          "focus:outline-none focus-visible:bg-border/20",
          !hasDetails && "cursor-default"
        )}
      >
        {hasDetails ? (
          expanded ? (
            <CaretDown size={10} className="text-text-muted shrink-0" />
          ) : (
            <CaretRight size={10} className="text-text-muted shrink-0" />
          )
        ) : (
          <div className="w-[10px]" />
        )}

        <Icon size={12} className={cn("shrink-0", iconColor)} aria-hidden="true" />

        <span className="flex-1 text-[11px] text-text truncate">
          {entry.title}
        </span>

        <span className="text-[9px] text-text-muted shrink-0">
          {formatTime(entry.timestamp)}
        </span>
      </button>

      {/* Expanded details */}
      {expanded && hasDetails && (
        <div className="px-3 pb-2 pl-8">
          <div className="text-[10px] text-text-muted bg-bg/50 p-2 rounded font-mono">
            {Object.entries(entry.details).map(([key, value]) => (
              <div key={key} className="truncate">
                <span className="text-text-muted/70">{key}:</span>{" "}
                <span className="text-text">
                  {typeof value === "string"
                    ? value.length > 100
                      ? value.slice(0, 100) + "..."
                      : value
                    : JSON.stringify(value)}
                </span>
              </div>
            ))}
          </div>

          {/* Actions */}
          {hasActions && (
            <div className="flex gap-2 mt-2">
              {entry.undoable && entry.onUndo && (
                <button
                  onClick={entry.onUndo}
                  className={cn(
                    "flex items-center gap-1 px-2 py-1 text-[10px] rounded",
                    "bg-border/50 text-text-muted hover:text-text hover:bg-border",
                    "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50"
                  )}
                >
                  <ArrowCounterClockwise size={10} />
                  Undo
                </button>
              )}
              {entry.onRegenerate && (
                <button
                  onClick={entry.onRegenerate}
                  className={cn(
                    "flex items-center gap-1 px-2 py-1 text-[10px] rounded",
                    "bg-border/50 text-text-muted hover:text-text hover:bg-border",
                    "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50"
                  )}
                >
                  <ArrowsClockwise size={10} />
                  Regenerate
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function ActivityPanel() {
  const activityLog = useNotificationStore((s) => s.activityLog);
  const activityPanelOpen = useNotificationStore((s) => s.activityPanelOpen);
  const setActivityPanelOpen = useNotificationStore((s) => s.setActivityPanelOpen);
  const clearActivityLog = useNotificationStore((s) => s.clearActivityLog);

  const [filter, setFilter] = useState<ActivityType | "all">("all");

  const filteredLog = useMemo(() => {
    if (filter === "all") return activityLog;
    return activityLog.filter((entry) => entry.type === filter);
  }, [activityLog, filter]);

  const handleClose = useCallback(() => {
    setActivityPanelOpen(false);
  }, [setActivityPanelOpen]);

  const handleClear = useCallback(() => {
    clearActivityLog();
  }, [clearActivityLog]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape") {
        handleClose();
      }
    },
    [handleClose]
  );

  if (!activityPanelOpen) return null;

  return (
    <div
      role="complementary"
      aria-label="Activity log"
      onKeyDown={handleKeyDown}
      className={cn(
        "fixed left-4 bottom-4 z-40",
        "w-80 max-h-[60vh] flex flex-col",
        "border border-border bg-card shadow-2xl",
        "animate-in slide-in-from-left-4 duration-200"
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <span className="text-xs font-medium text-text">Activity</span>
        <div className="flex items-center gap-2">
          {/* Filter dropdown */}
          <div className="relative">
            <select
              value={filter}
              onChange={(e) => setFilter(e.target.value as ActivityType | "all")}
              className={cn(
                "appearance-none bg-transparent text-[10px] text-text-muted",
                "pr-4 cursor-pointer",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50"
              )}
            >
              <option value="all">All</option>
              <option value="ai-generation">AI</option>
              <option value="operation">Operations</option>
              <option value="export">Exports</option>
              <option value="error">Errors</option>
              <option value="warning">Warnings</option>
            </select>
            <Funnel
              size={10}
              className="absolute right-0 top-1/2 -translate-y-1/2 text-text-muted pointer-events-none"
            />
          </div>

          {/* Clear button */}
          {activityLog.length > 0 && (
            <button
              onClick={handleClear}
              className={cn(
                "text-text-muted hover:text-text",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50 rounded"
              )}
              aria-label="Clear activity log"
              title="Clear activity log"
            >
              <Trash size={12} />
            </button>
          )}

          {/* Close button */}
          <button
            onClick={handleClose}
            className={cn(
              "text-text-muted hover:text-text",
              "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50 rounded"
            )}
            aria-label="Close activity panel"
          >
            <X size={12} />
          </button>
        </div>
      </div>

      {/* Activity list */}
      <div className="flex-1 overflow-y-auto">
        {filteredLog.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-8 text-text-muted">
            <Sparkle size={24} className="opacity-30 mb-2" />
            <span className="text-xs">No activity yet</span>
          </div>
        ) : (
          filteredLog.map((entry) => (
            <ActivityEntryItem key={entry.id} entry={entry} />
          ))
        )}
      </div>

      {/* Footer with count */}
      {filteredLog.length > 0 && (
        <div className="px-3 py-1.5 border-t border-border text-[10px] text-text-muted">
          {filteredLog.length} item{filteredLog.length !== 1 ? "s" : ""}
          {filter !== "all" && ` (${filter})`}
        </div>
      )}
    </div>
  );
}
