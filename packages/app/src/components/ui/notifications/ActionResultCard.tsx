import { useCallback, useEffect, useState } from "react";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { cn } from "@/lib/utils";
import type { ActionResult, ActionButton } from "@/stores/notification-store";

interface ActionResultCardProps {
  result: ActionResult;
  onDismiss?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

function ActionButtonComponent({
  action,
  disabled,
}: {
  action: ActionButton;
  disabled: boolean;
}) {
  const handleClick = useCallback(() => {
    action.onClick();
  }, [action]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        action.onClick();
      }
    },
    [action]
  );

  return (
    <button
      onClick={handleClick}
      onKeyDown={handleKeyDown}
      disabled={disabled}
      className={cn(
        "px-2 py-1 text-[10px] rounded transition-colors",
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50",
        action.variant === "primary" &&
          "bg-accent text-white hover:bg-accent/90",
        action.variant === "destructive" &&
          "bg-red-500/10 text-red-400 hover:bg-red-500/20 border border-red-400/30",
        (!action.variant || action.variant === "secondary") &&
          "bg-border/50 text-text-muted hover:text-text hover:bg-border",
        disabled && "opacity-50 cursor-not-allowed"
      )}
    >
      {action.label}
    </button>
  );
}

export function ActionResultCard({
  result,
  onDismiss,
  onMouseEnter,
  onMouseLeave,
}: ActionResultCardProps) {
  const [visible, setVisible] = useState(false);
  const [exiting, setExiting] = useState(false);
  const [actionsDisabled] = useState(false);

  const prefersReducedMotion =
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  useEffect(() => {
    if (prefersReducedMotion) {
      setVisible(true);
      return;
    }
    requestAnimationFrame(() => setVisible(true));
  }, [prefersReducedMotion]);

  const handleDismiss = useCallback(() => {
    if (prefersReducedMotion) {
      onDismiss?.();
      return;
    }
    setExiting(true);
    setTimeout(() => onDismiss?.(), 200);
  }, [onDismiss, prefersReducedMotion]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape" && result.dismissible) {
        handleDismiss();
      }
    },
    [handleDismiss, result.dismissible]
  );

  const Icon = result.type === "success" ? CheckCircle : XCircle;
  const iconColor = result.type === "success" ? "text-green-400" : "text-red-400";
  const borderColor =
    result.type === "success" ? "border-green-400/30" : "border-red-400/30";

  return (
    <div
      role="status"
      aria-live="polite"
      tabIndex={result.dismissible ? 0 : undefined}
      onKeyDown={handleKeyDown}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "border bg-card shadow-2xl",
        borderColor,
        prefersReducedMotion
          ? "opacity-100"
          : cn(
              "transition-all duration-200",
              visible && !exiting
                ? "translate-x-0 opacity-100"
                : "translate-x-4 opacity-0"
            ),
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2">
        <div className="flex items-center gap-2">
          <Icon
            size={14}
            weight="fill"
            className={iconColor}
            aria-hidden="true"
          />
          <span className="text-xs font-medium text-text">{result.title}</span>
        </div>
        {result.dismissible && (
          <button
            onClick={handleDismiss}
            className={cn(
              "text-text-muted hover:text-text",
              "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50 rounded"
            )}
            aria-label="Dismiss notification"
          >
            <X size={12} aria-hidden="true" />
          </button>
        )}
      </div>

      {/* Description */}
      {result.description && (
        <div className="px-3 pb-2">
          <p className="text-[11px] text-text-muted">{result.description}</p>
        </div>
      )}

      {/* Actions */}
      {result.actions.length > 0 && (
        <div className="flex gap-2 px-3 py-2 border-t border-border/50 bg-bg/30">
          {result.actions.map((action, idx) => (
            <ActionButtonComponent
              key={idx}
              action={action}
              disabled={actionsDisabled}
            />
          ))}
        </div>
      )}
    </div>
  );
}
