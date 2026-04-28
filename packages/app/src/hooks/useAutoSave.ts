import { useEffect, useRef, useCallback, useState } from "react";
import { useDocumentStore, useUiStore, buildVcadFileFromState } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import {
  saveDocument,
  acquireLock,
  releaseLock,
  refreshLock,
  isStorageAvailable,
  isStorageWarning,
} from "@/lib/storage";

const DEBOUNCE_MS = 1000;
const LOCK_REFRESH_MS = 15000;

/**
 * Module-level pending-save registry.
 *
 * The autosave hook registers its current save closure here whenever a
 * debounced save is queued. Doc-switch call sites await `flushPendingSave()`
 * BEFORE swapping the engine, so the old doc's pending edit is persisted with
 * its own documentId — without this, switching docs within DEBOUNCE_MS of an
 * edit silently dropped that edit (the engine was replaced before the timer
 * fired).
 *
 * `pendingSaveFn` is the actual save call; it captures the OLD documentId at
 * dirty-tick time, so it remains valid after the store has moved on.
 */
let pendingSaveFn: (() => Promise<void>) | null = null;

export async function flushPendingSave(): Promise<void> {
  const fn = pendingSaveFn;
  if (!fn) return;
  pendingSaveFn = null;
  try {
    await fn();
  } catch (err) {
    console.error("flushPendingSave failed:", err);
  }
}

export function useAutoSave() {
  const documentId = useDocumentStore((s) => s.documentId);
  const documentName = useDocumentStore((s) => s.documentName);
  const isDirty = useDocumentStore((s) => s.isDirty);
  const markSaved = useDocumentStore((s) => s.markSaved);
  const readOnlyShare = useUiStore((s) => s.readOnlyShare);

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lockRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [hasLock, setHasLock] = useState(false);

  const save = useCallback(async () => {
    if (!documentId) return;

    // Check storage availability
    const available = await isStorageAvailable();
    if (!available) {
      useNotificationStore.getState().addToast(
        "Storage full - cannot save",
        "error",
        5000
      );
      return;
    }

    // Check for warning
    const warning = await isStorageWarning();
    if (warning) {
      useNotificationStore.getState().addToast(
        "Storage nearly full (80%+)",
        "info",
        3000
      );
    }

    try {
      const state = useDocumentStore.getState();
      const vcadFile = buildVcadFileFromState(state);
      if (!vcadFile) {
        // Engine not ready yet — skip this save cycle; next dirty tick retries.
        return;
      }

      await saveDocument(documentId, documentName, vcadFile);
      markSaved();
    } catch (err) {
      console.error("Auto-save failed:", err);
      useNotificationStore.getState().addToast("Auto-save failed", "error");
    }
  }, [documentId, documentName, markSaved]);

  // Acquire lock when document changes
  useEffect(() => {
    // Reset lock state on document change to avoid stale true.
    setHasLock(false);
    if (!documentId) return;
    // Read-only share sessions never persist anything, so no lock needed.
    if (readOnlyShare) return;

    let cancelled = false;

    async function tryAcquireLock() {
      const acquired = await acquireLock(documentId!);
      if (cancelled) return;

      if (!acquired) {
        useNotificationStore.getState().addToast(
          "Document is open in another tab",
          "info",
          5000
        );
      }
      setHasLock(acquired);
    }

    tryAcquireLock();

    return () => {
      cancelled = true;
      if (documentId) {
        releaseLock(documentId);
      }
    };
  }, [documentId, readOnlyShare]);

  // Periodically refresh lock
  useEffect(() => {
    if (!documentId || !hasLock) return;

    lockRefreshRef.current = setInterval(async () => {
      const refreshed = await refreshLock(documentId);
      if (!refreshed) {
        setHasLock(false);
        if (debounceRef.current) {
          clearTimeout(debounceRef.current);
          debounceRef.current = null;
        }
        useNotificationStore.getState().addToast(
          "Lost document lock - another tab may have taken control",
          "error"
        );
      }
    }, LOCK_REFRESH_MS);

    return () => {
      if (lockRefreshRef.current) {
        clearInterval(lockRefreshRef.current);
        lockRefreshRef.current = null;
      }
    };
  }, [documentId, hasLock]);

  // Debounced auto-save when dirty
  useEffect(() => {
    if (!isDirty || !documentId || !hasLock) return;
    // Read-only share sessions must not persist anything to local storage.
    if (readOnlyShare) return;

    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }

    // Register `save` as the pending flush. The closure captures the CURRENT
    // documentId / documentName, so even if the store has switched docs by
    // the time `flushPendingSave()` is awaited, the persisted row is the old
    // doc — which is exactly what we want.
    pendingSaveFn = save;

    debounceRef.current = setTimeout(() => {
      pendingSaveFn = null;
      save();
    }, DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
      // Effect cleanup runs when documentId changes (or unmount). Clear our
      // registration so a stale closure can't run later — the flush should
      // have been awaited by whoever triggered the doc switch.
      if (pendingSaveFn === save) pendingSaveFn = null;
    };
  }, [isDirty, documentId, save, hasLock, readOnlyShare]);

  return { save };
}
