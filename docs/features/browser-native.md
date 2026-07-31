# Browser-Native CAD

**Score: 83/100** | **Priority: #7**

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |

> Verified against repo 2026-07-30. Shipped: Rust kernel compiled to WASM (`crates/vcad-kernel-wasm`, `packages/kernel-wasm`), full app at vcad.io (`packages/app`).

## Overview

Full parametric CAD runs entirely in the browser. No download, no install, no plugins. Open a URL and start designing.

The vcad kernel—written in Rust—compiles to WebAssembly and executes client-side. Boolean operations, constraint solving, tessellation, and STEP export all happen locally in your browser tab.

## Why It Matters

| Tool | Getting Started |
|------|-----------------|
| SolidWorks | 10GB+ install, Windows only, license server |
| Fusion 360 | Download app, create account, cloud dependency |
| FreeCAD | Install, configure preferences, learn quirks |
| **vcad** | **vcad.io → designing in seconds** |

The friction to try professional CAD tools is enormous. Browser-native eliminates that barrier entirely:

- **Students** can start learning CAD on school Chromebooks
- **Makers** can design on any device without IT approval
- **Teams** can share a link instead of a file + software requirements
- **Reviewers** can view and comment without installing anything

## Technical Implementation

### WASM Kernel

The Rust kernel compiles to WebAssembly:

- **Size**: ~500KB-1MB gzipped (streaming compilation)
- **Includes**: BRep topology, boolean operations, constraint solver, tessellation
- **Threading**: Web Workers for parallel operations (where supported)

```
vcad-kernel (Rust) → wasm-bindgen → @vcad/kernel-wasm (npm) → Browser
```

### Frontend Stack

- **React 18**: Component architecture with Suspense for loading states
- **Three.js / React Three Fiber**: GPU-accelerated 3D viewport
- **Zustand**: Lightweight state management
- **TypeScript**: Full type safety across the stack

### Storage

- **IndexedDB**: Local document storage via `idb-keyval`
- **File System Access API**: Native file save/open (Chrome, Edge)
- **Fallback**: Download/upload for other browsers

### Offline Support

- **Service Worker**: Caches all static assets
- **PWA Manifest**: Installable to home screen/desktop
- **Background Sync**: Queue changes when offline, sync when reconnected

## User Experience

### Zero-Friction Access

1. Navigate to `vcad.io`
2. Start designing immediately
3. Save to browser storage or export file

No account required for basic use. No download. No plugins.

### Shareable Links

```
vcad.io/d/abc123
```

Share a design by URL. Recipients can view, fork, or remix without setup.

### Device Flexibility

Works anywhere with a modern browser:

- **Desktop**: Full experience with keyboard shortcuts
- **Chromebook**: Perfect for education
- **Tablet**: Touch-friendly viewport navigation
- **Borrowed laptop**: No permission needed to install
- **Phone**: View designs, basic editing (limited by screen size)

### Optional PWA Install

For users who want desktop-like experience:

- Install prompt after second visit
- Desktop icon, standalone window
- Works offline
- File type associations (`.vcad` files)

## Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| Initial load | <3s on 4G | Streaming WASM compilation |
| Time to interactive | <5s | Can manipulate viewport while modules load |
| WASM size | <1MB gzipped | Core kernel only |
| First paint | <1s | Shell renders before WASM ready |

### Lazy Loading Strategy

```
Core (immediate):     Viewport, primitives, transforms
Deferred (on demand): Boolean operations, constraints
Lazy (background):    STEP export, physics, advanced features
```

### Optimization Techniques

- **Streaming compilation**: Browser compiles WASM while downloading
- **Code splitting**: React.lazy() for UI panels
- **Worker threads**: Heavy computation off main thread
- **GPU instancing**: Efficient rendering of patterns
- **LOD**: Reduce mesh complexity at distance

## Offline Capability

After first load, vcad works without network:

### What Works Offline

- Create and edit designs
- All modeling operations
- Export to STL/GLB
- Save to IndexedDB
- Open previously cached designs

### What Requires Network

- Share links (initial creation)
- Cloud sync (if enabled)
- STEP import (large library, lazy-loaded)
- Software updates

### Sync Strategy

1. All changes save to IndexedDB immediately
2. If logged in, queue sync operations
3. When online, batch sync in background
4. Conflict resolution: last-write-wins with version history

## Browser Support

| Browser | Support Level |
|---------|---------------|
| Chrome 90+ | Full |
| Firefox 90+ | Full |
| Safari 15+ | Full (some File System API limitations) |
| Edge 90+ | Full |
| Mobile Chrome | Full |
| Mobile Safari | Full |
| Samsung Internet | Full |

**Target**: 95%+ of global browser traffic

### Required APIs

- WebAssembly (98%+ support)
- WebGL 2.0 (96%+ support)
- IndexedDB (98%+ support)
- Service Workers (97%+ support)

## Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Time to first interaction | <5 seconds | Lighthouse, RUM |
| Browser compatibility | 95%+ | Analytics |
| Offline success rate | 99%+ | Service worker metrics |
| PWA install rate | 10%+ of returning users | Analytics |
| Mobile usability score | 90+ | Lighthouse |

## Competitive Advantage

### vs. Onshape

Onshape pioneered browser-based CAD, but their kernel runs on servers:

| Aspect | Onshape | vcad |
|--------|---------|------|
| Kernel location | Cloud servers | Client browser |
| Network required | Always | After first load |
| Latency | Variable (50-200ms per op) | None |
| Data privacy | Files on their servers | Local by default |
| Offline mode | None | Full functionality |

### vs. Desktop CAD

| Aspect | Desktop CAD | vcad |
|--------|-------------|------|
| Installation | 10GB+, admin rights | None |
| Updates | Manual, disruptive | Automatic, instant |
| License management | Complex | URL access |
| Cross-platform | Often Windows-only | Any browser |
| Sharing | Export + install | Link |

### Unique Position

**vcad is the only professional parametric CAD that runs a full BRep kernel entirely client-side in the browser.**

This enables use cases impossible with other tools:

- **Education**: Deploy to 1000 students with a URL
- **Embedded**: CAD inside other web apps via iframe
- **Privacy-sensitive**: Design never leaves device
- **Air-gapped**: Full functionality on isolated networks
- **Low-bandwidth**: Works after one-time download in remote areas

## Future Enhancements

- **WebGPU**: 10x rendering performance when available
- **SharedArrayBuffer**: True multi-threading for complex operations
- **OPFS**: Faster file storage with Origin Private File System
- **Compression**: Brotli for even smaller WASM bundles
- **Edge caching**: Sub-second loads via CDN
