/**
 * React hook that wires the Supabase Realtime collab transport to the
 * document store. When a cloud-synced document is open, this hook:
 *
 * 1. Opens a broadcast channel for the document.
 * 2. After every local mutation (isDirty transition), broadcasts new ops.
 * 3. Merges incoming remote ops into the local CRDT engine.
 *
 * Two browser tabs on the same signed-in account editing the same doc
 * will see each other's changes in real time.
 */

import { useEffect, useRef, useState } from "react";
import { useDocumentStore, useUiStore } from "@vcad/core";
import {
  joinCollabChannel,
  isApplyingRemoteOps,
  enableCloudSync,
  useAuthStore,
  type CollabChannel,
} from "@vcad/auth";
import { useBootStore } from "@/stores/boot-store";
import { loadDocument as loadStoredDocument } from "@/lib/storage";

export function useCollabSync() {
  const bootPhase = useBootStore((s) => s.phase);
  const documentId = useDocumentStore((s) => s.documentId);
  const readOnly = useUiStore((s) => s.readOnlyShare);
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  // Anonymous Supabase sessions exist for chat-thread RLS scoping; cloud
  // sync + collab features are gated to permanent identities only.
  const isCloudUser = !!user && !isAnonymous;
  const [cloudId, setCloudId] = useState<string | null>(null);
  const channelRef = useRef<CollabChannel | null>(null);

  // Don't do anything until bootstrap is complete — the CRDT engine,
  // document store, and auth session aren't ready before that.
  const ready = bootPhase === "ready";

  // Resolve cloudId from local storage whenever documentId changes.
  useEffect(() => {
    setCloudId(null);
    if (!ready || !documentId || !isCloudUser || readOnly) return;

    let cancelled = false;
    (async () => {
      try {
        const stored = await loadStoredDocument(documentId);
        if (cancelled) return;
        const resolved = stored?.cloudId ?? null;
        console.log("[collab] resolved cloudId:", resolved, "for localId:", documentId);

        if (!resolved) {
          // Doc isn't cloud-synced yet. Promote to pending and trigger sync.
          // Retry a few times because triggerSync may bail if another sync
          // is already in flight (e.g. the auto-sync on sign-in).
          console.log("[collab] no cloudId — promoting to cloud sync");
          try {
            await enableCloudSync(documentId);
          } catch (syncErr) {
            console.warn("[collab] cloud sync promotion failed:", syncErr);
          }

          // Poll for cloudId — the sync may complete asynchronously.
          for (let attempt = 0; attempt < 5; attempt++) {
            if (cancelled) return;
            await new Promise((r) => setTimeout(r, 2000));
            if (cancelled) return;
            const updated = await loadStoredDocument(documentId);
            if (cancelled) return;
            const newCloudId = updated?.cloudId ?? null;
            if (newCloudId) {
              console.log("[collab] post-sync cloudId:", newCloudId, `(attempt ${attempt + 1})`);
              setCloudId(newCloudId);
              return;
            }
            // Retry the sync in case the previous attempt was blocked.
            try {
              await enableCloudSync(documentId);
            } catch {
              // ignore
            }
          }
          console.log("[collab] post-sync cloudId: null after retries");
          return;
        }

        setCloudId(resolved);
      } catch (err) {
        console.warn("[collab] storage lookup failed:", err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [ready, documentId, isCloudUser, readOnly]);

  // Open/close the collab channel when cloudId becomes available. Debounced
  // with a short delay so Vite HMR unmount+remount doesn't cause a
  // leave→rejoin cycle on every hot update.
  useEffect(() => {
    // Clean up previous channel immediately.
    channelRef.current?.leave();
    channelRef.current = null;

    if (!ready || !cloudId || !isCloudUser || readOnly) return;

    const timer = setTimeout(() => {
      console.log("[collab] joining channel for", cloudId);
      const channel = joinCollabChannel(cloudId, {
        getSyncClock: () => useDocumentStore.getState().getSyncClock(),
        getOpsSince: (clock) => useDocumentStore.getState().getOpsSince(clock),
        mergeRemoteOps: (ops) => useDocumentStore.getState().mergeRemoteOps(ops),
      });
      channelRef.current = channel;
    }, 200);

    return () => {
      clearTimeout(timer);
      if (channelRef.current) {
        console.log("[collab] leaving channel for", cloudId);
        channelRef.current.leave();
        channelRef.current = null;
      }
    };
  }, [ready, cloudId, isCloudUser, readOnly]);

  // Broadcast after every local mutation. We detect mutations by watching
  // the `parts` array reference — every mutation produces a new array via
  // applyApiResult/applyLegacyResult. Debounce with rAF to batch rapid
  // parameter scrubs into a single broadcast.
  const broadcastPending = useRef(false);
  const prevPartsRef = useRef<unknown>(null);

  useEffect(() => {
    const unsubscribe = useDocumentStore.subscribe((state) => {
      if (state.parts !== prevPartsRef.current && prevPartsRef.current !== null) {
        // Parts reference changed → a mutation happened. Skip if the change
        // came from merging remote ops (avoid re-broadcasting what we received).
        if (channelRef.current && !broadcastPending.current && !isApplyingRemoteOps()) {
          broadcastPending.current = true;
          requestAnimationFrame(() => {
            channelRef.current?.broadcastOps();
            broadcastPending.current = false;
          });
        }
      }
      prevPartsRef.current = state.parts;
    });
    return unsubscribe;
  }, []);
}
