# Boolean Operations

Combine solids with union, difference, and intersection operations.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `@cam` |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Shipped: `crates/vcad-kernel-booleans` (4-stage pipeline: AABB filter, surface-surface intersection, face classification, sewing).

## Problem

Simple primitive shapes (boxes, cylinders, spheres, cones) cannot represent real-world geometry. Users need to combine these shapes into complex forms:

- A box with a hole through it (difference)
- Two overlapping cylinders joined together (union)
- The common volume where two shapes overlap (intersection)

Without boolean operations, parametric CAD is limited to isolated primitives. This is the foundational capability that makes CAD useful.

## Solution

Three boolean operations that combine pairs of BRep solids:

| Operation | Description |
|-----------|-------------|
| **Union** | Merge two solids into one, keeping all exterior surfaces |
| **Difference** | Subtract one solid from another, creating cavities or holes |
| **Intersection** | Keep only the volume where both solids overlap |

All operations maintain valid BRep topology, handle edge cases (coincident faces, tangent surfaces), and produce watertight meshes.

**Not included:** N-ary booleans (combine >2 shapes at once), partial booleans (keep specific regions).

## UX Details

### Interaction Flow

1. Select first solid in viewport or feature tree
2. Invoke boolean operation (toolbar button or keyboard shortcut)
3. Select second solid (operand)
4. Preview shows result with ghost material
5. Confirm to commit operation

### Edge Cases

| Case | Behavior |
|------|----------|
| Non-intersecting solids | Union: keep both, Difference: keep first unchanged, Intersection: empty result |
| Coincident faces | Merge coplanar faces, remove internal surfaces |
| Tangent surfaces | Handle single-point contact correctly |
| Self-intersecting input | Fail with clear error message |

## Implementation

### 4-Stage Boolean Pipeline

The boolean algorithm processes operations in four stages:

#### Stage 1: AABB Filter (Broadphase)

Axis-aligned bounding box filtering quickly eliminates non-intersecting face pairs:

- Compute AABB for each face in both solids
- Build BVH (bounding volume hierarchy) for efficient queries
- Only pass potentially intersecting pairs to Stage 2
- Early exit if no AABBs overlap

#### Stage 2: Surface-Surface Intersection

Compute intersection curves between face pairs:

| Surface Pair | Method |
|--------------|--------|
| Plane-Plane | Analytic (line or coincident) |
| Plane-Cylinder | Analytic (line, ellipse, or two lines) |
| Plane-Sphere | Analytic (circle) |
| Cylinder-Cylinder | Analytic (conics) or sampled |
| NURBS-Any | Sampled with adaptive refinement |

Falls back to sampled intersection when analytic solutions are unavailable or numerically unstable.

#### Stage 3: Face Classification

Determine which faces belong to the result:

- **Ray casting**: Shoot rays from face centroids, count intersections
- **Winding number**: Compute generalized winding number for robust inside/outside classification
- **Classification**: IN (inside operand), OUT (outside operand), ON (on boundary)

| Operation | Keep from A | Keep from B |
|-----------|-------------|-------------|
| Union | OUT | OUT |
| Difference | OUT | IN (flip normals) |
| Intersection | IN | IN |

#### Stage 4: Sewing

Build the result solid with topology repair:

1. **Trim**: Cut faces along intersection curves
2. **Split**: Divide edges at intersection points
3. **Merge**: Combine vertices within tolerance (1e-9)
4. **Repair**: Close gaps, fix orientation, validate topology

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-booleans/src/lib.rs` | Public API and operation dispatch |
| `crates/vcad-kernel-booleans/src/aabb.rs` | AABB filtering and BVH |
| `crates/vcad-kernel-booleans/src/intersection.rs` | Surface-surface intersection |
| `crates/vcad-kernel-booleans/src/classify.rs` | Face classification |
| `crates/vcad-kernel-booleans/src/sew.rs` | Topology sewing and repair |

### Dependencies

- `rayon` for parallel processing (AABB checks, face classification)
- `vcad-kernel-topo` for half-edge topology
- `vcad-kernel-geom` for surface representations

### Performance

- Rayon parallelization for face-pair intersection tests
- BVH acceleration for broadphase filtering
- Adaptive tolerance for numerical stability

## Tasks

### Core Implementation

- [x] AABB broadphase filter with BVH
- [x] Plane-plane intersection (analytic)
- [x] Plane-cylinder intersection (analytic)
- [x] Plane-sphere intersection (analytic)
- [x] Sampled intersection fallback
- [x] Ray casting face classification
- [x] Winding number classification
- [x] Topology sewing and repair
- [x] Union operation
- [x] Difference operation
- [x] Intersection operation
- [x] Rayon parallelization

### Integration

- [x] WASM bindings in `vcad-kernel-wasm`
- [x] IR operation types in `vcad-ir`
- [x] Engine evaluation in `packages/engine`
- [x] App UI integration

## Acceptance Criteria

- [x] Union of two overlapping boxes produces valid watertight mesh
- [x] Difference creates holes (cylinder from box)
- [x] Intersection extracts common volume
- [x] Coincident faces handled without artifacts
- [x] Non-intersecting solids handled gracefully
- [x] Performance: <100ms for typical operations (simple primitives)
- [x] All tests pass in `crates/vcad-kernel-booleans`

## Future Enhancements

- [ ] Exact predicates (Shewchuk) for robust geometric computations
- [ ] GPU acceleration for parallel face classification
- [ ] N-ary booleans (combine multiple shapes in one operation)
- [ ] Partial booleans (keep specific regions interactively)
- [ ] Boolean operation preview with transparency
