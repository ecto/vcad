# Materials

PBR material system for realistic appearance of parts in the viewport.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `n/a` |
| Priority | `p1` |
| Effort | `n/a` (complete) |

> Verified against repo 2026-07-30. Confirmed: `MaterialDef` in `crates/vcad-ir/src/lib.rs`, `setPartMaterial` in `packages/core/src/stores/document-store.ts`, `previewMaterial`/`favoriteMaterials`/`recentMaterials` (with `vcad:favoriteMaterials` localStorage persistence) in `packages/core/src/stores/ui-store.ts`.

## Problem

Parts need realistic appearance for visualization and rendering. Without materials:

- Parts all look the same gray color
- No way to distinguish aluminum from steel from plastic
- Renders lack realism for presentations and design review
- Physical properties like density aren't associated with appearance

## Solution

PBR (Physically Based Rendering) material system with:

### Material Properties

Each material defines:

| Property | Type | Description |
|----------|------|-------------|
| `name` | `string` | Unique identifier (e.g., "aluminum", "abs_white") |
| `color` | `[r, g, b]` | Base color in 0.0..1.0 range |
| `metallic` | `f64` | 0.0 = dielectric, 1.0 = metal |
| `roughness` | `f64` | 0.0 = mirror smooth, 1.0 = fully rough |
| `density` | `f64?` | Optional, kg/m^3 for mass calculations |
| `friction` | `f64?` | Optional, for physics simulations |

### Per-Part Assignment

- Each part can have one material assigned
- Materials are referenced by name key
- Default material applied when none specified

### Material Selector UX

- **Preview on hover**: Hovering a material in the selector shows a live preview on the selected part
- **Favorites list**: Pin frequently-used materials for quick access
- **Recent history**: Last 6 used materials shown at top of selector
- **Persistence**: Favorites and recents stored in localStorage

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Hover material | Live preview on selected part |
| Click material | Apply to part, add to recents |
| Star icon click | Toggle favorite status |
| Leave selector | Clear preview, revert to actual material |

### Material Preview

When hovering a material in the selector:
1. Store current material temporarily
2. Apply hovered material to viewport
3. On mouse leave, revert to stored material
4. On click, commit the hovered material

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-ir/src/lib.rs` | `MaterialDef` struct definition |
| `packages/core/src/stores/document-store.ts` | `setPartMaterial` action |
| `packages/core/src/stores/ui-store.ts` | `previewMaterial`, `favoriteMaterials`, `recentMaterials` state |

### Data Structures

**Rust (vcad-ir):**

```rust
pub struct MaterialDef {
    pub name: String,
    pub color: [f64; 3],
    pub metallic: f64,
    pub roughness: f64,
    pub density: Option<f64>,
    pub friction: Option<f64>,
}
```

**TypeScript (ui-store):**

```typescript
interface UiState {
  previewMaterial: MaterialPreview | null;
  recentMaterials: string[];  // Last 6 used material keys
  favoriteMaterials: string[];  // User-pinned material keys
}
```

### Storage

- `vcad:recentMaterials` - localStorage key for recent materials array
- `vcad:favoriteMaterials` - localStorage key for favorite materials array

## Tasks

All tasks completed.

- [x] Define `MaterialDef` struct in vcad-ir
- [x] Add `materials` HashMap to Document
- [x] Add `part_materials` HashMap for per-part assignment
- [x] Implement `setPartMaterial` action in document-store
- [x] Add preview material state to ui-store
- [x] Implement recent materials tracking (max 6)
- [x] Implement favorite materials with persistence
- [x] Build material selector UI component

## Acceptance Criteria

- [x] Materials have PBR properties (color, metallic, roughness)
- [x] Materials can optionally include density and friction
- [x] Each part can have a material assigned
- [x] Hovering a material shows live preview on selected part
- [x] Clicking a material applies it and adds to recents
- [x] Recent materials list shows last 6 used
- [x] Favorite materials persist across sessions
- [x] Materials stored in .vcad document format

## Future Enhancements

- [ ] Material library with presets (metals, plastics, wood, etc.)
- [ ] Texture/image support for color maps
- [ ] Normal maps for surface detail
- [ ] Subsurface scattering for translucent materials
- [ ] Emissive materials for lights
- [ ] Material import from external sources (MatCap, etc.)
