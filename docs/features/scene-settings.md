# Scene Settings

**Score: 85/100** | **Priority: High**

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |

> Verified against repo 2026-07-30. `SceneSettings` exists in `crates/vcad-ir/src/lib.rs`; `updateEnvironment`/`addLight`/`updatePostProcessing` live in `packages/core/src/stores/document-store.ts` (not a separate scene store). Bloom remains unshipped ("coming soon" below is still accurate).

## Overview

Scene settings provide unified control over lighting, environment, backgrounds, and post-processing effects in vcad. The same scene configuration renders identically in the viewport and exports—no more "it looks different when I export" surprises.

Settings are stored in the document itself, making them shareable and reproducible. Share a `.vcad` file and the recipient sees exactly what you see.

## Why It Matters

| Traditional CAD | vcad |
|-----------------|------|
| Viewport and export render differently | Same settings, same output |
| Manual lighting setup for each render | Smart defaults + presets |
| Scene config lost between sessions | Saved in document |
| No MCP/API access to lighting | Full programmatic control |

## Features

### Environment Presets

HDR environment maps provide realistic ambient lighting:

| Preset | Description |
|--------|-------------|
| `studio` | Professional photo studio (default) |
| `warehouse` | Industrial with skylights |
| `apartment` | Modern interior natural light |
| `park` | Outdoor park environment |
| `city` | Urban cityscape |
| `dawn` | Dawn/dusk sky tones |
| `night` | Night with artificial lights |
| `sunset` | Warm sunset colors |
| `forest` | Dappled forest light |
| `neutral` | Flat gray for technical review |

Each preset includes configurable intensity (0.0–2.0).

### Custom Lights

Add individual lights beyond the environment:

```typescript
{
  id: "key",
  kind: { type: "Directional", direction: { x: 0.5, y: -0.8, z: 0.4 } },
  color: [1, 0.98, 0.95],
  intensity: 1.2,
  castShadow: true,
  enabled: true
}
```

**Light Types:**
- **Directional** – Sun-like parallel rays (direction vector)
- **Point** – Omnidirectional from a position
- **Spot** – Cone of light with angle and penumbra
- **Area** – Rectangular emitter (future)

### Background Options

| Type | Description |
|------|-------------|
| `Environment` | Use environment map as backdrop |
| `Solid` | Single color (`{ type: "Solid", color: [r, g, b] }`) |
| `Gradient` | Top-to-bottom gradient (`{ type: "Gradient", top: [...], bottom: [...] }`) |
| `Transparent` | For compositing in external tools |

### Post-Processing

Built-in effects that match viewport and export:

**Ambient Occlusion:**
```typescript
{
  enabled: true,
  intensity: 1.5,  // 0.0–3.0
  radius: 0.5      // Scene units
}
```

**Vignette:**
```typescript
{
  enabled: true,
  offset: 0.3,     // Distance from edge
  darkness: 0.3    // 0.0–1.0
}
```

**Bloom** (coming soon):
```typescript
{
  enabled: true,
  intensity: 0.5,
  threshold: 0.8
}
```

### Scene Presets

One-click setups for common use cases:

| Preset | Use Case |
|--------|----------|
| **Technical** | Clean neutral lighting for engineering review |
| **Hero Shot** | Dramatic lighting for presentations |
| **Lifestyle** | Warm, inviting atmosphere |
| **Workshop** | Industrial overhead lighting |

## IR Schema

Scene settings live in the document root:

```typescript
interface Document {
  // ... other fields
  scene?: SceneSettings;
}

interface SceneSettings {
  environment?: Environment;
  lights?: Light[];
  background?: Background;
  postProcessing?: PostProcessing;
  cameraPresets?: CameraPreset[];
}
```

Full type definitions in:
- **Rust**: `crates/vcad-ir/src/lib.rs`
- **TypeScript**: `packages/ir/src/index.ts`

## API Usage

### Document Store (App)

```typescript
import { useDocumentStore } from "@vcad/core";

// Get current scene
const scene = useDocumentStore((s) => s.document.scene);

// Update environment
const updateEnvironment = useDocumentStore((s) => s.updateEnvironment);
updateEnvironment({ type: "Preset", preset: "studio", intensity: 0.5 });

// Add a light
const addLight = useDocumentStore((s) => s.addLight);
addLight({
  id: "fill",
  kind: { type: "Directional", direction: { x: -0.5, y: -0.5, z: -0.5 } },
  color: [0.95, 0.97, 1.0],
  intensity: 0.4,
});

// Update post-processing
const updatePostProcessing = useDocumentStore((s) => s.updatePostProcessing);
updatePostProcessing({
  ambientOcclusion: { enabled: true, intensity: 2.0 },
  vignette: { enabled: true, darkness: 0.4 },
});
```

### MCP Integration

Create documents with scene settings via MCP:

```typescript
// Via create_cad_document tool
{
  parts: [
    { name: "bracket", primitive: { type: "cube", size: { x: 50, y: 30, z: 5 } } }
  ],
  scene: {
    environment: { type: "Preset", preset: "studio", intensity: 0.4 },
    background: { type: "Gradient", top: [0.15, 0.15, 0.18], bottom: [0.05, 0.05, 0.06] },
    postProcessing: {
      ambientOcclusion: { enabled: true, intensity: 1.5 },
      vignette: { enabled: true, darkness: 0.3 }
    }
  }
}
```

## UI Access

1. Click the **menu** button (top-right)
2. Select **Scene** → opens scene panel
3. Choose preset or customize individual settings

**Panel sections:**
- **Presets**: Quick scene setups
- **Environment**: HDR preset + intensity
- **Background**: Type and colors
- **Lights**: List with add/remove/toggle
- **Post-Processing**: AO and vignette controls

## Smart Defaults

When `scene` is undefined, vcad applies smart defaults:

```typescript
{
  environment: { type: "Preset", preset: "studio", intensity: 0.4 },
  lights: [
    { id: "key", kind: "Directional", intensity: 1.2, castShadow: true },
    { id: "fill", kind: "Directional", intensity: 0.4 },
    { id: "rim", kind: "Directional", intensity: 0.2 }
  ],
  background: { type: "Environment" },
  postProcessing: {
    ambientOcclusion: { enabled: true, intensity: 1.5 },
    vignette: { enabled: true, darkness: 0.3 }
  }
}
```

These defaults adapt to light/dark theme automatically.

## Backward Compatibility

Documents without `scene` field load and render correctly with smart defaults. Saving the document does not add a `scene` field unless you explicitly change scene settings (preserving minimal file size).

## Future Enhancements

- **Hero Shot Algorithm**: Auto-compute optimal camera angle via PCA + view scoring
- **Turntable Export**: Animated GIF/MP4 orbit renders
- **Custom HDRI Upload**: User-provided environment maps
- **Bloom Effect**: Glow on bright surfaces
- **Tone Mapping Options**: Reinhard, ACES, AgX

## Related Features

- [Headless API](./headless-api.md) – Programmatic scene control
- [Document Persistence](./document-persistence.md) – Scene settings storage
- [Browser Native](./browser-native.md) – WebGL rendering pipeline
