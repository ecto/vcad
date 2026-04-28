/**
 * Persistent "last-opened document id" slot.
 *
 * Bootstrap consults this AFTER URL routes but BEFORE most-recent-by-modifiedAt
 * so that startup picks the doc the user actually had open last, not whichever
 * row happens to have the highest `modifiedAt` after a background sync.
 *
 * localStorage is shared between web and Tauri webviews and survives reloads.
 * A read-only share session must NOT poison the slot — the writer skips when
 * `useUiStore.readOnlyShare` is set.
 */

import { useDocumentStore, useUiStore } from "@vcad/core";

const STORAGE_KEY = "vcad:last-opened-doc-id";

function safeStorage(): Storage | null {
  try {
    if (typeof window === "undefined") return null;
    return window.localStorage;
  } catch {
    return null;
  }
}

export function getLastOpenedDocId(): string | null {
  const storage = safeStorage();
  if (!storage) return null;
  try {
    const v = storage.getItem(STORAGE_KEY);
    return v && v.length > 0 ? v : null;
  } catch {
    return null;
  }
}

export function setLastOpenedDocId(id: string | null): void {
  const storage = safeStorage();
  if (!storage) return;
  try {
    if (id) {
      storage.setItem(STORAGE_KEY, id);
    } else {
      storage.removeItem(STORAGE_KEY);
    }
  } catch {
    // Quota / disabled storage — non-fatal.
  }
}

export function clearLastOpenedDocId(): void {
  setLastOpenedDocId(null);
}

/**
 * Subscribe to documentId changes and mirror them into the last-opened slot.
 * Idempotent — safe to call once at app boot. Returns an unsubscribe fn for
 * tests / hot-reload.
 *
 * Skips writes during read-only share sessions: a user landing on /view/<token>
 * must not have their saved last-opened doc clobbered by the share id.
 */
export function installLastOpenedTracker(): () => void {
  return useDocumentStore.subscribe((state, prev) => {
    if (state.documentId === prev.documentId) return;
    if (useUiStore.getState().readOnlyShare) return;
    setLastOpenedDocId(state.documentId);
  });
}
