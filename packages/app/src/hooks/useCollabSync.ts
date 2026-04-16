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
    if (!documentId || !user || readOnly) return;

    let cancelled = false;
    (async () => {
      try {
        const stored = await loadStoredDocument(documentId);
        if (cancelled) return;
        setCloudId(stored?.cloudId ?? null);
      } catch {
        // Storage lookup failed — no collab for this doc.
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

    if (!cloudId || !user || readOnly) return;

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
