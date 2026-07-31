# Fillets & Chamfers

Round or bevel edges to create realistic, manufacturable geometry.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `s` |

> Verified against repo 2026-07-30. Corrected State from `partial` to `shipped`: the kernel is well beyond the planar-only MVP described below (`crates/vcad-kernel-fillet` includes `fillet_curved.rs`, `rolling_ball.rs`, `blend_loft.rs`, `fillet_subset.rs`), the IR has `Fillet`, `Chamfer`, and selective `EdgeBlend` (with `EdgeQuery`) ops, and the app ships fillet/chamfer UI (feature table in CLAUDE.md).

## Problem

Sharp edges are unrealistic and often unmanufacturable:

1. **Aesthetics**: Real-world objects have rounded transitions
2. **Stress concentration**: Sharp internal corners create failure points
3. **Manufacturing**: CNC tools cannot produce perfectly sharp edges
4. **3D printing**: Sharp edges are fragile and prone to chipping
5. **Assembly fit**: Beveled edges ease part insertion and alignment

Without edge modification operations, users must model complex fillet/chamfer geometry manually or accept unrealistic sharp-edged parts.

## Solution

Two edge modification operations that transform sharp edges into smooth or beveled transitions:

### Fillet

Replaces an edge with a cylindrical blend surface tangent to both adjacent faces.

| Parameter | Type | Description |
|-----------|------|-------------|
| `radius` | `f64` | Radius of the cylindrical blend (mm) |

The fillet creates a smooth quarter-cylinder transition between the two faces meeting at the edge. For a cube with radius `r`, this removes material and creates 12 cylindrical surfaces.

### Chamfer

Replaces an edge with a planar bevel face.

| Parameter | Type | Description |
|-----------|------|-------------|
| `distance` | `f64` | Setback distance from the edge (mm) |

The chamfer creates a flat angled face cutting across the edge. Simpler than fillet but still removes sharp corners.

**Not included in MVP:** Variable radius, asymmetric chamfer, face fillet, auto-selection.

## UX Details

### Proposed Creation Flow

1. Select fillet or chamfer operation from toolbar
2. Click edges in viewport to select (multi-select with Shift)
3. Property panel shows radius/distance parameter
4. Scrub or type value to adjust
5. Live preview shows edge modification
6. Enter commits, Escape cancels

### Interaction States

| State | Behavior |
|-------|----------|
| Hover | Edge highlights on hover |
| Selected | Edge shows selection color, adds to selection set |
| Preview | Ghosted fillet/chamfer geometry overlaid on model |
| Applied | Edge modification committed to document |

### Edge Cases

- **Radius too large**: Show error if radius exceeds half the shortest adjacent edge
- **Intersecting fillets**: Warn if multiple fillets would overlap at a vertex
- **Non-planar faces**: Currently only planar faces supported, show error for curved

## Implementation

### Current State (Kernel Complete)

The kernel fully implements both operations in `crates/vcad-kernel-fillet`:

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-fillet/src/lib.rs` | Core fillet and chamfer algorithms |

**Key functions:**
- `chamfer_all_edges(brep, distance)` - Chamfer all edges with uniform distance
- `fillet_all_edges(brep, radius)` - Fillet all edges with uniform radius

**Algorithm overview:**

1. **Extract topology** - Gather face and edge information from BRep
2. **Compute trim vertices** - Calculate inward-offset positions for each vertex-face pair
3. **Build modified faces** - Original faces with trimmed vertices
4. **Build edge faces** - Chamfer (planar) or fillet (cylindrical) surfaces
5. **Build vertex faces** - Triangular/polygonal faces at corners
6. **Pair half-edges** - Establish twin relationships for manifold topology

### Missing (UI Needed)

| File | Changes Needed |
|------|----------------|
| `packages/app/src/components/PropertyPanel.tsx` | Add fillet/chamfer parameter inputs |
| `packages/app/src/components/ViewportContent.tsx` | Edge selection highlighting |
| `packages/core/src/stores/selection-store.ts` | Support edge selection mode |
| `packages/ir/src/index.ts` | IR types for selective edge operations |
| `packages/engine/src/evaluate.ts` | Evaluation of edge-specific fillet/chamfer |

### Data Structures Needed

```typescript
// IR operation for selective fillet
interface FilletOp {
  type: 'fillet';
  target: string;      // Reference to source solid
  edges: number[];     // Edge indices to fillet (empty = all)
  radius: number;
}

// IR operation for selective chamfer
interface ChamferOp {
  type: 'chamfer';
  target: string;
  edges: number[];
  distance: number;
}
```

## Tasks

### Phase 1: Core Infrastructure (Complete)

- [x] Fillet algorithm in kernel (`m`)
- [x] Chamfer algorithm in kernel (`m`)
- [x] Topology extraction helpers (`s`)
- [x] Trim vertex computation (`s`)
- [x] Twin half-edge pairing (`xs`)
- [x] Unit tests for cube chamfer/fillet (`s`)

### Phase 2: UI Implementation

- [ ] Edge selection mode in selection store (`s`)
- [ ] Edge hover highlighting in viewport (`s`)
- [x] Fillet/chamfer property panel UI (`s`)
- [ ] Preview mode for edge modifications (`s`)
- [x] IR types for selective edge operations (`EdgeBlend` + `EdgeQuery`) (`xs`)
- [x] Engine evaluation for selective fillet/chamfer (`s`)

## Acceptance Criteria

- [x] `chamfer_all_edges` produces valid manifold topology
- [x] `fillet_all_edges` creates cylindrical surfaces for each edge
- [x] Chamfered/filleted cube passes volume tests
- [x] All half-edges properly paired in output
- [ ] Can select individual edges in viewport
- [ ] Property panel shows radius/distance input
- [ ] Preview shows fillet/chamfer before committing
- [ ] Operation appears in feature tree
- [ ] Undo/redo works for fillet/chamfer operations

## Future Enhancements

- [ ] Variable radius fillet (radius changes along edge length)
- [ ] Asymmetric chamfer (different distances on each face)
- [ ] Face fillet (blend between faces rather than edges)
- [ ] Chain selection (select all connected tangent edges)
- [ ] Auto-fillet for printability (suggest radii based on material)
- [x] Fillet/chamfer on curved face edges (non-planar support) (`fillet_curved.rs`)
- [ ] G2-continuous fillets (curvature-continuous blends)
- [x] Rolling ball blend for complex edge configurations (`rolling_ball.rs`)
