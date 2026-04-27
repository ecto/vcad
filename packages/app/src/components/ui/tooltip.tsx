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

export function RichTooltip({
  children,
  title,
  description,
  shortcut,
  accent,
  side = "top",
  align = "center",
}: {
  children: ReactNode;
  title: string;
  description?: string;
  shortcut?: string;
  /** A `bg-…` Tailwind class used for the left accent stripe. */
  accent?: string;
  side?: "top" | "bottom" | "left" | "right";
  align?: "start" | "center" | "end";
}) {
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          align={align}
          sideOffset={8}
          className={cn(
            "z-50 max-w-[260px] overflow-hidden",
            "bg-card text-text border border-border shadow-xl",
            "data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-95",
          )}
        >
          <div className="flex items-stretch">
            {accent && <div className={cn("w-[3px] shrink-0", accent)} />}
            <div className="flex-1 px-2.5 py-1.5">
              <div className="flex items-baseline justify-between gap-3">
                <span className="text-[11px] font-medium tracking-tight">{title}</span>
                {shortcut && (
                  <span className="text-[10px] font-mono uppercase text-text-muted border border-border/70 px-1 leading-[14px]">
                    {shortcut}
                  </span>
                )}
              </div>
              {description && (
                <p className="mt-1 text-[10.5px] text-text-muted leading-snug">
                  {description}
                </p>
              )}
            </div>
          </div>
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}
