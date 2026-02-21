import { useEffect, useState, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { cn } from "@/lib/utils";
import type { Toast as ToastType } from "@/stores/notification-store";

const ICONS = {
  success: CheckCircle,
  error: XCircle,
  info: Info,
  warning: Warning,
};

const COLORS = {
  success: {
    border: "border-green-400/30",
    icon: "text-green-400",
    bg: "bg-green-400/5",
  },
  error: {
    border: "border-red-400/30",
    icon: "text-red-400",
    bg: "bg-red-400/5",
  },
  info: {
    border: "border-blue-400/30",
    icon: "text-blue-400",
    bg: "bg-blue-400/5",
  },
  warning: {
    border: "border-yellow-400/30",
    icon: "text-yellow-400",
    bg: "bg-yellow-400/5",
  },
};

interface ToastProps {
  toast: ToastType;
  onDismiss?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

export function Toast({ toast, onDismiss, onMouseEnter, onMouseLeave }: ToastProps) {
  const [visible, setVisible] = useState(false);
  const [exiting, setExiting] = useState(false);

  const Icon = ICONS[toast.type];
  const colors = COLORS[toast.type];

  // Check for reduced motion preference
  const prefersReducedMotion =
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  useEffect(() => {
    if (prefersReducedMotion) {
      setVisible(true);
      return;
    }

    // Trigger enter animation
    requestAnimationFrame(() => setVisible(true));
  }, [prefersReducedMotion]);

  const handleDismiss = useCallback(() => {
    if (prefersReducedMotion) {
      onDismiss?.();
      return;
    }

    setExiting(true);
    // Wait for exit animation
    setTimeout(() => onDismiss?.(), 200);
  }, [onDismiss, prefersReducedMotion]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape" && toast.dismissible) {
        handleDismiss();
      }
    },
    [handleDismiss, toast.dismissible]
  );

  return (
    <div
      role="alert"
      aria-live={toast.type === "error" ? "assertive" : "polite"}
      tabIndex={toast.dismissible ? 0 : undefined}
      onKeyDown={handleKeyDown}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "flex items-center gap-2 border bg-card px-3 py-2 shadow-2xl",
        colors.border,
        colors.bg,
        // Animation classes
        prefersReducedMotion
          ? "opacity-100"
          : cn(
              "transition-all duration-200",
              visible && !exiting
                ? "translate-x-0 opacity-100"
                : "translate-x-4 opacity-0"
            ),
        // Focus ring for keyboard navigation
        "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
      )}
    >
      <Icon
        size={16}
        weight="fill"
        className={cn("shrink-0", colors.icon)}
        aria-hidden="true"
      />
      <span className="flex-1 text-xs text-text">{toast.message}</span>
      {toast.dismissible && (
        <button
          onClick={handleDismiss}
          className={cn(
            "ml-2 shrink-0 text-text-muted hover:text-text",
            "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50 rounded"
          )}
          aria-label="Dismiss notification"
        >
          <X size={12} aria-hidden="true" />
        </button>
      )}
    </div>
  );
}
