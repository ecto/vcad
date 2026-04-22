import { requireSupabase } from "./client";
import { triggerSync, type StorageAdapter } from "./sync";

/**
 * Version history entry from cloud storage
 */
export interface DocumentVersion {
  id: string;
  versionNumber: number;
  content: unknown;
  deviceModifiedAt: number;
  createdAt: string;
  /** Non-null when the user promoted this row to a named version. */
  label?: string | null;
  labeledAt?: string | null;
  labeledBy?: string | null;
}

/**
 * Named-version list row (subset of `DocumentVersion`, no content blob).
 * Returned by `listNamedVersions` — intentionally skips `content` so the list
 * view stays cheap for long histories.
 */
export interface NamedVersion {
  id: string;
  versionNumber: number;
  label: string;
  labeledAt: string;
  labeledBy: string | null;
  deviceModifiedAt: number;
  createdAt: string;
}

// Storage adapter - shared with sync module
let storageAdapter: StorageAdapter | null = null;

/**
 * Configure storage adapter for version history operations.
 * Usually called by the same configureStorage in sync.ts
 */
export function configureVersionHistoryStorage(adapter: StorageAdapter): void {
  storageAdapter = adapter;
}

function requireStorage(): StorageAdapter {
  if (!storageAdapter) {
    throw new Error("Storage adapter not configured");
  }
  return storageAdapter;
}

/**
 * Get version history for a cloud-synced document.
 *
 * @param cloudDocId - The Supabase document ID (not local ID)
 * @returns Array of versions, newest first
 * @throws If document is not cloud-synced or versions cannot be fetched
 */
export async function getVersionHistory(
  cloudDocId: string
): Promise<DocumentVersion[]> {
  const supabase = requireSupabase();

  const { data, error } = await supabase
    .from("document_versions")
    .select("*")
    .eq("document_id", cloudDocId)
    .order("version_number", { ascending: false });

  if (error) {
    throw new Error(`Failed to fetch version history: ${error.message}`);
  }

  return (data ?? []).map((v) => ({
    id: v.id,
    versionNumber: v.version_number,
    content: v.content,
    deviceModifiedAt: v.device_modified_at,
    createdAt: v.created_at,
    label: v.label ?? null,
    labeledAt: v.labeled_at ?? null,
    labeledBy: v.labeled_by ?? null,
  }));
}

/**
 * Attach a human-readable label to an existing auto-version row, promoting
 * it to a named version. Label cannot be empty; RPC enforces ownership.
 */
export async function labelVersion(
  versionId: string,
  label: string,
): Promise<void> {
  const supabase = requireSupabase();
  const { error } = await supabase.rpc("label_version", {
    p_version_id: versionId,
    p_label: label,
  });
  if (error) throw new Error(`Failed to label version: ${error.message}`);
}

/** Remove a named-version label. The auto-save row itself is preserved. */
export async function unlabelVersion(versionId: string): Promise<void> {
  const supabase = requireSupabase();
  const { error } = await supabase.rpc("unlabel_version", {
    p_version_id: versionId,
  });
  if (error) throw new Error(`Failed to unlabel version: ${error.message}`);
}

/**
 * List the labeled versions for a document, newest-labeled-first. Skips the
 * `content` column — callers wanting the blob should load the specific
 * version through `getVersionHistory` and find by id.
 */
export async function listNamedVersions(
  cloudDocId: string,
): Promise<NamedVersion[]> {
  const supabase = requireSupabase();
  const { data, error } = await supabase.rpc("list_named_versions", {
    p_document_id: cloudDocId,
  });
  if (error) {
    throw new Error(`Failed to fetch named versions: ${error.message}`);
  }
  return (
    data as Array<{
      id: string;
      version_number: number;
      label: string;
      labeled_by: string | null;
      labeled_at: string;
      device_modified_at: number;
      created_at: string;
    }>
  ).map((v) => ({
    id: v.id,
    versionNumber: v.version_number,
    label: v.label,
    labeledBy: v.labeled_by,
    labeledAt: v.labeled_at,
    deviceModifiedAt: v.device_modified_at,
    createdAt: v.created_at,
  }));
}

/**
 * Restore a document to a previous version.
 *
 * This updates the local document with the version's content
 * and triggers a sync to push the restored version to cloud.
 *
 * @param localDocId - The local IndexedDB document ID
 * @param version - The version to restore
 */
export async function restoreVersion(
  localDocId: string,
  version: DocumentVersion
): Promise<void> {
  const storage = requireStorage();

  // Update local document with version content
  await storage.updateDocument(localDocId, {
    document: version.content,
    modifiedAt: Date.now(),
    version: version.versionNumber + 1, // Increment to indicate change
    syncStatus: "pending", // Will sync the restored version
  });

  // Trigger sync to push restored version to cloud
  await triggerSync();
}

/**
 * Get the cloud ID for a local document.
 * Returns null if document is not synced to cloud.
 */
export async function getCloudIdForDocument(
  localDocId: string
): Promise<string | null> {
  const storage = requireStorage();
  const doc = await storage.getDocument(localDocId);
  return doc?.cloudId ?? null;
}
