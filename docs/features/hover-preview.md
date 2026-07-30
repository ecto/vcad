# Hover Preview

Highlight affected geometry in the 3D viewport when hovering over features in the tree.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `s` |

> Verified against repo 2026-07-30. Still unbuilt — no `hoveredFeatureId` state exists in the app stores.

## Problem

The feature tree lists operations like "Cut-Extrude3" or "Fillet2" with no visual feedback until you click. Users are forced to:

- Guess what each feature does based on cryptic names
- Click through features one-by-one to understand history
- Mentally map tree items to geometry

Desktop CAD (SolidWorks, Fusion 360, Onshape) all suffer from this — hover does nothing useful.

## Solution

When hovering over a feature in the tree, immediately highlight the affected geometry in the 3D viewport.

**Core behavior:**
- Hover → highlight faces/edges added or modified by that feature
- Optional: show "ghost" of previous state to visualize the delta

**Not included:** Hover preview for assembly joints, sketch constraints (separate features).

## UX Details

### Highlight Style

| Element | Style |
|---------|-------|
| Added geometry | Cyan overlay (`#00FFFF`, 40% opacity) |
| Removed geometry | Red wireframe (`#FF4444`) |
| Modified edges | Yellow outline (2px) |

### Ghost/Delta Mode

When enabled (toggle in settings or hold `Shift`):
- Show semi-transparent wireframe of geometry *before* the hovered feature
- Makes subtractive operations (cuts, holes) instantly comprehensible
- Ghost uses white wireframe at 20% opacity

### Interaction States

| State | Behavior |
|-------|----------|
| Hover | 150ms delay before highlight triggers (prevents flicker during fast scrolling) |
| Highlight transition | 100ms fade-in |
| Mouse leave | Immediate clear |
| Shift+Hover | Show delta/ghost mode |

### Edge Cases

- **Root/origin:** No highlight
- **Suppressed feature:** Show dashed outline (what *would* exist)
- **Assembly instance:** Highlight all geometry from that instance

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Emit hover events with feature ID |
| `packages/core/src/stores/ui-store.ts` | Add `hoveredFeatureId`, `hoverPreviewEnabled`, `hoverShowDelta` |
| `packages/app/src/components/ViewportContent.tsx` | Receive hover events, render highlights |
| `packages/engine/src/mesh.ts` | Add mesh diffing for affected faces |

### State Additions

```typescript
// ui-store.ts
interface UiState {
  hoveredFeatureId: string | null;
  hoverPreviewEnabled: boolean;  // user preference
  hoverShowDelta: boolean;       // shift-key or setting
}
```

### Event Interface

```typescript
// Emitted from FeatureTree component
interface HoverPreviewEvent {
  mode: 'highlight-feature';
  featureId: string | null;  // null to clear
  showDelta: boolean;        // show ghost of previous state
}

// Example
emitViewportEvent({
  mode: 'highlight-feature',
  featureId: 'cut-extrude-3',
  showDelta: true
});
```

### Algorithm

1. User hovers feature row in tree
2. After 150ms debounce, set `hoveredFeatureId` in store
3. ViewportContent subscribes to `hoveredFeatureId`
4. Compute affected faces by diffing mesh before/after feature
5. Apply highlight material as second render pass (additive blend)
6. For delta mode: render previous state mesh with ghost material
7. On mouse leave: clear `hoveredFeatureId`, remove highlights immediately

### Performance

- Cache mesh snapshots at each feature for instant delta display
- Limit ghost rendering to <50k triangles (simplify if needed)
- Debounce rapid hover changes

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add `hoveredFeatureId` to ui-store (`xs`)
- [ ] Add `hoverPreviewEnabled` preference to ui-store (`xs`)
- [ ] Add hover event handlers to FeatureTree rows (`xs`)

### Phase 2: Viewport Highlighting

- [ ] Create highlight material (cyan overlay, additive blend) (`s`)
- [ ] Implement mesh diffing to find affected faces (`m`)
- [ ] Render highlight pass in ViewportContent (`s`)

### Phase 3: Delta/Ghost Mode

- [ ] Add `hoverShowDelta` state and Shift key handler (`xs`)
- [ ] Cache mesh snapshots per feature for instant lookup (`s`)
- [ ] Render ghost wireframe with previous state mesh (`s`)

### Phase 4: Polish

- [ ] Add 150ms debounce to prevent flicker (`xs`)
- [ ] Add 100ms fade-in transition for highlight (`xs`)
- [ ] Handle edge cases (root, suppressed, assembly instance) (`s`)
- [ ] Add settings toggle for hover preview preference (`xs`)

## Acceptance Criteria

- [ ] Hovering feature highlights affected geometry within 200ms
- [ ] Highlight visually distinct from selection
- [ ] Delta mode shows ghost of previous state when Shift held
- [ ] Works for all feature types (extrude, cut, fillet, pattern, etc.)
- [ ] No performance regression on models with 100+ features
- [ ] Preference to disable if user finds it distracting
- [ ] Hovering root/origin shows no highlight
- [ ] Hovering suppressed feature shows dashed outline

## Future Enhancements

- [ ] Hover preview for assembly joints (show joint axis/range)
- [ ] Hover preview for sketch constraints (show constraint geometry)
- [ ] Show feature dimensions/parameters as overlay text on hover
- [ ] Animate delta transition (morph from before to after)

## Competitive Advantage

No major CAD tool does this well:
- **SolidWorks:** Hover shows tooltip only
- **Fusion 360:** No hover preview
- **Onshape:** Minimal highlight, no delta view

This small feature dramatically improves model comprehension and positions vcad as more intuitive than incumbents.
