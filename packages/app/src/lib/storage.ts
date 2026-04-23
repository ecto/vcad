import { openDB, type IDBPDatabase } from "idb";
import type { VcadFile, VcadFileLegacy } from "@vcad/core";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

const DB_NAME = "vcad-documents";
const DB_VERSION = 1;
const DOCUMENTS_STORE = "documents";
const LOCKS_STORE = "locks";

export interface StoredDocument {
  id: string;
  name: string;
  document: VcadFile;
  createdAt: number;
  modifiedAt: number;
  version: number;
  syncStatus: "local" | "synced" | "pending";
  cloudId?: string;
  thumbnail?: Blob;
}

export interface DocumentMeta {
  id: string;
  name: string;
  createdAt: number;
  modifiedAt: number;
  syncStatus: "local" | "synced" | "pending";
  cloudId?: string;
}

export interface DocumentLock {
  documentId: string;
  tabId: string;
  acquiredAt: number;
}

// Generate unique tab ID for locking
const TAB_ID = crypto.randomUUID();

let dbInstance: IDBPDatabase | null = null;

async function getDb(): Promise<IDBPDatabase> {
  if (dbInstance) return dbInstance;

  dbInstance = await openDB(DB_NAME, DB_VERSION, {
    upgrade(db) {
      // Documents store
      if (!db.objectStoreNames.contains(DOCUMENTS_STORE)) {
        const store = db.createObjectStore(DOCUMENTS_STORE, { keyPath: "id" });
        store.createIndex("modifiedAt", "modifiedAt");
        store.createIndex("name", "name");
      }

      // Locks store for multi-tab coordination
      if (!db.objectStoreNames.contains(LOCKS_STORE)) {
        db.createObjectStore(LOCKS_STORE, { keyPath: "documentId" });
      }
    },
  });

  return dbInstance;
}

export async function saveDocument(
  id: string,
  name: string,
  vcadFile: VcadFile,
  thumbnail?: Blob
): Promise<void> {
  const db = await getDb();
  const existing = await db.get(DOCUMENTS_STORE, id);

  const doc: StoredDocument = {
    id,
    name,
    document: vcadFile,
    createdAt: existing?.createdAt ?? Date.now(),
    modifiedAt: Date.now(),
    version: (existing?.version ?? 0) + 1,
    syncStatus: "local",
    thumbnail,
  };

  await db.put(DOCUMENTS_STORE, doc);
}

export async function loadDocument(id: string): Promise<StoredDocument | null> {
  const db = await getDb();
  const doc = await db.get(DOCUMENTS_STORE, id);
  if (!doc) return null;
  return {
    ...doc,
    document: adoptPersistedVcadFile(doc.document),
  };
}

/**
 * Normalize a stored VcadFile to the current tagged-union shape.
 *
 * Pre-v0.4 IndexedDB rows were written as the flat legacy object
 * (`{document, parts, nextNodeId, ...}` with no `kind` discriminator). This
 * adapter tags them as `kind: "legacy"` at read time so the tagged-union
 * type stays sound end-to-end. Rows with loonSource get tagged as `loon`.
 * The backfill job rewrites them in-place to the new shape on next save.
 */
export function adoptPersistedVcadFile(persisted: unknown): VcadFile {
  if (persisted && typeof persisted === "object") {
    const obj = persisted as Record<string, unknown>;
    if (typeof obj.kind === "string") {
      // Already a tagged variant — trust the discriminator.
      return persisted as VcadFile;
    }
    if (typeof obj.loonSource === "string" && obj.loonSource) {
      return {
        kind: "loon",
        version: "0.3",
        loonSource: obj.loonSource,
        document: obj.document as Document,
        parts: (obj.parts as PartInfo[] | undefined) ?? [],
        nextNodeId: (obj.nextNodeId as number | undefined) ?? 1,
        nextPartNum: obj.nextPartNum as number | undefined,
      };
    }
    if (obj.document && typeof obj.document === "object") {
      const legacy: VcadFileLegacy = {
        kind: "legacy",
        version: "0.1",
        document: obj.document as Document,
        parts: (obj.parts as PartInfo[] | undefined) ?? [],
        consumedParts: obj.consumedParts as Record<string, PartInfo> | undefined,
        nextNodeId: (obj.nextNodeId as number | undefined) ?? 1,
        nextPartNum: obj.nextPartNum as number | undefined,
      };
      return legacy;
    }
  }
  throw new Error("adoptPersistedVcadFile: unrecognized stored document shape");
}

export async function listDocuments(): Promise<DocumentMeta[]> {
  const db = await getDb();
  const docs = await db.getAllFromIndex(DOCUMENTS_STORE, "modifiedAt");

  // Return newest first, only metadata (not full document content)
  return docs
    .map((d) => ({
      id: d.id,
      name: d.name,
      createdAt: d.createdAt,
      modifiedAt: d.modifiedAt,
      syncStatus: d.syncStatus,
      cloudId: d.cloudId,
    }))
    .reverse();
}

export async function deleteDocument(id: string): Promise<void> {
  const db = await getDb();
  await db.delete(DOCUMENTS_STORE, id);
  // Also release any lock
  await db.delete(LOCKS_STORE, id);
}

export async function renameDocument(id: string, name: string): Promise<void> {
  const db = await getDb();
  const doc = await db.get(DOCUMENTS_STORE, id);
  if (!doc) return;

  doc.name = name;
  doc.modifiedAt = Date.now();
  await db.put(DOCUMENTS_STORE, doc);
}

/**
 * Update a document with partial data.
 * Used by sync service to update sync status, cloudId, etc.
 */
export async function updateDocument(
  id: string,
  updates: Partial<Omit<StoredDocument, "id">>
): Promise<void> {
  const db = await getDb();
  const doc = await db.get(DOCUMENTS_STORE, id);
  if (!doc) return;

  const updated = { ...doc, ...updates };
  await db.put(DOCUMENTS_STORE, updated);
}

/**
 * Get all documents with full data.
 * Used by sync service to find pending uploads.
 */
export async function getAllDocuments(): Promise<StoredDocument[]> {
  const db = await getDb();
  const docs = await db.getAll(DOCUMENTS_STORE);
  return docs.map((doc) => ({
    ...doc,
    document: adoptPersistedVcadFile(doc.document),
  }));
}

/**
 * Save a complete document object.
 * Used by sync service when creating documents from cloud.
 */
export async function saveCompleteDocument(doc: StoredDocument): Promise<void> {
  const db = await getDb();
  await db.put(DOCUMENTS_STORE, doc);
}

// Document locking for multi-tab awareness

export async function acquireLock(documentId: string): Promise<boolean> {
  const db = await getDb();
  const existing = await db.get(LOCKS_STORE, documentId);

  // Lock is stale if older than 30 seconds (tab closed without releasing)
  const isStale = existing && Date.now() - existing.acquiredAt > 30000;

  if (existing && !isStale && existing.tabId !== TAB_ID) {
    return false; // Another tab has the lock
  }

  const lock: DocumentLock = {
    documentId,
    tabId: TAB_ID,
    acquiredAt: Date.now(),
  };
  await db.put(LOCKS_STORE, lock);
  return true;
}

export async function releaseLock(documentId: string): Promise<void> {
  const db = await getDb();
  const existing = await db.get(LOCKS_STORE, documentId);

  // Only release if we own it
  if (existing?.tabId === TAB_ID) {
    await db.delete(LOCKS_STORE, documentId);
  }
}

export async function refreshLock(documentId: string): Promise<boolean> {
  const db = await getDb();
  const existing = await db.get(LOCKS_STORE, documentId);

  if (existing?.tabId !== TAB_ID) {
    return false; // We don't own this lock
  }

  const lock: DocumentLock = {
    documentId,
    tabId: TAB_ID,
    acquiredAt: Date.now(),
  };
  await db.put(LOCKS_STORE, lock);
  return true;
}

export async function isDocumentLocked(documentId: string): Promise<boolean> {
  const db = await getDb();
  const existing = await db.get(LOCKS_STORE, documentId);

  if (!existing) return false;

  // Lock is stale if older than 30 seconds
  const isStale = Date.now() - existing.acquiredAt > 30000;
  if (isStale) return false;

  // We own the lock, so it's not "locked" from our perspective
  if (existing.tabId === TAB_ID) return false;

  return true;
}

// Storage quota utilities

export async function getStorageUsage(): Promise<{
  used: number;
  quota: number;
  percentage: number;
}> {
  if (!navigator.storage?.estimate) {
    return { used: 0, quota: 0, percentage: 0 };
  }

  const { usage = 0, quota = 0 } = await navigator.storage.estimate();
  const percentage = quota > 0 ? (usage / quota) * 100 : 0;

  return { used: usage, quota, percentage };
}

export async function isStorageAvailable(): Promise<boolean> {
  const { percentage } = await getStorageUsage();
  return percentage < 95; // Warn at 80%, block at 95%
}

export async function isStorageWarning(): Promise<boolean> {
  const { percentage } = await getStorageUsage();
  return percentage >= 80 && percentage < 95;
}

// Generate next unique document name
export async function generateDocumentName(): Promise<string> {
  const docs = await listDocuments();
  const untitledDocs = docs.filter((d) => d.name.startsWith("Untitled"));

  if (untitledDocs.length === 0) return "Untitled";

  // Find the highest number
  let maxNum = 0;
  for (const doc of untitledDocs) {
    const match = doc.name.match(/^Untitled(?: (\d+))?$/);
    if (match) {
      const num = match[1] ? parseInt(match[1], 10) : 1;
      maxNum = Math.max(maxNum, num);
    }
  }

  return `Untitled ${maxNum + 1}`;
}

// Get the most recently modified document
export async function getMostRecentDocument(): Promise<DocumentMeta | null> {
  const docs = await listDocuments();
  return docs[0] ?? null;
}

// Release all locks owned by this tab (call on unload)
export async function releaseAllLocks(): Promise<void> {
  const db = await getDb();
  const locks = await db.getAll(LOCKS_STORE);

  for (const lock of locks) {
    if (lock.tabId === TAB_ID) {
      await db.delete(LOCKS_STORE, lock.documentId);
    }
  }
}

// Setup lock cleanup on tab close
if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", () => {
    // Can't await in beforeunload, so we do best-effort sync
    releaseAllLocks().catch(() => {});
  });
}
