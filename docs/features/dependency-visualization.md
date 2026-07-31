# Dependency Visualization

Make parametric dependencies visible and warn before destructive actions.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `m` |

> Verified against repo 2026-07-30. Still `proposed` — no dependency graph, hover tooltip, or delete-confirm implementation found in `packages/app/src`.

## Problem

Parametric models have hidden dependencies. A fillet depends on an edge, which depends on a boolean, which depends on a sketch. Users discover these relationships only when something breaks—delete a sketch and watch 12 features fail with cryptic "reference lost" errors.

This creates:
- Fear of editing existing features
- Cascading failures during refactoring
- Wasted time debugging broken references
- Models that become "frozen" because nobody dares touch them

## Solution

Three core features to surface dependencies:

### Hover Context Tooltip

When hovering over a feature in the tree, show a tooltip after 300ms:

```
Box1
────────────────
Uses: (none)
Used by: Fillet2, Hole3, Pattern1
```

For features with parents:

```
Fillet2
────────────────
Uses: Box1 (edge), Sketch1 (profile)
Used by: Shell1
```

### Dependency Badge

Show a small badge on features that have dependents:

```
Box1 ↓3
├─ Fillet2 ↓1
├─ Hole3
└─ Pattern1
```

The `↓3` indicates "3 features depend on this." Features with no dependents show no badge (reduces clutter).

### Pre-Delete Warning

Before deleting a feature with dependents, show impact:

```
┌─────────────────────────────────────────┐
│  Delete "Sketch1"?                      │
│                                         │
│  4 features will be affected:           │
│  • Extrude1 (will be deleted)           │
│  • Fillet2 (will be deleted)            │
│  • Hole3 (will be deleted)              │
│  • Pattern1 (will be deleted)           │
│                                         │
│  [Cancel]  [Delete Sketch1 Only]  [Delete All]  │
└─────────────────────────────────────────┘
```

Options:
- **Cancel** — abort
- **Delete X Only** — remove feature, leave dependents broken (they'll show error state)
- **Delete All** — cascade delete all affected features

**Not included in MVP:** Viewport geometry highlighting (see Future Enhancements).

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Default | Badge shows dependent count (if > 0), muted gray |
| Hover | Tooltip appears after 300ms delay, badge highlights |
| Delete pending | Badge turns red to indicate breaking change |
| Broken reference | Feature shows error state, excluded from "affected" count |

### Progressive Disclosure

- Badge shows count at a glance
- Hover shows feature names
- Click expands full dependency tree in sidebar (future)

### Edge Cases

- **No dependents**: No badge shown (reduces clutter)
- **Long dependency lists**: Tooltip truncates at 5 items, shows "+N more"
- **Circular dependencies**: Should be impossible in DAG; validate and warn if detected
- **External references**: Features referencing geometry from other parts need cross-part tracking
- **Large models**: Cache traversal results, invalidate on edit

### Preferences

- Power users can disable warnings in preferences
- Undo always available if they proceed with deletion

## Implementation

### Data Model

Build a dependency graph alongside the parametric DAG:

```typescript
interface DependencyGraph {
  // feature ID -> IDs of features it depends on
  parents: Map<string, Set<string>>;
  // feature ID -> IDs of features that depend on it
  children: Map<string, Set<string>>;
}
```

Update incrementally when features are added/removed/modified.

### Graph Construction

Walk the document's operation list. For each operation, extract references:
- `Extrude` references a sketch ID
- `Fillet` references edge IDs (which belong to a face, which belongs to a feature)
- `Boolean` references two solid IDs
- `Pattern` references a feature ID

Map geometry IDs back to their source feature to build the full graph.

### Traversal Algorithm

```typescript
function getAffectedFeatures(featureId: string, graph: DependencyGraph): string[] {
  const affected: string[] = [];
  const queue = [...graph.children.get(featureId) ?? []];
  const visited = new Set<string>();

  while (queue.length > 0) {
    const id = queue.shift()!;
    if (visited.has(id)) continue;
    visited.add(id);
    affected.push(id);
    queue.push(...graph.children.get(id) ?? []);
  }

  return affected;
}
```

### Files to Modify

| File | Changes |
|------|---------|
| `packages/core/src/stores/document-store.ts` | Add `dependencyGraph`, `hoveredFeature`, computed getters |
| `packages/app/src/components/FeatureTree.tsx` | Add badge rendering, hover handlers |
| `packages/app/src/components/FeatureTooltip.tsx` | New component for dependency info |
| `packages/app/src/components/DeleteConfirmDialog.tsx` | New component for impact warning |

### Store Integration

Add to document store:

```typescript
interface DocumentStore {
  // ... existing fields
  dependencyGraph: DependencyGraph;
  hoveredFeature: string | null;

  // Computed
  getParents(id: string): string[];
  getChildren(id: string): string[];
  getAffectedByDelete(id: string): string[];
}
```

## Tasks

### Phase 1: Data Model

- [ ] Define `DependencyGraph` interface in document store (`xs`)
- [ ] Implement graph construction from operation list (`s`)
- [ ] Add incremental update on feature add/remove/modify (`s`)
- [ ] Add `getParents`, `getChildren`, `getAffectedByDelete` computed getters (`xs`)

### Phase 2: Hover Tooltip

- [ ] Add `hoveredFeature` state to store (`xs`)
- [ ] Create `<FeatureTooltip>` component (`s`)
- [ ] Wire hover handlers in `FeatureTree.tsx` (`xs`)
- [ ] Add 300ms hover delay logic (`xs`)

### Phase 3: Dependency Badge

- [ ] Add badge rendering to feature tree items (`s`)
- [ ] Style badge (muted gray default, highlight on hover) (`xs`)
- [ ] Add red badge state for pending deletions (`xs`)

### Phase 4: Delete Warning Dialog

- [ ] Create `<DeleteConfirmDialog>` component (`s`)
- [ ] Show affected feature list with impact preview (`s`)
- [ ] Implement "Delete X Only" action (leave dependents broken) (`s`)
- [ ] Implement "Delete All" cascade delete action (`m`)
- [ ] Add preference to disable warnings (`xs`)

## Acceptance Criteria

- [ ] Hovering a feature for 300ms shows tooltip with "Uses" and "Used by" lists
- [ ] Features with dependents display a badge showing the count
- [ ] Badge count accurately reflects transitive dependents
- [ ] Deleting a feature with dependents shows confirmation dialog
- [ ] Dialog lists all affected features by name
- [ ] "Cancel" aborts deletion with no changes
- [ ] "Delete X Only" removes feature and leaves dependents in error state
- [ ] "Delete All" removes feature and all transitive dependents
- [ ] Undo restores deleted features correctly
- [ ] Graph updates correctly when features are added, removed, or modified

## Future Enhancements

- [ ] Hover highlighting in viewport (parents blue, children orange)
- [ ] Faint lines connecting related geometry in 3D view
- [ ] Click badge to expand full dependency tree in sidebar
- [ ] Alt+hover modifier to enable highlighting without always-on mode
- [ ] Cross-part dependency tracking for assemblies
- [ ] Analytics: track reduction in "reference lost" errors
