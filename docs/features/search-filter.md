# Search & Filter

Type-to-filter with smart matching for navigating complex feature trees.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `s` |

> Verified against repo 2026-07-30. Still unimplemented — no `featureSearchQuery` in `packages/core/src/stores/ui-store.ts`; state `proposed` is accurate.

## Problem

Long feature lists become hard to scan in complex models. A 200-feature assembly turns the feature tree into a wall of text. Users waste time:

- Scrolling through dozens of "Fillet", "Cut-Extrude", "Hole" entries
- Hunting for that one sketch they need to edit
- Locating all features affected by a parameter change

## Solution

Type-to-filter with smart matching. Start typing and the tree narrows instantly to matching features.

### Search Box

- Appears at top of feature tree panel
- Press `/` anywhere to focus (vim-style, also common in Slack/GitHub)
- Placeholder: "Search features..."
- Clear button (X) on right when active

### Matching Behavior

| Query | Matches |
|-------|---------|
| `fillet` | All fillet features |
| `hole` | Hole features, "Hole Wizard" sketches |
| `5mm` | Any feature with "5mm" in parameters |
| `cylinder` | Cylinder primitives |
| `sketch` | All sketches |

- Case-insensitive
- Matches against: name, type, parameter values
- Fuzzy matching optional (e.g., "cyl" matches "Cylinder")

**Not included:** Full-text search across document metadata, cross-document search.

## UX Details

### Visual Feedback

Two modes (user preference):

1. **Hide mode** (default): Non-matches collapse/hide entirely
2. **Dim mode**: Non-matches shown at 30% opacity, matches full brightness

Matched text highlighted with background color (`#FFE066`).

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `/` | Focus search box |
| `Escape` | Clear search, unfocus |
| `Enter` | Select first match |
| `Up/Down` | Navigate matches |

### Advanced Filters

Power users can use prefix syntax for targeted searches:

#### By Type
```
type:fillet
type:extrude
type:sketch
```

#### By Status
```
status:error      # Features with evaluation errors
status:suppressed # Suppressed features
status:warning    # Features with warnings
```

#### By Dependency
```
uses:Sketch1      # Features that reference Sketch1
affects:Body2     # Features that modify Body2
```

#### Combined
```
type:hole 5mm     # Hole features with "5mm" in parameters
```

### Edge Cases

| State | Behavior |
|-------|----------|
| Empty query | Show full tree |
| No matches | Show "No features match" message |
| Long queries | Truncate display, full query in tooltip |
| Special characters | Escape regex characters in query |

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/core/src/stores/ui-store.ts` | Add `featureSearchQuery`, `featureSearchMode` state |
| `packages/app/src/components/FeatureTree.tsx` | Add `<SearchBox>` component, filter logic |
| `packages/app/src/components/SearchBox.tsx` | New component for search input |

### State Additions

```typescript
// ui-store.ts
interface UiState {
  featureSearchQuery: string;
  featureSearchMode: 'hide' | 'dim';
}
```

### Filter Logic

```typescript
function matchesSearch(feature: Feature, query: string): boolean {
  const q = query.toLowerCase();

  // Handle prefix filters
  if (q.startsWith('type:')) {
    return feature.type.toLowerCase() === q.slice(5);
  }
  if (q.startsWith('status:')) {
    return feature.status === q.slice(7);
  }
  if (q.startsWith('uses:')) {
    return feature.dependencies.includes(q.slice(5));
  }

  // General search: name, type, serialized params
  const searchable = [
    feature.name,
    feature.type,
    JSON.stringify(feature.parameters)
  ].join(' ').toLowerCase();

  return searchable.includes(q);
}
```

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add `featureSearchQuery` to ui-store (`xs`)
- [ ] Add `featureSearchMode` state with persistence (`xs`)
- [ ] Create `<SearchBox>` component shell (`s`)

### Phase 2: Search Functionality

- [ ] Implement basic text matching against feature names (`xs`)
- [ ] Add type/status prefix filter parsing (`s`)
- [ ] Add `uses:` dependency filter (`s`)
- [ ] Extend matching to include parameter values (`xs`)

### Phase 3: UI Integration

- [ ] Wire SearchBox into FeatureTree header (`xs`)
- [ ] Implement hide mode (filter array before render) (`xs`)
- [ ] Implement dim mode (opacity styling on non-matches) (`xs`)
- [ ] Add match highlighting with background color (`s`)

### Phase 4: Keyboard & Polish

- [ ] Add `/` global keyboard shortcut to focus search (`xs`)
- [ ] Add Escape to clear and unfocus (`xs`)
- [ ] Add Up/Down navigation through matches (`s`)
- [ ] Add Enter to select first match (`xs`)

## Acceptance Criteria

- [ ] Search box visible at top of feature tree
- [ ] `/` shortcut focuses search from anywhere in app
- [ ] Typing filters tree within 50ms (no perceptible delay)
- [ ] Escape clears search and returns to full tree
- [ ] `type:`, `status:`, `uses:` filters work as documented
- [ ] Dim/hide mode preference persists across sessions
- [ ] Works with 500+ feature models without lag

## Future Enhancements

- [ ] Fuzzy matching (e.g., "cyl" matches "Cylinder")
- [ ] `affects:` filter for downstream dependencies
- [ ] Search history with recent queries dropdown
- [ ] Saved searches / bookmarks
- [ ] Cross-document search in workspace
- [ ] Regex mode for power users
