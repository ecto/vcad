# Offline-First & Local-First Privacy

Your geometry stays on your device. Full CAD functionality without network dependency.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p1` |
| Score | 82/100 |

> Verified against repo 2026-07-30. Core claims confirmed: WASM kernel (`crates/vcad-kernel-wasm`), IndexedDB storage (`packages/app/src/lib/storage.ts`), offline export. One correction: opt-in cloud sync now exists (Supabase via `@vcad/auth`, `packages/auth/src/sync.ts` — user-controlled, matching the "Optional Sync" section, no longer future). Kernel is ~285K LOC now, not ~27K.

## Problem

Cloud-dependent CAD tools create serious barriers for security-conscious organizations:

1. **Defense contractors** - ITAR regulations prohibit storing controlled technical data on third-party servers
2. **Medical device companies** - IP concerns around proprietary designs and FDA compliance requirements
3. **Enterprise security** - Corporate policies often prohibit design data on external cloud infrastructure
4. **Remote/offline work** - Field engineers, offshore platforms, and aircraft need CAD without connectivity

Traditional solutions:
- **SolidWorks** - Desktop-only, but increasingly cloud-connected features
- **Onshape** - Cloud-only architecture, designs stored on their servers
- **Fusion 360** - Hybrid model with cloud-dependent features

## Solution

vcad is architected for complete offline operation with zero geometry data transmission.

### Core Architecture

```
Browser Runtime
├── WASM Kernel (vcad-kernel-wasm)
│   ├── BRep operations
│   ├── Boolean engine
│   ├── Tessellation
│   └── STEP import/export
├── IndexedDB Storage
│   ├── Documents store
│   ├── Thumbnails (client-generated)
│   └── Undo history
└── Local Filesystem Access
    └── File System Access API for export
```

**No server required** - After initial page load, vcad operates entirely client-side.

### Privacy Guarantees

| Data Type | Storage Location | Network Transmission |
|-----------|------------------|---------------------|
| Geometry (BRep, meshes) | Browser IndexedDB | Never without explicit export |
| Undo history | Browser memory/IndexedDB | Never |
| Thumbnails | Generated client-side | Never |
| Document metadata | Browser IndexedDB | Never |
| UI analytics | Optional | Interaction events only, no design content |

### What We Collect (Optional)

If analytics are enabled:
- Button clicks and navigation patterns
- Feature usage frequency
- Performance metrics (render times)

What we **never** collect:
- Geometry coordinates or topology
- Part names or document content
- File contents or export data
- Screenshots or thumbnails

## Technical Implementation

### WASM-Based Computation

All CAD operations execute in WebAssembly on the client:

```typescript
// packages/engine/src/evaluate.ts
// Geometry evaluation happens entirely in browser
const mesh = await engine.evaluate(document);
// No network calls - pure WASM computation
```

The kernel (~27K lines of Rust) compiles to ~2MB WASM, cached by the browser.

### IndexedDB Persistence

```typescript
// packages/app/src/lib/storage.ts
interface StoredDocument {
  id: string;
  name: string;
  document: VcadFile;      // Full parametric model
  createdAt: number;
  modifiedAt: number;
  thumbnail?: Blob;        // Client-generated preview
  syncStatus: "local";     // Always local by default
}
```

Storage operations:
- `saveDocument()` - Write to IndexedDB
- `loadDocument()` - Read from IndexedDB
- `listDocuments()` - Query local store
- `deleteDocument()` - Remove from IndexedDB

No cloud sync by default. All data stays in browser storage.

### Export Always Available

Export works offline via browser APIs:

```typescript
// STL export - binary generation in WASM
const stlBuffer = await engine.exportSTL(mesh);
downloadBlob(stlBuffer, "part.stl");

// STEP export - BRep serialization in WASM
const stepData = await engine.exportSTEP(solid);
downloadBlob(stepData, "part.step");

// GLB export - glTF binary in JS
const glbBuffer = exportGLB(mesh, materials);
downloadBlob(glbBuffer, "part.glb");
```

### Optional Sync (User-Controlled)

For users who want backup/sync, future support for:

| Storage Option | Control Level |
|---------------|---------------|
| Local filesystem | Full user control |
| Self-hosted S3 | User-owned infrastructure |
| On-premise server | Enterprise IT managed |
| Cloud storage | Explicit opt-in only |

Sync is **never automatic**. Users must explicitly:
1. Enable sync feature
2. Configure storage endpoint
3. Authenticate
4. Choose which documents to sync

## Offline Capabilities

### Full Functionality Without Network

| Feature | Offline Status |
|---------|---------------|
| Create/edit geometry | Full support |
| Boolean operations | Full support |
| Sketch mode with constraints | Full support |
| Assembly with joints | Full support |
| Forward kinematics | Full support |
| Undo/redo | Full support |
| Export (STL, STEP, GLB) | Full support |
| Import (STEP files) | Full support |
| Document save/load | Full support |

### Network-Dependent Features (Future)

| Feature | Degraded Behavior |
|---------|-------------------|
| Cloud sync | Queued for when online |
| Collaboration | Read-only cached state |
| Remote rendering | Falls back to local |

### Progressive Web App

vcad can be installed as a PWA for true offline-first experience:

```javascript
// Service worker caches all assets
// Works in airplane mode after initial install
navigator.serviceWorker.register('/sw.js');
```

## Enterprise Features

### Current

- **Air-gapped operation** - No network calls after initial load
- **Local-only storage** - IndexedDB with no cloud dependency
- **Export freedom** - STL, STEP, GLB always available
- **Open source** - Audit the code yourself

### Planned

| Feature | Description | Timeline |
|---------|-------------|----------|
| Self-hosted deployment | Docker image for internal hosting | Q2 |
| On-premise storage | Connect to internal S3/MinIO | Q2 |
| LDAP/SAML integration | Enterprise identity providers | Q3 |
| Audit logging | Track document access for compliance | Q3 |
| Data residency controls | Enforce geographic storage policies | Q4 |

### Deployment Options

```
Option 1: Public vcad.io (current)
├── Initial load from CDN
├── All computation client-side
└── No data leaves browser

Option 2: Self-hosted (planned)
├── Deploy to internal infrastructure
├── Host WASM bundles internally
└── Zero external network access

Option 3: Air-gapped (planned)
├── Package as offline installer
├── No network access required
└── Update via internal channels
```

## Competitive Comparison

| Capability | vcad | Onshape | Fusion 360 | SolidWorks |
|------------|------|---------|------------|------------|
| Fully offline operation | Yes | No | Partial | Yes |
| Geometry stays on device | Yes | No | Partial | Yes |
| Browser-based | Yes | Yes | No | No |
| Open source | Yes | No | No | No |
| Self-hosted option | Planned | No | No | No |
| No account required | Yes | No | No | No |

### Why This Matters

**Onshape**: Every edit syncs to their cloud. Documents stored on Onshape servers. No offline mode. Unsuitable for ITAR or sensitive IP.

**Fusion 360**: Cloud-connected features (generative design, simulation credits). Designs sync to Autodesk servers. Limited offline mode.

**SolidWorks**: Desktop-only with network license checks. 3DExperience platform increasingly cloud-dependent.

**vcad**: Your geometry, your device, your choice. Open source means you can verify these claims.

## Success Metrics

### Technical

- [ ] 100% feature parity offline vs online
- [ ] Zero geometry data in network requests (verified via browser DevTools)
- [ ] <3 second cold start after PWA install
- [ ] <100ms save to IndexedDB

### Compliance

- [ ] Pass security review for defense contractor use
- [ ] Meet FDA 21 CFR Part 11 requirements (with audit logging)
- [ ] SOC 2 Type II for self-hosted deployment
- [ ] GDPR compliant (no PII collection)

### User Experience

- [ ] "Works on airplane" testimonials from users
- [ ] Adoption by security-conscious organizations
- [ ] Zero data breach incidents (nothing to breach)

## Implementation Checklist

### Completed

- [x] WASM kernel with no network dependencies
- [x] IndexedDB document storage
- [x] Client-side thumbnail generation
- [x] Local-only undo/redo history
- [x] Offline export (STL, STEP, GLB)
- [x] Multi-tab document locking (local)
- [x] Storage quota management

### In Progress

- [ ] PWA with service worker caching
- [ ] Offline-first sync queue

### Planned

- [ ] Self-hosted deployment package
- [ ] On-premise storage connectors
- [ ] Enterprise audit logging
- [ ] Air-gapped installer

## Privacy Policy Summary

**We collect**: Optional anonymized usage analytics (UI interactions only)

**We never collect**:
- Your CAD geometry or models
- Document names or content
- Exported files
- Any design data whatsoever

**You control**:
- Where your data is stored (your browser)
- When/if to export or share
- Whether to enable analytics

**Our commitment**: vcad will never require cloud storage for core CAD functionality.
