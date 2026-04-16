import { cn } from "@/lib/utils";
import type { ReactNode } from "react";

export function PanelHeader({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex h-10 shrink-0 items-center gap-2 border-b border-border/40 px-3 text-xs font-bold uppercase tracking-wider text-text-muted",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function PanelBody({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex-1 overflow-y-auto p-2", className)}>
      {children}
    </div>
  );
}
