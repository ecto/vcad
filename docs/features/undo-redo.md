# Undo/Redo

Revert and replay document changes to recover from mistakes and explore design alternatives.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. State `shipped` is correct, but the implementation has diverged from this doc: undo/redo is now CRDT-based, delegated to the engine (`undo()` / `can_undo()` on the engine handle in `packages/core/src/stores/document-store.ts`) rather than the 50-step JSON-snapshot stacks described below. The Snapshot/`undoStack`/`MAX_UNDO` sections are historical.

## Problem

Users make mistakes and need to backtrack. Without undo/redo:

1. Accidental deletions would be permanent
2. Experimenting with design changes would be risky
3. Parameter tweaking would require careful manual tracking
4. Boolean operations that produce unexpected results couldn't be reversed

Every professional CAD tool provides robust undo/redo functionality. Users expect it to work reliably across all operations.

## Solution

Full undo/redo with snapshot-based history:

### History Management

- **50-step history** (`MAX_UNDO = 50`)
- **Undo stack**: Previous document states
- **Redo stack**: States undone but available to replay
- **Automatic snapshot**: Captured before each document mutation

### Keyboard Shortcuts

| Action | macOS | Windows/Linux |
|--------|-------|---------------|
| Undo | `Cmd+Z` | `Ctrl+Z` |
| Redo | `Cmd+Shift+Z` | `Ctrl+Shift+Z` |

### Supported Operations

All document mutations are undoable:

- Add/remove primitives
- Transform operations (translate, rotate, scale)
- Boolean operations (union, difference, intersection)
- Parameter edits
- Rename operations
- Material assignments
- Sketch operations (extrude, revolve, sweep, loft)
- Assembly operations (instances, joints)
- Duplicate operations

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Undo available | Previous state exists on undo stack |
| Redo available | State exists on redo stack (cleared on new edit) |
| At oldest state | Undo stack empty, no further undo possible |
| At newest state | Redo stack empty, at current document |

### Edge Cases

- **Rapid undo/redo**: Immediate response, no debouncing
- **Undo during preview**: Preview cancelled, document reverts
- **File load**: Clears both undo and redo stacks (fresh history)
- **50+ operations**: Oldest snapshots dropped from undo stack (FIFO)

## Implementation

### Files

| File | Purpose |
|------|---------|
| `packages/core/src/stores/document-store.ts` | Undo/redo state and logic |
| `packages/app/src/hooks/useKeyboardShortcuts.ts` | Keyboard shortcut handling |

### State Structure

```typescript
// document-store.ts
interface Snapshot {
  document: string;      // JSON-serialized Document
  parts: PartInfo[];
  consumedParts: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum: number;
  actionName: string;    // Describes what action created this snapshot
}

interface DocumentState {
  undoStack: Snapshot[];
  redoStack: Snapshot[];
  undo: () => void;
  redo: () => void;
  pushUndoSnapshot: () => void;
}
```

### Algorithm

**On mutation:**
1. Capture current state as snapshot with action name
2. Push to undo stack
3. Clear redo stack (new branch invalidates redo history)
4. If undo stack exceeds 50 items, drop oldest

**On undo:**
1. Pop snapshot from undo stack
2. Push current state to redo stack
3. Restore document from popped snapshot

**On redo:**
1. Pop snapshot from redo stack
2. Push current state to undo stack
3. Restore document from popped snapshot

### Snapshot Approach

The implementation uses **full document snapshots** rather than operation-based diffing:

- **Pros**: Simple, reliable, works with any operation type
- **Cons**: Memory overhead for large documents, limited to 50 steps

The document is JSON-serialized for each snapshot, ensuring deep copies without reference issues.

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Cmd/Ctrl+Z undoes the last operation
- [x] Cmd/Ctrl+Shift+Z redoes an undone operation
- [x] Undo/redo works with all document operations
- [x] History limited to 50 steps (oldest dropped when exceeded)
- [x] Redo stack clears when new operation is performed
- [x] File load resets undo/redo history
- [x] Keyboard shortcuts work when viewport has focus
- [x] Shortcuts ignored when typing in input fields

## Future Enhancements

- [ ] Infinite undo (persist snapshots to IndexedDB)
- [ ] Branching history (explore multiple design paths)
- [ ] Operation-based undo (store diffs instead of full snapshots)
- [ ] Undo history panel (visual timeline of changes)
- [ ] Named checkpoints (user-defined save points)
- [ ] Collaborative undo (per-user history in multiplayer)
