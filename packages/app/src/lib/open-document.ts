/**
 * Programmatic "open this local document" helper.
 *
 * Used by DocumentPicker, conflict-fork notifications, command palette etc.
 * Centralizing the flow ensures every entry point flushes pending autosave
 * BEFORE the engine swap, sets the document meta (so the URL and last-opened
 * slot mirror correctly), and surfaces failures uniformly.
 */

import { useDocumentStore } from "@vcad/core";
import { loadDocument as loadDocumentFromDb } from "@/lib/storage";
import { flushPendingSave } from "@/hooks/useAutoSave";
import { useNotificationStore } from "@/stores/notification-store";

export async function openLocalDocumentById(id: string): Promise<boolean> {
  await flushPendingSave();
  try {
    const stored = await loadDocumentFromDb(id);
    if (!stored) {
      useNotificationStore.getState().addToast("Document not found", "error");
      return false;
    }
    useDocumentStore.getState().loadDocument(stored.document);
    useDocumentStore.getState().setDocumentMeta(stored.id, stored.name);
    return true;
  } catch (err) {
    console.error("openLocalDocumentById failed:", err);
    useNotificationStore.getState().addToast("Failed to open document", "error");
    return false;
  }
}
