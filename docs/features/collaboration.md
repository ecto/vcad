# Real-Time Cloud Collaboration

> **This document is a design spec; a real-time collaboration implementation has since shipped and diverged from it.** The shipped system uses a custom CRDT engine with Supabase Realtime broadcast channels — not Yjs as specced below. See `packages/app/src/hooks/useCollabSync.ts`, `packages/app/src/stores/collab-session-store.ts`, `packages/auth/src/sync.ts` (`joinCollabChannel`), plus XR presence (`xr-presence-store.ts`).

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |

> Verified against repo 2026-07-30. State row added; shipped via custom CRDT + Supabase Realtime rather than the Yjs architecture described below.

Technical specification for real-time collaboration in vcad using CRDT-based synchronization.

## Overview

Team adoption requires seamless collaboration. This specification outlines a CRDT-based approach using Yjs that enables real-time co-editing, offline support, and version control without the complexity of operational transforms.

## Technology Choice: Yjs (CRDT)

Yjs provides conflict-free replicated data types optimized for collaborative editing.

**Why Yjs over Operational Transforms:**

- **Offline-first architecture** — Changes apply locally, sync when connected
- **No server dependency for transforms** — Clients resolve conflicts independently
- **Battle-tested adoption** — Evernote, Proton Docs, JupyterCad, Notion
- **Multiple sync providers** — WebSocket, WebRTC, IndexedDB out of the box
- **Small wire format** — Binary encoding with efficient delta updates

**Trade-offs:**

- Slightly larger memory footprint than OT
- Eventual consistency (acceptable for CAD workflows)
- Tombstone accumulation requires periodic compaction

## Architecture

Hybrid client-server model with optional peer-to-peer capability.

```
┌─────────────────────────────────────────────────────────────────┐
│                         Client                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
│  │  Y.Doc      │──│ IndexedDB   │  │  Awareness Protocol     │ │
│  │  (local)    │  │ Provider    │  │  (presence, cursors)    │ │
│  └──────┬──────┘  └─────────────┘  └───────────┬─────────────┘ │
│         │                                       │               │
│  ┌──────┴───────────────────────────────────────┴─────────────┐│
│  │              WebSocket Provider (y-websocket)              ││
│  └──────────────────────────┬─────────────────────────────────┘│
└─────────────────────────────┼───────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Collaboration Server                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
│  │  y-websocket│  │  Persistence│  │  Permission Checker     │ │
│  │  Server     │──│  Layer      │──│  (RBAC)                 │ │
│  └─────────────┘  └──────┬──────┘  └─────────────────────────┘ │
└──────────────────────────┼──────────────────────────────────────┘
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
     ┌─────────────────┐      ┌─────────────────┐
     │   PostgreSQL    │      │    S3 / R2      │
     │   (documents,   │      │   (assets,      │
     │   versions)     │      │   exports)      │
     └─────────────────┘      └─────────────────┘
```

**Sync Providers (priority order):**

1. **IndexedDB** — Instant local persistence, enables offline
2. **WebSocket** — Primary server sync via `y-websocket`
3. **WebRTC** — Optional direct peer connections for reduced latency

## Data Model

Map vcad's document structure to Yjs shared types.

### Document Structure

```typescript
interface CollaborativeDocument {
  // Root Y.Doc
  doc: Y.Doc;

  // Parametric DAG nodes
  nodes: Y.Map<NodeId, Y.Map<string, any>>;

  // Material definitions
  materials: Y.Map<MaterialId, Y.Map<string, any>>;

  // Scene graph roots (part instances at top level)
  sceneRoots: Y.Array<InstanceId>;

  // Part instances
  instances: Y.Map<InstanceId, Y.Map<string, any>>;

  // Assembly joints
  joints: Y.Map<JointId, Y.Map<string, any>>;

  // Sketches with constraints
  sketches: Y.Map<SketchId, Y.Map<string, any>>;

  // Document metadata
  meta: Y.Map<string, any>;
}
```

### Node Representation

Each parametric node maps to a `Y.Map`:

```typescript
// Example: Cylinder node
nodes.set('node-abc123', new Y.Map([
  ['type', 'Cylinder'],
  ['radius', 10.0],
  ['height', 25.0],
  ['parent', 'node-xyz789'],  // DAG reference
  ['transform', { ... }],
  ['_meta', {
    createdBy: 'user-1',
    createdAt: 1706889600000,
    modifiedBy: 'user-2',
    modifiedAt: 1706889700000
  }]
]));
```

### Stable ID Generation

Prevent ID collisions across clients:

```typescript
function generateId(clientId: number, type: string): string {
  const counter = getAndIncrementCounter(clientId);
  return `${type}-${clientId.toString(36)}-${counter.toString(36)}`;
}

// Examples:
// node-5f-1a (client 95, counter 26)
// inst-2b-3  (client 43, counter 3)
```

## Conflict Resolution

### Property Conflicts (Last-Write-Wins)

Yjs uses Lamport timestamps for automatic LWW resolution:

```typescript
// Both users edit cylinder radius simultaneously
// User A: radius = 15 (timestamp: 100)
// User B: radius = 20 (timestamp: 101)
// Result: radius = 20 (higher timestamp wins)
```

For critical properties, store history:

```typescript
interface PropertyWithHistory {
  value: number;
  history: Array<{
    value: number;
    author: string;
    timestamp: number;
  }>;
}
```

### Structural Conflicts (DAG Integrity)

The parametric DAG requires special handling:

**Cycle Detection:**

```typescript
function setParent(nodeId: string, newParentId: string): boolean {
  // Check if newParentId is a descendant of nodeId
  if (isDescendant(newParentId, nodeId)) {
    // Reject: would create cycle
    showConflictUI({
      type: 'cycle',
      message: `Cannot parent ${nodeId} under ${newParentId}: would create cycle`,
      options: ['Keep current parent', 'Move to root']
    });
    return false;
  }
  nodes.get(nodeId).set('parent', newParentId);
  return true;
}
```

**Orphan Reconnection:**

When a parent node is deleted, reconnect children:

```typescript
function onNodeDeleted(nodeId: string): void {
  const orphans = findChildNodes(nodeId);
  const grandparent = nodes.get(nodeId)?.get('parent') ?? null;

  for (const orphan of orphans) {
    nodes.get(orphan).set('parent', grandparent);
  }
}
```

### Semantic Conflicts (CAD-Specific)

Some conflicts require user decision:

| Conflict Type | Detection | Resolution Options |
|---------------|-----------|-------------------|
| Constraint over-constrained | Solver fails | Remove newest constraint, Remove conflicting, Manual edit |
| Boolean fails (no intersection) | Kernel error | Undo operation, Adjust geometry, Delete operation |
| Reference deleted | Dangling reference | Reconnect to similar, Delete dependent, Undo delete |

**Conflict UI Component:**

```typescript
interface ConflictResolution {
  id: string;
  type: 'structural' | 'semantic' | 'property';
  description: string;
  affectedNodes: string[];
  options: Array<{
    label: string;
    action: () => void;
    isRecommended?: boolean;
  }>;
  timestamp: number;
  authors: string[];  // Users involved in conflict
}
```

## Version Control (Microversions)

Inspired by Onshape's append-only version log.

### Microversion Structure

```typescript
interface Microversion {
  id: string;           // Hash of content
  parentId: string | null;
  timestamp: number;
  authorId: string;
  delta: Uint8Array;    // Yjs update binary
  size: number;         // Delta size in bytes
}

interface Branch {
  id: string;
  name: string;
  documentId: string;
  headMicroversionId: string;
  createdAt: number;
  createdBy: string;
}

interface Version {
  id: string;
  name: string;         // User-provided name
  description: string;
  branchId: string;
  microversionId: string;  // Immutable snapshot
  createdAt: number;
  createdBy: string;
  thumbnail?: string;   // S3 URL
}
```

### Version Operations

**Create Named Version:**

```typescript
async function createVersion(name: string, description: string): Promise<Version> {
  const branch = await getCurrentBranch();
  const microversion = await getHeadMicroversion(branch.id);

  // Generate thumbnail
  const thumbnail = await captureViewportThumbnail();
  const thumbnailUrl = await uploadToS3(thumbnail);

  return await db.versions.create({
    name,
    description,
    branchId: branch.id,
    microversionId: microversion.id,
    thumbnail: thumbnailUrl
  });
}
```

**Branch from Version:**

```typescript
async function createBranch(name: string, fromVersionId?: string): Promise<Branch> {
  const sourceVersion = fromVersionId
    ? await db.versions.get(fromVersionId)
    : await getHeadVersion();

  return await db.branches.create({
    name,
    documentId: sourceVersion.documentId,
    headMicroversionId: sourceVersion.microversionId
  });
}
```

**Merge Branches:**

```typescript
async function mergeBranch(sourceBranchId: string, targetBranchId: string): Promise<MergeResult> {
  const sourceDoc = await loadDocAtBranch(sourceBranchId);
  const targetDoc = await loadDocAtBranch(targetBranchId);

  // Find common ancestor
  const ancestor = await findCommonAncestor(sourceBranchId, targetBranchId);

  // Three-way merge using Yjs
  const mergedUpdate = Y.mergeUpdates([
    Y.encodeStateAsUpdate(sourceDoc),
    Y.encodeStateAsUpdate(targetDoc)
  ]);

  // Apply to target
  Y.applyUpdate(targetDoc, mergedUpdate);

  // Check for conflicts
  const conflicts = detectConflicts(sourceDoc, targetDoc, ancestor);

  if (conflicts.length > 0) {
    return { status: 'conflicts', conflicts };
  }

  return { status: 'success', newMicroversionId: await persistMicroversion(targetDoc) };
}
```

## Offline Support

### Sync Strategy

```typescript
class OfflineManager {
  private indexeddbProvider: IndexeddbPersistence;
  private websocketProvider: WebsocketProvider | null = null;
  private pendingUpdates: Uint8Array[] = [];

  async initialize(doc: Y.Doc, documentId: string): Promise<void> {
    // 1. Load from IndexedDB first (instant)
    this.indexeddbProvider = new IndexeddbPersistence(documentId, doc);
    await this.indexeddbProvider.whenSynced;

    // 2. Connect to server when online
    if (navigator.onLine) {
      this.connect(doc, documentId);
    }

    // 3. Handle online/offline transitions
    window.addEventListener('online', () => this.connect(doc, documentId));
    window.addEventListener('offline', () => this.disconnect());
  }

  private connect(doc: Y.Doc, documentId: string): void {
    this.websocketProvider = new WebsocketProvider(
      COLLAB_SERVER_URL,
      documentId,
      doc,
      { connect: true }
    );

    this.websocketProvider.on('sync', (isSynced: boolean) => {
      if (isSynced) {
        this.flushPendingUpdates();
        this.showSyncStatus('synced');
      }
    });
  }
}
```

### Conflict Detection on Reconnect

```typescript
interface SyncConflict {
  nodeId: string;
  localValue: any;
  remoteValue: any;
  localTimestamp: number;
  remoteTimestamp: number;
  propertyPath: string;
}

function detectSyncConflicts(
  localDoc: Y.Doc,
  remoteState: Uint8Array
): SyncConflict[] {
  const conflicts: SyncConflict[] = [];

  // Capture local state before merge
  const localSnapshot = captureSnapshot(localDoc);

  // Apply remote updates
  Y.applyUpdate(localDoc, remoteState);

  // Compare and detect semantic conflicts
  const newSnapshot = captureSnapshot(localDoc);

  for (const [nodeId, localNode] of localSnapshot.nodes) {
    const newNode = newSnapshot.nodes.get(nodeId);
    if (newNode && hasSemanticConflict(localNode, newNode)) {
      conflicts.push({
        nodeId,
        localValue: localNode,
        remoteValue: newNode,
        // ... timestamps from _meta
      });
    }
  }

  return conflicts;
}
```

## Awareness Protocol (Presence)

Real-time presence using Yjs awareness.

### Awareness State

```typescript
interface AwarenessState {
  user: {
    id: string;
    name: string;
    color: string;      // Assigned unique color
    avatar?: string;
  };

  editing: {
    selection: string[];           // Selected node IDs
    activeSketch: string | null;   // Currently editing sketch
    activeTool: string | null;     // Current tool (extrude, fillet, etc.)
  };

  viewport: {
    cameraPosition: [number, number, number];
    cameraTarget: [number, number, number];
    zoom: number;
  };

  cursor3d: {
    position: [number, number, number] | null;
    normal: [number, number, number] | null;
    snappedTo: string | null;  // Entity ID if snapped
  };
}
```

### Presence Features

**3D Cursors:**

```typescript
function render3DCursors(awarenessStates: Map<number, AwarenessState>): void {
  for (const [clientId, state] of awarenessStates) {
    if (clientId === localClientId) continue;
    if (!state.cursor3d.position) continue;

    renderCursor({
      position: state.cursor3d.position,
      color: state.user.color,
      label: state.user.name
    });
  }
}
```

**Selection Highlighting:**

```typescript
function highlightRemoteSelections(awarenessStates: Map<number, AwarenessState>): void {
  for (const [clientId, state] of awarenessStates) {
    if (clientId === localClientId) continue;

    for (const nodeId of state.editing.selection) {
      highlightNode(nodeId, {
        color: state.user.color,
        style: 'dashed',  // Distinguish from local selection
        label: state.user.name
      });
    }
  }
}
```

**Follow Mode:**

```typescript
function followUser(targetUserId: string): void {
  const unsubscribe = awareness.on('change', () => {
    const targetState = findUserState(targetUserId);
    if (!targetState) return;

    // Sync camera to target's viewport
    setCameraPosition(targetState.viewport.cameraPosition);
    setCameraTarget(targetState.viewport.cameraTarget);
    setZoom(targetState.viewport.zoom);
  });

  return unsubscribe;
}
```

## Permission System (RBAC)

Role-based access control for documents and teams.

### Roles and Permissions

```typescript
type Role = 'viewer' | 'commenter' | 'editor' | 'admin' | 'owner';

interface Permissions {
  document: {
    read: boolean;
    comment: boolean;
    edit: boolean;
    share: boolean;
    delete: boolean;
    export: boolean;
  };
  branch: {
    create: boolean;
    delete: boolean;
    merge: boolean;
  };
  version: {
    create: boolean;
    restore: boolean;
  };
}

const ROLE_PERMISSIONS: Record<Role, Permissions> = {
  viewer: {
    document: { read: true, comment: false, edit: false, share: false, delete: false, export: true },
    branch: { create: false, delete: false, merge: false },
    version: { create: false, restore: false }
  },
  commenter: {
    document: { read: true, comment: true, edit: false, share: false, delete: false, export: true },
    branch: { create: false, delete: false, merge: false },
    version: { create: false, restore: false }
  },
  editor: {
    document: { read: true, comment: true, edit: true, share: false, delete: false, export: true },
    branch: { create: true, delete: false, merge: false },
    version: { create: true, restore: false }
  },
  admin: {
    document: { read: true, comment: true, edit: true, share: true, delete: false, export: true },
    branch: { create: true, delete: true, merge: true },
    version: { create: true, restore: true }
  },
  owner: {
    document: { read: true, comment: true, edit: true, share: true, delete: true, export: true },
    branch: { create: true, delete: true, merge: true },
    version: { create: true, restore: true }
  }
};
```

### Server-Side Enforcement

```typescript
// WebSocket message handler with permission checks
async function handleUpdate(
  clientId: string,
  documentId: string,
  update: Uint8Array
): Promise<void> {
  const access = await getDocumentAccess(clientId, documentId);

  if (!access || !ROLE_PERMISSIONS[access.role].document.edit) {
    throw new ForbiddenError('No edit permission');
  }

  // Validate update doesn't modify protected fields
  const decodedUpdate = decodeUpdate(update);
  if (decodedUpdate.modifiesProtectedFields && access.role !== 'owner') {
    throw new ForbiddenError('Cannot modify protected fields');
  }

  await applyAndBroadcast(documentId, update, clientId);
}
```

### Sharing Model

```typescript
interface DocumentAccess {
  documentId: string;

  // Direct user access
  userId?: string;

  // Team access
  teamId?: string;

  // Link sharing
  linkToken?: string;

  role: Role;
  createdAt: number;
  expiresAt?: number;
}

// Share with user
async function shareWithUser(documentId: string, email: string, role: Role): Promise<void> {
  const user = await findUserByEmail(email);
  await db.documentAccess.create({
    documentId,
    userId: user.id,
    role
  });
  await sendShareNotification(user, documentId);
}

// Create shareable link
async function createShareLink(documentId: string, role: Role): Promise<string> {
  const token = generateSecureToken();
  await db.documentAccess.create({
    documentId,
    linkToken: token,
    role
  });
  return `${APP_URL}/d/${documentId}?token=${token}`;
}
```

## Storage Backend

### PostgreSQL Schema

```sql
-- Documents
CREATE TABLE documents (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  owner_id UUID NOT NULL REFERENCES users(id),
  created_at TIMESTAMPTZ DEFAULT NOW(),
  updated_at TIMESTAMPTZ DEFAULT NOW(),
  deleted_at TIMESTAMPTZ,
  settings JSONB DEFAULT '{}'
);

-- Microversions (append-only)
CREATE TABLE microversions (
  id TEXT PRIMARY KEY,  -- Content hash
  document_id UUID NOT NULL REFERENCES documents(id),
  parent_id TEXT REFERENCES microversions(id),
  author_id UUID NOT NULL REFERENCES users(id),
  created_at TIMESTAMPTZ DEFAULT NOW(),
  delta BYTEA NOT NULL,  -- Yjs update binary
  size_bytes INTEGER NOT NULL
);

CREATE INDEX idx_microversions_document ON microversions(document_id, created_at DESC);

-- Branches
CREATE TABLE branches (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id UUID NOT NULL REFERENCES documents(id),
  name TEXT NOT NULL,
  head_microversion_id TEXT NOT NULL REFERENCES microversions(id),
  created_at TIMESTAMPTZ DEFAULT NOW(),
  created_by UUID NOT NULL REFERENCES users(id),
  UNIQUE(document_id, name)
);

-- Named versions (immutable snapshots)
CREATE TABLE versions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id UUID NOT NULL REFERENCES documents(id),
  branch_id UUID NOT NULL REFERENCES branches(id),
  microversion_id TEXT NOT NULL REFERENCES microversions(id),
  name TEXT NOT NULL,
  description TEXT,
  thumbnail_url TEXT,
  created_at TIMESTAMPTZ DEFAULT NOW(),
  created_by UUID NOT NULL REFERENCES users(id)
);

-- Document access control
CREATE TABLE document_access (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  document_id UUID NOT NULL REFERENCES documents(id),
  user_id UUID REFERENCES users(id),
  team_id UUID REFERENCES teams(id),
  link_token TEXT,
  role TEXT NOT NULL CHECK (role IN ('viewer', 'commenter', 'editor', 'admin', 'owner')),
  created_at TIMESTAMPTZ DEFAULT NOW(),
  expires_at TIMESTAMPTZ,
  CHECK (user_id IS NOT NULL OR team_id IS NOT NULL OR link_token IS NOT NULL)
);

CREATE INDEX idx_document_access_user ON document_access(user_id, document_id);
CREATE INDEX idx_document_access_link ON document_access(link_token) WHERE link_token IS NOT NULL;
```

### S3/R2 Storage

Large assets stored in object storage:

| Path Pattern | Content |
|--------------|---------|
| `documents/{docId}/imports/{hash}.step` | Imported STEP files |
| `documents/{docId}/exports/{hash}.stl` | Exported meshes |
| `documents/{docId}/thumbnails/{versionId}.png` | Version thumbnails |
| `documents/{docId}/snapshots/{microversionId}.ydoc` | Full document snapshots (periodic) |

## Implementation Phases

### Phase 1: Foundation (4-6 weeks)

**Goals:** Basic real-time sync, offline capability

- [ ] Set up y-websocket server with authentication
- [ ] Implement IndexedDB persistence provider
- [ ] Map vcad document model to Yjs shared types
- [ ] Basic presence (user list, online status)
- [ ] Reconnection handling with sync

**Deliverables:**
- Two users can edit same document in real-time
- Changes persist locally and sync on reconnect
- User list shows who's online

### Phase 2: Collaboration UX (4 weeks)

**Goals:** Rich collaborative experience

- [ ] 3D cursor rendering for remote users
- [ ] Selection highlighting with user colors
- [ ] Follow mode (camera sync)
- [ ] Conflict resolution UI for semantic conflicts
- [ ] "Someone is editing this" indicators

**Deliverables:**
- See where other users are working
- Follow another user's viewport
- Resolve conflicts through UI

### Phase 3: Version Control (4-6 weeks)

**Goals:** Full version history and branching

- [ ] Microversion persistence layer
- [ ] Branch create/switch/delete
- [ ] Named version creation with thumbnails
- [ ] Version diff visualization
- [ ] Branch merging with conflict detection
- [ ] Restore to previous version

**Deliverables:**
- Create named versions at any point
- Branch for experimental changes
- Merge branches back
- Visual diff between versions

### Phase 4: Permissions & Sharing (3-4 weeks)

**Goals:** Secure multi-user access

- [ ] RBAC permission system
- [ ] Share dialog with role selection
- [ ] Shareable links with configurable access
- [ ] Team support
- [ ] Audit logging

**Deliverables:**
- Share documents with specific users
- Create view-only or edit links
- Manage team access
- Track who did what

### Phase 5: Scale & Polish (4 weeks)

**Goals:** Production readiness

- [ ] WebSocket server clustering (Redis pub/sub)
- [ ] Microversion compaction
- [ ] Performance monitoring and alerts
- [ ] Load testing (100+ concurrent editors)
- [ ] Mobile/tablet presence support
- [ ] Keyboard shortcuts for collaboration

**Deliverables:**
- Handles production load
- Monitoring dashboards
- Sub-second sync latency P95

## Performance Considerations

### Update Batching

```typescript
// Batch rapid updates to reduce network traffic
const debouncedSync = debounce((doc: Y.Doc) => {
  const update = Y.encodeStateAsUpdate(doc);
  websocketProvider.send(update);
}, 50);  // 50ms debounce

doc.on('update', debouncedSync);
```

### Lazy Loading

```typescript
// Load document structure first, geometry on demand
async function loadDocument(documentId: string): Promise<void> {
  // 1. Load metadata and DAG structure (fast)
  const structureUpdate = await fetchDocumentStructure(documentId);
  Y.applyUpdate(doc, structureUpdate);

  // 2. Load visible geometry
  const visibleNodes = getVisibleNodes();
  await loadGeometry(visibleNodes);

  // 3. Background load remaining geometry
  queueGeometryLoad(getRemainingNodes());
}
```

### Compaction

```typescript
// Periodic full-state snapshots to bound history size
async function compactMicroversions(documentId: string): Promise<void> {
  const doc = await loadFullDocument(documentId);
  const fullState = Y.encodeStateAsUpdate(doc);

  // Store as snapshot
  await uploadSnapshot(documentId, fullState);

  // Mark old microversions as compacted (keep for audit, exclude from sync)
  await markCompacted(documentId, beforeTimestamp);
}
```

## Security Considerations

### Transport Security

- All WebSocket connections over WSS (TLS)
- JWT authentication on connection
- Token refresh without disconnection

### Update Validation

```typescript
// Server-side update validation
function validateUpdate(update: Uint8Array, userRole: Role): ValidationResult {
  const decoded = decodeYjsUpdate(update);

  // Check for forbidden operations
  if (decoded.deletesNodes && userRole === 'commenter') {
    return { valid: false, reason: 'Commenters cannot delete nodes' };
  }

  // Check for malformed data
  if (decoded.hasMalformedStructure) {
    return { valid: false, reason: 'Malformed update structure' };
  }

  // Size limits
  if (update.byteLength > MAX_UPDATE_SIZE) {
    return { valid: false, reason: 'Update exceeds size limit' };
  }

  return { valid: true };
}
```

### Rate Limiting

```typescript
const rateLimiter = new RateLimiter({
  updates: { max: 100, windowMs: 1000 },      // 100 updates/sec
  connections: { max: 10, windowMs: 60000 },  // 10 connections/min
  documents: { max: 50, windowMs: 3600000 }   // 50 docs/hour
});
```
