import { useState, useEffect } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { GitFork } from "@phosphor-icons/react/dist/ssr/GitFork";
import { AuthModal, useAuthStore } from "@vcad/auth";
import { useUiStore } from "@vcad/core";
import { cn } from "@/lib/utils";

/**
 * Modal that prompts a read-only share viewer to sign in and fork when they
 * attempt to edit the document. Opens in response to `vcad:fork-prompt`
 * custom events dispatched by the Proxy-wrapped document store mutations,
 * by the read-only banner, and by UI-level guards (tool palette, keyboard
 * shortcuts).
 *
 * Fork flow: clicking "Sign in to fork" opens the existing AuthModal. Once
 * the user completes sign-in (useAuthStore.user becomes non-null), we clear
 * readOnlyShare — the doc already lives in the local document store, so
 * clearing the flag is sufficient to turn the session into a normal
 * editable one. Auto-sync will persist it to the new user's cloud on next
 * save.
 */
export function ForkPromptModal() {
  const [open, setOpen] = useState(false);
  const [authOpen, setAuthOpen] = useState(false);
  const [waitingForSignIn, setWaitingForSignIn] = useState(false);
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  // Anon sessions don't count as "signed in" for the fork-prompt flow:
  // forking a shared document still requires linking a permanent identity.
  const isSignedIn = !!user && !isAnonymous;
  const readOnlyShare = useUiStore((s) => s.readOnlyShare);
  const setReadOnlyShare = useUiStore((s) => s.setReadOnlyShare);

  // Listen for fork-prompt events from anywhere in the app.
  useEffect(() => {
    const handleForkPrompt = () => {
      setOpen(true);
    };
    window.addEventListener("vcad:fork-prompt", handleForkPrompt);
    return () => window.removeEventListener("vcad:fork-prompt", handleForkPrompt);
  }, []);

  // When the user signs in while we're waiting, clear the read-only flag and
  // the doc becomes editable.
  useEffect(() => {
    if (waitingForSignIn && isSignedIn) {
      setReadOnlyShare(null);
      setWaitingForSignIn(false);
      setAuthOpen(false);
      setOpen(false);
    }
  }, [waitingForSignIn, isSignedIn, setReadOnlyShare]);

  const handleSignInClick = () => {
    setWaitingForSignIn(true);
    setAuthOpen(true);
  };

  const handleKeepViewing = () => {
    setOpen(false);
  };

  // Nothing to fork if we're not in a read-only share.
  if (!readOnlyShare && open) {
    setOpen(false);
  }

  return (
    <>
      <Dialog.Root open={open} onOpenChange={setOpen}>
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
          <Dialog.Content
            data-tauri-drag-region=""
            className={cn(
              "fixed left-1/2 top-1/2 z-50 w-full max-w-sm -translate-x-1/2 -translate-y-1/2",
              "bg-surface p-6 shadow-2xl select-none",
              "focus:outline-none",
            )}
          >
            <Dialog.Close className="absolute right-3 top-3 p-1.5 text-text-muted hover:text-text transition-colors cursor-pointer">
              <X size={14} />
            </Dialog.Close>

            <div className="flex items-start gap-3 mb-5">
              <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-brand/15">
                <GitFork size={16} className="text-brand" />
              </div>
              <div className="min-w-0">
                <Dialog.Title className="text-sm font-semibold text-text">
                  This document is read-only
                </Dialog.Title>
                <Dialog.Description className="mt-1 text-xs text-text-muted leading-relaxed">
                  Sign in to create your own editable copy. Your edits won't
                  touch the original — you'll get a fresh fork in your account.
                </Dialog.Description>
              </div>
            </div>

            <div className="flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={handleKeepViewing}
                className={cn(
                  "px-3 py-1.5 text-xs font-medium",
                  "text-text-muted hover:text-text hover:bg-hover transition-colors",
                )}
              >
                Keep viewing
              </button>
              <button
                type="button"
                onClick={handleSignInClick}
                className={cn(
                  "px-3 py-1.5 text-xs font-medium",
                  "bg-brand text-white hover:bg-brand/90 transition-colors",
                )}
              >
                Sign in to fork
              </button>
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>

      <AuthModal open={authOpen} onOpenChange={setAuthOpen} />
    </>
  );
}
