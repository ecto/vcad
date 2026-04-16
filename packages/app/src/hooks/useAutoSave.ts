import { useEffect, useRef, useCallback, useState } from "react";
import { useDocumentStore, useUiStore } from "@vcad/core";
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
      const vcadFile = {
        document: state.document,
        parts: state.parts,
        consumedParts: state.consumedParts,
        nextNodeId: state.nextNodeId,
      };

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

    debounceRef.current = setTimeout(() => {
      save();
    }, DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
    };
  }, [isDirty, documentId, save, hasLock, readOnlyShare]);

  return { save };
}
