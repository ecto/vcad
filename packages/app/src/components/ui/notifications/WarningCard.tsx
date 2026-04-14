import { useCallback, useEffect, useState } from "react";
import { Warning as WarningIcon } from "@phosphor-icons/react/dist/ssr/Warning";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { ArrowSquareOut } from "@phosphor-icons/react/dist/ssr/ArrowSquareOut";
import { cn } from "@/lib/utils";
import type { Warning, ActionButton } from "@/stores/notification-store";

interface WarningCardProps {
  warning: Warning;
  onDismiss?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

const CATEGORY_LABELS: Record<Warning["category"], string> = {
  dfm: "Manufacturability",
  validation: "Validation",
  performance: "Performance",
  suggestion: "Suggestion",
};

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

  return (
    <button
      onClick={handleClick}
      disabled={disabled}
      className={cn(
        "px-2 py-1 text-[10px] rounded transition-colors",
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50",
        action.variant === "primary" &&
          "bg-yellow-500/20 text-yellow-200 hover:bg-yellow-500/30",
        action.variant === "destructive" &&
          "bg-red-500/10 text-red-400 hover:bg-red-500/20",
        (!action.variant || action.variant === "secondary") &&
          "bg-border/50 text-text-muted hover:text-text hover:bg-border",
        disabled && "opacity-50 cursor-not-allowed"
      )}
    >
      {action.label}
    </button>
  );
}

export function WarningCard({
  warning,
  onDismiss,
  onMouseEnter,
  onMouseLeave,
}: WarningCardProps) {
  const [visible, setVisible] = useState(false);
  const [exiting, setExiting] = useState(false);

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
      if (e.key === "Escape" && warning.dismissible) {
        handleDismiss();
      }
    },
    [handleDismiss, warning.dismissible]
  );

  return (
    <div
      role="alert"
      aria-live="polite"
      tabIndex={warning.dismissible ? 0 : undefined}
      onKeyDown={handleKeyDown}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "border border-yellow-400/30 bg-card shadow-2xl",
        prefersReducedMotion
          ? "opacity-100"
          : cn(
              "transition-all duration-200",
              visible && !exiting
                ? "translate-x-0 opacity-100"
                : "translate-x-4 opacity-0"
            ),
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50"
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border/50">
        <div className="flex items-center gap-2">
          <WarningIcon
            size={14}
            weight="fill"
            className="text-yellow-400"
            aria-hidden="true"
          />
          <span className="text-xs font-medium text-text">{warning.title}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[9px] text-yellow-400/70 uppercase tracking-wider">
            {CATEGORY_LABELS[warning.category]}
          </span>
          {warning.dismissible && (
            <button
              onClick={handleDismiss}
              className={cn(
                "text-text-muted hover:text-text",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50 rounded"
              )}
              aria-label="Dismiss warning"
            >
              <X size={12} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>

      {/* Description */}
      <div className="px-3 py-2">
        <p className="text-[11px] text-text-muted">{warning.description}</p>
      </div>

      {/* Actions + Learn More */}
      {(warning.actions.length > 0 || warning.learnMoreUrl) && (
        <div className="flex items-center justify-between px-3 py-2 border-t border-border/50 bg-bg/30">
          <div className="flex gap-2">
            {warning.actions.map((action, idx) => (
              <ActionButtonComponent key={idx} action={action} disabled={false} />
            ))}
          </div>
          {warning.learnMoreUrl && (
            <a
              href={warning.learnMoreUrl}
              target="_blank"
              rel="noopener noreferrer"
              className={cn(
                "flex items-center gap-1 text-[10px] text-text-muted hover:text-brand",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50 rounded"
              )}
            >
              Learn more
              <ArrowSquareOut size={10} aria-hidden="true" />
            </a>
          )}
        </div>
      )}
    </div>
  );
}
