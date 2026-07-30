# Cloud Document Selection

Browse and open cloud documents without full sync.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `m` |

> Verified against repo 2026-07-30. State corrected `in-progress` → `shipped`: `listCloudDocuments()`/`fetchCloudDocument()` live in `packages/auth/src/sync.ts` and are wired into `packages/app/src/components/DocumentPicker.tsx` (cloud-only rows, download-on-open, offline fallback).

## Problem

Currently, cloud documents are only accessible after a full sync. Users need to:
1. Wait for sync to complete before seeing all their documents
2. Have no visibility into cloud-only documents (created on other devices)
3. Cannot selectively download individual documents

## Solution

Add lightweight cloud document browsing to the DocumentPicker:

1. **Metadata-only fetch** — New `listCloudDocuments()` function fetches document metadata (no content)
2. **On-demand download** — New `fetchCloudDocument()` downloads a single document when opened
3. **Unified view** — DocumentPicker shows both local and cloud documents with sync status indicators

### Unified Document Model

```typescript
interface UnifiedDocumentMeta {
  id: string;           // Local ID or cloud ID if cloud-only
  cloudId?: string;     // Cloud ID if synced
  localId?: string;     // Local ID if exists locally
  name: string;
  modifiedAt: number;
  source: "local" | "cloud" | "both";
  syncStatus: "local" | "synced" | "pending" | "cloud-only";
}
```

### Document States

| State | Icon | Behavior |
|-------|------|----------|
| `local` | `HardDrives` | Local only, not synced |
| `synced` | `Cloud` (accent) | Exists locally and in cloud, in sync |
| `pending` | `Cloud` (warning) | Local changes pending upload |
| `cloud-only` | `CloudArrowDown` | Only in cloud, needs download |

**Not included:** Multi-select download, background prefetch, offline caching of metadata.

## UX Details

### Opening DocumentPicker

1. Immediately show local documents (instant)
2. If signed in, fetch cloud metadata in parallel
3. Merge results into unified list
4. Show loading indicator while fetching cloud metadata

### Opening a Cloud-Only Document

1. User clicks a cloud-only document
2. Show "Downloading..." state on the row
3. Fetch full document content
4. Save to IndexedDB with `syncStatus: "synced"`
5. Open document in editor

### Offline Behavior

- Show local documents only
- Gray out "Cloud" column/indicator
- Show subtle "Offline" indicator

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/auth/src/sync.ts` | Add `listCloudDocuments()`, `fetchCloudDocument()` |
| `packages/auth/src/index.ts` | Export new functions |
| `packages/app/src/components/DocumentPicker.tsx` | Major update for unified view |

### New API Functions

```typescript
// sync.ts

/**
 * Lightweight metadata fetch (no content).
 * Returns documents sorted by modified date, newest first.
 */
export async function listCloudDocuments(): Promise<CloudDocumentMeta[]>

interface CloudDocumentMeta {
  id: string;         // Cloud document ID
  local_id: string;   // Original local ID
  name: string;
  device_modified_at: number;
  created_at: string;
  updated_at: string;
}

/**
 * Fetch a single document from cloud and save locally.
 * Returns the local document ID.
 */
export async function fetchCloudDocument(cloudId: string): Promise<string>
```

### DocumentPicker Changes

1. Add state for cloud documents and loading
2. Merge local + cloud documents on open
3. De-duplicate by `local_id` (prefer local if exists)
4. Add download handler for cloud-only documents
5. Add offline detection and fallback

## Tasks

### Phase 1: API Functions (`s`)

- [x] Add `listCloudDocuments()` to `sync.ts` (metadata-only query)
- [x] Add `fetchCloudDocument()` for on-demand download
- [x] Export functions from `@vcad/auth`

### Phase 2: DocumentPicker Update (`m`)

- [x] Add cloud document state and loading indicator
- [x] Implement document merging logic
- [x] Update `DocumentRow` to show cloud-only indicator
- [x] Add download-on-open handler for cloud-only docs
- [x] Handle loading state during download

### Phase 3: Offline Handling (`xs`)

- [x] Detect offline state
- [x] Graceful fallback to local-only view
- [ ] Show offline indicator

## Acceptance Criteria

- [ ] Opening DocumentPicker while signed in shows both local and cloud documents
- [ ] Cloud-only documents show download icon
- [ ] Clicking a cloud-only document downloads and opens it
- [ ] Download shows loading state
- [ ] Going offline shows local documents only
- [ ] Documents are correctly de-duplicated (no duplicates when synced)

## Future Enhancements

- [ ] Multi-select download (download several at once)
- [ ] Background prefetch of recent cloud documents
- [ ] Search/filter in document list
- [ ] View mode tabs (All / Local / Cloud)
- [ ] Document thumbnails from cloud
