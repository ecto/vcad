//! C ABI bridge exposing the vcad kernel to a native Swift / Apple app.
//!
//! Design rules for everything in this crate:
//! - Opaque handles (`*mut VcadSolid`, `*mut VcadMesh`) cross the boundary;
//!   C never sees Rust struct internals.
//! - Every entry point wraps work in [`std::panic::catch_unwind`] so a kernel
//!   panic becomes a null/zero return instead of unwinding into Swift (UB).
//! - The caller owns returned handles and must free them with the matching
//!   `*_free` function. `_view` borrows; its pointers are valid only until the
//!   owning handle is freed.
//!
//! Buffer layout matches [`vcad_kernel_tessellate::TriangleMesh`] exactly:
//! `vertices`/`normals` are flat `f32` triples, `indices` are flat `u32` —
//! a direct match for Metal / RealityKit `LowLevelMesh`, no conversion.

#![allow(clippy::missing_safety_doc)]
// Every entry point is a `#[no_mangle] extern "C"` boundary that takes raw
// pointers from Swift and dereferences them behind explicit null checks +
// catch_unwind. That is the whole point of an FFI crate, so the
// not-unsafe-ptr-arg lint doesn't apply here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use vcad_eval::{evaluate_document, EvalOptions, EvaluatedScene};
use vcad_ir::Document;
use vcad_kernel::Solid;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::{Bvh, Ray};
use vcad_kernel_tessellate::TriangleMesh;

/// Opaque handle to a kernel solid.
pub struct VcadSolid {
    inner: Solid,
}

/// Opaque handle to a tessellated mesh (owns its buffers).
pub struct VcadMesh {
    inner: TriangleMesh,
}

/// A borrowed view into a [`VcadMesh`]'s buffers. Pointers are valid until the
/// owning mesh is freed. Lengths are **element counts** (number of `f32`s /
/// `u32`s), not bytes — there are 3 floats per vertex and 3 indices per triangle.
#[repr(C)]
pub struct VcadMeshView {
    pub vertices: *const f32,
    pub vertices_len: usize,
    pub normals: *const f32,
    pub normals_len: usize,
    pub indices: *const u32,
    pub indices_len: usize,
}

impl VcadMeshView {
    fn empty() -> Self {
        Self {
            vertices: ptr::null(),
            vertices_len: 0,
            normals: ptr::null(),
            normals_len: 0,
            indices: ptr::null(),
            indices_len: 0,
        }
    }
}

/// ABI version, bumped on any breaking change to these signatures. Lets the
/// Swift side assert it linked a compatible static lib.
#[no_mangle]
pub extern "C" fn vcad_ffi_abi_version() -> u32 {
    1
}

/// Create a box (corner at origin, extends to `(sx, sy, sz)`). Returns null on panic.
#[no_mangle]
pub extern "C" fn vcad_solid_cube(sx: f64, sy: f64, sz: f64) -> *mut VcadSolid {
    catch_unwind(|| Box::into_raw(Box::new(VcadSolid { inner: Solid::cube(sx, sy, sz) })))
        .unwrap_or(ptr::null_mut())
}

/// Create a cylinder along Z. Returns null on panic.
#[no_mangle]
pub extern "C" fn vcad_solid_cylinder(radius: f64, height: f64, segments: u32) -> *mut VcadSolid {
    catch_unwind(|| {
        Box::into_raw(Box::new(VcadSolid {
            inner: Solid::cylinder(radius, height, segments),
        }))
    })
    .unwrap_or(ptr::null_mut())
}

/// Create a sphere centered at origin. Returns null on panic.
#[no_mangle]
pub extern "C" fn vcad_solid_sphere(radius: f64, segments: u32) -> *mut VcadSolid {
    catch_unwind(|| Box::into_raw(Box::new(VcadSolid { inner: Solid::sphere(radius, segments) })))
        .unwrap_or(ptr::null_mut())
}

/// Tessellate a solid into a triangle mesh. `segments` controls curved-surface
/// fidelity. Returns null on null input or panic.
#[no_mangle]
pub extern "C" fn vcad_solid_to_mesh(solid: *const VcadSolid, segments: u32) -> *mut VcadMesh {
    if solid.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadSolid = unsafe { &*solid };
        Box::into_raw(Box::new(VcadMesh { inner: s.inner.to_mesh(segments) }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Borrow the buffers of a mesh. The returned pointers are valid until the mesh
/// is freed. On null input, returns an all-zero view.
#[no_mangle]
pub extern "C" fn vcad_mesh_view(mesh: *const VcadMesh) -> VcadMeshView {
    if mesh.is_null() {
        return VcadMeshView::empty();
    }
    let m: &VcadMesh = unsafe { &*mesh };
    VcadMeshView {
        vertices: m.inner.vertices.as_ptr(),
        vertices_len: m.inner.vertices.len(),
        normals: m.inner.normals.as_ptr(),
        normals_len: m.inner.normals.len(),
        indices: m.inner.indices.as_ptr(),
        indices_len: m.inner.indices.len(),
    }
}

/// Free a mesh handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_mesh_free(mesh: *mut VcadMesh) {
    if !mesh.is_null() {
        drop(unsafe { Box::from_raw(mesh) });
    }
}

/// Free a solid handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_solid_free(solid: *mut VcadSolid) {
    if !solid.is_null() {
        drop(unsafe { Box::from_raw(solid) });
    }
}

/// Axis-aligned bounding box of a solid, in kernel units.
#[repr(C)]
pub struct VcadAabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Fillet all edges of a solid with the given radius. Returns a new solid; the
/// kernel returns the input unchanged if a blend would diverge. Null on null
/// input or panic.
#[no_mangle]
pub extern "C" fn vcad_solid_fillet(solid: *const VcadSolid, radius: f64) -> *mut VcadSolid {
    if solid.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadSolid = unsafe { &*solid };
        Box::into_raw(Box::new(VcadSolid { inner: s.inner.fillet(radius) }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Chamfer all edges of a solid by the given distance.
#[no_mangle]
pub extern "C" fn vcad_solid_chamfer(solid: *const VcadSolid, distance: f64) -> *mut VcadSolid {
    if solid.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadSolid = unsafe { &*solid };
        Box::into_raw(Box::new(VcadSolid { inner: s.inner.chamfer(distance) }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Axis-aligned bounding box of a solid. Returns a zero box on null input.
#[no_mangle]
pub extern "C" fn vcad_solid_bbox(solid: *const VcadSolid) -> VcadAabb {
    if solid.is_null() {
        return VcadAabb { min: [0.0; 3], max: [0.0; 3] };
    }
    let s: &VcadSolid = unsafe { &*solid };
    let (min, max) = s.inner.bounding_box();
    VcadAabb { min, max }
}

// =========================================================================
// Document evaluation — parse and evaluate a full .vcad document in Rust.
// =========================================================================

/// Opaque handle to an evaluated scene (owns all part meshes).
pub struct VcadScene {
    inner: EvaluatedScene,
}

/// Parse a `.vcad` JSON document (UTF-8 bytes of length `json_len`) and
/// evaluate it into a scene. Returns null on invalid UTF-8, parse error,
/// evaluation error, or panic.
#[no_mangle]
pub extern "C" fn vcad_scene_from_json(json: *const u8, json_len: usize) -> *mut VcadScene {
    if json.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let text = match std::str::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => return ptr::null_mut(),
        };
        let doc = match Document::from_json(text) {
            Ok(d) => d,
            Err(_) => return ptr::null_mut(),
        };
        match evaluate_document(&doc, &EvalOptions::default()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene { inner: scene })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Parse a `.vcad` **loon** source program (UTF-8 bytes of length `loon_len`)
/// and evaluate it into a scene. This is the AI-intent path: an agent emits a
/// loon program, which compiles to the same `Document` IR a `.vcad` file holds
/// and evaluates identically. Module resolution (`[use …]`) is disabled
/// (`base_dir = None`), so programs must be self-contained — the bundled vcad
/// loon library (types + constructors) is always available. Returns null on
/// invalid UTF-8, a loon parse/eval error, an evaluation error, or panic.
#[no_mangle]
pub extern "C" fn vcad_scene_from_loon(loon: *const u8, loon_len: usize) -> *mut VcadScene {
    if loon.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(loon, loon_len) };
        let text = match std::str::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => return ptr::null_mut(),
        };
        let doc = match vcad_loon::eval_vcad(text, None) {
            Ok(d) => d,
            Err(_) => return ptr::null_mut(),
        };
        match evaluate_document(&doc, &EvalOptions::default()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene { inner: scene })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Number of evaluated parts (visible document roots) in the scene.
#[no_mangle]
pub extern "C" fn vcad_scene_part_count(scene: *const VcadScene) -> usize {
    if scene.is_null() {
        return 0;
    }
    let s: &VcadScene = unsafe { &*scene };
    s.inner.parts.len()
}

/// Borrow the `index`-th part's mesh buffers. Per risk #4 (normals length is
/// not guaranteed by the kernel), `normals_len` is reported as 0 unless it
/// exactly matches the position count — the caller then synthesizes normals.
/// Returns an empty view if `index` is out of range.
#[no_mangle]
pub extern "C" fn vcad_scene_part_mesh(scene: *const VcadScene, index: usize) -> VcadMeshView {
    if scene.is_null() {
        return VcadMeshView::empty();
    }
    let s: &VcadScene = unsafe { &*scene };
    let Some(part) = s.inner.parts.get(index) else {
        return VcadMeshView::empty();
    };
    let m = &part.mesh;
    let (normals, normals_len) = match &m.normals {
        Some(n) if n.len() == m.positions.len() => (n.as_ptr(), n.len()),
        _ => (ptr::null(), 0),
    };
    VcadMeshView {
        vertices: m.positions.as_ptr(),
        vertices_len: m.positions.len(),
        normals,
        normals_len,
        indices: m.indices.as_ptr(),
        indices_len: m.indices.len(),
    }
}

/// Free a scene handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_scene_free(scene: *mut VcadScene) {
    if !scene.is_null() {
        drop(unsafe { Box::from_raw(scene) });
    }
}

/// Result of a ray-pick against a solid's BRep.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VcadHit {
    pub hit: u8,
    pub point: [f64; 3],
    pub normal: [f64; 3],
    pub t: f64,
}

/// Cast a ray (origin + direction, kernel coords) against a solid and return
/// the closest analytic surface hit. Builds a BVH per call — fine for
/// interactive picking. Returns hit=0 on miss / null / non-BRep / panic.
#[no_mangle]
pub extern "C" fn vcad_solid_raycast(
    solid: *const VcadSolid,
    origin: *const f64,
    dir: *const f64,
) -> VcadHit {
    let miss = VcadHit { hit: 0, point: [0.0; 3], normal: [0.0; 3], t: 0.0 };
    if solid.is_null() || origin.is_null() || dir.is_null() {
        return miss;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadSolid = unsafe { &*solid };
        let o = unsafe { std::slice::from_raw_parts(origin, 3) };
        let d = unsafe { std::slice::from_raw_parts(dir, 3) };
        let Some(brep) = s.inner.as_brep() else {
            return miss;
        };
        let bvh = Bvh::build(brep);
        let ray = Ray::new(Point3::new(o[0], o[1], o[2]), Vec3::new(d[0], d[1], d[2]));
        match bvh.trace_closest(&ray) {
            Some(h) => VcadHit {
                hit: 1,
                point: [h.point.x, h.point.y, h.point.z],
                normal: [h.normal.x, h.normal.y, h.normal.z],
                t: h.t,
            },
            None => miss,
        }
    }))
    .unwrap_or(miss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_roundtrips_to_a_nonempty_mesh() {
        let solid = vcad_solid_cube(10.0, 20.0, 30.0);
        assert!(!solid.is_null());
        let mesh = vcad_solid_to_mesh(solid, 16);
        assert!(!mesh.is_null());
        let view = vcad_mesh_view(mesh);
        assert!(view.vertices_len > 0, "cube should tessellate to >0 vertices");
        assert_eq!(view.vertices_len, view.normals_len, "one normal per position");
        assert!(view.indices_len % 3 == 0, "indices form whole triangles");
        vcad_mesh_free(mesh);
        vcad_solid_free(solid);
    }

    #[test]
    fn loads_and_evaluates_an_example_document() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/plate.vcad");
        let json = std::fs::read(path).expect("read example .vcad");
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null(), "example document should parse + evaluate");
        let n = vcad_scene_part_count(scene);
        assert!(n > 0, "expected >= 1 evaluated part, got {n}");
        let view = vcad_scene_part_mesh(scene, 0);
        assert!(view.vertices_len > 0, "part 0 should have geometry");
        assert!(view.indices_len % 3 == 0, "indices form whole triangles");
        vcad_scene_free(scene);
    }

    #[test]
    fn evaluates_a_loon_program() {
        // The AI-intent path: a loon program (as an agent would emit) compiles
        // and evaluates to geometry just like a loaded .vcad file.
        let src = "[root [fillet 2.0 [cylinder 10 30]] \"brass\"]";
        let scene = vcad_scene_from_loon(src.as_ptr(), src.len());
        assert!(!scene.is_null(), "loon program should compile + evaluate");
        let n = vcad_scene_part_count(scene);
        assert!(n > 0, "expected >= 1 part, got {n}");
        let view = vcad_scene_part_mesh(scene, 0);
        assert!(view.vertices_len > 0, "part 0 should have geometry");
        assert!(view.indices_len % 3 == 0, "indices form whole triangles");
        vcad_scene_free(scene);
    }

    #[test]
    fn bad_loon_returns_null() {
        let src = "this is not loon";
        let scene = vcad_scene_from_loon(src.as_ptr(), src.len());
        assert!(scene.is_null(), "garbage loon should fail to a null scene, not panic");
        assert!(vcad_scene_from_loon(ptr::null(), 0).is_null());
    }

    #[test]
    fn raycast_hits_a_cube_face() {
        let cube = vcad_solid_cube(30.0, 30.0, 30.0);
        let origin = [15.0_f64, 15.0, 100.0];
        let dir = [0.0_f64, 0.0, -1.0];
        let hit = vcad_solid_raycast(cube, origin.as_ptr(), dir.as_ptr());
        assert_eq!(hit.hit, 1, "a ray down +Z should hit the top face");
        assert!((hit.point[2] - 30.0).abs() < 1e-6, "hit z should be 30, got {}", hit.point[2]);
        assert!(hit.normal[2] > 0.9, "top-face normal should point +Z");
        vcad_solid_free(cube);
    }

    #[test]
    fn null_inputs_are_safe() {
        assert!(vcad_solid_to_mesh(ptr::null(), 8).is_null());
        let view = vcad_mesh_view(ptr::null());
        assert_eq!(view.vertices_len, 0);
        vcad_mesh_free(ptr::null_mut());
        vcad_solid_free(ptr::null_mut());
    }
}
