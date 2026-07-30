# Command Palette

Quick access to all commands via a searchable keyboard-driven interface.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p1` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Shipped: `packages/app/src/components/CommandPalette.tsx`, command registry in `packages/app/src/hooks/useAppCommands.ts` (⌘K).

## Problem

Menu-based navigation is slow. Users need to:

1. Move hand from keyboard to mouse
2. Click through nested menus to find commands
3. Remember which menu contains which action
4. Wait for menu animations

Power users make dozens of actions per session. The friction adds up—especially for keyboard-first workflows common in professional CAD tools.

## Solution

Searchable command palette activated with `Cmd+K` (Mac) / `Ctrl+K` (Windows/Linux):

```
┌─────────────────────────────────────────┐
│ 🔍 Type a command...              [esc] │
├─────────────────────────────────────────┤
│ 📦 Add Box                              │
│ 🔵 Add Cylinder                         │
│ 🌐 Add Sphere                           │
│ ⊕  Union                    Ctrl+Shift+U│
│ ⊖  Difference               Ctrl+Shift+D│
│ ↩  Undo                          Ctrl+Z │
│ ...                                     │
└─────────────────────────────────────────┘
```

### Features

- **Fuzzy search**: Matches command labels and keywords (e.g., "cyl" finds "Add Cylinder")
- **Keyboard-driven**: Arrow keys to navigate, Enter to execute, Escape to close
- **Shortcuts displayed**: Shows keyboard shortcut for each command when available
- **Context-aware enabling**: Commands disabled when not applicable (e.g., "Union" requires 2 selected parts)
- **Match highlighting**: Search query highlighted in results

### Commands Available

| Category | Commands |
|----------|----------|
| Primitives | Add Box, Add Cylinder, Add Sphere |
| Booleans | Union, Difference, Intersection |
| Transform | Move Mode, Rotate Mode, Scale Mode |
| Edit | Undo, Redo, Delete, Duplicate, Deselect All |
| View | Toggle Wireframe, Toggle Grid Snap, Toggle Sidebar |
| File | Save, Open, Export STL, Export GLB |
| Assembly | Create Part Definition, Insert Instance, Add Fixed/Revolute/Slider Joint, Set as Ground |
| Help | About vcad |

**Not included:** Command history, custom commands, command chaining (see Future Enhancements).

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Open | Focus on search input, full command list visible |
| Typing | List filters in real-time, selection resets to first match |
| Arrow Up/Down | Moves selection highlight, scrolls into view |
| Enter | Executes selected command, closes palette |
| Escape | Closes palette without action |
| Click command | Executes that command, closes palette |
| Click outside | Closes palette |

### Visual States

| State | Behavior |
|-------|----------|
| Default | White background, icon + label + shortcut |
| Hover | Light gray background |
| Selected | Accent color highlight |
| Disabled | 40% opacity, cursor not-allowed |

### Edge Cases

- **No matches**: Shows "No commands found" message
- **Empty query**: Shows full command list
- **Long labels**: Truncated with ellipsis (not currently an issue)
- **Disabled commands**: Shown but not executable, provides discoverability

## Implementation

### Files

| File | Purpose |
|------|---------|
| `packages/app/src/components/CommandPalette.tsx` | Main component with Radix Dialog |
| `packages/core/src/commands.ts` | Command registry and types |
| `packages/core/src/index.ts` | Exports command utilities |

### Architecture

```typescript
// Command type
interface Command {
  id: string;
  label: string;
  icon: string;
  keywords: string[];
  shortcut?: string;
  action: () => void;
  enabled?: () => boolean;
}
```

1. `createCommandRegistry()` builds command list with actions bound to store methods
2. Commands filtered by query matching label or keywords (case-insensitive substring)
3. Radix Dialog handles modal behavior and focus trap
4. Keyboard navigation managed via `onKeyDown` handler
5. Selection scrolled into view via `scrollIntoView({ block: "nearest" })`

### Dependencies

- `@radix-ui/react-dialog`: Modal/overlay behavior
- `@phosphor-icons/react`: Command icons

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] `Cmd+K` / `Ctrl+K` opens command palette
- [x] Typing filters commands by label and keywords
- [x] Arrow keys navigate selection, Enter executes
- [x] Escape closes palette without action
- [x] Keyboard shortcuts displayed for commands that have them
- [x] Disabled commands shown with reduced opacity
- [x] Search query highlighted in matching results
- [x] Selected item scrolls into view
- [x] Works on both Mac and Windows/Linux

## Future Enhancements

- [ ] Recent commands prioritized at top of list
- [ ] Command history (show last N used commands)
- [ ] Custom user-defined commands/macros
- [ ] Command chaining (run multiple commands in sequence)
- [ ] Categorized sections with headers
- [ ] Fuzzy matching (typo tolerance, word reordering)
