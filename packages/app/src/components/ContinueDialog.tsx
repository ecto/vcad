import { useState, useEffect, useCallback } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { ArrowSquareOut } from "@phosphor-icons/react/dist/ssr/ArrowSquareOut";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import {
  useAuthStore,
  createShare,
  listSharesForDocument,
} from "@vcad/auth";
import { useDocumentStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { loadDocument as loadStoredDocument } from "@/lib/storage";
import {
  buildContinueTargets,
  encodeDocForSeed,
  type ContinueTarget,
  type ContinueHost,
} from "@/lib/continue-links";
import { analytics } from "@/lib/analytics";
import { cn } from "@/lib/utils";

interface ContinueDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const LAST_HOST_KEY = "vcad.continue.lastHost";

/** Trigger a deep link: a new tab for http(s), the OS protocol handler for
 *  custom schemes (claude:// / cursor:// / vscode:). An anchor click fires the
 *  custom scheme without navigating the app away. */
function openLink(url: string): void {
  const a = document.createElement("a");
  a.href = url;
  a.target = /^https?:/i.test(url) ? "_blank" : "_self";
  a.rel = "noopener noreferrer";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export function ContinueDialog({ open, onOpenChange }: ContinueDialogProps) {
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const isSignedIn = !!user && !isAnonymous;
  const documentId = useDocumentStore((s) => s.documentId);
  const documentName = useDocumentStore((s) => s.documentName);

  const [token, setToken] = useState<string | null>(null);
  const [inlineBlob, setInlineBlob] = useState<string | null>(null);
  const [prepError, setPrepError] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [lastHost, setLastHost] = useState<ContinueHost | null>(null);
  const [copiedHost, setCopiedHost] = useState<ContinueHost | null>(null);

  // Prepare the handoff when the dialog opens. Preferred path: a signed-in,
  // cloud-synced doc → a durable share token the model resolves server-side (no
  // size limit, persists at vcad.io). Fallback: an accountless inline handoff
  // that compresses the live geometry into the seed itself.
  useEffect(() => {
    if (!open || !documentId) {
      setToken(null);
      setInlineBlob(null);
      setPrepError(null);
      return;
    }
    try {
      const stored = localStorage.getItem(LAST_HOST_KEY);
      if (stored) setLastHost(stored as ContinueHost);
    } catch {
      /* private mode — no last-host memory */
    }
    let cancelled = false;
    (async () => {
      setPreparing(true);
      setPrepError(null);
      try {
        if (isSignedIn) {
          const stored = await loadStoredDocument(documentId);
          const cid = stored?.cloudId ?? null;
          if (cid && !cancelled) {
            // Reuse an existing share token, else mint one. Continue needs only
            // the token (the capability) — it deliberately does NOT publish to
            // the public /@user/slug profile, so there's no username picker.
            const existing = await listSharesForDocument(cid);
            if (cancelled) return;
            const tok = existing[0]?.token ?? (await createShare(cid)).token;
            if (!cancelled) {
              setToken(tok);
              setInlineBlob(null);
            }
            return;
          }
        }
        // Accountless (or not-yet-synced) → inline handoff from the live IR.
        const doc = useDocumentStore.getState().document;
        const blob = await encodeDocForSeed(doc);
        if (cancelled) return;
        if (blob) {
          setInlineBlob(blob);
          setToken(null);
        } else {
          setPrepError(
            "This part is too large for an accountless handoff. Sign in to " +
              "continue it in Claude.",
          );
        }
      } catch (err) {
        console.error("[continue-dialog] prepare failed:", err);
        if (!cancelled) {
          setPrepError(
            `Couldn't prepare the handoff: ${(err as Error).message}`,
          );
        }
      } finally {
        if (!cancelled) setPreparing(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, documentId, isSignedIn]);

  const remember = useCallback((host: ContinueHost) => {
    setLastHost(host);
    try {
      localStorage.setItem(LAST_HOST_KEY, host);
    } catch {
      /* private mode */
    }
  }, []);

  const runTarget = useCallback(
    async (target: ContinueTarget) => {
      // Copy the seed first when the host can't prefill it (web) or has no URL
      // (Claude Code), so the clipboard is ready by the time the host opens.
      const needsCopy = !!target.clipboard && (target.copyWithOpen || !target.url);
      if (needsCopy && target.clipboard) {
        try {
          await navigator.clipboard.writeText(target.clipboard);
          setCopiedHost(target.host);
          setTimeout(() => setCopiedHost(null), 1500);
        } catch {
          useNotificationStore
            .getState()
            .toast.error("Couldn't copy the starter prompt to the clipboard.");
        }
      }
      if (target.url) openLink(target.url);
      remember(target.host);
      analytics.continueHandoff(target.host, token ? "token" : "inline");

      const msg = target.url
        ? needsCopy
          ? `Opening ${target.label} — paste the copied prompt to start.`
          : `Opening ${target.label} with your part.`
        : `Copied the install command + prompt for ${target.label}.`;
      useNotificationStore.getState().toast.success(msg);
    },
    [remember, token],
  );

  const mode: "token" | "inline" | null = token
    ? "token"
    : inlineBlob
      ? "inline"
      : null;

  const targets = token
    ? buildContinueTargets({ token, docName: documentName || undefined })
    : inlineBlob
      ? buildContinueTargets({ inlineDoc: inlineBlob, docName: documentName || undefined })
      : [];
  // Surface the last-used host first; Claude Desktop leads otherwise.
  const ordered = [...targets].sort((a, b) => {
    if (a.host === lastHost) return -1;
    if (b.host === lastHost) return 1;
    return 0;
  });

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          data-tauri-drag-region=""
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-md -translate-x-1/2 -translate-y-1/2",
            "rounded-xl border border-border bg-surface p-6 shadow-xl select-none",
            "focus:outline-none",
          )}
        >
          <Dialog.Close className="absolute right-3 top-3 p-1.5 text-text-muted hover:text-text transition-colors cursor-pointer">
            <X size={14} />
          </Dialog.Close>

          <div className="flex items-center gap-2 mb-1">
            <Sparkle size={14} className="text-brand" weight="fill" />
            <Dialog.Title className="text-sm font-semibold text-text truncate">
              Continue {documentName || "this part"} in Claude
            </Dialog.Title>
          </div>
          <p className="text-[11px] text-text-muted leading-relaxed mb-4">
            Hand this part to an AI you can talk to — it loads your exact
            geometry and picks up where you left off.
          </p>

          {prepError && (
            <div className="text-xs text-text-muted py-4">{prepError}</div>
          )}

          {!prepError && (
            <div className="space-y-1.5">
              {preparing && !mode && (
                <div className="text-[11px] text-text-muted py-3">
                  Preparing your handoff…
                </div>
              )}
              {ordered.map((target) => {
                const isCopyOnly = !target.url;
                return (
                  <button
                    key={target.host}
                    type="button"
                    disabled={!token}
                    onClick={() => runTarget(target)}
                    className={cn(
                      "w-full flex items-center gap-3 px-3 py-2 text-left",
                      "border border-border bg-bg hover:border-brand hover:bg-hover",
                      "transition-colors disabled:opacity-50 disabled:cursor-wait",
                    )}
                  >
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-medium text-text">
                          {target.label}
                        </span>
                        {target.host === lastHost && (
                          <span className="text-[9px] uppercase tracking-wide text-brand">
                            Last used
                          </span>
                        )}
                      </div>
                      <div className="text-[10px] text-text-muted/80 leading-snug mt-0.5">
                        {target.hint}
                      </div>
                    </div>
                    {copiedHost === target.host ? (
                      <span className="text-[10px] text-brand">Copied</span>
                    ) : isCopyOnly ? (
                      <Copy size={13} className="text-text-muted shrink-0" />
                    ) : (
                      <ArrowSquareOut
                        size={13}
                        className="text-text-muted shrink-0"
                      />
                    )}
                  </button>
                );
              })}
              {mode === "inline" && (
                <p className="text-[10px] text-text-muted/70 leading-relaxed pt-1">
                  Handing off without an account.{" "}
                  <span className="text-text-muted">
                    Sign in to hand off larger parts and keep them in your
                    account.
                  </span>
                </p>
              )}
              <p className="text-[10px] text-text-muted/70 leading-relaxed pt-2">
                Adding the vcad connector is a one-time step. After that, every
                handoff is one click.
              </p>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
