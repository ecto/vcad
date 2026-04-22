import { requireSupabase, isAuthEnabled } from "./client";
import { useAuthStore } from "./stores/auth-store";
import { useSyncStore } from "./stores/sync-store";

/**
 * Cloud document shape as stored in Supabase
 */
export interface CloudDocument {
  id: string;
  local_id: string;
  name: string;
  content: unknown;
  version: number;
  device_modified_at: number;
  is_public: boolean;
  created_at: string;
  updated_at: string;
}

/**
 * Lightweight cloud document metadata (no content).
 * Used for listing documents without downloading full content.
 */
export interface CloudDocumentMeta {
  id: string;
  local_id: string;
  name: string;
  device_modified_at: number;
  created_at: string;
  updated_at: string;
}

/**
 * Interface for local document storage.
 * This should be implemented by the app's storage module.
 */
export interface StorageAdapter {
  getAllDocuments: () => Promise<LocalDocument[]>;
  getDocument: (id: string) => Promise<LocalDocument | null>;
  saveDocument: (doc: LocalDocument) => Promise<void>;
  updateDocument: (
    id: string,
    updates: Partial<LocalDocument>
  ) => Promise<void>;
}

export interface LocalDocument {
  id: string;
  name: string;
  document: unknown;
  createdAt: number;
  modifiedAt: number;
  version: number;
  syncStatus: "local" | "synced" | "pending";
  cloudId?: string;
  thumbnail?: Blob;
}

// Storage adapter - set by the app
let storageAdapter: StorageAdapter | null = null;

/**
 * Configure the storage adapter for sync operations.
 * Call this during app initialization.
 */
export function configureStorage(adapter: StorageAdapter): void {
  storageAdapter = adapter;
}

function requireStorage(): StorageAdapter {
  if (!storageAdapter) {
    throw new Error("Storage adapter not configured. Call configureStorage()");
  }
  return storageAdapter;
}

// Debounce timer for sync
let syncDebounceTimer: ReturnType<typeof setTimeout> | null = null;

// Guard against concurrent syncs
let syncInProgress = false;

// Backoff state for error handling
let consecutiveErrors = 0;
let lastErrorTime = 0;
const MIN_ERROR_BACKOFF = 30_000; // 30s minimum between retries after error
const MAX_ERROR_BACKOFF = 300_000; // 5 min max

/**
 * Trigger a sync operation.
 * Uploads pending local documents and downloads new cloud documents.
 *
 * Safe to call frequently - operations are debounced internally.
 * Implements exponential backoff on repeated errors.
 */
export async function triggerSync(): Promise<void> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    return;
  }

  // Prevent concurrent syncs
  if (syncInProgress) {
    return;
  }

  // Exponential backoff on errors
  if (consecutiveErrors > 0) {
    const backoff = Math.min(
      MIN_ERROR_BACKOFF * Math.pow(2, consecutiveErrors - 1),
      MAX_ERROR_BACKOFF
    );
    const timeSinceError = Date.now() - lastErrorTime;
    if (timeSinceError < backoff) {
      return;
    }
  }

  syncInProgress = true;

  const { setSyncStatus, setLastSyncAt, setError, setPendingCount } =
    useSyncStore.getState();

  setSyncStatus("syncing");
  setError(null);

  try {
    // 1. Upload pending local documents
    await uploadPendingDocuments();

    // 2. Download new/updated cloud documents
    await downloadCloudDocuments();

    // Update pending count
    const storage = requireStorage();
    const docs = await storage.getAllDocuments();
    const pending = docs.filter((d) => d.syncStatus === "pending").length;
    setPendingCount(pending);

    setSyncStatus("synced");
    setLastSyncAt(Date.now());

    // Reset error state on success
    consecutiveErrors = 0;
  } catch (error) {
    console.error("Sync failed:", error);
    setSyncStatus("error");
    setError((error as Error).message);

    // Track consecutive errors for backoff
    consecutiveErrors++;
    lastErrorTime = Date.now();
  } finally {
    syncInProgress = false;
  }
}

/**
 * Debounced sync trigger - waits 5 seconds after last call before syncing.
 * Use this for auto-sync on document changes.
 */
export function debouncedSync(delay = 5000): void {
  if (syncDebounceTimer) {
    clearTimeout(syncDebounceTimer);
  }
  syncDebounceTimer = setTimeout(() => {
    syncDebounceTimer = null;
    triggerSync();
  }, delay);
}

/**
 * Upload documents where syncStatus='pending'
 */
async function uploadPendingDocuments(): Promise<void> {
  const storage = requireStorage();
  const localDocs = await storage.getAllDocuments();
  const pendingDocs = localDocs.filter((d) => d.syncStatus === "pending");

  for (const doc of pendingDocs) {
    await uploadDocument(doc);
  }
}

/**
 * Upload a single document to cloud
 */
async function uploadDocument(doc: LocalDocument): Promise<void> {
  const supabase = requireSupabase();
  const storage = requireStorage();
  const { user } = useAuthStore.getState();
  if (!user) throw new Error("Not signed in");

  // Check if document already exists in cloud
  const { data: existing } = await supabase
    .from("documents")
    .select("id, version, device_modified_at")
    .eq("local_id", doc.id)
    .maybeSingle();

  if (existing) {
    // Document exists in cloud - check for conflict
    if (existing.device_modified_at > doc.modifiedAt) {
      // Cloud is newer - conflict! For now, last-write-wins (cloud).
      // TODO(#13): prompt user or hand off to CRDT merge strategy.
      console.warn(
        `Conflict on ${doc.id}: cloud is newer, keeping cloud version`
      );
      return;
    }

    // Local is newer - update cloud
    const { error } = await supabase
      .from("documents")
      .update({
        name: doc.name,
        content: doc.document,
        version: doc.version,
        device_modified_at: doc.modifiedAt,
      })
      .eq("id", existing.id);

    if (error) throw error;
  } else {
    // New document - insert. Explicitly set user_id so the RLS policy
    // passes even if PostgREST hasn't picked up the auth.uid() default.
    const { data, error } = await supabase
      .from("documents")
      .insert({
        user_id: user.id,
        local_id: doc.id,
        name: doc.name,
        content: doc.document,
        version: doc.version,
        device_modified_at: doc.modifiedAt,
      })
      .select("id")
      .single();

    if (error) throw error;

    // Store cloud ID in local doc
    if (data) {
      await storage.updateDocument(doc.id, {
        cloudId: data.id,
      });
    }
  }

  // Mark as synced
  await storage.updateDocument(doc.id, { syncStatus: "synced" });
}

/**
 * Download documents from cloud, merge into local
 */
async function downloadCloudDocuments(): Promise<void> {
  const supabase = requireSupabase();
  const storage = requireStorage();

  const { data: cloudDocs, error } = await supabase
    .from("documents")
    .select("*");

  if (error) throw error;
  if (!cloudDocs) return;

  const localDocs = await storage.getAllDocuments();
  const localByLocalId = new Map(localDocs.map((d) => [d.id, d]));

  for (const cloudDoc of cloudDocs as CloudDocument[]) {
    const localDoc = localByLocalId.get(cloudDoc.local_id);

    if (!localDoc) {
      // New document from cloud - create locally
      await createDocumentFromCloud(cloudDoc);
    } else if (cloudDoc.device_modified_at > localDoc.modifiedAt) {
      // Cloud is newer - update local
      await updateDocumentFromCloud(localDoc.id, cloudDoc);
    }
    // If local is newer, uploadPendingDocuments handles it
  }
}

/**
 * Create a new local document from cloud data
 */
async function createDocumentFromCloud(cloudDoc: CloudDocument): Promise<void> {
  const storage = requireStorage();

  const newDoc: LocalDocument = {
    id: cloudDoc.local_id,
    name: cloudDoc.name,
    document: cloudDoc.content,
    createdAt: new Date(cloudDoc.created_at).getTime(),
    modifiedAt: cloudDoc.device_modified_at,
    version: cloudDoc.version,
    syncStatus: "synced",
    cloudId: cloudDoc.id,
  };

  await storage.saveDocument(newDoc);
}

/**
 * Update local document from cloud data
 */
async function updateDocumentFromCloud(
  localId: string,
  cloudDoc: CloudDocument
): Promise<void> {
  const storage = requireStorage();

  await storage.updateDocument(localId, {
    name: cloudDoc.name,
    document: cloudDoc.content,
    modifiedAt: cloudDoc.device_modified_at,
    version: cloudDoc.version,
    syncStatus: "synced",
    cloudId: cloudDoc.id,
  });
}

/**
 * Enable cloud sync for a local-only document.
 * Marks the document as pending and triggers sync.
 * Resets any error backoff so the sync runs immediately.
 */
export async function enableCloudSync(documentId: string): Promise<void> {
  const storage = requireStorage();
  await storage.updateDocument(documentId, { syncStatus: "pending" });
  // Reset backoff so the explicit sync request isn't blocked by earlier errors.
  consecutiveErrors = 0;
  lastErrorTime = 0;
  await triggerSync();
}

/**
 * Initialize sync listeners for automatic sync.
 * Call this during app initialization.
 */
export function initSyncListeners(): void {
  // Sync when window gains focus
  window.addEventListener("focus", () => {
    const { user } = useAuthStore.getState();
    if (user) {
      triggerSync();
    }
  });

  // Sync when network comes back online
  window.addEventListener("online", () => {
    const { user } = useAuthStore.getState();
    if (user) {
      triggerSync();
    }
  });

  // Sync when visibility changes (e.g., switching tabs back)
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      const { user } = useAuthStore.getState();
      if (user) {
        // Use debounced sync to avoid rapid-fire syncs
        debouncedSync(1000);
      }
    }
  });
}

/**
 * List cloud documents (metadata only, no content).
 * Returns documents sorted by modified date, newest first.
 *
 * Use this for browsing documents without downloading full content.
 */
export async function listCloudDocuments(): Promise<CloudDocumentMeta[]> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    return [];
  }

  const supabase = requireSupabase();

  const { data, error } = await supabase
    .from("documents")
    .select("id, local_id, name, device_modified_at, created_at, updated_at")
    .order("device_modified_at", { ascending: false });

  if (error) throw error;

  return (data ?? []) as CloudDocumentMeta[];
}

// ---------------------------------------------------------------------------
// Share links (Phase 0)
// ---------------------------------------------------------------------------

/** A row in document_shares — pointer only, no duplicated doc state. */
export interface ShareRecord {
  token: string;
  document_id: string;
  created_at: string;
}

/** Safe public fields returned by the get_shared_document() RPC. */
export interface SharedDocumentResult {
  id: string;
  name: string;
  content: unknown; // VcadFile JSON — validated with parseVcadFile() in the app
  version: number;
  updated_at: string;
}

/**
 * Create a public read-only share link for a cloud-synced document.
 * Returns the inserted row. Requires the user to be signed in and own the doc.
 */
export async function createShare(
  cloudDocumentId: string,
): Promise<ShareRecord> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    throw new Error("Must be signed in to create a share");
  }
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("document_shares")
    .insert({ document_id: cloudDocumentId, created_by: user.id })
    .select("token, document_id, created_at")
    .single();
  if (error) throw error;
  if (!data) throw new Error("Share creation returned no row");
  return data as ShareRecord;
}

/**
 * Revoke a share link by its token. RLS ensures only the owner can delete.
 */
export async function revokeShare(token: string): Promise<void> {
  if (!isAuthEnabled()) return;
  const supabase = requireSupabase();
  const { error } = await supabase
    .from("document_shares")
    .delete()
    .eq("token", token);
  if (error) throw error;
}

/**
 * List all active share tokens for a cloud document owned by the current user.
 * Used by the share dialog to check for an existing share before creating one.
 */
export async function listSharesForDocument(
  cloudDocumentId: string,
): Promise<ShareRecord[]> {
  if (!isAuthEnabled()) return [];
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("document_shares")
    .select("token, document_id, created_at")
    .eq("document_id", cloudDocumentId)
    .order("created_at", { ascending: false });
  if (error) throw error;
  return (data ?? []) as ShareRecord[];
}

/**
 * Fetch a public document by its share token. Anonymous callers allowed —
 * hits the get_shared_document() SECURITY DEFINER RPC. Returns null if the
 * token is invalid or has been revoked.
 */
export async function fetchSharedDocument(
  token: string,
): Promise<SharedDocumentResult | null> {
  const supabase = requireSupabase();
  const { data, error } = await supabase.rpc("get_shared_document", {
    p_token: token,
  });
  if (error) {
    console.warn("[sync] fetchSharedDocument error:", error);
    return null;
  }
  if (!data || (Array.isArray(data) && data.length === 0)) return null;
  const row = Array.isArray(data) ? data[0] : data;
  return row as SharedDocumentResult;
}

/**
 * Fetch a single document from cloud by its cloud ID.
 * Downloads the full document content and saves it locally.
 *
 * @param cloudId The cloud document ID to fetch
 * @returns The local document ID after saving
 */
export async function fetchCloudDocument(cloudId: string): Promise<string> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    throw new Error("User not signed in");
  }

  const supabase = requireSupabase();
  const storage = requireStorage();

  // Fetch full document from cloud
  const { data: cloudDoc, error } = await supabase
    .from("documents")
    .select("*")
    .eq("id", cloudId)
    .single();

  if (error) throw error;
  if (!cloudDoc) throw new Error("Document not found");

  const doc = cloudDoc as CloudDocument;

  // Check if we already have this document locally
  const localDocs = await storage.getAllDocuments();
  const existingLocal = localDocs.find((d) => d.cloudId === cloudId);

  if (existingLocal) {
    // Update existing local document
    await storage.updateDocument(existingLocal.id, {
      name: doc.name,
      document: doc.content,
      modifiedAt: doc.device_modified_at,
      version: doc.version,
      syncStatus: "synced",
    });
    return existingLocal.id;
  }

  // Create new local document
  const newDoc: LocalDocument = {
    id: doc.local_id,
    name: doc.name,
    document: doc.content,
    createdAt: new Date(doc.created_at).getTime(),
    modifiedAt: doc.device_modified_at,
    version: doc.version,
    syncStatus: "synced",
    cloudId: doc.id,
  };

  await storage.saveDocument(newDoc);
  return newDoc.id;
}
