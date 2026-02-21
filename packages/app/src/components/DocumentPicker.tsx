import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { File } from "@phosphor-icons/react/dist/ssr/File";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Pencil } from "@phosphor-icons/react/dist/ssr/Pencil";
import { CloudSlash } from "@phosphor-icons/react/dist/ssr/CloudSlash";
import { Cloud } from "@phosphor-icons/react/dist/ssr/Cloud";
import { CloudArrowDown } from "@phosphor-icons/react/dist/ssr/CloudArrowDown";
import { HardDrives } from "@phosphor-icons/react/dist/ssr/HardDrives";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { WifiSlash } from "@phosphor-icons/react/dist/ssr/WifiSlash";
import { cn } from "@/lib/utils";
import { useEffect, useState, useCallback } from "react";
import { useDocumentStore } from "@vcad/core";
import {
  useAuthStore,
  listCloudDocuments,
  fetchCloudDocument,
  type CloudDocumentMeta,
} from "@vcad/auth";
import { useNotificationStore } from "@/stores/notification-store";
import {
  listDocuments,
  loadDocument,
  deleteDocument as deleteDocumentFromDb,
  renameDocument as renameDocumentInDb,
  generateDocumentName,
  getStorageUsage,
  isDocumentLocked,
} from "@/lib/storage";

function formatDate(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  // Within last hour
  if (diff < 60 * 60 * 1000) {
    const mins = Math.floor(diff / 60000);
    return mins <= 1 ? "Just now" : `${mins}m ago`;
  }

  // Within last day
  if (diff < 24 * 60 * 60 * 1000) {
    const hours = Math.floor(diff / (60 * 60 * 1000));
    return `${hours}h ago`;
  }

  // Within last week
  if (diff < 7 * 24 * 60 * 60 * 1000) {
    const days = Math.floor(diff / (24 * 60 * 60 * 1000));
    return `${days}d ago`;
  }

  // Otherwise show date
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

/**
 * Unified document metadata combining local and cloud documents.
 */
interface UnifiedDocumentMeta {
  id: string;           // Local ID or cloud ID if cloud-only
  cloudId?: string;     // Cloud ID if synced
  localId?: string;     // Local ID if exists locally
  name: string;
  modifiedAt: number;
  source: "local" | "cloud" | "both";
  syncStatus: "local" | "synced" | "pending" | "cloud-only";
}

interface DocumentRowProps {
  doc: UnifiedDocumentMeta;
  isSelected: boolean;
  isLocked: boolean;
  isDownloading: boolean;
  onSelect: () => void;
  onOpen: () => void;
  onDelete: () => void;
  onRename: (name: string) => void;
}

function DocumentRow({
  doc,
  isSelected,
  isLocked,
  isDownloading,
  onSelect,
  onOpen,
  onDelete,
  onRename,
}: DocumentRowProps) {
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(doc.name);

  const handleDoubleClick = useCallback(() => {
    if (!isLocked && !isDownloading) {
      onOpen();
    }
  }, [isLocked, isDownloading, onOpen]);

  const handleRename = useCallback(() => {
    if (editName.trim() && editName !== doc.name) {
      onRename(editName.trim());
    }
    setIsEditing(false);
  }, [editName, doc.name, onRename]);

  // Cloud-only documents can't be edited or deleted until downloaded
  const isCloudOnly = doc.syncStatus === "cloud-only";

  return (
    <div
      className={cn(
        "group flex items-center gap-3 px-3 py-2 cursor-pointer border border-transparent",
        "hover:bg-surface/50",
        isSelected && "bg-accent/10 border-accent/30",
        isDownloading && "opacity-60"
      )}
      onClick={onSelect}
      onDoubleClick={handleDoubleClick}
    >
      <File size={16} weight="regular" className="text-text-muted shrink-0" />

      <div className="flex-1 min-w-0">
        {isEditing ? (
          <input
            type="text"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onBlur={handleRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleRename();
              if (e.key === "Escape") setIsEditing(false);
            }}
            className="w-full bg-transparent border-b border-accent text-sm text-text outline-none"
            autoFocus
          />
        ) : (
          <div className="text-sm text-text truncate">{doc.name}</div>
        )}
        <div className="text-[10px] text-text-muted">
          {formatDate(doc.modifiedAt)}
        </div>
      </div>

      <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
        {isDownloading ? (
          <span className="text-[10px] text-text-muted px-2">Downloading...</span>
        ) : isLocked ? (
          <span className="text-[10px] text-text-muted px-2">In use</span>
        ) : isCloudOnly ? (
          <span className="text-[10px] text-text-muted px-2">Click to download</span>
        ) : (
          <>
            <button
              onClick={(e) => {
                e.stopPropagation();
                setEditName(doc.name);
                setIsEditing(true);
              }}
              className="p-1 text-text-muted hover:text-text hover:bg-border/50 transition-colors"
              title="Rename"
            >
              <Pencil size={14} />
            </button>
            <button
              onClick={(e) => {
                e.stopPropagation();
                onDelete();
              }}
              className="p-1 text-text-muted hover:text-danger hover:bg-border/50 transition-colors"
              title="Delete"
            >
              <Trash size={14} />
            </button>
          </>
        )}
      </div>

      <div className="text-text-muted shrink-0" title={
        doc.syncStatus === "synced" ? "Synced" :
        doc.syncStatus === "pending" ? "Pending sync" :
        doc.syncStatus === "cloud-only" ? "Cloud only (click to download)" : "Local only"
      }>
        {isDownloading ? (
          <SpinnerGap size={14} className="text-accent animate-spin" />
        ) : doc.syncStatus === "synced" ? (
          <Cloud size={14} className="text-accent" />
        ) : doc.syncStatus === "pending" ? (
          <Cloud size={14} className="text-warning" />
        ) : doc.syncStatus === "cloud-only" ? (
          <CloudArrowDown size={14} className="text-accent" />
        ) : (
          <CloudSlash size={14} />
        )}
      </div>
    </div>
  );
}

export function DocumentPicker({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [documents, setDocuments] = useState<UnifiedDocumentMeta[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [lockedIds, setLockedIds] = useState<Set<string>>(new Set());
  const [downloadingIds, setDownloadingIds] = useState<Set<string>>(new Set());
  const [storageUsage, setStorageUsage] = useState({ used: 0, quota: 0, percentage: 0 });
  const [loading, setLoading] = useState(true);
  const [loadingCloud, setLoadingCloud] = useState(false);
  const [isOffline, setIsOffline] = useState(!navigator.onLine);

  const user = useAuthStore((s) => s.user);
  const currentDocId = useDocumentStore((s) => s.documentId);

  // Track online/offline status
  useEffect(() => {
    const handleOnline = () => setIsOffline(false);
    const handleOffline = () => setIsOffline(true);
    window.addEventListener("online", handleOnline);
    window.addEventListener("offline", handleOffline);
    return () => {
      window.removeEventListener("online", handleOnline);
      window.removeEventListener("offline", handleOffline);
    };
  }, []);

  const loadDocuments = useCallback(async () => {
    setLoading(true);
    try {
      // Always load local documents first (fast)
      const localDocs = await listDocuments();

      // Convert to unified format
      const unifiedDocs: UnifiedDocumentMeta[] = localDocs.map((d) => ({
        id: d.id,
        cloudId: d.cloudId,
        localId: d.id,
        name: d.name,
        modifiedAt: d.modifiedAt,
        source: d.cloudId ? "both" : "local",
        syncStatus: d.syncStatus,
      }));

      setDocuments(unifiedDocs);

      // Check which docs are locked
      const locked = new Set<string>();
      for (const doc of localDocs) {
        if (await isDocumentLocked(doc.id)) {
          locked.add(doc.id);
        }
      }
      setLockedIds(locked);

      // Get storage usage
      const usage = await getStorageUsage();
      setStorageUsage(usage);

      // If signed in and online, also fetch cloud documents
      if (user && !isOffline) {
        setLoadingCloud(true);
        try {
          const cloudDocs = await listCloudDocuments();

          // Merge cloud documents with local
          mergeCloudDocuments(unifiedDocs, cloudDocs, setDocuments);
        } catch (err) {
          console.error("Failed to load cloud documents:", err);
          // Don't fail the whole load, just skip cloud docs
        } finally {
          setLoadingCloud(false);
        }
      }
    } catch (err) {
      console.error("Failed to load documents:", err);
    } finally {
      setLoading(false);
    }
  }, [user, isOffline]);

  useEffect(() => {
    if (open) {
      loadDocuments();
    }
  }, [open, loadDocuments]);

  const handleNewDocument = useCallback(async () => {
    const name = await generateDocumentName();
    const id = crypto.randomUUID();
    useDocumentStore.getState().newDocument(id, name);
    onOpenChange(false);
    useNotificationStore.getState().addToast(`Created "${name}"`, "success");
  }, [onOpenChange]);

  const handleOpenDocument = useCallback(async () => {
    if (!selectedId) return;

    const doc = documents.find((d) => d.id === selectedId);
    if (!doc) return;

    // Cloud-only document - download first
    if (doc.syncStatus === "cloud-only" && doc.cloudId) {
      setDownloadingIds((prev) => new Set(prev).add(selectedId));
      try {
        const localId = await fetchCloudDocument(doc.cloudId);

        // Update the document in our list
        setDocuments((prev) =>
          prev.map((d) =>
            d.id === selectedId
              ? { ...d, id: localId, localId, syncStatus: "synced" as const, source: "both" as const }
              : d
          )
        );

        // Now open it
        const stored = await loadDocument(localId);
        if (stored) {
          useDocumentStore.getState().loadDocument(stored.document);
          useDocumentStore.getState().setDocumentMeta(stored.id, stored.name);
          onOpenChange(false);
        }
      } catch (err) {
        console.error("Failed to download document:", err);
        useNotificationStore.getState().addToast("Failed to download document", "error");
      } finally {
        setDownloadingIds((prev) => {
          const next = new Set(prev);
          next.delete(selectedId);
          return next;
        });
      }
      return;
    }

    // Local document
    const isLocked = await isDocumentLocked(selectedId);
    if (isLocked) {
      useNotificationStore.getState().addToast(
        "Document is open in another tab",
        "error"
      );
      return;
    }

    try {
      const stored = await loadDocument(selectedId);
      if (!stored) {
        useNotificationStore.getState().addToast("Document not found", "error");
        return;
      }

      useDocumentStore.getState().loadDocument(stored.document);
      useDocumentStore.getState().setDocumentMeta(stored.id, stored.name);
      onOpenChange(false);
    } catch (err) {
      console.error("Failed to open document:", err);
      useNotificationStore.getState().addToast("Failed to open document", "error");
    }
  }, [selectedId, documents, onOpenChange]);

  const handleDeleteDocument = useCallback(async (id: string) => {
    const doc = documents.find((d) => d.id === id);

    // Can't delete cloud-only documents
    if (doc?.syncStatus === "cloud-only") {
      useNotificationStore.getState().addToast(
        "Download the document first to delete it",
        "error"
      );
      return;
    }

    if (id === currentDocId) {
      useNotificationStore.getState().addToast(
        "Cannot delete the current document",
        "error"
      );
      return;
    }

    try {
      await deleteDocumentFromDb(id);
      setDocuments((prev) => prev.filter((d) => d.id !== id));
      if (selectedId === id) {
        setSelectedId(null);
      }
      useNotificationStore.getState().addToast("Document deleted", "success");
    } catch (err) {
      console.error("Failed to delete document:", err);
      useNotificationStore.getState().addToast("Failed to delete document", "error");
    }
  }, [selectedId, currentDocId, documents]);

  const handleRenameDocument = useCallback(async (id: string, name: string) => {
    const doc = documents.find((d) => d.id === id);

    // Can't rename cloud-only documents
    if (doc?.syncStatus === "cloud-only") {
      useNotificationStore.getState().addToast(
        "Download the document first to rename it",
        "error"
      );
      return;
    }

    try {
      await renameDocumentInDb(id, name);
      setDocuments((prev) =>
        prev.map((d) => (d.id === id ? { ...d, name } : d))
      );

      // If this is the current doc, update the store too
      if (id === currentDocId) {
        useDocumentStore.getState().setDocumentMeta(id, name);
      }
    } catch (err) {
      console.error("Failed to rename document:", err);
      useNotificationStore.getState().addToast("Failed to rename document", "error");
    }
  }, [currentDocId, documents]);

  const selectedDoc = documents.find((d) => d.id === selectedId);
  const isSelectedCloudOnly = selectedDoc?.syncStatus === "cloud-only";
  const isSelectedDownloading = selectedId ? downloadingIds.has(selectedId) : false;

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/60" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-md -translate-x-1/2 -translate-y-1/2",
            "border border-border bg-card shadow-2xl",
            "max-h-[85vh] flex flex-col",
            "focus:outline-none"
          )}
        >
          {/* Header */}
          <div className="flex items-center justify-between p-4 border-b border-border">
            <div className="flex items-center gap-2">
              <Dialog.Title className="text-sm font-bold text-text">
                Documents
              </Dialog.Title>
              {loadingCloud && (
                <SpinnerGap size={12} className="text-accent animate-spin" />
              )}
              {isOffline && user && (
                <div className="flex items-center gap-1 text-[10px] text-text-muted" title="Offline - showing local documents only">
                  <WifiSlash size={12} />
                  Offline
                </div>
              )}
            </div>
            <Dialog.Close className="p-1 text-text-muted hover:bg-border/50 hover:text-text transition-colors cursor-pointer">
              <X size={16} />
            </Dialog.Close>
          </div>

          {/* Actions */}
          <div className="p-3 border-b border-border">
            <button
              onClick={handleNewDocument}
              className="flex items-center gap-2 w-full px-3 py-2 text-sm text-text hover:bg-surface/50 transition-colors"
            >
              <Plus size={16} className="text-accent" />
              New Document
            </button>
          </div>

          {/* Document list */}
          <div className="flex-1 overflow-y-auto min-h-[200px]">
            {loading ? (
              <div className="flex items-center justify-center h-full text-text-muted text-sm">
                Loading...
              </div>
            ) : documents.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full gap-2 text-text-muted">
                <File size={32} weight="thin" />
                <div className="text-sm">No saved documents</div>
                <div className="text-[10px]">Create a new document to get started</div>
              </div>
            ) : (
              <div className="divide-y divide-border/50">
                {documents.map((doc) => (
                  <DocumentRow
                    key={doc.id}
                    doc={doc}
                    isSelected={selectedId === doc.id}
                    isLocked={lockedIds.has(doc.id)}
                    isDownloading={downloadingIds.has(doc.id)}
                    onSelect={() => setSelectedId(doc.id)}
                    onOpen={handleOpenDocument}
                    onDelete={() => handleDeleteDocument(doc.id)}
                    onRename={(name) => handleRenameDocument(doc.id, name)}
                  />
                ))}
              </div>
            )}
          </div>

          {/* Footer */}
          <div className="flex items-center justify-between p-3 border-t border-border bg-surface/30">
            <div className="flex items-center gap-2 text-[10px] text-text-muted">
              <HardDrives size={14} />
              <span>
                {formatBytes(storageUsage.used)} / {formatBytes(storageUsage.quota)}
              </span>
              {storageUsage.percentage >= 80 && (
                <span className="text-warning">({storageUsage.percentage.toFixed(0)}%)</span>
              )}
            </div>
            <button
              onClick={handleOpenDocument}
              disabled={!selectedId || lockedIds.has(selectedId) || isSelectedDownloading}
              className={cn(
                "px-4 py-1.5 text-xs font-medium transition-colors",
                selectedId && !lockedIds.has(selectedId) && !isSelectedDownloading
                  ? "bg-accent text-white hover:bg-accent/90"
                  : "bg-border text-text-muted cursor-not-allowed"
              )}
            >
              {isSelectedCloudOnly ? "Download & Open" : "Open"}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * Merge cloud documents into the unified document list.
 * De-duplicates by cloud ID and adds cloud-only documents.
 */
function mergeCloudDocuments(
  currentDocs: UnifiedDocumentMeta[],
  cloudDocs: CloudDocumentMeta[],
  setDocuments: React.Dispatch<React.SetStateAction<UnifiedDocumentMeta[]>>
) {
  // Create a map of existing cloud IDs
  const existingCloudIds = new Set(
    currentDocs.filter((d) => d.cloudId).map((d) => d.cloudId)
  );

  // Find cloud-only documents
  const cloudOnlyDocs: UnifiedDocumentMeta[] = cloudDocs
    .filter((c) => !existingCloudIds.has(c.id))
    .map((c) => ({
      id: c.id, // Use cloud ID as the document ID
      cloudId: c.id,
      localId: undefined,
      name: c.name,
      modifiedAt: c.device_modified_at,
      source: "cloud" as const,
      syncStatus: "cloud-only" as const,
    }));

  if (cloudOnlyDocs.length > 0) {
    setDocuments((prev) => {
      // Combine and sort by modified date
      const combined = [...prev, ...cloudOnlyDocs];
      combined.sort((a, b) => b.modifiedAt - a.modifiedAt);
      return combined;
    });
  }
}
