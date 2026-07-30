# Health & Intelligence

AI-assisted health monitoring that proactively warns, detects waste, and suggests improvements.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p3` |
| Effort | `l` |

> Verified against repo 2026-07-30. Still unbuilt as described (no health badges/`HealthCheck` in the app). Note: manufacturing DFM checks — listed here as a future enhancement — shipped separately via the `dfm_check`/`dfm_suggest_fix` MCP tools and `lib/dfm` rule packs.

## Problem

Traditional CAD workflows surface problems too late:

- **Late error discovery** - Geometry issues only appear after rebuild, often cascading through the feature tree
- **Tree clutter** - Unused or ineffective features accumulate, making models harder to understand and modify
- **Suboptimal modeling** - Users miss opportunities to simplify or consolidate features, leading to slower rebuilds and fragile models

## Solution

Three integrated subsystems that monitor model health:

### 1. Warning Badges

Proactive warnings before features break or cause downstream issues.

| Warning | Trigger | Severity |
|---------|---------|----------|
| Geometry too thin | Wall thickness below material/process threshold | Warning |
| Self-intersection detected | Boolean or sweep creates invalid geometry | Error |
| Fragile dependency | Feature will become invalid if referenced dimension changes by >X% | Warning |
| Tangent edge loss | Fillet/chamfer edge disappears at certain parameter values | Warning |
| Near-zero volume | Cut removes negligible material (<0.01mm^3) | Info |

### 2. Dead Feature Detection

Identify features with no geometric effect.

| Detection | Example |
|-----------|---------|
| Zero-volume operation | Cut that doesn't intersect body |
| Redundant feature | Mirror that overlaps original |
| Orphaned suppression | Suppressed feature with no active references |
| Shadow feature | Feature fully consumed by later operation |

### 3. Optimization Hints

Suggest modeling improvements.

| Hint | Condition | Benefit |
|------|-----------|---------|
| Consolidate sketches | Multiple extrudes from coplanar sketches | Fewer features, faster rebuild |
| Use symmetry | Linear pattern with count=2 and symmetric spacing | Cleaner intent, easier modification |
| Combine booleans | Sequential unions/subtractions on same body | Reduced tree depth |
| Simplify fillet chain | Multiple fillets with same radius on connected edges | Single fillet operation |
| Reorder for stability | Feature references face that may change | More robust tree |

**Not included (MVP):** LLM-assisted suggestions, manufacturing DFM checks, cost estimation.

## UX Details

### Badge System

```
[!] Warning - hover for details
[x] Error - must fix before export
[i] Info - optimization opportunity
[~] Dead - no geometric effect
```

Badges appear:
- Feature tree: inline with feature name
- Viewport: 3D indicator on affected geometry (optional)
- Status bar: summary count

### Hover Details

```
+----------------------------------+
| Geometry too thin                |
| Wall thickness: 0.3mm            |
| Minimum for FDM: 0.8mm           |
|                                  |
| [Fix: Add offset] [Ignore]       |
+----------------------------------+
```

### Quick Actions

| Action | Behavior |
|--------|----------|
| Fix | Apply suggested correction (with preview) |
| Ignore | Suppress this warning for this feature |
| Ignore All | Suppress warning type globally |
| Delete | Remove dead feature |
| Learn More | Link to documentation |

### Settings

- Enable/disable by warning type
- Set thresholds (min wall thickness, near-zero volume tolerance)
- Choose check frequency (live, on-save, manual)

### Edge Cases

- **Many warnings**: Collapse into summary, expand on click
- **Stale warnings**: Re-validate on feature edit, clear if resolved
- **Conflicting hints**: Prioritize by severity, show most important first

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/app/src/components/FeatureTree.tsx` | Render health badges on feature nodes |
| `packages/core/src/stores/document-store.ts` | Add health check results to document state |
| `packages/engine/src/evaluate.ts` | Run health checks after feature evaluation |
| `packages/app/src/components/HealthBadge.tsx` | New component for badge display |
| `packages/app/src/components/HealthPopover.tsx` | New component for hover details |

### State Additions

```typescript
// document-store.ts
interface HealthCheck {
  featureId: string;
  type: 'warning' | 'error' | 'info' | 'dead';
  code: string;
  message: string;
  details?: Record<string, unknown>;
  fix?: HealthFix;
}

interface HealthFix {
  label: string;
  action: () => void;
  preview?: () => Mesh;
}

interface DocumentState {
  healthChecks: HealthCheck[];
  suppressedWarnings: Set<string>; // featureId:code
}
```

### Algorithm

1. After feature evaluation completes:
   a. Run lightweight geometry checks (thin walls, intersections)
   b. Compare volume/topology before vs after for dead detection
   c. Pattern match tree structure for optimization hints
2. Cache results keyed by feature hash
3. Only recompute on dependency changes
4. Store suppressions in document metadata

### Performance Considerations

- Incremental checks: only re-evaluate affected features
- Background thread for expensive analysis (thin wall detection)
- Debounce during rapid edits (300ms)

## Tasks

### Phase 1: Infrastructure

- [ ] Add `HealthCheck` type definitions (`xs`)
- [ ] Add `healthChecks` array to document store (`xs`)
- [ ] Create `<HealthBadge>` component shell (`s`)
- [ ] Wire badge rendering into FeatureTree (`s`)

### Phase 2: Warning Badges

- [ ] Implement self-intersection detection check (`m`)
- [ ] Implement thin wall detection check (`m`)
- [ ] Implement near-zero volume check (`s`)
- [ ] Create `<HealthPopover>` with details view (`s`)

### Phase 3: Dead Feature Detection

- [ ] Track volume delta per feature during evaluation (`s`)
- [ ] Implement zero-volume operation detection (`s`)
- [ ] Implement redundant feature detection (`m`)
- [ ] Add "Delete" quick action for dead features (`xs`)

### Phase 4: Optimization Hints

- [ ] Implement pattern matcher for tree structure (`m`)
- [ ] Add "consolidate sketches" hint (`s`)
- [ ] Add "combine booleans" hint (`s`)
- [ ] Add "simplify fillet chain" hint (`s`)

### Phase 5: Polish

- [ ] Add suppression persistence to document format (`s`)
- [ ] Add settings panel for thresholds (`s`)
- [ ] Add status bar summary count (`xs`)
- [ ] Performance optimization and caching (`m`)

## Acceptance Criteria

- [ ] Warning badges appear on features with geometry issues
- [ ] Error badges block export until resolved
- [ ] Dead features are visually distinct and deletable with one click
- [ ] Hovering a badge shows details and available actions
- [ ] "Fix" action applies correction with undo support
- [ ] "Ignore" suppresses warning for that feature only
- [ ] Suppressions persist when document is saved and reopened
- [ ] Health checks run incrementally (not full recompute on every edit)
- [ ] Settings allow adjusting thresholds and check frequency

## Future Enhancements

- [ ] Manufacturing DFM checks - Integrate with slicer/CAM rules
- [ ] Cost estimation - Flag expensive-to-machine features
- [ ] Collaboration warnings - "This feature was modified by X, review changes"
- [ ] Learning mode - Track which hints users accept to improve suggestions
- [ ] LLM-assisted suggestions for complex optimization opportunities
- [ ] 3D viewport indicators on affected geometry
