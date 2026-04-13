import * as RadixDialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface BottomSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title?: string;
  /** "auto" = height fits content up to 85dvh, "full" = always 85dvh */
  size?: "auto" | "full";
  children: ReactNode;
}

/**
 * Mobile-friendly bottom sheet. Slides up from the bottom of the screen,
 * has a drag-handle affordance, and dismisses on backdrop tap or X.
 */
export function BottomSheet({
  open,
  onOpenChange,
  title,
  size = "auto",
  children,
}: BottomSheetProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-50 bg-black/50 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0" />
        <RadixDialog.Content
          className={cn(
            "fixed inset-x-0 bottom-0 z-50",
            "flex flex-col",
            "bg-surface border-t border-border",
            "rounded-t-2xl",
            "shadow-[0_-8px_32px_rgba(0,0,0,0.4)]",
            "focus:outline-none",
            size === "full" ? "h-[85dvh]" : "max-h-[85dvh]",
            "pb-[env(safe-area-inset-bottom)]",
            "data-[state=open]:animate-in data-[state=closed]:animate-out",
            "data-[state=closed]:slide-out-to-bottom data-[state=open]:slide-in-from-bottom",
          )}
        >
          <div className="flex items-center justify-center pt-2 pb-1 shrink-0">
            <div className="h-1 w-10 rounded-full bg-border" />
          </div>
          {title && (
            <div className="flex h-11 shrink-0 items-center justify-between px-4 border-b border-border">
              <RadixDialog.Title className="text-sm font-medium text-text">
                {title}
              </RadixDialog.Title>
              <RadixDialog.Close className="flex h-10 w-10 -mr-2 items-center justify-center text-text-muted hover:text-text">
                <X size={18} />
              </RadixDialog.Close>
            </div>
          )}
          <div className="flex-1 min-h-0 overflow-y-auto">{children}</div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
