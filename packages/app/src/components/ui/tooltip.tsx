import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export function TooltipProvider({ children }: { children: ReactNode }) {
  return (
    <TooltipPrimitive.Provider delayDuration={0}>
      {children}
    </TooltipPrimitive.Provider>
  );
}

export function Tooltip({
  children,
  content,
  side = "bottom",
}: {
  children: ReactNode;
  content: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
}) {
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={6}
          className="z-50 bg-card px-2.5 py-1.5 text-xs text-text shadow-lg border border-border"
        >
          {content}
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}

/** Mono shortcut chip used in rich tooltips and elsewhere. */
export function KbdChip({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        "inline-flex items-center justify-center min-w-[16px]",
        "px-1 leading-[14px] text-[10px] font-mono tracking-wider uppercase",
        "text-text border border-border/70 bg-surface/80",
        className,
      )}
    >
      {children}
    </span>
  );
}

export function RichTooltip({
  children,
  title,
  description,
  shortcut,
  accent,
  icon,
  preview,
  tip,
  side = "top",
  align = "center",
  delayDuration = 0,
}: {
  children: ReactNode;
  title: string;
  description?: string;
  shortcut?: string;
  /** A `bg-…` Tailwind class used for the brand-color top hairline. */
  accent?: string;
  /** Optional hero icon rendered in a framed block on the left of the header. */
  icon?: ReactNode;
  /** Optional ReactNode rendered in the divider section beneath the description. */
  preview?: ReactNode;
  /** Optional one-line pro-tip rendered at the bottom in italic muted type. */
  tip?: string;
  side?: "top" | "bottom" | "left" | "right";
  align?: "start" | "center" | "end";
  /** Override the per-tooltip delay. Defaults to 0 — instant. */
  delayDuration?: number;
}) {
  return (
    <TooltipPrimitive.Root delayDuration={delayDuration}>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          align={align}
          sideOffset={8}
          collisionPadding={8}
          className={cn(
            "z-50 max-w-[300px] overflow-hidden",
            // Borland-style framed card: square corners, soft shadow, faint
            // top-down gradient for depth.
            "bg-gradient-to-b from-card to-surface text-text border border-border shadow-xl",
            "data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-95",
            "data-[state=closed]:animate-out data-[state=closed]:fade-out-0",
          )}
        >
          {/* Brand hairline at the top edge, color-matched to the tab. */}
          {accent && <div className={cn("h-[2px] w-full", accent)} />}
          <div className="px-3 py-2">
            <div className="flex items-start gap-2.5">
              {icon && (
                <div
                  className={cn(
                    "flex items-center justify-center shrink-0",
                    "w-7 h-7 border border-border/60 bg-surface/60",
                  )}
                >
                  {icon}
                </div>
              )}
              <div className="flex-1 min-w-0">
                <div className="flex items-baseline justify-between gap-3">
                  <span className="text-[12px] font-semibold tracking-tight truncate">
                    {title}
                  </span>
                  {shortcut && <KbdChip>{shortcut}</KbdChip>}
                </div>
                {description && (
                  <p className="mt-0.5 text-[10.5px] text-text-muted leading-snug">
                    {description}
                  </p>
                )}
              </div>
            </div>
            {preview && (
              <div className="mt-2 pt-2 border-t border-border/40">{preview}</div>
            )}
            {tip && (
              <p className="mt-2 text-[10px] text-text-muted/80 italic leading-snug">
                {tip}
              </p>
            )}
          </div>
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}
