import { useEffect, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { ClockCounterClockwise } from "@phosphor-icons/react/dist/ssr/ClockCounterClockwise";
import { useDocumentStore } from "@vcad/core";
import { VersionHistoryPanel, useAuthStore } from "@vcad/auth";
import { loadDocument as loadStoredDocument } from "@/lib/storage";
import { useNotificationStore } from "@/stores/notification-store";
import { cn } from "@/lib/utils";

interface VersionHistoryModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Modal wrapper around `VersionHistoryPanel`. Resolves the cloud ID for the
 * currently open document from local storage; the panel itself handles the
 * not-signed-in / not-synced cases with a friendly message.
 */
export function VersionHistoryModal({
  open,
  onOpenChange,
}: VersionHistoryModalProps) {
  const documentId = useDocumentStore((s) => s.documentId);
  const documentName = useDocumentStore((s) => s.documentName);
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const isSignedIn = !!user && !isAnonymous;

  const [cloudId, setCloudId] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !documentId) {
      setCloudId(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const stored = await loadStoredDocument(documentId);
        if (cancelled) return;
        setCloudId(stored?.cloudId ?? null);
      } catch {
        if (!cancelled) setCloudId(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, documentId]);

  const handleRestore = () => {
    useNotificationStore.getState().addToast("Version restored", "success");
    onOpenChange(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-lg -translate-x-1/2 -translate-y-1/2",
            "bg-surface shadow-2xl focus:outline-none max-h-[80vh] overflow-auto",
          )}
        >
          <Dialog.Close className="absolute right-3 top-3 p-1.5 text-text-muted hover:text-text transition-colors cursor-pointer">
            <X size={14} />
          </Dialog.Close>

          <div className="flex items-center gap-2 px-6 pt-6 pb-2">
            <ClockCounterClockwise size={14} className="text-brand" />
            <Dialog.Title className="text-sm font-semibold text-text truncate">
              Version history · {documentName || "document"}
            </Dialog.Title>
          </div>

          {!isSignedIn ? (
            <div className="p-6 text-sm text-text-muted">
              Sign in to enable cloud sync and access version history.
            </div>
          ) : documentId ? (
            <VersionHistoryPanel
              localDocId={documentId}
              cloudDocId={cloudId}
              onRestore={handleRestore}
            />
          ) : (
            <div className="p-6 text-sm text-text-muted">
              No document open.
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
