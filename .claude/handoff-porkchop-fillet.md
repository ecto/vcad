# Pork-chop fillet — handoff for the next pass

## What you're inheriting

An arc-extruded kidney profile (the "pork-chop") with a 4mm fillet. The BRep pipeline works (`Solid::extrude(profile, vec).fillet(4.0)`), but the tessellated mesh has **32 of 192 torus-blend triangles winding inward near armpit junctions**. The resulting render shows 4-6 small dark patches at each cap corner — topologically the mesh is watertight, but Three.js's shading on the wrong-wound tris makes them look like holes.

## What's been done (on `nurbs-fillet-porkchop`)

### Kernel pipeline (works correctly)
- Analytic cylinder extrude from arc profile — one `CylinderSurface` per arc, not tessellated planar quads
- Rolling-ball fillet routing for non-planar BReps → `fillet_edges_detailed`
- Per-arc torus blend construction via `build_blend_quad` → `TorusSurface`
- Spherical vertex blends at convex 3-face junctions via `plan_convex_junction_blends` + `build_spherical_vertex_blends_from_plans`
- Trim snapping at junction arc slivers (`snap_trims_at_junctions`) — collapses the last 2-3 micro torus segments into zero-area quads that `build_blend_quad` rejects

### Tessellation-level fixes (work correctly)
- Mesh-level fan-fill for armpit hex gaps (`fill_tiny_boundary_loops` in `vcad-kernel-tessellate/src/lib.rs:381`). Gated to loops that touch a Sphere-or-Torus vertex position, so plate-with-hole cutouts are untouched. All 36 fan triangles wind outward; all 36 sphere-patch triangles wind outward.
- Per-edge fan winding: each fan triangle's CCW normal is checked against the local outward (loop centroid minus solid centroid) and flipped if needed
- Fan centroid offset reduced to `avg_edge * 0.05` to avoid visible fin protrusions

### Diagnostic tools (usable as-is)
- `TriangleMesh::boundary_edges() / boundary_loops() / non_manifold_edges()` — finds tessellation holes in O(tri) time
- Viewport boundary-edge overlay — red lines over unpaired edges. Toggle: `Ctrl+Shift+B` or `__VCAD_DEBUG_OVERLAY.getState().setShowBoundaryEdges(true)`
- **Click-to-inspect triangle picker** — clicking any triangle shows its index, source face kind, 3 vertex IDs + positions, CCW normal, and outward-alignment dot. Toggle: `Ctrl+Shift+T`. Requires the `faceKinds: Uint8Array` field on the mesh (one u8 per triangle), which is populated automatically by `tessellate_brep`.
- `cargo run --example porkchop_diag --release -p vcad-kernel -- /tmp/diag` — dumps per-face OBJ, per-junction trace CSV, boundary loops. ~3s iteration vs ~45s for wasm-pack cycle.
- Per-vertex fillet trace: `fillet_edges_detailed_with_trace(..., capture_trace=true)` returns a `FilletTrace` with per-junction outcomes (built patch + ball/tangent points, or skip reason).

## The actual remaining bug (all detail)

**32 of 192 torus triangles wind inward.** Concentrated at armpit junctions (2-4 per torus face, 16 per cap × 2 caps).

Diagnostic signature — from running the triangle inspector on every inward tri:

- Per-vertex **analytical** normals (set from `surface.normal(uv)` in `tessellate_toroidal_face`): **outward** (+0.94 avg dot with solid-outward)
- Per-triangle **CCW-from-winding** face normal (what Three.js's `computeVertexNormals` recomputes): **inward** (-0.80 dot)
- So the torus surface's own `normal(uv)` disagrees with the CCW winding of its own `evaluate(uv)`-generated mesh vertices — for *those specific quads*

Three.js SceneMesh (`packages/app/src/components/SceneMesh.tsx:538`) uses kernel normals when supplied, but the mesh still renders dark at those tris. Reason TBD — possibly a creasing pass (`toCreasedNormals`) in the ImportedMesh branch overwriting, or a backface interaction inside the PBR shader with DoubleSide + inward CCW.

My `tessellate_toroidal_face` attempts so far (`crates/vcad-kernel-tessellate/src/lib.rs:3188`):

- First-quad check comparing CCW against `torus.normal(uv_mid)` — no effect (maybe first-quad happens to agree locally)
- **Current code**: per-quad check, `torus.normal(uv_mid)` as reference. Still 32 inward. The `torus.normal()` reference is itself suspect — for armpit-junction torus faces after trim snapping, the analytical normal direction may be ambiguous.

### The specific fix to try

The `build_blend_quad_surface` (in `crates/vcad-kernel-fillet/src/fillet_curved.rs:1036`) already has a correct outward heuristic: sum of face_a's planar-normal (cap) + face_b's radial-outward (cyl radial from axis to edge midpoint). That vector is locally correct even at snap-shifted armpit trim points.

Plumb that same heuristic into `tessellate_toroidal_face`:
1. Pass the face's neighbor info (cap_normal, cyl_axis, cyl_center) through `FaceInfo` or a new per-face debug struct attached to the BRep face during fillet construction.
2. In `tessellate_toroidal_face`, compare each quad's CCW normal against `cap_normal + cyl_radial_at_quad_center` instead of `torus.normal(uv_mid)`.
3. Flip winding per-quad when they disagree.

Alternative: bypass the problem entirely by caching the outward hint on the BRep face itself — add `pub blend_outward: Option<Vec3>` to `Face` in `vcad-kernel-topo`, populated by fillet's `build_blend_quad_surface`, consumed by the tessellator.

## Starting points / invocation

- Run the harness: `cargo run --example porkchop_diag --release -p vcad-kernel -- /tmp/diag` (harness file `crates/vcad-kernel/examples/porkchop_diag.rs`)
- Open `/tmp/diag/porkchop.obj` in MeshLab to visually inspect per-face groups
- Check remaining inward tris in a browser: start dev server (`npm run dev -w @vcad/app`), open, then in console: toggle `Ctrl+Shift+T`, click a dark patch. The `[triangle-inspector]` console log gives you the triangle index + face kind + winding diagnosis directly.
- Regression tests: `cargo test -p vcad-kernel test_fillet_arc_profile_has_sphere_vertex_blend_faces` and `cargo test -p vcad-kernel diag_porkchop_boundary_edges -- --nocapture --ignored`

## Overlap with GitHub #44 (boolean degenerate test suite)

Mild thematic overlap, different bug class:

- Both involve **degenerate geometry handling** in the kernel, but #44 is specifically the **4-stage boolean pipeline** (`vcad-kernel-booleans`) and this branch is the **fillet pipeline + tessellator** (`vcad-kernel-fillet` + `vcad-kernel-tessellate`).
- Both have "sphere-cap pole loops" / "degenerate cap faces" as a pattern. Our sphere vertex-blend patches at convex junctions are structurally similar to the degenerate pole loops that `trim.rs` handles on the boolean side — could be worth running the boolean pipeline on a pork-chop fillet output as part of #44's fixture suite.
- The arc-split TODOs at `lib.rs:731` and `lib.rs:983` (in booleans) might intersect with our arc-extruded BRep — arc-extruded cylinder faces currently have many small segments; if the boolean arc-split TODO gets resolved, the pork-chop fillet pipeline might produce tidier topology (fewer per-segment torus blends).
- `collapse_degenerate_half_edges` from `repair.rs:313` could be a post-processing pass to fold the zero-area snap-sliver blends that `build_blend_quad` currently rejects rather than cleanly collapsing.

Not a blocking dependency either way. Our branch is self-contained and can merge independently.
