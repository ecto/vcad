# Timeline Scrubber

A timeline scrubber that lets users drag through feature history and see the model at any point in its construction.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `m` |

> Verified against repo 2026-07-30. Still `proposed`: no `TimelineScrubber.tsx`/`timelineIndex` feature-history scrubbing exists. (Not to be confused with the shipped animation timeline `AnimationTimeline.tsx`, which scrubs simulation time, not feature history.)

## Problem

Users cannot easily visualize how a model was built step-by-step. Understanding the construction history of a complex part requires mentally reconstructing each operation, which is error-prone and time-consuming. This is especially problematic when:

- Learning from someone else's model
- Debugging why a boolean operation failed
- Teaching CAD techniques
- Reviewing design decisions

Desktop CAD tools (SolidWorks, Fusion 360) have feature trees but no intuitive way to "replay" model construction.

## Solution

A timeline scrubber that lets users drag through feature history and see the model at any point in its construction. Displays "Feature 7 of 12" and renders the model state at that step.

Place the scrubber in a **dedicated toolbar below the 3D viewport**, horizontally spanning the viewport width:

```
┌─────────────────────────────────────────┐
│                                         │
│              3D Viewport                │
│                                         │
├─────────────────────────────────────────┤
│ ◀ ▶ ▷  ───●─────────────  Feature 7/12 │
└─────────────────────────────────────────┘
```

**Not included:** Sketch state scrubbing, assembly timeline, editing while rolled back (see Future Enhancements).

## UX Details

### Controls

| Control | Action |
|---------|--------|
| Drag slider | Scrub to any feature |
| Click track | Jump to that position |
| `←` / `→` | Step backward/forward one feature |
| `Home` / `End` | Jump to first/last feature |
| `Space` | Play/pause animation |
| Play button (▷) | Animate through history at 1 feature/sec |
| Speed control | 0.5x / 1x / 2x playback speed |

### Visual Indicators

- **Current position:** Filled circle on track, feature count label ("Feature 7 of 12")
- **Feature tree sync:** Highlight the corresponding feature in the tree, dim features after current position
- **Rollback state badge:** Show "Rolled Back" indicator when not at latest feature
- **Transition animation:** Brief 150ms fade when changing states (optional, toggle in preferences)

### Interaction States

| State | Behavior |
|-------|----------|
| Normal editing | Scrubber at end, all features visible |
| Scrubbing | Model updates in real-time as user drags |
| Rolled back | Editing disabled, "Resume to edit" button shown |
| Playing | Auto-advancing, pause on click anywhere |

### Edge Cases

- **Empty document:** Hide scrubber or show disabled state
- **Single feature:** Show "Feature 1 of 1", disable navigation
- **Failed feature:** Show error state at that point, allow scrubbing past
- **Heavy models:** Show loading indicator during evaluation, debounce rapid scrubbing

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/TimelineScrubber.tsx` | New main scrubber component |
| `packages/app/src/components/TimelinePlayButton.tsx` | New play/pause control |
| `packages/app/src/components/TimelineTrack.tsx` | New slider track with markers |
| `packages/core/src/stores/document-store.ts` | Add timeline state and actions |
| `packages/app/src/components/ViewportContent.tsx` | Subscribe to timeline index for rendering |
| `packages/app/src/components/FeatureTree.tsx` | Sync highlighting with timeline position |

### State Additions

```typescript
// In document store
interface TimelineState {
  currentFeatureIndex: number;
  totalFeatures: number;
  isPlaying: boolean;
  playbackSpeed: number;
}

// Add to document-store.ts:
// - timelineIndex: number (current position, 0 to operations.length - 1)
// - setTimelineIndex(n) (update position, trigger re-evaluation)
// - isRolledBack: boolean (derived from timelineIndex < operations.length - 1)
```

### Algorithm

```typescript
function evaluateAtFeature(doc: Document, index: number): Mesh {
  const truncatedOps = doc.operations.slice(0, index + 1);
  return evaluate(truncatedOps);
}
```

### Caching Strategy

Cache intermediate mesh states to enable smooth scrubbing:

1. **Eager caching:** On document load, evaluate and cache all states in background
2. **LRU eviction:** Keep last N states in memory (configurable, default 20)
3. **Invalidation:** Clear cache from modified feature onward when document changes

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add `timelineIndex` and `setTimelineIndex` to document-store (`xs`)
- [ ] Add `isRolledBack` derived state (`xs`)
- [ ] Create `evaluateAtFeature()` function in engine (`s`)
- [ ] Implement LRU cache for intermediate mesh states (`s`)

### Phase 2: Scrubber UI

- [ ] Create `<TimelineTrack>` slider component (`s`)
- [ ] Create `<TimelineScrubber>` container component (`s`)
- [ ] Add scrubber toolbar below viewport (`xs`)
- [ ] Wire up slider to `setTimelineIndex` (`xs`)

### Phase 3: Playback Controls

- [ ] Create `<TimelinePlayButton>` component (`xs`)
- [ ] Implement play/pause animation logic with `requestAnimationFrame` (`s`)
- [ ] Add playback speed control (0.5x/1x/2x) (`xs`)

### Phase 4: Feature Tree Integration

- [ ] Highlight current feature in tree based on timeline index (`s`)
- [ ] Dim features after current position (`xs`)
- [ ] Show "Rolled Back" badge when not at latest (`xs`)

### Phase 5: Polish

- [ ] Add keyboard shortcuts (←/→, Home/End, Space) (`s`)
- [ ] Add 150ms fade transition between states (`xs`)
- [ ] Handle edge cases (empty doc, single feature, failed feature) (`s`)
- [ ] Add loading indicator for heavy model evaluation (`xs`)
- [ ] Debounce rapid scrubbing to prevent jank (`xs`)

## Acceptance Criteria

- [ ] Dragging the slider updates the 3D viewport to show the model at that feature index
- [ ] "Feature N of M" label displays correct count and updates during scrubbing
- [ ] Arrow keys (←/→) step one feature at a time
- [ ] Home/End jump to first/last feature
- [ ] Space toggles play/pause animation
- [ ] Play button animates through history at configurable speed
- [ ] Feature tree highlights the current feature and dims later features
- [ ] "Rolled Back" indicator shows when not at latest feature
- [ ] Scrubbing is smooth (<100ms latency) for models with up to 20 features
- [ ] Empty documents hide or disable the scrubber gracefully

## Future Enhancements

- [ ] Show sketch states during scrubbing, not just solid operations
- [ ] Allow editing while rolled back (insert feature at current position)
- [ ] Include assembly timeline (joint additions, not just part operations)
- [ ] Export timeline animation as video/GIF for presentations
- [ ] Add markers on timeline for failed/warning features
- [ ] Success metrics tracking: time to understand model, feature adoption rate
