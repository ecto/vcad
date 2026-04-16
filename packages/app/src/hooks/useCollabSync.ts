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
import { loadDocument as loadStoredDocument } from "@/lib/storage";

export function useCollabSync() {
  const documentId = useDocumentStore((s) => s.documentId);
  const readOnly = useUiStore((s) => s.readOnlyShare);
  const user = useAuthStore((s) => s.user);
  const [cloudId, setCloudId] = useState<string | null>(null);
  const channelRef = useRef<CollabChannel | null>(null);

  // Resolve cloudId from local storage whenever documentId changes.
  useEffect(() => {
    setCloudId(null);
    if (!documentId) {
      console.log("[collab] no documentId, skipping");
      return;
    }
    if (!user) {
      console.log("[collab] not signed in, skipping");
      return;
    }
    if (readOnly) {
      console.log("[collab] read-only share, skipping");
      return;
    }

    let cancelled = false;
    (async () => {
      try {
        const stored = await loadStoredDocument(documentId);
        if (cancelled) return;
        const resolved = stored?.cloudId ?? null;
        console.log("[collab] resolved cloudId:", resolved, "for localId:", documentId);

        if (!resolved) {
          // Doc isn't cloud-synced yet. Promote to pending and trigger sync
          // so we get a cloudId. Re-check after sync completes.
          console.log("[collab] no cloudId — promoting to cloud sync");
          try {
            await enableCloudSync(documentId);
            // Re-read after sync
            const updated = await loadStoredDocument(documentId);
            if (cancelled) return;
            const newCloudId = updated?.cloudId ?? null;
            console.log("[collab] post-sync cloudId:", newCloudId);
            setCloudId(newCloudId);
          } catch (syncErr) {
            console.warn("[collab] cloud sync promotion failed:", syncErr);
          }
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
  }, [documentId, user, readOnly]);

  // Open/close the collab channel when cloudId becomes available.
  useEffect(() => {
    // Clean up previous channel.
    channelRef.current?.leave();
    channelRef.current = null;

    if (!cloudId || !user || readOnly) {
      console.log("[collab] channel precondition not met:", { cloudId, user: !!user, readOnly: !!readOnly });
      return;
    }

    console.log("[collab] joining channel for", cloudId);

    const channel = joinCollabChannel(cloudId, {
      getSyncClock: () => useDocumentStore.getState().getSyncClock(),
      getOpsSince: (clock) => useDocumentStore.getState().getOpsSince(clock),
      mergeRemoteOps: (ops) => useDocumentStore.getState().mergeRemoteOps(ops),
    });

    channelRef.current = channel;

    return () => {
      console.log("[collab] leaving channel for", cloudId);
      channel?.leave();
      channelRef.current = null;
    };
  }, [cloudId, user, readOnly]);

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
