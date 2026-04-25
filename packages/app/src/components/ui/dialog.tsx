import * as RadixDialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { cn } from "@/lib/utils";
import type { ReactNode } from "react";

export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}

export function Dialog({ open, onOpenChange, children }: DialogProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {children}
    </RadixDialog.Root>
  );
}

export function DialogContent({
  children,
  className,
  title,
}: {
  children: ReactNode;
  className?: string;
  title: string;
}) {
  return (
    <RadixDialog.Portal>
      <RadixDialog.Overlay className="fixed inset-0 z-50 bg-black/50" />
      <RadixDialog.Content
        className={cn(
          "fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2",
          "w-full max-w-md select-none",
          "border border-border bg-surface shadow-xl",
          "focus:outline-none",
          className,
        )}
      >
        <div
          data-tauri-drag-region=""
          className="flex h-10 items-center justify-between border-b border-border/40 px-4"
        >
          <RadixDialog.Title className="text-sm font-medium text-text">
            {title}
          </RadixDialog.Title>
          <RadixDialog.Close className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover">
            <X size={14} />
          </RadixDialog.Close>
        </div>
        <div className="p-4">{children}</div>
      </RadixDialog.Content>
    </RadixDialog.Portal>
  );
}

export function DialogFooter({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-end gap-2 pt-4 border-t border-border/40 mt-4 -mx-4 px-4 -mb-4 pb-4",
        className,
      )}
    >
      {children}
    </div>
  );
}
