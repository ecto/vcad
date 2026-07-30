# Selection System

Select and manipulate parts in the viewport.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `@cam` |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30.

## Problem

Users need to identify which parts to operate on before performing any action. Without a selection system, operations like transforms, booleans, and deletions have no target. This is foundational to any CAD workflow.

## Solution

Multi-part selection system with intuitive mouse interactions:

- **Click** to select single part (replaces current selection)
- **Shift+click** to toggle/add to selection (multi-select)
- **Click empty space** to deselect all
- **Hover highlighting** shows what will be selected before clicking
- **Feature tree sync** - selection state is shared between viewport and feature tree

```
Viewport                          Feature Tree
┌─────────────────────┐          ┌──────────────────┐
│                     │          │ ○ Part A         │
│   ┌───┐             │  ←sync→  │ ● Part B (sel)   │
│   │sel│  ○ hover    │          │ ◐ Part C (hover) │
│   └───┘             │          │ ○ Part D         │
└─────────────────────┘          └──────────────────┘
```

## UX Details

### Interaction States

| State | Viewport | Feature Tree |
|-------|----------|--------------|
| Default | Normal appearance | Normal row |
| Hover | Highlight outline | Background highlight |
| Selected | Selection outline (blue) | Bold + blue accent |
| Hover + Selected | Both outlines | Bold + hover background |

### Edge Cases

- **Click on overlapping parts**: Selects frontmost (ray intersection)
- **Shift+click selected part**: Deselects it (toggle behavior)
- **Empty document**: Click does nothing gracefully
- **During gizmo drag**: Selection locked, clicks ignored

## Implementation

### Files

| File | Purpose |
|------|---------|
| `packages/core/src/stores/ui-store.ts` | Selection state and actions |
| `packages/app/src/components/ViewportContent.tsx` | Click handling, hover detection |
| `packages/app/src/components/SceneMesh.tsx` | Per-mesh selection/hover visuals |
| `packages/app/src/components/FeatureTree.tsx` | Tree row selection sync |

### State

```typescript
// packages/core/src/stores/ui-store.ts
interface UiState {
  selectedPartIds: Set<string>;
  hoveredPartId: string | null;

  select: (partId: string | null) => void;
  toggleSelect: (partId: string) => void;
  selectMultiple: (partIds: string[]) => void;
  clearSelection: () => void;
  setHoveredPartId: (partId: string | null) => void;
}
```

### Selection Logic

```typescript
// Single select (click)
select: (partId) =>
  set({ selectedPartIds: partId ? new Set([partId]) : new Set() })

// Toggle select (shift+click)
toggleSelect: (partId) =>
  set((s) => {
    const next = new Set(s.selectedPartIds);
    if (next.has(partId)) {
      next.delete(partId);
    } else {
      next.add(partId);
    }
    return { selectedPartIds: next };
  })

// Multi-select (programmatic)
selectMultiple: (partIds) =>
  set({ selectedPartIds: new Set(partIds) })

// Clear (click empty)
clearSelection: () => set({ selectedPartIds: new Set() })
```

## Tasks

All tasks complete:

- [x] Add `selectedPartIds: Set<string>` to ui-store
- [x] Add `hoveredPartId: string | null` to ui-store
- [x] Implement `select()` for single selection
- [x] Implement `toggleSelect()` for shift+click
- [x] Implement `selectMultiple()` for programmatic use
- [x] Implement `clearSelection()` for deselect all
- [x] Wire up viewport click handler
- [x] Wire up viewport hover detection (pointer move + raycasting)
- [x] Add selection outline shader to SceneMesh
- [x] Add hover highlight shader to SceneMesh
- [x] Sync feature tree row selection with viewport
- [x] Handle click-on-empty to deselect

## Acceptance Criteria

- [x] Clicking a part selects it and deselects others
- [x] Shift+clicking adds/removes parts from selection
- [x] Clicking empty space clears selection
- [x] Hovering a part shows highlight before click
- [x] Feature tree selection syncs bidirectionally with viewport
- [x] Multiple parts can be selected simultaneously
- [x] Selection persists across viewport interactions (orbit, zoom)

## Future Enhancements

- [ ] Box select (drag rectangle to select enclosed parts)
- [ ] Lasso select (freeform polygon selection)
- [ ] Select by type (all cylinders, all primitives)
- [ ] Select by material (all parts with same material)
- [ ] Select similar (same dimensions, same feature type)
- [ ] Invert selection
- [ ] Select all (Cmd+A)
- [ ] Selection history (previous/next selection)
