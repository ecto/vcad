# Drag-Drop Feature Reordering

Drag-and-drop reordering with live preview showing the full impact before the drop is committed.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p3` |
| Effort | `m` |

> Verified against repo 2026-07-30. Still unbuilt — no `reorderFeature` action or drag state exists in the app stores.

## Problem

Moving features in the parametric history tree is risky:
- Users can't predict what will break before committing
- Invalid reorders (moving a feature before its dependencies) cause cryptic errors
- No visual indication of downstream impact
- Mistakes require undo or manual repair

In competing tools, reordering is either disabled or produces confusing failure states.

## Solution

Drag-and-drop reordering with **live preview** showing the full impact before the drop is committed.

### Constraints

Certain reorders are **invalid** and must be blocked or warned:
- Cannot move feature before any of its dependencies (inputs)
- Cannot move sketch after features that consume it
- Cannot move assembly joint before the parts it connects

The system should compute the valid range for each feature and restrict drops accordingly.

**Not included:** Multi-select drag, cross-part reordering, drag between assemblies.

## UX Details

### Initiating Drag
- **Drag handle** (grip icon) on left side of feature row, OR
- **Drag entire row** when clicking and holding for 200ms
- Cursor changes to grabbing hand
- **Ghost image** follows cursor (semi-transparent copy of row)

### During Drag
- **Drop indicator**: horizontal line between rows showing insertion point
- **Live preview**: as user hovers over different positions, immediately show:
  - Features that would **break** (red highlight + error icon)
  - Features that would be **affected** but remain valid (yellow highlight)
  - Dependency arrows showing why something breaks
- **Invalid zones**: visually dim or disable positions where the feature cannot legally go (before its dependencies)

### Before Drop
- If drop would cause errors:
  - Show warning popover: "This will break 3 features: Fillet1, Pattern2, Shell1"
  - Options: "Drop Anyway" / "Cancel"
- If drop is clean: commit immediately on release

### After Drop
- Full undo support (Cmd+Z reverts to original order)
- Toast notification: "Moved Extrude2 to position 4" with Undo button

### Visual Feedback Summary

| State | Visual |
|-------|--------|
| Dragging | Ghost row at 50% opacity following cursor |
| Valid drop zone | Blue insertion line, subtle highlight on adjacent rows |
| Invalid drop zone | Grayed out, no insertion line, cursor shows "not allowed" |
| Would break features | Red highlight on affected rows, error icon |
| Would affect features | Yellow highlight on affected rows |

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Add drag handlers, drop zones, ghost rendering |
| `packages/core/src/stores/document-store.ts` | Add `reorderFeature` action |
| `packages/core/src/stores/ui-store.ts` | Add drag state (`draggedFeatureId`, `dropTargetIndex`) |
| `packages/engine/src/evaluate.ts` | Add `computeReorderImpact` for preview |

### Key Considerations

- Reuse existing dependency graph from evaluation engine
- Preview computation should be <50ms for responsive feel
- Consider debouncing preview updates during fast drag
- Ghost image via HTML5 drag API or custom render

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add drag state to ui-store (`draggedFeatureId`, `dropTargetIndex`, `reorderPreview`) (`xs`)
- [ ] Add `reorderFeature` action to document-store (`s`)
- [ ] Add `computeReorderImpact` function to engine (`m`)

### Phase 2: Drag Interaction

- [ ] Add drag handle to feature row (`xs`)
- [ ] Implement drag initiation with 200ms hold delay (`xs`)
- [ ] Render ghost image during drag (`s`)
- [ ] Show drop indicator line between rows (`s`)

### Phase 3: Live Preview

- [ ] Compute valid drop range from dependency graph (`s`)
- [ ] Highlight invalid zones during drag (`s`)
- [ ] Show red/yellow highlights on affected features (`s`)
- [ ] Display warning popover for breaking changes (`s`)

### Phase 4: Commit and Polish

- [ ] Execute reorder on drop (`xs`)
- [ ] Integrate with undo/redo system (`s`)
- [ ] Show toast notification with undo button (`xs`)
- [ ] Keyboard accessibility (arrow keys to move selected feature) (`s`)

## Acceptance Criteria

- [ ] User can drag any feature to a new position in the tree
- [ ] Ghost image follows cursor during drag
- [ ] Drop indicator shows insertion point between rows
- [ ] Invalid positions are visually dimmed and blocked
- [ ] Features that would break are highlighted red during hover
- [ ] Warning popover appears before committing a breaking reorder
- [ ] Cmd+Z undoes the reorder
- [ ] Toast notification confirms the move with undo button
- [ ] Preview computation completes in <50ms

## Future Enhancements

- [ ] Multi-select drag (reorder multiple features at once)
- [ ] Cross-part reordering in assemblies
- [ ] Drag to change feature parent (reparenting)
- [ ] Keyboard shortcut to move selected feature up/down (Alt+Up/Down)
