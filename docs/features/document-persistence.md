# Document Persistence

Save and load .vcad files with offline-first sync.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Shipped: IndexedDB storage (`packages/app/src/lib/storage.ts`) plus Supabase cloud sync (`packages/auth/src/sync.ts`).

## Problem

Users need their work saved reliably, even when offline. Traditional CAD tools either:

1. **Require manual saves** - Users lose work when forgetting to save or on unexpected crashes
2. **Depend on cloud connectivity** - No access to files without internet
3. **Lock files to single devices** - Can't work on multiple machines or browser tabs

For a web-based CAD tool, these problems are amplified:
- Browser tabs can close unexpectedly
- Network connections are unreliable
- Users expect modern apps to "just save" automatically
- Multiple tabs open can corrupt data without coordination

## Solution

Offline-first document storage with automatic persistence:

### Document Format

`.vcad` files are JSON containing the complete parametric model:

```json
{
  "name": "My Part",
  "parts": [
    {
      "id": "part-1",
      "name": "Main Body",
      "ops": [
        { "type": "box", "width": 100, "height": 50, "depth": 25 },
        { "type": "translate", "ref": 0, "x": 0, "y": 0, "z": 10 }
      ]
    }
  ],
  "instances": [...],
  "joints": [...],
  "materials": {...},
  "sketches": [...]
}
```

**Contents:**
- **Parametric DAG** - Operations reference parent indices
- **Part definitions** - Named parts with operation sequences
- **Instances** - Part placements with transforms
- **Joints** - Kinematic relationships between instances
- **Material assignments** - Per-part or per-face materials
- **Sketches with constraints** - 2D geometry and solver constraints

### Storage Architecture

```
Browser                          Cloud (future)
  |                                   |
  v                                   v
IndexedDB -------- sync --------> Cloud Storage
  |                                   |
  +-- documents store                 |
  |   +-- id, name, document          |
  |   +-- createdAt, modifiedAt       |
  |   +-- version, syncStatus         |
  |   +-- cloudId, thumbnail          |
  |                                   |
  +-- locks store                     |
      +-- documentId, tabId           |
      +-- acquiredAt                  |
```

### Features

| Feature | Description |
|---------|-------------|
| **Auto-save** | Saves on every change, no manual action needed |
| **IndexedDB storage** | Works offline, persists across sessions |
| **Multi-tab coordination** | Lock system prevents concurrent editing conflicts |
| **Document listing** | Browse recent documents sorted by modification date |
| **File export/import** | Share .vcad files via download/upload |
| **Storage quota awareness** | Warns at 80%, blocks at 95% capacity |

## UX Details

### Document Lifecycle

| Action | Behavior |
|--------|----------|
| New document | Auto-generates "Untitled N" name, saves immediately |
| Edit | Auto-saves on every change |
| Rename | Updates name in storage, reflects in document list |
| Delete | Removes from IndexedDB with confirmation |
| Export | Downloads .vcad JSON file |
| Import | Loads from uploaded file into new document |

### Multi-Tab Behavior

| Scenario | Handling |
|----------|----------|
| Same document in two tabs | Second tab shows "locked" warning, read-only |
| Tab closes | Lock auto-releases (30s stale timeout for crashed tabs) |
| Background tab | Lock refreshed periodically to maintain ownership |

### Storage States

| State | UI Indicator |
|-------|--------------|
| `local` | Saved locally, not synced to cloud |
| `pending` | Changes waiting for cloud sync |
| `synced` | Fully synchronized with cloud |

### Edge Cases

- **Storage full**: Shows warning dialog, suggests deleting old documents
- **IndexedDB unavailable**: Falls back to session-only mode with warning
- **Corrupted document**: Parser validates JSON, shows error on invalid format
- **Large documents**: No size limit, but performance degrades over ~10MB

## Implementation

### Files

| File | Purpose |
|------|---------|
| `packages/app/src/lib/storage.ts` | IndexedDB operations, locking, document CRUD |
| `packages/auth/src/stores/sync-store.ts` | Sync status state management |
| `packages/core/src/index.ts` | `VcadFile` type definition |

### Key Types

```typescript
// storage.ts
interface StoredDocument {
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

interface DocumentLock {
  documentId: string;
  tabId: string;
  acquiredAt: number;
}
```

### Storage API

```typescript
// Core operations
saveDocument(id, name, vcadFile, thumbnail?): Promise<void>
loadDocument(id): Promise<StoredDocument | null>
listDocuments(): Promise<DocumentMeta[]>
deleteDocument(id): Promise<void>
renameDocument(id, name): Promise<void>

// Multi-tab locking
acquireLock(documentId): Promise<boolean>
releaseLock(documentId): Promise<void>
refreshLock(documentId): Promise<boolean>
isDocumentLocked(documentId): Promise<boolean>

// Storage management
getStorageUsage(): Promise<{ used, quota, percentage }>
isStorageAvailable(): Promise<boolean>
isStorageWarning(): Promise<boolean>
generateDocumentName(): Promise<string>
```

### Sync Store

```typescript
// sync-store.ts
interface SyncState {
  syncStatus: "idle" | "syncing" | "synced" | "error";
  lastSyncAt: number | null;
  pendingCount: number;
  error: string | null;
}
```

## Tasks

### Completed

- [x] Define `VcadFile` JSON schema (`xs`)
- [x] IndexedDB database setup with versioning (`s`)
- [x] Document CRUD operations (`s`)
- [x] Document listing sorted by modification date (`xs`)
- [x] Multi-tab locking with stale timeout (`s`)
- [x] Lock cleanup on tab unload (`xs`)
- [x] Storage quota monitoring (`xs`)
- [x] Auto-save on document changes (`s`)
- [x] Sync store for status tracking (`xs`)
- [x] Document name generation ("Untitled N") (`xs`)

## Acceptance Criteria

- [x] Documents persist across browser sessions
- [x] Auto-save triggers on every edit
- [x] Document list shows all saved documents sorted by recency
- [x] Multi-tab locking prevents concurrent edits
- [x] Stale locks auto-release after 30 seconds
- [x] Storage warnings appear at 80% capacity
- [x] Documents can be renamed and deleted
- [x] Works fully offline (no network required)
- [x] Exported .vcad files can be re-imported

## Future Enhancements

- [ ] Cloud sync with authenticated storage
- [ ] Document versioning and history
- [ ] Real-time collaboration (multiple users editing)
- [ ] Automatic cloud backup
- [ ] Conflict resolution for concurrent edits
- [ ] Document sharing via shareable links
- [ ] Thumbnail generation for document previews
- [ ] Import from cloud storage providers (Google Drive, Dropbox)
