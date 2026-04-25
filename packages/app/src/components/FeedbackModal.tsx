import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Megaphone } from "@phosphor-icons/react/dist/ssr/Megaphone";
import { cn } from "@/lib/utils";

const ISSUE_URL = "https://github.com/ecto/vcad/issues/new";

export function FeedbackModal({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [text, setText] = useState("");

  function submit() {
    const trimmed = text.trim();
    if (!trimmed) return;
    const body = `${trimmed}\n\n---\n_Sent from vcad ${__APP_VERSION__} · ${navigator.userAgent}_`;
    const url = `${ISSUE_URL}?body=${encodeURIComponent(body)}`;
    window.open(url, "_blank", "noopener,noreferrer");
    setText("");
    onOpenChange(false);
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-md -translate-x-1/2 -translate-y-1/2",
            "bg-surface p-6 shadow-2xl",
            "focus:outline-none",
          )}
        >
          <Dialog.Close className="absolute right-3 top-3 p-1.5 text-text-muted hover:text-text transition-colors cursor-pointer">
            <X size={14} />
          </Dialog.Close>

          <div className="flex items-center gap-2 mb-1">
            <Megaphone size={16} className="text-brand" />
            <Dialog.Title className="text-sm font-medium text-text">
              Send feedback
            </Dialog.Title>
          </div>
          <Dialog.Description className="text-xs text-text-muted mb-3">
            Bug, idea, or wish? Opens a GitHub issue with your message
            prefilled.
          </Dialog.Description>

          <textarea
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                e.preventDefault();
                submit();
              }
            }}
            placeholder="What's on your mind?"
            rows={6}
            className={cn(
              "w-full resize-none bg-bg p-3 text-xs text-text",
              "placeholder:text-text-muted/60 focus:outline-none",
              "border border-border focus:border-brand/50 transition-colors",
            )}
          />

          <div className="mt-3 flex items-center justify-between gap-2">
            <span className="text-[10px] text-text-muted/60">
              ⌘↵ to submit
            </span>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => onOpenChange(false)}
                className={cn(
                  "h-7 px-3 text-xs text-text-muted hover:text-text",
                  "hover:bg-hover transition-colors",
                )}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={submit}
                disabled={!text.trim()}
                className={cn(
                  "h-7 px-3 text-xs font-medium",
                  "bg-brand text-white hover:bg-brand/90 transition-colors",
                  "disabled:opacity-40 disabled:cursor-not-allowed",
                )}
              >
                Send
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
