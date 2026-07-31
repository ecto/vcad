# Direct BRep Ray Tracing

Pixel-perfect rendering of CAD geometry without tessellation artifacts.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p1` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. `crates/vcad-kernel-raytrace` (incl. `gpu/` compute pipeline) and `RayTracedViewport.tsx` exist as described.

## Problem

Traditional CAD rendering relies on tessellation, converting smooth analytic surfaces into triangle meshes for display. This approach has fundamental limitations:

1. **Visible faceting** — At high zoom levels, tessellated edges appear as polygonal approximations rather than smooth curves. Cylinder edges become hexagons, spheres become geodesic domes.

2. **Tessellation latency** — Every parameter change requires re-tessellation before display. For complex models, this creates noticeable lag between editing and seeing results.

3. **LOD management complexity** — Systems must choose tessellation density: too coarse and edges look jagged, too fine and memory/performance suffer. Zoom changes may require re-tessellation.

4. **Edge fidelity** — Sharp edges and silhouettes are only as accurate as the underlying triangle mesh, making precise visual inspection difficult.

Every commercial CAD system (SolidWorks, Fusion 360, Onshape, CATIA) uses tessellation for viewport display. This is vcad's opportunity for differentiation.

## Solution

Ray trace BRep surfaces directly using analytic intersection algorithms. Each pixel casts a ray that intersects the actual mathematical surface definition, producing exact results regardless of zoom level.

### Surface Intersection Algorithms

Each surface type has a specialized intersection solver:

| Surface | Algorithm | Complexity |
|---------|-----------|------------|
| Plane | Closed-form ray-plane intersection | O(1) |
| Cylinder | Quadratic equation | O(1) |
| Sphere | Quadratic equation | O(1) |
| Cone | Quadratic equation | O(1) |
| Torus | Quartic equation (Ferrari's method) | O(1) |
| NURBS | Newton iteration with subdivision fallback | O(n) iterations |

### BVH Acceleration

A Bounding Volume Hierarchy accelerates ray traversal:

- **Construction**: Surface Area Heuristic (SAH) for optimal splits
- **Traversal**: Front-to-back ordering with early termination
- **Leaf size**: 4 faces per leaf node
- **Flattening**: GPU-friendly array layout for compute shaders

### Trimmed Surface Handling

BRep faces are bounded regions of underlying surfaces. After finding a ray-surface intersection:

1. Compute (u, v) parameters at the hit point
2. Project into the face's 2D parameter space
3. Test point-in-polygon against outer loop (winding number algorithm)
4. Verify point is outside all inner loops (holes)

Only hits passing the trim test are visible.

### Quality Levels

Three render quality settings trade resolution for performance:

| Quality | Resolution | Use Case |
|---------|------------|----------|
| Draft | 0.5x viewport | Interactive manipulation |
| Standard | 1.0x viewport | Normal viewing |
| High | 2.0x viewport | Precise inspection |

Resolution is capped at 640x480 equivalent pixels to maintain interactive frame rates.

## UX Details

### Viewport Overlay

The ray tracer renders to an HTML canvas that overlays the Three.js scene. This allows:

- Compositing ray-traced geometry with Three.js gizmos and UI
- Progressive quality improvement (draft while moving, high when idle)
- Face picking via ray casting at click coordinates

### Interaction States

| State | Behavior |
|-------|----------|
| Camera moving | Draft quality, continuous re-render |
| Camera idle | Standard/high quality based on setting |
| Parameter editing | Live re-render on solid changes |
| Face selection mode | Cursor changes to crosshair, clicks report face index |

### Quality Toggle

Settings panel includes a ray trace quality dropdown:

```
Render Quality: [ Draft ▼ ]
                  Draft
                  Standard
                  High
```

### Edge Cases

- **Empty scene**: Ray tracer returns immediately, shows nothing
- **No WebGPU**: Falls back to tessellated rendering (standard Three.js path)
- **Large renders**: Resolution auto-scales down to maintain interactivity

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-raytrace/src/lib.rs` | Crate root, exports `Ray`, `RayHit`, `Bvh` |
| `crates/vcad-kernel-raytrace/src/ray.rs` | Ray representation, AABB intersection |
| `crates/vcad-kernel-raytrace/src/bvh.rs` | BVH construction and traversal |
| `crates/vcad-kernel-raytrace/src/trim.rs` | Trimmed surface point-in-face testing |
| `crates/vcad-kernel-raytrace/src/intersect/` | Surface-specific intersection algorithms |
| `crates/vcad-kernel-raytrace/src/intersect/plane.rs` | Ray-plane intersection |
| `crates/vcad-kernel-raytrace/src/intersect/cylinder.rs` | Ray-cylinder intersection (quadratic) |
| `crates/vcad-kernel-raytrace/src/intersect/sphere.rs` | Ray-sphere intersection (quadratic) |
| `crates/vcad-kernel-raytrace/src/intersect/cone.rs` | Ray-cone intersection (quadratic) |
| `crates/vcad-kernel-raytrace/src/intersect/torus.rs` | Ray-torus intersection (quartic) |
| `crates/vcad-kernel-raytrace/src/intersect/bspline.rs` | Ray-NURBS intersection (Newton iteration) |
| `crates/vcad-kernel-raytrace/src/gpu/` | WebGPU compute shader pipeline |
| `packages/app/src/components/RayTracedViewport.tsx` | React component for ray-traced overlay |
| `packages/core/src/stores/ui-store.ts` | `raytraceQuality` and `renderMode` state |
| `packages/engine/src/gpu.ts` | WebGPU context and ray tracer initialization |

### State

```typescript
// ui-store.ts
type RaytraceQuality = "draft" | "standard" | "high";
type RenderMode = "standard" | "raytrace";

interface UiState {
  renderMode: RenderMode;
  raytraceQuality: RaytraceQuality;
  raytraceAvailable: boolean;
  setRaytraceQuality: (quality: RaytraceQuality) => void;
  setRaytraceAvailable: (available: boolean) => void;
}
```

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Web App (React)                          │
├─────────────────────────────────────────────────────────────────┤
│  RayTracedViewport.tsx                                          │
│  - Subscribes to camera/scene changes                           │
│  - Calls rayTracer.render() on each frame                       │
│  - Draws RGBA pixels to canvas overlay                          │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                     WASM (vcad-kernel-wasm)                     │
├─────────────────────────────────────────────────────────────────┤
│  - uploadSolid(solid): Build BVH, upload to GPU                 │
│  - render(camera, size, fov): Execute compute shader            │
│  - pick(camera, size, fov, x, y): Return face index at pixel    │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                    Rust (vcad-kernel-raytrace)                  │
├─────────────────────────────────────────────────────────────────┤
│  Bvh::build(brep)           → Construct acceleration structure  │
│  bvh.trace(ray)             → Find all intersections            │
│  bvh.trace_closest(ray)     → Find nearest intersection         │
│  point_in_face(brep, fid, uv) → Trim boundary test              │
│  intersect_surface(ray, srf)  → Surface-specific algorithm      │
└─────────────────────────────────────────────────────────────────┘
```

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Planes render with pixel-perfect edges
- [x] Cylinders render as smooth curves at any zoom
- [x] Spheres render without geodesic faceting
- [x] Cones render with smooth silhouettes
- [x] Tori render correctly (quartic solver)
- [x] Trimmed surfaces respect boundary loops
- [x] Holes in faces render correctly (inner loops)
- [x] BVH accelerates ray traversal (no per-pixel full traversal)
- [x] Camera movement triggers re-render
- [x] Quality setting affects render resolution
- [x] Face picking returns correct face index
- [x] No tessellation latency on parameter changes
- [x] Works in Chrome/Edge with WebGPU

## Future Enhancements

- [ ] Shadows (secondary ray casting)
- [ ] Reflections for metallic materials
- [ ] Ambient occlusion for depth perception
- [ ] NURBS ray tracing optimization (B-spline specific solver)
- [ ] Progressive refinement (draft → high over multiple frames)
- [ ] Multi-solid scenes (current: single solid upload)
- [ ] Edge highlighting (detect silhouette edges from normal discontinuity)
- [ ] Transparent materials with refraction
