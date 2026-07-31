# Feature Grouping

Organize features into named groups (folders) for better model navigation.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p3` |
| Effort | `s` |

> Verified against repo 2026-07-30. Still unbuilt — no `FeatureGroup` type or `featureGroups` state exists in the codebase.

## Problem

The feature tree is a flat chronological list. As models grow, this becomes unwieldy:

- A 50-feature model shows 50 items with no hierarchy
- Users mentally segment their design ("the handle", "the body", "mounting hardware") but the tree doesn't reflect this
- Finding specific features requires scrolling and reading names
- No way to collapse irrelevant sections while working on one area

Real designs have logical structure. The feature tree should too.

## Solution

Let users create named groups (folders) to organize features visually. Groups are purely organizational — they don't affect evaluation order or feature dependencies.

### Example Structure

```
Body
  ├─ Base Extrude
  ├─ Side Cut
  └─ Edge Fillets
Handle
  ├─ Handle Extrude
  ├─ Grip Pattern
  └─ Handle Fillets
Hardware
  ├─ Mounting Holes
  └─ Screw Counterbores
```

**Not included:** Nested groups (only one level of hierarchy).

## UX Details

### Creating Groups

| Action | Method |
|--------|--------|
| New empty group | Right-click tree → "New Group" |
| Group from selection | Select features → right-click → "Group Selected" |
| Rename group | Double-click group name |
| Delete group | Right-click → "Ungroup" (features move to root, not deleted) |

### Organizing Features

- **Drag to reorder:** Drag features within a group to change visual order
- **Drag between groups:** Move features from one group to another
- **Drag to root:** Pull feature out of group to ungroup it
- **Multi-select:** Shift/Cmd-click to select multiple features, then drag together

### Collapse/Expand

- Click chevron to collapse/expand a group
- Double-click group row to toggle
- Right-click → "Collapse All" / "Expand All"
- Collapsed groups show feature count: `Handle (3)`

### Visual Design

- Groups have folder icon + bold name
- Indentation shows hierarchy (one level only — no nested groups)
- Subtle background tint per group (optional, user preference)
- Color coding: optional per-group color shows as left border stripe

### Color Coding (Optional)

Users can assign a color to each group:
- Right-click group → "Set Color"
- Color appears as 3px left border on all features in group
- Same color can highlight geometry in viewport when group is selected
- Default: no color (neutral gray)

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Cmd+G` | Group selected features |
| `Cmd+Shift+G` | Ungroup (move to root) |
| `Left/Right` | Collapse/expand focused group |

### Behavior Rules

1. **Visual only:** Groups don't affect build order. Feature A in group "Handle" can depend on Feature B in group "Body".
2. **New features:** Added to currently expanded/selected group, or root if none selected.
3. **Suppression:** Suppressing a group suppresses all features within it.
4. **Undo:** Group operations (create, delete, move) are undoable.
5. **Search:** Search still finds features regardless of group. Found features auto-expand their parent group.

## Implementation

### Data Model

Groups stored in document alongside features:

```typescript
interface FeatureGroup {
  id: string;
  name: string;
  color?: string;        // hex color for visual coding
  featureIds: string[];  // ordered list of feature IDs in this group
  collapsed: boolean;    // UI state
}

interface Document {
  features: Feature[];
  featureGroups: FeatureGroup[];  // features not in any group shown at root
}
```

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Render groups, handle collapse/expand |
| `packages/core/src/stores/document-store.ts` | Add `featureGroups` to document state |
| `packages/core/src/stores/ui-store.ts` | Track collapsed group IDs |
| `packages/ir/src/index.ts` | Add `FeatureGroup` type definition |

### Dependencies

- `@dnd-kit` or similar for drag-and-drop between groups

## Tasks

### Phase 1: Data Model

- [ ] Add `FeatureGroup` type to IR (`xs`)
- [ ] Add `featureGroups` array to document store (`xs`)
- [ ] Add collapsed group IDs to UI store (`xs`)

### Phase 2: Basic UI

- [ ] Render group rows in FeatureTree (`s`)
- [ ] Collapse/expand toggle with chevron (`xs`)
- [ ] Show feature count on collapsed groups (`xs`)
- [ ] Group context menu (New Group, Rename, Ungroup) (`s`)

### Phase 3: Drag and Drop

- [ ] Add drag-drop library (`xs`)
- [ ] Drag features into groups (`s`)
- [ ] Drag features out of groups to root (`xs`)
- [ ] Drag features between groups (`xs`)
- [ ] Multi-select drag (`s`)

### Phase 4: Polish

- [ ] Keyboard shortcuts (Cmd+G, Cmd+Shift+G) (`xs`)
- [ ] Color coding per group (`s`)
- [ ] Search auto-expands matching groups (`xs`)
- [ ] Undo/redo integration for group operations (`s`)

## Acceptance Criteria

- [ ] Can create empty group via context menu
- [ ] Can create group from selected features with Cmd+G
- [ ] Can rename groups by double-clicking
- [ ] Can delete groups (features move to root, not deleted)
- [ ] Can drag features into/out of/between groups
- [ ] Collapse/expand groups with chevron or keyboard
- [ ] Collapsed groups show feature count
- [ ] Group state persists in document save/load
- [ ] Optional color coding per group with left border stripe
- [ ] Suppressing group suppresses all contained features
- [ ] Search auto-expands groups containing matches
- [ ] All group operations are undoable

## Future Enhancements

- [ ] Nested groups (multiple levels of hierarchy)
- [ ] Auto-grouping suggestions based on feature dependencies
- [ ] Group templates for common patterns (e.g., "Hole Pattern" group)
- [ ] Bulk operations on groups (suppress all, hide all)
