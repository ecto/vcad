import { useState, useEffect, useCallback, lazy, Suspense } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Link } from "@phosphor-icons/react/dist/ssr/Link";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import {
  useAuthStore,
  createShare,
  revokeShare,
  listSharesForDocument,
  getMyProfile,
  publishDocument,
  slugify,
  createShareRedirect,
  type ShareRecord,
  type Profile,
} from "@vcad/auth";
import { useDocumentStore } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { loadDocument as loadStoredDocument } from "@/lib/storage";
import { captureViewerState, encodeViewerState } from "@/lib/viewer-state";
import { cn } from "@/lib/utils";

const UsernamePickerModal = lazy(() =>
  import("@/components/UsernamePickerModal").then((m) => ({
    default: m.UsernamePickerModal,
  })),
);

interface ShareDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function buildShareUrl(
  profile: Profile | null,
  slug: string | null,
  token: string,
  withViewerState: boolean,
): string {
  // Prefer canonical /@username/slug when available
  const base =
    profile && slug
      ? `${window.location.origin}/@${profile.username}/${slug}`
      : `${window.location.origin}/view/${token}`;
  if (!withViewerState) return base;
  const state = captureViewerState();
  const encoded = encodeViewerState(state);
  return `${base}?at=${encoded}`;
}

export function ShareDialog({ open, onOpenChange }: ShareDialogProps) {
  const user = useAuthStore((s) => s.user);
  const documentId = useDocumentStore((s) => s.documentId);
  const documentName = useDocumentStore((s) => s.documentName);

  const [cloudId, setCloudId] = useState<string | null>(null);
  const [cloudLookupDone, setCloudLookupDone] = useState(false);
  const [existingShare, setExistingShare] = useState<ShareRecord | null>(null);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [docSlug, setDocSlug] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);
  const [usernamePickerOpen, setUsernamePickerOpen] = useState(false);

  // Resolve cloudId + profile + existing share whenever the dialog opens.
  useEffect(() => {
    if (!open || !documentId) {
      setCloudLookupDone(false);
      setCloudId(null);
      setExistingShare(null);
      setProfile(null);
      setDocSlug(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const [stored, myProfile] = await Promise.all([
          loadStoredDocument(documentId),
          getMyProfile(),
        ]);
        if (cancelled) return;
        setCloudId(stored?.cloudId ?? null);
        setProfile(myProfile);
        setCloudLookupDone(true);
        if (stored?.cloudId) {
          const shares = await listSharesForDocument(stored.cloudId);
          if (!cancelled && shares.length > 0 && shares[0]) {
            setExistingShare(shares[0]);
          }
        }
      } catch (err) {
        console.error("[share-dialog] lookup failed:", err);
        if (!cancelled) setCloudLookupDone(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, documentId]);

  const handleToggle = useCallback(async () => {
    if (!cloudId) return;

    // If turning on and no profile → open username picker first.
    if (!existingShare && !profile) {
      setUsernamePickerOpen(true);
      return;
    }

    setLoading(true);
    try {
      if (existingShare) {
        await revokeShare(existingShare.token);
        setExistingShare(null);
        setDocSlug(null);
      } else {
        // Generate slug and publish the document
        const slug = slugify(documentName || "untitled");
        if (profile) {
          await publishDocument(cloudId, slug, "public");
          setDocSlug(slug);
        }

        const share = await createShare(cloudId);
        setExistingShare(share);

        // Record redirect so /view/<token> → /@username/slug works
        if (profile) {
          await createShareRedirect(share.token, profile.username, slug).catch(
            () => {
              /* best effort */
            },
          );
        }
      }
    } catch (err) {
      console.error("[share-dialog] toggle failed:", err);
      useNotificationStore
        .getState()
        .toast.error(`Share action failed: ${(err as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, [cloudId, existingShare, profile, documentName]);

  // After username picker completes, set profile and retry the share toggle.
  const handleUsernamePickerComplete = useCallback(
    async (_username: string) => {
      const myProfile = await getMyProfile();
      setProfile(myProfile);
      // Auto-toggle share on now that we have a profile.
      if (cloudId && !existingShare) {
        setLoading(true);
        try {
          const slug = slugify(documentName || "untitled");
          await publishDocument(cloudId, slug, "public");
          setDocSlug(slug);
          const share = await createShare(cloudId);
          setExistingShare(share);
          if (myProfile) {
            await createShareRedirect(
              share.token,
              myProfile.username,
              slug,
            ).catch(() => {});
          }
        } catch (err) {
          console.error("[share-dialog] post-username share failed:", err);
          useNotificationStore
            .getState()
            .toast.error(`Share failed: ${(err as Error).message}`);
        } finally {
          setLoading(false);
        }
      }
    },
    [cloudId, existingShare, documentName],
  );

  const copyUrl = useCallback(
    async (withViewerState: boolean) => {
      if (!existingShare) return;
      const url = buildShareUrl(
        profile,
        docSlug,
        existingShare.token,
        withViewerState,
      );
      try {
        await navigator.clipboard.writeText(url);
        setCopied(true);
        useNotificationStore
          .getState()
          .toast.success(
            withViewerState
              ? "Link copied with current view"
              : "Link copied",
          );
        setTimeout(() => setCopied(false), 1500);
      } catch (err) {
        console.error("[share-dialog] clipboard write failed:", err);
        useNotificationStore
          .getState()
          .toast.error("Could not copy to clipboard");
      }
    },
    [existingShare, profile, docSlug],
  );

  const blocker: string | null = !user
    ? "Sign in to share this document."
    : cloudLookupDone && !cloudId
      ? "Save this document to the cloud before sharing."
      : null;

  const toggleOn = !!existingShare;
  const shareUrl = existingShare
    ? profile && docSlug
      ? `${window.location.origin}/@${profile.username}/${docSlug}`
      : `${window.location.origin}/view/${existingShare.token}`
    : "";

  return (
    <>
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

            <div className="flex items-center gap-2 mb-5">
              <Link size={14} className="text-brand" />
              <Dialog.Title className="text-sm font-semibold text-text truncate">
                Share {documentName || "document"}
              </Dialog.Title>
            </div>

            {blocker && (
              <div className="text-xs text-text-muted py-4">{blocker}</div>
            )}

            {!blocker && (
              <>
                <label className="flex items-center justify-between gap-3 py-3 border-t border-b border-border">
                  <span className="text-xs text-text">
                    Anyone with the link can view
                  </span>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={toggleOn}
                    disabled={loading}
                    onClick={handleToggle}
                    className={cn(
                      "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors",
                      toggleOn ? "bg-brand" : "bg-border",
                      loading && "opacity-50 cursor-wait",
                    )}
                  >
                    <span
                      className={cn(
                        "inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform",
                        toggleOn ? "translate-x-5" : "translate-x-1",
                      )}
                    />
                  </button>
                </label>

                {toggleOn && (
                  <div className="mt-4 space-y-2">
                    <div className="flex items-center gap-2">
                      <input
                        type="text"
                        readOnly
                        value={shareUrl}
                        onFocus={(e) => e.currentTarget.select()}
                        className={cn(
                          "flex-1 min-w-0 px-2 py-1.5 text-[11px] font-mono",
                          "bg-bg border border-border text-text",
                          "focus:outline-none focus:border-brand",
                        )}
                      />
                      <button
                        type="button"
                        onClick={() => copyUrl(false)}
                        className={cn(
                          "flex items-center gap-1 px-2 py-1.5 text-[11px] font-medium",
                          "bg-brand text-white hover:bg-brand/90 transition-colors",
                        )}
                      >
                        {copied ? <Check size={12} /> : <Copy size={12} />}
                        <span>{copied ? "Copied" : "Copy"}</span>
                      </button>
                    </div>

                    <button
                      type="button"
                      onClick={() => copyUrl(true)}
                      className={cn(
                        "w-full text-left px-2 py-1.5 text-[11px]",
                        "text-text-muted hover:text-text hover:bg-hover transition-colors",
                        "border border-dashed border-border",
                      )}
                      title="Includes camera, selection, and active feature in the URL"
                    >
                      Copy with current view →
                    </button>

                    <p className="text-[10px] text-text-muted/80 leading-relaxed pt-1">
                      Viewers can see this document read-only. They'll be
                      prompted to sign in and fork if they try to edit.
                    </p>
                  </div>
                )}
              </>
            )}
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>

      <Suspense fallback={null}>
        <UsernamePickerModal
          open={usernamePickerOpen}
          onOpenChange={setUsernamePickerOpen}
          onComplete={handleUsernamePickerComplete}
        />
      </Suspense>
    </>
  );
}
