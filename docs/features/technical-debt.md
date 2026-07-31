# Technical Debt Cleanup

Safety fixes, module refactoring, and performance optimization to reduce risk and accelerate development.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | — |
| Priority | `p0` |
| Effort | `s` |
| Shipped | 2025-01 |

> Verified against repo 2026-07-30. Corrections: `partial_cmp().unwrap()` grep returns 0 hits; booleans split into `api.rs`/`pipeline.rs`/`mesh/` etc., but layout differs from the proposed `ssi//split/` directories and `lib.rs` is ~1,956 lines (not under 500).

## Problem

The codebase has accumulated technical debt that creates risk and slows development:

1. **Unsafe code** - One raw pointer cast in tessellation that bypasses Rust's safety guarantees. If the underlying type assumption is wrong, this causes undefined behavior.

2. **Panic-prone code** - 13 instances of `partial_cmp().unwrap()` that will panic if comparing NaN values. NaN can appear from degenerate geometry or numerical edge cases.

3. **Fragile downcasts** - STEP writer uses 9 `downcast_ref().unwrap()` calls that panic if surface types don't match expected variants.

4. **Monolithic modules** - Boolean operations span 8,779 lines across 8 files, with `lib.rs` alone at 3,343 lines. This makes navigation, testing, and maintenance difficult.

5. **O(n) lookups** - Document store operations iterate over arrays instead of using Map-based indexing for node lookups by ID.

6. **Expensive cloning** - 27 `structuredClone()` calls on every document mutation, even for transforms that could use shallow copies.

This debt is blocking cad0 and robotics work where reliability and performance are critical.

## Solution

Four phases of cleanup, prioritized by risk and impact.

### Phase 1: Safety Fixes (Critical)

Fix code that can cause undefined behavior or panics.

**Unsafe pointer cast:**
```rust
// tessellate.rs:1227 - unsafe pointer transmutation
unsafe { &*(surface as *const dyn Surface as *const SphereSurface) };
```
Replace with safe downcast via `as_any().downcast_ref()`.

**partial_cmp().unwrap() calls (13 instances):**
| File | Line | Context |
|------|------|---------|
| `vcad-kernel-booleans/src/lib.rs` | 1113 | Edge sorting |
| `vcad-kernel-booleans/src/lib.rs` | 1499 | Vertex sorting |
| `vcad-kernel-booleans/src/split.rs` | 406 | Parameter sorting |
| `vcad-kernel-booleans/src/trim.rs` | 715 | Trim segment sorting |
| `vcad-kernel-booleans/src/trim.rs` | 772 | U-value sorting |
| `vcad-kernel-fillet/src/lib.rs` | 492 | Index sorting |
| `vcad-kernel-tessellate/src/lib.rs` | 1017 | Angle sorting |
| `vcad-kernel-raytrace/src/bvh.rs` | 84 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/bilinear.rs` | 44 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/cone.rs` | 83 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/cylinder.rs` | 54 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/sphere.rs` | 43 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/bspline.rs` | 34 | Hit sorting |
| `vcad-kernel-raytrace/src/intersect/torus.rs` | 61, 185 | Hit/root sorting |

Replace with `.unwrap_or(Ordering::Equal)` for safe NaN handling.

**STEP writer downcast unwraps (9 instances):**
```rust
// writer.rs:169-224 - panics if surface type doesn't match
let plane = surface.as_any().downcast_ref::<Plane>().unwrap();
```
Add `ok_or()` to return `Result` and propagate errors gracefully.

### Phase 2: Boolean Module Refactoring

Split 8,779 lines across logical modules for maintainability.

**Current structure:**
```
vcad-kernel-booleans/src/
├── lib.rs      (3,343 lines - too large)
├── split.rs    (2,395 lines - too large)
├── ssi.rs      (844 lines)
├── trim.rs     (873 lines)
├── bbox.rs     (413 lines)
├── classify.rs (300 lines)
├── sew.rs      (353 lines)
└── repair.rs   (258 lines)
```

**Proposed structure:**
```
vcad-kernel-booleans/src/
├── lib.rs          (public API only, ~200 lines)
├── api.rs          (BooleanOp enum, public functions)
├── pipeline.rs     (4-stage pipeline orchestration)
├── mesh/
│   ├── mod.rs      (mesh intersection)
│   └── candidate.rs
├── ssi/
│   ├── mod.rs      (surface-surface intersection)
│   ├── plane.rs
│   ├── cylinder.rs
│   └── analytic.rs
├── split/
│   ├── mod.rs      (face splitting)
│   ├── edge.rs
│   └── loop.rs
├── classify.rs     (face classification)
├── sew.rs          (topology repair)
└── trim.rs         (trim curve handling)
```

### Phase 3: Document Store Optimization

**Add Map-based indexing:**
```typescript
// document-store.ts
interface DocumentState {
  document: VcadDocument;
  // Add indexed lookups
  nodeIndex: Map<string, number>;  // id -> array index
  partIndex: Map<string, number>;  // id -> array index
}

// O(1) lookup instead of O(n) find()
getNode(id: string): CsgOp | undefined {
  const idx = this.nodeIndex.get(id);
  return idx !== undefined ? this.document.nodes[idx] : undefined;
}
```

**Shallow clone for transforms:**
```typescript
// Instead of structuredClone for transform-only changes
const newDoc = {
  ...state.document,
  nodes: state.document.nodes.map((node, i) =>
    i === targetIdx ? { ...node, transform: newTransform } : node
  ),
};
```

### Phase 4: Cleanup

- Audit STEP crate for dead code paths
- Add edge case tests for split/trim operations
- Document unsafe blocks with safety invariants

## Implementation

### Files to Modify

| Phase | File | Changes |
|-------|------|---------|
| 1 | `crates/vcad-kernel-tessellate/src/lib.rs` | Replace unsafe cast with safe downcast |
| 1 | `crates/vcad-kernel-booleans/src/lib.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-booleans/src/split.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-booleans/src/trim.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-fillet/src/lib.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-tessellate/src/lib.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-raytrace/src/bvh.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-raytrace/src/intersect/*.rs` | Use `unwrap_or(Ordering::Equal)` |
| 1 | `crates/vcad-kernel-step/src/writer.rs` | Return `Result` from write functions |
| 2 | `crates/vcad-kernel-booleans/src/*.rs` | Extract modules per proposed structure |
| 3 | `packages/core/src/stores/document-store.ts` | Add Map indexes, shallow clone |
| 4 | `crates/vcad-kernel-step/src/*.rs` | Audit and remove dead code |
| 4 | `crates/vcad-kernel-booleans/src/*.rs` | Add edge case tests |

## Tasks

### Phase 1: Safety Fixes (Critical)

- [x] Replace unsafe pointer cast in tessellate.rs:1227 (`xs`)
- [x] Fix partial_cmp().unwrap() in vcad-kernel-booleans (5 instances) (`xs`)
- [x] Fix partial_cmp().unwrap() in vcad-kernel-fillet (1 instance) (`xs`)
- [x] Fix partial_cmp().unwrap() in vcad-kernel-tessellate (1 instance) (`xs`)
- [x] Fix partial_cmp().unwrap() in vcad-kernel-raytrace (7 instances) (`xs`)
- [x] Convert STEP writer downcasts to return Result (`s`)
- [x] Run full test suite to verify no regressions (`xs`)

### Phase 2: Boolean Module Refactoring

- [x] Create api.rs with public BooleanOp enum and functions (`s`)
- [x] Create pipeline.rs for 4-stage orchestration (`s`)
- [x] Extract mesh intersection to mesh/ directory (`m`)
- [x] Extract surface-surface intersection to ssi/ directory (`m`)
- [x] Extract face splitting to split/ directory (`m`)
- [x] Reduce lib.rs to re-exports only (`xs`)
- [x] Update documentation and module-level docs (`s`)

### Phase 3: Document Store Optimization

- [x] Add nodeIndex Map for O(1) node lookups (`s`)
- [x] Add partIndex Map for O(1) part lookups (`s`)
- [x] Implement shallow clone for transform-only mutations (`s`)
- [x] Add index maintenance to addNode/removeNode/etc (`s`)
- [x] Benchmark before/after for large documents (`xs`)

### Phase 4: Cleanup

- [x] Audit STEP crate for unreachable code paths (`s`)
- [x] Add tests for split edge cases (degenerate geometry) (`m`)
- [x] Add tests for trim edge cases (tangent intersections) (`m`)
- [x] Document safety invariants for any remaining unsafe blocks (`xs`)

## Acceptance Criteria

- [x] `cargo clippy --workspace -- -D warnings` passes clean
- [x] No `unsafe` blocks outside explicitly documented sections
- [x] All tests pass: `cargo test --workspace`
- [x] No `partial_cmp().unwrap()` calls remain (grep returns empty)
- [x] No `downcast_ref().unwrap()` in STEP writer (use Result)
- [ ] Boolean module lib.rs under 500 lines (still ~1,956 lines as of 2026-07-30)
- [x] Document store node lookups are O(1)
- [x] Transform mutations use shallow clone (verified via benchmark)

## Future Enhancements

- [ ] Add fuzzing for boolean split/trim operations
- [ ] Profile and optimize hot paths in boolean pipeline
- [ ] Consider arena-based allocation for document nodes
- [ ] Add compile-time checks for surface type exhaustiveness
