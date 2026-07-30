# Smart Auto-Names

Generate descriptive feature names automatically from parameters and context.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `s` |

> Verified against repo 2026-07-30. Still `proposed`: no `packages/engine/src/naming.ts` and no `nameSource` field exist.

## Problem

CAD features get meaningless auto-generated names:
- "Cut-Extrude1"
- "Fillet1"
- "Boolean1"
- "Pattern2"

Every CAD user knows they *should* rename features for clarity. Nobody does. Feature trees become unreadable, making it hard to:
- Find specific operations later
- Understand model intent
- Collaborate with others
- Debug failed operations

## Solution

Generate descriptive names from feature parameters, target geometry, and context:

| Before | After |
|--------|-------|
| Cut-Extrude1 | Ø5 Hole at Center |
| Fillet1 | 2mm Fillet on Box Edges |
| Boolean1 | Subtract Cylinder from Cube |
| Pattern1 | 4x Mounting Holes |

Names should be:
- **Descriptive** - convey what the feature does
- **Concise** - fit in the feature tree without truncation
- **Unique** - distinguish similar operations

### Name Generation Formula

```
[Qualifier] + [Operation] + [Target Context]
```

Where:
- **Qualifier** = key dimension or count (Ø5, 2mm, 4x, 45°)
- **Operation** = what it does (Hole, Fillet, Cut, Boss)
- **Target Context** = where it applies (on Box Edges, at Center, from Cube)

### Feature Type Examples

#### Extrude
| Parameters | Generated Name |
|------------|----------------|
| Cut, circular profile, Ø5mm | Ø5 Hole |
| Cut, circular, Ø5mm, through all | Ø5 Through Hole |
| Cut, rectangular, 10x20mm | 10x20 Pocket |
| Add, rectangular, 50x30mm, 10mm deep | 50x30x10 Boss |
| Add, circular, Ø8mm, 5mm tall | Ø8x5 Pin |

#### Revolve
| Parameters | Generated Name |
|------------|----------------|
| Add, 360°, circular profile | Turned Cylinder |
| Add, 360°, complex profile | Turned Profile |
| Cut, 360°, on cylinder | Turned Groove |
| Add, 90° | 90° Revolved Section |

#### Fillet
| Parameters | Generated Name |
|------------|----------------|
| 2mm radius, 4 edges on box | 2mm Fillet on Box Edges |
| 1mm radius, single edge | 1mm Edge Fillet |
| 5mm radius, all edges | 5mm Full Fillet |
| Variable 2-5mm | Variable Fillet (2-5mm) |

#### Chamfer
| Parameters | Generated Name |
|------------|----------------|
| 1mm x 45° | 1mm Chamfer |
| 2mm x 3mm | 2x3mm Chamfer |
| On hole edge | Hole Chamfer |

#### Boolean
| Parameters | Generated Name |
|------------|----------------|
| Subtract cylinder from cube | Subtract Cylinder from Cube |
| Union two boxes | Join Boxes |
| Intersect sphere and cube | Sphere-Cube Intersection |

#### Pattern
| Parameters | Generated Name |
|------------|----------------|
| Linear, 4 copies, of hole | 4x Holes Linear |
| Circular, 6 copies, 60° spacing | 6x Circular Pattern |
| Linear, 3x2 grid | 3x2 Grid Pattern |

#### Shell
| Parameters | Generated Name |
|------------|----------------|
| 2mm wall, open top | 2mm Shell (Open Top) |
| 1.5mm wall, closed | 1.5mm Shell |

#### Mirror
| Parameters | Generated Name |
|------------|----------------|
| Mirror about XY plane | Mirror about XY |
| Mirror hole feature | Mirrored Hole |

### Edge Cases

- **Duplicate names**: Append suffix (A, B, C) not numbers
- **Complex geometry**: Fall back to generic + target ("Cut on Face 3")
- **User renamed**: Never overwrite manual names
- **Name too long**: Truncate with ellipsis, full name in tooltip

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Feature created | Auto-name generated immediately |
| Parameter changed | Auto-name updates if `nameSource: 'auto'` |
| User edits name | Sets `nameSource: 'user'`, stops auto-updates |
| Right-click feature | Context menu shows "Reset to auto-name" option |
| Name truncated | Full name shown in tooltip on hover |

### Edge Cases

- **Long parameter names**: Truncate with ellipsis, full name in tooltip
- **Similar features**: Append suffix (A, B, C) to distinguish
- **Renamed features**: Never overwrite, show "Reset to auto-name" in context menu

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/engine/src/naming.ts` | New file: name generation logic |
| `packages/ir/src/index.ts` | Add `nameSource` field to Feature type |
| `packages/app/src/stores/document-store.ts` | Integrate auto-naming on feature creation/edit |
| `packages/app/src/components/FeatureTree.tsx` | Show truncated names with tooltip |
| `packages/app/src/components/FeatureContextMenu.tsx` | Add "Reset to auto-name" option |

### State Additions

```typescript
// Feature type addition
interface Feature {
  id: string;
  name: string;
  nameSource: 'auto' | 'user';  // Track origin
  // ...
}
```

### API

```typescript
interface FeatureNamer {
  // Generate name for a feature
  generateName(feature: Feature, context: ModelContext): string;

  // Check if name was auto-generated (vs user-set)
  isAutoGenerated(feature: Feature): boolean;

  // Regenerate name after feature edit
  updateName(feature: Feature, context: ModelContext): void;
}
```

## Tasks

### Phase 1: Core Infrastructure

- [ ] Add `nameSource` field to Feature type in IR (`xs`)
- [ ] Create `packages/engine/src/naming.ts` with `FeatureNamer` interface (`xs`)
- [ ] Implement `generateName()` for primitive operations (box, cylinder, sphere, cone) (`s`)

### Phase 2: Feature-Specific Naming

- [ ] Implement naming for extrude operations (`s`)
- [ ] Implement naming for revolve operations (`xs`)
- [ ] Implement naming for fillet/chamfer operations (`xs`)
- [ ] Implement naming for boolean operations (`xs`)
- [ ] Implement naming for pattern operations (`xs`)
- [ ] Implement naming for shell/mirror operations (`xs`)

### Phase 3: Integration

- [ ] Integrate auto-naming into document store on feature creation (`s`)
- [ ] Update auto-name when feature parameters change (`s`)
- [ ] Add "Reset to auto-name" context menu option (`xs`)

### Phase 4: Polish

- [ ] Handle duplicate name suffixing (A, B, C) (`xs`)
- [ ] Add tooltip for truncated names in feature tree (`xs`)
- [ ] Write unit tests for name generation (`s`)

## Acceptance Criteria

- [ ] New features receive descriptive auto-generated names (not "Extrude1")
- [ ] Auto-names update when feature parameters change
- [ ] Manually renamed features are never overwritten by auto-naming
- [ ] Duplicate names get unique suffixes (A, B, C)
- [ ] "Reset to auto-name" option appears in feature context menu
- [ ] Truncated names show full text in tooltip
- [ ] All feature types have meaningful name templates

## Future Enhancements

- [ ] LLM-powered naming for complex features ("Mounting bracket cutout")
- [ ] Learn naming patterns from user edits
- [ ] Batch rename with find/replace
- [ ] Name templates configurable per-user
- [ ] Metrics tracking: % auto vs user names, feature tree readability
