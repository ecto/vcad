# Inline Parameter Editing

Click a feature to expand it inline, showing key parameters for direct editing with live preview.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `m` |

> Verified against repo 2026-07-30. Shipped: `FeatureTree.tsx` expands features inline (`inlineExpandedIds`) with scrub-input dimensions, position, rotation, and material sections (`InlineProperties.tsx`, `ScrubInput`). Implementation diverged from this spec: edits apply directly to the document (no separate ghost-preview/`previewParams` layer, no `expandedFeatureId` in ui-store).

## Problem

Every parameter change requires opening a modal dialog. User flow today:

1. Double-click feature in tree
2. Wait for dialog to open
3. Find the parameter
4. Edit value
5. Click OK
6. See result

This friction compounds across dozens of edits per session. Competitors (SolidWorks, Fusion 360) have similar dialog-heavy workflows—opportunity to differentiate.

## Solution

Click a feature to expand it inline, showing key parameters for direct editing with live preview.

```
▼ Ø5 Hole at Center
   Diameter: [5.0] mm
   Depth: [Through]
   Position: (25, 25)

▶ Fillet R2

▼ Extrude 10mm
   Distance: [10.0] mm
   Direction: [Both]
   Draft: [0°]
```

**Not included:** Inline sketch editing (too complex, keep in dedicated mode), multi-feature batch editing, expressions/formulas (keep in dialog).

## UX Details

### Parameter Selection

Show only key parameters inline—not the full property sheet. Criteria:

| Show Inline | Keep in Dialog |
|-------------|----------------|
| Primary dimensions (diameter, depth, distance) | Advanced options |
| Position/placement | Tolerances |
| Direction/orientation | Construction geometry toggles |
| Draft angles | Feature-specific edge cases |

Rule of thumb: If users change it >50% of the time, show it inline.

### Input Controls

- **Numeric values:** Scrub inputs (drag to adjust, click to type)
- **Enums:** Dropdown (Through/Blind, Both/One Side)
- **Positions:** Coordinate display, click to reselect in viewport
- **Angles:** Scrub with 1° increments, Shift for 0.1°

### Live Preview

- Update viewport in real-time as values change
- Use ghost/preview material for uncommitted state
- Debounce expensive operations (booleans) to 100ms

### Interaction States

| State | Behavior |
|-------|----------|
| Collapsed | Default, shows feature name only (▶) |
| Expanded | Shows parameters, viewport highlights feature (▼) |
| Editing | Input focused, blue border |
| Modified | Uncommitted changes, italic values |

### Keyboard

| Key | Action |
|-----|--------|
| Enter | Commit changes |
| Escape | Cancel, revert to previous |
| Tab | Next parameter |
| Shift+Tab | Previous parameter |

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Add expanded state, render inline parameters |
| `packages/core/src/stores/ui-store.ts` | Add `expandedFeatureId` and `previewParams` state |
| `packages/app/src/components/ViewportContent.tsx` | Subscribe to preview state, render ghost mesh |
| `packages/engine/src/evaluate.ts` | Add preview evaluation mode |

### State Additions

```typescript
// ui-store.ts
interface UiState {
  expandedFeatureId: string | null;
  previewParams: Record<string, unknown> | null;
}
```

### Algorithm

1. User clicks feature row
2. Set `expandedFeatureId` in store
3. Render inline parameter inputs (reuse scrub input from property panel)
4. On parameter change:
   a. Update `previewParams` in store
   b. Engine re-evaluates with preview params
   c. Viewport renders preview mesh with ghost material
5. On commit: Apply params to document (single undo operation), clear preview

### Notes

- Store expanded/collapsed state per-feature in UI store (not document)
- Preview state lives in a separate "pending" layer, not undo stack
- Debounce expensive operations to 100ms

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add `expandedFeatureId` to ui-store (`xs`)
- [ ] Add `previewParams` state for live preview (`xs`)
- [x] Add expand/collapse interaction to FeatureTree (`s`)

### Phase 2: Parameter Inputs

- [x] Create inline parameter editor component (`s`)
- [x] Implement scrub input for numeric values (reuse existing) (`xs`)
- [ ] Implement dropdown for enum values (`xs`)
- [ ] Add validation and error display (`s`)

### Phase 3: Live Preview

- [ ] Add preview evaluation mode to engine (`m`)
- [ ] Render preview mesh with ghost material (`s`)
- [ ] Debounce preview updates for expensive operations (`xs`)

### Phase 4: Polish

- [ ] Keyboard navigation (Tab between inputs) (`xs`)
- [ ] Enter to commit, Escape to cancel (`xs`)
- [ ] Undo/redo integration (single operation per commit) (`s`)

## Acceptance Criteria

- [x] Clicking a feature expands to show its key parameters inline
- [x] Scrubbing a numeric value updates the viewport in real-time
- [ ] Pressing Enter commits the change and updates the document
- [ ] Pressing Escape reverts to the previous value
- [ ] Invalid values show an error message and prevent commit
- [ ] Tab/Shift+Tab navigates between parameters
- [ ] Commit creates a single undo operation
- [ ] Average time for single-parameter edits reduced by 60%+

## Future Enhancements

- [ ] Inline sketch constraint editing (requires sketch param exposure)
- [ ] Multi-feature batch editing
- [ ] Parameter expressions (e.g., `diameter * 2`)
- [ ] Parameter linking (one param drives another)
- [ ] Mobile touch support for scrub inputs
