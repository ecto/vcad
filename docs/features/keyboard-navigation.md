# Keyboard Navigation

Full keyboard control for power users in the feature tree.

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `s` |

> Verified against repo 2026-07-30. Core navigation shipped, implemented in `packages/app/src/hooks/useKeyboardShortcuts.ts` (not FeatureTree.tsx as planned): j/k/arrow tree navigation with `treeFocusedPartId` in ui-store, Enter to select, Backspace/Delete with confirmation dialog, Tab cycles focus zones (viewport/tree/property). Not shipped: Shift+j/k multi-select extend, `r` rename key, `/` search focus, gg/G jumps, ARIA tree attributes.

## Problem

Mouse-only navigation in the feature tree is slow for experienced users. Power users switching from vim, VS Code, or other keyboard-centric tools expect:

- Navigate without moving hands to mouse
- Rapid multi-selection for bulk operations
- Quick access to common actions (rename, delete, edit)

Current state forces mouse clicks for every tree interaction, breaking flow.

## Solution

Vim-inspired keyboard shortcuts for full feature tree control. When the tree has focus, single-key shortcuts navigate and act on features.

### Primary Key Bindings

| Key | Action |
|-----|--------|
| `j` or `↓` | Move selection down |
| `k` or `↑` | Move selection up |
| `Enter` | Edit selected feature (open inline params or dialog) |
| `Delete` / `Backspace` | Delete with confirmation |
| `r` | Rename (inline edit) |
| `Space` | Toggle expand/collapse (for groups, patterns, assemblies) |
| `/` | Focus search/filter input |
| `Escape` | Clear selection / close dialog / exit rename mode |
| `Shift+j` / `Shift+k` | Extend selection up/down |
| `Shift+Click` | Range select (standard behavior) |

### Secondary Bindings

| Key | Action |
|-----|--------|
| `g` then `g` | Jump to first feature |
| `G` | Jump to last feature |
| `Ctrl+a` | Select all |
| `d` | Duplicate selected |
| `s` | Suppress/unsuppress toggle |

## UX Details

### Focus Management

- Tree gains focus on click or when switching to Model tab
- Visual focus ring around tree container when focused
- Focus indicator on current item (distinct from selection highlight)
- `Tab` moves focus to next panel; `Shift+Tab` moves back
- Clicking viewport should not steal focus unless intentional

### Visual Focus Indicator

| State | Style |
|-------|-------|
| Focused item | 2px outline, `var(--accent-color)` |
| Selected item | Background fill, `var(--selection-bg)` |
| Focused + Selected | Both outline and fill |

### Multi-Select Behavior

- `Shift+j/k` extends selection from anchor point
- `Ctrl+Click` toggles individual items (standard)
- Bulk delete prompts: "Delete 5 features?"
- Some actions disabled for multi-select (rename, edit)

### Confirmation Dialogs

Delete confirmation shows:
- Feature name(s)
- Dependent features that will also be deleted
- "Don't ask again" checkbox (stored in preferences)

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Add keyboard event handler, focus management |
| `packages/core/src/stores/ui-store.ts` | Add `featureTreeFocused`, `selectionAnchor` state |
| `packages/app/src/styles/` | Focus indicator styles |

### State Additions

```typescript
// ui-store.ts
featureTreeFocused: boolean;
selectionAnchor: string | null;  // for shift-extend selection
```

### Event Handling

```typescript
// In FeatureTree.tsx
const handleKeyDown = (e: KeyboardEvent) => {
  if (!treeFocused) return;

  switch (e.key) {
    case 'j':
    case 'ArrowDown':
      e.preventDefault();
      moveSelection(e.shiftKey ? 'extend-down' : 'down');
      break;
    case 'k':
    case 'ArrowUp':
      e.preventDefault();
      moveSelection(e.shiftKey ? 'extend-up' : 'up');
      break;
    case 'Enter':
      editFeature(selectedId);
      break;
    case 'Delete':
    case 'Backspace':
      confirmDelete(selectedIds);
      break;
    case 'r':
      startRename(selectedId);
      break;
    case ' ':
      e.preventDefault();
      toggleExpand(selectedId);
      break;
    case '/':
      e.preventDefault();
      focusSearch();
      break;
    case 'Escape':
      clearSelection();
      break;
  }
};
```

### Accessibility

- Implements standard `aria-activedescendant` pattern
- Screen reader announces: "Cut-Extrude3, 5 of 12"
- `role="tree"` and `role="treeitem"` on elements

## Tasks

### Phase 1: Core Navigation

- [ ] Add `featureTreeFocused` state to ui-store (`xs`)
- [ ] Add `selectionAnchor` state for shift-extend selection (`xs`)
- [x] Add keyboard event handler (in `useKeyboardShortcuts.ts`) (`s`)
- [x] Implement j/k and arrow key navigation (`xs`)
- [ ] Add visual focus indicator styles (`xs`)

### Phase 2: Actions

- [ ] Wire up Enter to edit feature (`xs`)
- [x] Wire up Delete/Backspace with confirmation dialog (`s`)
- [ ] Implement inline rename with `r` key (`s`)
- [ ] Implement Space to toggle expand/collapse (`xs`)
- [ ] Wire up `/` to focus search input (`xs`)
- [ ] Implement Escape to clear selection (`xs`)

### Phase 3: Multi-Select

- [ ] Implement Shift+j/k to extend selection (`s`)
- [ ] Add selection anchor tracking (`xs`)
- [ ] Update bulk delete confirmation UI (`xs`)

### Phase 4: Secondary Bindings

- [ ] Implement `gg` to jump to first feature (`xs`)
- [ ] Implement `G` to jump to last feature (`xs`)
- [ ] Implement Ctrl+a to select all (`xs`)
- [ ] Implement `d` to duplicate (`xs`)
- [ ] Implement `s` to suppress/unsuppress (`xs`)

### Phase 5: Accessibility

- [ ] Add ARIA attributes (`role="tree"`, `aria-activedescendant`) (`s`)
- [ ] Test with screen reader (`s`)

## Acceptance Criteria

- [x] `j/k` and arrow keys navigate feature tree
- [ ] `Enter` opens edit mode for selected feature
- [x] `Delete` prompts confirmation, then removes feature
- [ ] `r` starts inline rename
- [ ] `Space` expands/collapses groups
- [ ] `/` focuses search input
- [ ] `Escape` clears selection
- [ ] `Shift+j/k` extends multi-selection
- [ ] Focus indicator visible and distinct from selection
- [ ] Works with screen readers (ARIA compliant)

## Future Enhancements

- [ ] Customizable key bindings (user preferences)
- [ ] Vim-style command mode (`:` prefix for advanced commands)
- [ ] Macro recording and playback
- [ ] Jump to feature by typing name (fuzzy search)
