import { useCallback } from "react";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { cn } from "@/lib/utils";
import type { AIProgress, AIStage } from "@/stores/notification-store";
import { useNotificationStore } from "@/stores/notification-store";

interface AIProgressCardProps {
  progress: AIProgress;
  onDismiss?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

function StageIcon({ status }: { status: AIStage["status"] }) {
  switch (status) {
    case "complete":
      return (
        <CheckCircle
          size={12}
          weight="fill"
          className="text-green-400"
          aria-hidden="true"
        />
      );
    case "running":
      return (
        <SpinnerGap
          size={12}
          className="text-accent animate-spin"
          aria-hidden="true"
        />
      );
    case "error":
      return (
        <XCircle
          size={12}
          weight="fill"
          className="text-red-400"
          aria-hidden="true"
        />
      );
    case "pending":
    default:
      return (
        <Circle
          size={12}
          className="text-text-muted/50"
          aria-hidden="true"
        />
      );
  }
}

function StageLabel({ status }: { status: AIStage["status"] }) {
  switch (status) {
    case "complete":
      return "Complete";
    case "running":
      return "In progress";
    case "error":
      return "Failed";
    case "pending":
    default:
      return "Pending";
  }
}

export function AIProgressCard({
  progress,
  onMouseEnter,
  onMouseLeave,
}: AIProgressCardProps) {
  const cancelAIOperation = useNotificationStore((s) => s.cancelAIOperation);

  const handleCancel = useCallback(() => {
    cancelAIOperation(progress.id);
  }, [cancelAIOperation, progress.id]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape" && progress.cancelable) {
        handleCancel();
      }
    },
    [handleCancel, progress.cancelable]
  );

  // Find current stage
  const currentStage = progress.stages.find((s) => s.status === "running");
  const currentLabel = currentStage?.label ?? "Processing...";

  // Format download progress
  const formatBytes = (bytes: number) => {
    if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)}KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  };

  const downloadText = progress.downloadProgress
    ? `${formatBytes(progress.downloadProgress.loaded)} / ${formatBytes(progress.downloadProgress.total)}`
    : null;

  return (
    <div
      role="status"
      aria-live="polite"
      aria-busy="true"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "border border-accent/30 bg-card shadow-2xl",
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border/50">
        <div className="flex items-center gap-2">
          <SpinnerGap
            size={14}
            className="text-accent animate-spin"
            aria-hidden="true"
          />
          <span className="text-xs font-medium text-text">{currentLabel}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[10px] text-text-muted">
            {downloadText ?? `${progress.progress}%`}
          </span>
          {progress.cancelable && (
            <button
              onClick={handleCancel}
              className={cn(
                "text-text-muted hover:text-text",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50 rounded"
              )}
              aria-label="Cancel operation"
            >
              <X size={12} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>

      {/* Progress bar */}
      <div className="h-1 bg-border/30">
        <div
          className="h-full bg-accent transition-all duration-300 ease-out"
          style={{ width: `${progress.progress}%` }}
          role="progressbar"
          aria-valuenow={progress.progress}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>

      {/* Stages */}
      <div className="px-3 py-2 space-y-1">
        {progress.stages.map((stage, idx) => (
          <div
            key={idx}
            className="flex items-center gap-2"
          >
            <StageIcon status={stage.status} />
            <span
              className={cn(
                "text-[11px]",
                stage.status === "complete" && "text-text-muted line-through",
                stage.status === "running" && "text-text",
                stage.status === "pending" && "text-text-muted/50",
                stage.status === "error" && "text-red-400"
              )}
            >
              {stage.label}
            </span>
            {/* Screen reader only status */}
            <span className="sr-only">
              <StageLabel status={stage.status} />
            </span>
          </div>
        ))}
      </div>

      {/* Prompt preview */}
      <div className="px-3 py-2 border-t border-border/50 bg-bg/50">
        <p className="text-[10px] text-text-muted truncate" title={progress.prompt}>
          {progress.prompt}
        </p>
      </div>
    </div>
  );
}
