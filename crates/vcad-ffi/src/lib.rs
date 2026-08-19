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

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use vcad_ecad_pcb::router::route_all;
// The native FFI is a kernel-direct consumer: it ALWAYS wants sheet-metal ops to
// fold to real BRep solids (the bracket is geometry here, not a web unfold/DXF
// payload). Alias the folding entry point so every solve in this crate folds; the
// plain `evaluate_document` (web/MCP contract: leave sheet-metal empty) is never
// what we want natively.
use vcad_eval::{
    evaluate_document_with_sheet_metal as evaluate_document, solve_forward_kinematics, EvalOptions,
    EvaluatedScene,
};
use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, Net, NetClassRules, Pad, PadShape, PadType,
    Pcb, PcbLayer, StackupLayer,
};
use vcad_ir::{
    BindingKey, CsgOp, Document, Expr, MaterialDef, Node, Parameter, SceneEntry, Transform3D, Vec2,
    Vec3 as IrVec3,
};
use vcad_kernel::Solid;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::{Bvh, Ray};
use vcad_kernel_tessellate::TriangleMesh;

mod err;
pub mod gym;
pub mod train;

pub use err::vcad_last_error;

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

/// Owned feature-edge segment buffer for wireframe overlays. Flat layout:
/// 6 `f32`s per segment (ax, ay, az, bx, by, bz).
pub struct VcadEdges {
    inner: Vec<f32>,
}

/// A borrowed view into a [`VcadEdges`] buffer. `floats_len` is the number of
/// `f32`s (a multiple of 6); pointers are valid until the owning handle is
/// freed.
#[repr(C)]
pub struct VcadEdgesView {
    pub floats: *const f32,
    pub floats_len: usize,
}

impl VcadEdgesView {
    fn empty() -> Self {
        Self {
            floats: ptr::null(),
            floats_len: 0,
        }
    }
}

/// ABI version, bumped on any breaking change to these signatures. Lets the
/// Swift side assert it linked a compatible static lib.
#[no_mangle]
pub extern "C" fn vcad_ffi_abi_version() -> u32 {
    // 6: added the simulation surface — `vcad_gym_*` (physics envs, the
    //    render seam, policy inference), `vcad_train_*` (in-process ARS), and
    //    the shared `vcad_last_error` diagnostic channel.
    6
}

/// Create a box (corner at origin, extends to `(sx, sy, sz)`). Returns null on panic.
#[no_mangle]
pub extern "C" fn vcad_solid_cube(sx: f64, sy: f64, sz: f64) -> *mut VcadSolid {
    catch_unwind(|| {
        Box::into_raw(Box::new(VcadSolid {
            inner: Solid::cube(sx, sy, sz),
        }))
    })
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
    catch_unwind(|| {
        Box::into_raw(Box::new(VcadSolid {
            inner: Solid::sphere(radius, segments),
        }))
    })
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
        Box::into_raw(Box::new(VcadMesh {
            inner: s.inner.to_mesh(segments),
        }))
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
        Box::into_raw(Box::new(VcadSolid {
            inner: match s.inner.fillet(radius) {
                Ok(f) => f,
                // The C ABI has no error channel here; a null return is
                // the honest answer — better than handing back an
                // unfilleted solid the caller believes is filleted.
                Err(_) => return ptr::null_mut(),
            },
        }))
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
        Box::into_raw(Box::new(VcadSolid {
            inner: match s.inner.chamfer(distance) {
                Ok(c) => c,
                Err(_) => return ptr::null_mut(),
            },
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Axis-aligned bounding box of a solid. Returns a zero box on null input.
#[no_mangle]
pub extern "C" fn vcad_solid_bbox(solid: *const VcadSolid) -> VcadAabb {
    if solid.is_null() {
        return VcadAabb {
            min: [0.0; 3],
            max: [0.0; 3],
        };
    }
    let s: &VcadSolid = unsafe { &*solid };
    let (min, max) = s.inner.bounding_box();
    VcadAabb { min, max }
}

// =========================================================================
// Document evaluation — parse and evaluate a full .vcad document in Rust.
// =========================================================================

/// Opaque handle to an evaluated scene (owns all part meshes).
///
/// `doc` keeps the parsed source document alive when the scene came from
/// JSON/loon, so kinematic queries (`vcad_scene_solve_fk`) can re-solve joint
/// placement without re-parsing. Scenes produced by resident-doc re-solves
/// don't carry one (FK queries on them return 0).
pub struct VcadScene {
    inner: EvaluatedScene,
    doc: Option<Document>,
}

impl VcadScene {
    /// Instance ids in scene order — the index space every `vcad_scene_instance_*`
    /// entry point uses. Empty for a part-only document.
    ///
    /// Exposed for [`gym::vcad_gym_bind_scene`], which maps this ordering onto
    /// the physics world's (sorted) body ordering once, so the render loop can
    /// pull simulated transforms in scene order without per-frame lookups.
    pub(crate) fn instances(&self) -> Vec<String> {
        self.inner
            .instances
            .as_ref()
            .map(|list| list.iter().map(|i| i.instance_id.clone()).collect())
            .unwrap_or_default()
    }
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
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: Some(doc),
            })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Like [`vcad_scene_from_json`], but resolves relative `MeshImport` paths
/// against `base_dir` (the document's own directory).
///
/// A document that references vendored meshes relatively is portable; without a
/// base directory those paths resolve against the process working directory,
/// so the same file renders from one launch and comes back empty from another.
/// Passing a null/empty `base_dir` is exactly [`vcad_scene_from_json`].
#[no_mangle]
pub extern "C" fn vcad_scene_from_json_in(
    json: *const u8,
    json_len: usize,
    base_dir: *const u8,
    base_dir_len: usize,
) -> *mut VcadScene {
    if json.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return ptr::null_mut();
        };
        let Ok(mut doc) = Document::from_json(text) else {
            return ptr::null_mut();
        };
        if !base_dir.is_null() && base_dir_len > 0 {
            let db = unsafe { std::slice::from_raw_parts(base_dir, base_dir_len) };
            if let Ok(dir) = std::str::from_utf8(db) {
                vcad_eval::resolve_mesh_paths(&mut doc, std::path::Path::new(dir));
            }
        }
        match evaluate_document(&doc, &EvalOptions::default()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: Some(doc),
            })),
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
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: Some(doc),
            })),
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
    eval_mesh_view(&part.mesh)
}

/// Extract the `index`-th part's feature edges (boundary + creases sharper
/// than `angle_deg`) for a wireframe/edge overlay. Returns an owned handle;
/// read it with [`vcad_edges_view`] and release it with [`vcad_edges_free`].
/// Returns null on null scene, out-of-range index, or panic.
#[no_mangle]
pub extern "C" fn vcad_scene_part_edges(
    scene: *const VcadScene,
    index: usize,
    angle_deg: f32,
) -> *mut VcadEdges {
    if scene.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadScene = unsafe { &*scene };
        let Some(part) = s.inner.parts.get(index) else {
            return ptr::null_mut();
        };
        // EvaluatedMesh is positions/indices only — rewrap as a kernel
        // TriangleMesh for the feature-edge extractor.
        let mut mesh = TriangleMesh::new();
        mesh.vertices = part.mesh.positions.clone();
        mesh.indices = part.mesh.indices.clone();
        let segs = mesh.feature_edge_segments(angle_deg);
        let mut flat = Vec::with_capacity(segs.len() * 6);
        for s in segs {
            flat.extend_from_slice(&s);
        }
        Box::into_raw(Box::new(VcadEdges { inner: flat }))
    }))
    .unwrap_or(ptr::null_mut())
}

// =========================================================================
// Assembly instances + kinematic joint playback
//
// Assembly documents place geometry via `partDefs` + `instances` + `joints`
// (the FK contract: joints fully place children). The evaluator exposes each
// instance's mesh in part-def-LOCAL coordinates with its world transform
// separate, which is exactly the per-frame playback shape: static mesh
// entities, per-frame transforms from `vcad_scene_solve_fk`.
// =========================================================================

/// Row-major 3x3 rotation from Euler angles in degrees, composed Rz·Ry·Rx —
/// the same convention as `vcad_eval::kinematics` (and the web evaluator).
fn euler_deg_to_mat3(rot: &IrVec3) -> [[f64; 3]; 3] {
    let (rx, ry, rz) = (rot.x.to_radians(), rot.y.to_radians(), rot.z.to_radians());
    let (cx, sx) = (rx.cos(), rx.sin());
    let (cy, sy) = (ry.cos(), ry.sin());
    let (cz, sz) = (rz.cos(), rz.sin());
    [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ]
}

/// Write a `Transform3D` (T · R · S) into `out` as a 4x4 matrix in
/// COLUMN-MAJOR order (out[col*4 + row]) — the layout `simd_double4x4` /
/// `float4x4` columns expect on the Swift side.
fn write_transform_col_major(t: &Transform3D, out: &mut [f64]) {
    let r = euler_deg_to_mat3(&t.rotation);
    let s = [t.scale.x, t.scale.y, t.scale.z];
    for col in 0..3 {
        for row in 0..3 {
            out[col * 4 + row] = r[row][col] * s[col];
        }
        out[col * 4 + 3] = 0.0;
    }
    out[12] = t.translation.x;
    out[13] = t.translation.y;
    out[14] = t.translation.z;
    out[15] = 1.0;
}

/// Number of assembly instances in the scene (0 for part-only documents).
/// When non-zero, renderers should draw instances INSTEAD of the root parts
/// (mirroring the web viewport) — assembly docs may carry both.
#[no_mangle]
pub extern "C" fn vcad_scene_instance_count(scene: *const VcadScene) -> usize {
    if scene.is_null() {
        return 0;
    }
    let s: &VcadScene = unsafe { &*scene };
    s.inner.instances.as_ref().map_or(0, |i| i.len())
}

/// Borrow the `index`-th instance's mesh buffers, in part-def-LOCAL
/// coordinates (apply the instance transform to place it). Same normals
/// contract as [`vcad_scene_part_mesh`]. Empty view when out of range.
#[no_mangle]
pub extern "C" fn vcad_scene_instance_mesh(scene: *const VcadScene, index: usize) -> VcadMeshView {
    if scene.is_null() {
        return VcadMeshView::empty();
    }
    let s: &VcadScene = unsafe { &*scene };
    match s.inner.instances.as_ref().and_then(|i| i.get(index)) {
        Some(inst) => eval_mesh_view(&inst.mesh),
        None => VcadMeshView::empty(),
    }
}

/// Borrow the `index`-th instance's id as UTF-8 bytes (NOT NUL-terminated;
/// `*out_len` receives the byte length). The pointer stays valid for the
/// scene's lifetime. Null + len 0 when out of range.
#[no_mangle]
pub extern "C" fn vcad_scene_instance_id(
    scene: *const VcadScene,
    index: usize,
    out_len: *mut usize,
) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    if scene.is_null() {
        return ptr::null();
    }
    let s: &VcadScene = unsafe { &*scene };
    match s.inner.instances.as_ref().and_then(|i| i.get(index)) {
        Some(inst) => {
            if !out_len.is_null() {
                unsafe { *out_len = inst.instance_id.len() };
            }
            inst.instance_id.as_ptr()
        }
        None => ptr::null(),
    }
}

/// Borrow the `index`-th instance's resolved material name (same borrow rules
/// as [`vcad_scene_instance_id`]).
#[no_mangle]
pub extern "C" fn vcad_scene_instance_material(
    scene: *const VcadScene,
    index: usize,
    out_len: *mut usize,
) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    if scene.is_null() {
        return ptr::null();
    }
    let s: &VcadScene = unsafe { &*scene };
    match s.inner.instances.as_ref().and_then(|i| i.get(index)) {
        Some(inst) => {
            if !out_len.is_null() {
                unsafe { *out_len = inst.material.len() };
            }
            inst.material.as_ptr()
        }
        None => ptr::null(),
    }
}

/// The `index`-th instance's world transform at the document's AUTHORED joint
/// states, written to `out` as 16 doubles (column-major, see
/// [`vcad_scene_solve_fk`]). Returns 1 on success, 0 when out of range/null.
#[no_mangle]
pub extern "C" fn vcad_scene_instance_transform(
    scene: *const VcadScene,
    index: usize,
    out: *mut f64,
) -> u8 {
    if scene.is_null() || out.is_null() {
        return 0;
    }
    let s: &VcadScene = unsafe { &*scene };
    let Some(inst) = s.inner.instances.as_ref().and_then(|i| i.get(index)) else {
        return 0;
    };
    let t = inst.transform.unwrap_or_else(Transform3D::identity);
    let slice = unsafe { std::slice::from_raw_parts_mut(out, 16) };
    write_transform_col_major(&t, slice);
    1
}

/// Kinematic joint playback: solve forward kinematics at the given joint
/// values and return every instance's world transform.
///
/// `joints_json` is a UTF-8 JSON object mapping joint id (falling back to
/// joint name) to its driven value — degrees for revolute, mm for slider —
/// e.g. `{"shoulder": 45.0, "elbow": -30.0}`. Joints absent from the map keep
/// their authored state. This is the exact kernel FK the web evaluator runs
/// per playback frame, so native playback matches it by construction.
///
/// `out` receives `instance_count * 16` doubles: one column-major 4x4 world
/// matrix per instance, in [`vcad_scene_instance_mesh`] index order.
/// `out_cap` is the capacity of `out` in doubles. Returns the number of
/// instances written, or 0 on any error (null/undersized buffer, bad JSON, or
/// a scene that didn't come from `vcad_scene_from_json`/`_loon`).
#[no_mangle]
pub extern "C" fn vcad_scene_solve_fk(
    scene: *const VcadScene,
    joints_json: *const u8,
    json_len: usize,
    out: *mut f64,
    out_cap: usize,
) -> usize {
    if scene.is_null() || joints_json.is_null() || out.is_null() {
        return 0;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadScene = unsafe { &*scene };
        let Some(doc) = &s.doc else { return 0 };
        let Some(instances) = s.inner.instances.as_ref() else {
            return 0;
        };
        if instances.is_empty() || out_cap < instances.len() * 16 {
            return 0;
        }
        let bytes = unsafe { std::slice::from_raw_parts(joints_json, json_len) };
        let Ok(values) = serde_json::from_slice::<std::collections::HashMap<String, f64>>(bytes)
        else {
            return 0;
        };

        let mut posed = doc.clone();
        if let Some(joints) = posed.joints.as_mut() {
            for joint in joints.iter_mut() {
                let v = values
                    .get(&joint.id)
                    .or_else(|| joint.name.as_ref().and_then(|n| values.get(n)));
                if let Some(&v) = v {
                    joint.state = v;
                }
            }
        }
        let world = solve_forward_kinematics(&posed);

        let slice = unsafe { std::slice::from_raw_parts_mut(out, instances.len() * 16) };
        for (i, inst) in instances.iter().enumerate() {
            let t = world
                .get(&inst.instance_id)
                .copied()
                .or(inst.transform)
                .unwrap_or_else(Transform3D::identity);
            write_transform_col_major(&t, &mut slice[i * 16..i * 16 + 16]);
        }
        instances.len()
    }))
    .unwrap_or(0)
}

/// Extract feature edges from a standalone mesh (same semantics as
/// [`vcad_scene_part_edges`]). Returns null on null input or panic.
#[no_mangle]
pub extern "C" fn vcad_mesh_edges(mesh: *const VcadMesh, angle_deg: f32) -> *mut VcadEdges {
    if mesh.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let m: &VcadMesh = unsafe { &*mesh };
        let segs = m.inner.feature_edge_segments(angle_deg);
        let mut flat = Vec::with_capacity(segs.len() * 6);
        for s in segs {
            flat.extend_from_slice(&s);
        }
        Box::into_raw(Box::new(VcadEdges { inner: flat }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Borrow the buffer of an edge handle. On null input, returns an empty view.
#[no_mangle]
pub extern "C" fn vcad_edges_view(edges: *const VcadEdges) -> VcadEdgesView {
    if edges.is_null() {
        return VcadEdgesView::empty();
    }
    let e: &VcadEdges = unsafe { &*edges };
    VcadEdgesView {
        floats: e.inner.as_ptr(),
        floats_len: e.inner.len(),
    }
}

/// Free an edge handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_edges_free(edges: *mut VcadEdges) {
    if !edges.is_null() {
        drop(unsafe { Box::from_raw(edges) });
    }
}

// =========================================================================
// Direct BRep ray tracing — the pixel-perfect render mode. Rays intersect
// the analytic surfaces (no tessellation), so silhouettes and bores are
// exact at any zoom. CPU path today; the signature is renderer-agnostic so
// the wgpu/Metal compute pipeline can swap in behind it.
// =========================================================================

/// Owned RGBA8 frame from [`vcad_scene_raytrace`].
pub struct VcadImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Borrowed view of a [`VcadImage`]: tightly packed RGBA8, row-major,
/// `pixels_len == width * height * 4`.
#[repr(C)]
pub struct VcadImageView {
    pub pixels: *const u8,
    pub pixels_len: usize,
    pub width: u32,
    pub height: u32,
}

impl VcadImageView {
    fn empty() -> Self {
        Self {
            pixels: ptr::null(),
            pixels_len: 0,
            width: 0,
            height: 0,
        }
    }
}

/// Ray-trace every BRep part of an evaluated scene from a pinhole camera.
/// All positions are kernel coordinates (Z-up, mm). `colors` is 3 `f32`s
/// per part (linear RGB); parts beyond `colors_len / 3` fall back to a
/// neutral tone. Parts without a retained BRep solid (pure mesh imports)
/// are skipped. Returns null on null/empty input or panic.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn vcad_scene_raytrace(
    scene: *const VcadScene,
    cam: *const f64,    // [x, y, z]
    target: *const f64, // [x, y, z]
    fov_deg: f64,
    width: u32,
    height: u32,
    colors: *const f32,
    colors_len: usize,
) -> *mut VcadImage {
    if scene.is_null() || cam.is_null() || target.is_null() || width == 0 || height == 0 {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let s: &VcadScene = unsafe { &*scene };
        let cam = unsafe { std::slice::from_raw_parts(cam, 3) };
        let target = unsafe { std::slice::from_raw_parts(target, 3) };
        let color_slice: &[f32] = if colors.is_null() || colors_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(colors, colors_len) }
        };

        let mut solids = Vec::new();
        let mut part_colors = Vec::new();
        for (i, part) in s.inner.parts.iter().enumerate() {
            let Some(brep) = part.solid.as_ref().and_then(|sol| sol.as_brep()) else {
                continue;
            };
            solids.push(std::sync::Arc::new(brep.clone()));
            if color_slice.len() >= (i + 1) * 3 {
                part_colors.extend_from_slice(&color_slice[i * 3..i * 3 + 3]);
            } else {
                part_colors.extend_from_slice(&[0.62, 0.66, 0.70]);
            }
        }
        if solids.is_empty() {
            return ptr::null_mut();
        }

        let pixels = vcad_kernel_raytrace::cpu::render_scene_transparent(
            &solids,
            &[], // identity transforms — parts are already in scene coords
            &part_colors,
            Point3::new(cam[0], cam[1], cam[2]),
            Point3::new(target[0], target[1], target[2]),
            vcad_kernel_math::Dir3::new_normalize(Vec3::new(0.0, 0.0, 1.0)), // Z-up
            width,
            height,
            fov_deg,
        );
        Box::into_raw(Box::new(VcadImage {
            pixels,
            width,
            height,
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Ray-trace the scene on the GPU (wgpu → Metal on macOS) with the full
/// web-parity pipeline: progressive-quality frame with analytic edges,
/// SSAO, sky + ground environment. Unlike [`vcad_scene_raytrace`] the
/// frame is opaque (the shader draws its own backdrop) and is meant to
/// fill the viewport. `metallic`/`roughness` ride per-part after `colors`
/// (3 f32 per part); parts beyond the array get a neutral metal.
///
/// Returns null when no GPU adapter is available (caller falls back to
/// the CPU path), when the scene has no BRep parts, or on panic. The
/// device + pipeline initialize once and are reused across calls.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn vcad_scene_raytrace_gpu(
    scene: *const VcadScene,
    cam: *const f64,
    target: *const f64,
    fov_deg: f64,
    width: u32,
    height: u32,
    colors: *const f32,
    colors_len: usize,
) -> *mut VcadImage {
    if scene.is_null() || cam.is_null() || target.is_null() || width == 0 || height == 0 {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        use vcad_kernel_raytrace::gpu::{GpuCamera, GpuRenderState, GpuScene, RayTracePipeline};

        // One-time device + pipeline init; None is remembered so a machine
        // without a GPU only pays the probe once.
        static PIPELINE: std::sync::OnceLock<Option<RayTracePipeline>> = std::sync::OnceLock::new();
        let Some(pipeline) = PIPELINE
            .get_or_init(|| {
                let ctx = vcad_kernel_gpu::GpuContext::init_blocking().ok()?;
                RayTracePipeline::new(ctx).ok()
            })
            .as_ref()
        else {
            return ptr::null_mut();
        };
        let Some(ctx) = vcad_kernel_gpu::GpuContext::get() else {
            return ptr::null_mut();
        };

        let s: &VcadScene = unsafe { &*scene };
        let cam = unsafe { std::slice::from_raw_parts(cam, 3) };
        let target = unsafe { std::slice::from_raw_parts(target, 3) };
        let color_slice: &[f32] = if colors.is_null() || colors_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(colors, colors_len) }
        };

        // Upload every BRep part, coloring each before the merge so the
        // per-scene material index survives the offset arithmetic.
        let mut merged: Option<GpuScene> = None;
        for (i, part) in s.inner.parts.iter().enumerate() {
            let Some(brep) = part.solid.as_ref().and_then(|sol| sol.as_brep()) else {
                continue;
            };
            let Ok(mut gs) = GpuScene::from_brep(brep) else {
                continue;
            };
            let c = if color_slice.len() >= (i + 1) * 3 {
                [
                    color_slice[i * 3],
                    color_slice[i * 3 + 1],
                    color_slice[i * 3 + 2],
                ]
            } else {
                [0.62, 0.66, 0.70]
            };
            gs.set_material(c[0], c[1], c[2], 0.85, 0.35);
            merged = Some(match merged {
                Some(acc) => acc.merge(gs),
                None => gs,
            });
        }
        let Some(gpu_scene) = merged else {
            return ptr::null_mut();
        };

        let camera = GpuCamera::new(
            [cam[0] as f32, cam[1] as f32, cam[2] as f32],
            [target[0] as f32, target[1] as f32, target[2] as f32],
            [0.0, 0.0, 1.0],
            fov_deg.to_radians() as f32,
            width,
            height,
        );

        let result = pollster::block_on(pipeline.render_with_render_state(
            ctx,
            &gpu_scene,
            &camera,
            width,
            height,
            None,
            GpuRenderState::new(1),
        ));
        match result {
            Ok((pixels, _accum)) if pixels.len() == (width * height * 4) as usize => {
                Box::into_raw(Box::new(VcadImage {
                    pixels,
                    width,
                    height,
                }))
            }
            _ => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Borrow an image's pixel buffer. On null input, returns an empty view.
#[no_mangle]
pub extern "C" fn vcad_image_view(image: *const VcadImage) -> VcadImageView {
    if image.is_null() {
        return VcadImageView::empty();
    }
    let img: &VcadImage = unsafe { &*image };
    VcadImageView {
        pixels: img.pixels.as_ptr(),
        pixels_len: img.pixels.len(),
        width: img.width,
        height: img.height,
    }
}

/// Free an image handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_image_free(image: *mut VcadImage) {
    if !image.is_null() {
        drop(unsafe { Box::from_raw(image) });
    }
}

/// Free a scene handle. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_scene_free(scene: *mut VcadScene) {
    if !scene.is_null() {
        drop(unsafe { Box::from_raw(scene) });
    }
}

// =========================================================================
// Resident parametric document — the cross-domain co-design hot loop.
//
// A `VcadDoc` keeps a `Document` (with its parameters + bindings) alive so a
// gesture can set ONE parameter and re-evaluate, and every node bound to that
// parameter re-solves together. This is how one `connector_x` drives both a
// PCB connector (electrical) and an enclosure cutout (mechanical) at once —
// the coupling rides the kernel's existing parameter/binding system
// (`vcad_eval` resolves bindings inside `evaluate_document`).
// =========================================================================

/// Opaque handle to a resident parametric document.
pub struct VcadDoc {
    inner: Document,
}

/// Parse a `.vcad` JSON document and keep it resident for live re-solve.
/// Returns null on invalid UTF-8, parse error, or panic.
#[no_mangle]
pub extern "C" fn vcad_doc_load(json: *const u8, json_len: usize) -> *mut VcadDoc {
    if json.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return ptr::null_mut();
        };
        match Document::from_json(text) {
            Ok(d) => Box::into_raw(Box::new(VcadDoc { inner: d })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Build the slice-1 worked example in process: a servo-gripper subset where a
/// single `connector_x` parameter is bound to BOTH an enclosure cutout
/// (mechanical) and a board connector (electrical). Dragging it re-solves both
/// domains together — the minimal Connector Drag.
#[no_mangle]
pub extern "C" fn vcad_doc_gripper_slice1() -> *mut VcadDoc {
    catch_unwind(|| {
        Box::into_raw(Box::new(VcadDoc {
            inner: build_gripper_slice1(),
        }))
    })
    .unwrap_or(ptr::null_mut())
}

/// Interactive eval options: skip the O(n^2) clash pass the native app doesn't render.
fn interactive_opts() -> EvalOptions {
    EvalOptions {
        skip_clash_detection: true,
        ..Default::default()
    }
}

/// Set a parameter on a resident document and re-evaluate to a fresh scene.
/// Bindings re-apply inside `evaluate_document`, so every node driven by this
/// parameter moves together — ALL roots, including the (expensive) sheet-metal
/// fold. This is the SETTLE path. Caller owns the scene (`vcad_scene_free`).
/// Returns null on null input, unknown parameter name encoding, eval error, or panic.
#[no_mangle]
pub extern "C" fn vcad_doc_set_param(
    doc: *mut VcadDoc,
    name: *const c_char,
    value: f64,
) -> *mut VcadScene {
    if doc.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let d: &mut VcadDoc = unsafe { &mut *doc };
        let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
            return ptr::null_mut();
        };
        // Overwrite the parameter value; bindings (the coupling) stay intact and
        // re-apply during evaluation.
        d.inner
            .parameters
            .insert(name.to_string(), Parameter::literal(value));
        match evaluate_document(&d.inner, &interactive_opts()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: None,
            })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Set a parameter and re-evaluate only the CHEAP roots — those whose subtree
/// contains no sheet-metal fold. This is the per-FRAME path during a drag: the
/// mechanical cutout + board connector follow the finger live (~15 ms) while the
/// expensive sheet-metal fold is left to the settle path (`vcad_doc_set_param`).
/// The parameter is still written to the resident doc, so the deferred full
/// solve picks it up. Same ownership as `vcad_doc_set_param`.
#[no_mangle]
pub extern "C" fn vcad_doc_set_param_cheap(
    doc: *mut VcadDoc,
    name: *const c_char,
    value: f64,
) -> *mut VcadScene {
    if doc.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let d: &mut VcadDoc = unsafe { &mut *doc };
        let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
            return ptr::null_mut();
        };
        d.inner
            .parameters
            .insert(name.to_string(), Parameter::literal(value));
        // Evaluate a clone whose expensive (sheet-metal) roots are dropped.
        let mut fast = d.inner.clone();
        let nodes = fast.nodes.clone();
        fast.roots
            .retain(|e| !subtree_has_sheet_metal(&nodes, e.root));
        match evaluate_document(&fast, &interactive_opts()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: None,
            })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
}

/// Per-FRAME honest min-wall: resolve the resident doc at its current
/// parameters and measure the REAL enclosure side-wall clearance (mm) from the
/// geometry — the same computation the full solve reports, but cheap enough to
/// run live during a drag (it evaluates only the cutout + housing nodes, no
/// sheet-metal fold, no routing). This is what the Receipt's live min-wall row
/// reads so the mid-drag number is geometry, not Swift-side arithmetic keyed to
/// the box's literal extents. Returns f64::INFINITY when unmeasurable (null doc,
/// no enclosure, resolve/eval failure) so a missing measure can't read as a
/// violated wall.
#[no_mangle]
pub extern "C" fn vcad_doc_min_wall(doc: *const VcadDoc) -> f64 {
    if doc.is_null() {
        return f64::INFINITY;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let d: &VcadDoc = unsafe { &*doc };
        let mut resolved = d.inner.clone();
        let _ = vcad_eval::resolve_document(&mut resolved);
        enclosure_min_wall(&resolved).unwrap_or(f64::INFINITY)
    }))
    .unwrap_or(f64::INFINITY)
}

/// Whether `op` is a sheet-metal fold op (expensive — booleans per bend).
fn is_sheet_metal_op(op: &CsgOp) -> bool {
    matches!(
        op,
        CsgOp::SheetMetalBaseFlangeRect { .. }
            | CsgOp::SheetMetalBaseFlangePolygon { .. }
            | CsgOp::SheetMetalEdgeFlange { .. }
            | CsgOp::SheetMetalHem { .. }
            | CsgOp::SheetMetalJog { .. }
            | CsgOp::SheetMetalBendRelief { .. }
    )
}

/// Structural child node-ids of an op (mirror of the IR's internal walk).
fn op_child_ids(op: &CsgOp) -> Vec<u64> {
    match op {
        CsgOp::Translate { child, .. }
        | CsgOp::Rotate { child, .. }
        | CsgOp::Scale { child, .. }
        | CsgOp::Fillet { child, .. }
        | CsgOp::Chamfer { child, .. }
        | CsgOp::Shell { child, .. }
        | CsgOp::LinearPattern { child, .. }
        | CsgOp::CircularPattern { child, .. } => vec![*child],
        CsgOp::Union { left, right }
        | CsgOp::Difference { left, right }
        | CsgOp::Intersection { left, right } => vec![*left, *right],
        CsgOp::SheetMetalEdgeFlange { parent, .. }
        | CsgOp::SheetMetalHem { parent, .. }
        | CsgOp::SheetMetalJog { parent, .. }
        | CsgOp::SheetMetalBendRelief { parent, .. } => vec![*parent],
        _ => vec![],
    }
}

/// True if the subtree rooted at `id` contains any sheet-metal fold op.
fn subtree_has_sheet_metal(nodes: &std::collections::HashMap<u64, Node>, id: u64) -> bool {
    let Some(node) = nodes.get(&id) else {
        return false;
    };
    if is_sheet_metal_op(&node.op) {
        return true;
    }
    op_child_ids(&node.op)
        .into_iter()
        .any(|c| subtree_has_sheet_metal(nodes, c))
}

/// Free a resident document. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_doc_free(doc: *mut VcadDoc) {
    if !doc.is_null() {
        drop(unsafe { Box::from_raw(doc) });
    }
}

/// The slice-1 gripper: an enclosure (a box minus a connector cutout) and a
/// board (a plate plus a connector body), with one `connector_x` parameter
/// bound to both the cutout's and the connector's X offset. Z-up, millimetres.
fn build_gripper_slice1() -> Document {
    let v = IrVec3::new;
    let mut doc = Document::new();

    // --- mechanical: enclosure = base box − connector cutout ---
    doc.nodes.insert(
        1,
        Node {
            id: 1,
            name: Some("enclosure".into()),
            op: CsgOp::Cube {
                size: v(80.0, 50.0, 30.0),
            },
        },
    );
    doc.nodes.insert(
        2,
        Node {
            id: 2,
            name: None,
            op: CsgOp::Cube {
                size: v(12.0, 8.0, 12.0),
            },
        },
    );
    // offset.x is bound below; y/z fixed (straddles the front wall at y≈0).
    doc.nodes.insert(
        3,
        Node {
            id: 3,
            name: None,
            op: CsgOp::Translate {
                child: 2,
                offset: v(0.0, -2.0, 9.0),
            },
        },
    );
    doc.nodes.insert(
        4,
        Node {
            id: 4,
            name: Some("housing".into()),
            op: CsgOp::Difference { left: 1, right: 3 },
        },
    );

    // --- electrical: board = plate + connector body ---
    doc.nodes.insert(
        5,
        Node {
            id: 5,
            name: None,
            op: CsgOp::Cube {
                size: v(70.0, 40.0, 2.0),
            },
        },
    );
    doc.nodes.insert(
        6,
        Node {
            id: 6,
            name: None,
            op: CsgOp::Translate {
                child: 5,
                offset: v(5.0, 5.0, 5.0),
            },
        },
    );
    doc.nodes.insert(
        7,
        Node {
            id: 7,
            name: None,
            op: CsgOp::Cube {
                size: v(10.0, 14.0, 6.0),
            },
        },
    );
    doc.nodes.insert(
        8,
        Node {
            id: 8,
            name: None,
            op: CsgOp::Translate {
                child: 7,
                offset: v(0.0, 2.0, 6.0),
            },
        },
    );
    doc.nodes.insert(
        9,
        Node {
            id: 9,
            name: Some("board".into()),
            op: CsgOp::Union { left: 6, right: 8 },
        },
    );

    // --- sheet metal: an L-bracket whose upstand height tracks connector_x ---
    // Foundation-tier sheet metal is now a first-class evaluator (vcad-eval), so
    // the bracket is just more nodes in this DAG — driven by the SAME binding
    // mechanism as the cubes, re-solved by the same evaluate_document. No
    // separate fold FFI: the cross-domain coupling rides one resolve.
    doc.nodes.insert(
        10,
        Node {
            id: 10,
            name: Some("bracket-base".into()),
            op: CsgOp::SheetMetalBaseFlangeRect {
                width: 70.0,
                depth: 18.0,
                thickness: 1.0,
                material: "al-soft".into(),
                shop_profile: None,
                engravings: None,
            },
        },
    );
    // Fold edge 2 (the back edge (70,18)->(0,18)) up so the upstand rises BEHIND
    // the connector body (Y≈2..16) and backs it, rather than in front of the board.
    doc.nodes.insert(
        11,
        Node {
            id: 11,
            name: Some("bracket".into()),
            op: CsgOp::SheetMetalEdgeFlange {
                parent: 10,
                panel_id: 0,
                edge_index: 2,
                length: 12.0,
                angle: std::f64::consts::FRAC_PI_2,
                radius: Some(1.0),
                direction: vcad_ir::SheetMetalDirection::Down,
                manual_k: Some(0.44),
            },
        },
    );
    doc.nodes.insert(
        12,
        Node {
            id: 12,
            name: None,
            op: CsgOp::Translate {
                child: 11,
                offset: v(5.0, 1.0, 3.5),
            },
        },
    );

    // --- the coupling: ONE parameter drives THREE domains ---
    doc.parameters
        .insert("connector_x".into(), Parameter::literal(40.0));
    doc.bindings.bind(
        BindingKey::new(3, "offset.x"),
        Expr::formula("connector_x - 6"),
    );
    doc.bindings.bind(
        BindingKey::new(8, "offset.x"),
        Expr::formula("connector_x - 5"),
    );
    // the bracket upstand grows toward the connector (length stays > 0 across the
    // 4..76 drag range: 5..23 mm).
    doc.bindings.bind(
        BindingKey::new(11, "length"),
        Expr::formula("connector_x * 0.25 + 4"),
    );

    // --- the PCB lives IN the document: a "pcb." binding moves the connector
    // footprint declaratively, so the board's copper re-routes from the SAME
    // resolve_document as the geometry. (node id 0 is a placeholder — the "pcb."
    // prefix routes the binding to the PCB, not a node.) The slope keeps J1 on
    // the 70 mm board (board-local ~10..61) across the 4..76 drag range.
    doc.pcb = Some(build_gripper_slice2_board(40.0));
    doc.bindings.bind(
        BindingKey::new(0, "pcb.J1.position.x"),
        Expr::formula("connector_x * 0.7 + 8"),
    );

    doc.roots.push(SceneEntry {
        root: 4,
        material: "aluminum".into(),
        visible: None,
    });
    doc.roots.push(SceneEntry {
        root: 9,
        material: "board".into(),
        visible: None,
    });
    doc.roots.push(SceneEntry {
        root: 12,
        material: "bracket".into(),
        visible: None,
    });
    doc.materials.insert(
        "aluminum".into(),
        MaterialDef {
            name: "aluminum".into(),
            color: [0.72, 0.74, 0.78],
            metallic: 0.6,
            roughness: 0.34,
            density: None,
            friction: None,
            ..Default::default()
        },
    );
    doc.materials.insert(
        "board".into(),
        MaterialDef {
            name: "board".into(),
            color: [0.12, 0.42, 0.18],
            metallic: 0.0,
            roughness: 0.6,
            density: None,
            friction: None,
            ..Default::default()
        },
    );
    doc.materials.insert(
        "bracket".into(),
        MaterialDef {
            name: "bracket".into(),
            color: [0.66, 0.68, 0.72],
            metallic: 0.85,
            roughness: 0.3,
            density: None,
            friction: None,
            ..Default::default()
        },
    );
    doc
}

// =========================================================================
// Slice 2: copper re-route in the connector-drag loop.
//
// The same `connector_x` that drives the slice-1 enclosure cutout + connector
// body also slides a connector footprint on a tiny 2-net board. Routing the
// board with the OHM auto-router gives back copper polylines that re-thread to
// the moved connector — the electrical domain of the Connector Drag. This is a
// SEPARATE FFI path from the resident document: `route_all` is stateless and
// reads only a `&Pcb`, so we build the board fresh at `connector_x` and route
// it. Swift drives both paths from one `connectorX`.
// =========================================================================

/// Build the slice-2 routable board at `connector_x` (mm, board-LOCAL — the
/// board plate's own 0..70 × 0..40 frame). One fixed 2-pad part sits at the
/// left; the connector part slides with `connector_x` so the SIG and GND nets
/// each have a moving endpoint the router must re-thread. 4 pads / 2 nets /
/// 2-per-net is the minimum that routes: a net with <2 resolved pads is
/// silently skipped by the ratsnest.
fn build_gripper_slice2_board(connector_x: f64) -> Pcb {
    // 70 × 40 rectangle, origin at corner — matches the slice-1 board plate.
    let outline = BoardOutline {
        vertices: vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(70.0, 0.0),
            Vec2::new(70.0, 40.0),
            Vec2::new(0.0, 40.0),
        ],
        cutouts: vec![],
        thickness: 1.6,
    };

    // 2-layer stackup; FCu carries this board, BCu present as an escape lane.
    let stackup = LayerStackup {
        layers: vec![
            StackupLayer {
                layer: PcbLayer::FCu,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(1.53),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            },
            StackupLayer {
                layer: PcbLayer::BCu,
                copper_thickness: Some(0.035),
                dielectric_thickness: None,
                dielectric_er: None,
                material: None,
            },
        ],
    };

    // The router keys nets off `pad.net`, so net id == name keeps trace.net
    // human-readable ("SIG"/"GND").
    let nets = vec![
        Net {
            id: "SIG".into(),
            name: "SIG".into(),
        },
        Net {
            id: "GND".into(),
            name: "GND".into(),
        },
    ];

    // Non-zero rules are load-bearing: zeroed clearance/width routes invalid copper.
    let rules = DesignRules {
        default_rules: NetClassRules {
            name: "default".into(),
            trace_width: 0.25,
            clearance: 0.2,
            via_diameter: 0.8,
            via_drill: 0.4,
            diff_pair_gap: None,
            diff_pair_width: None,
            target_impedance: None,
            target_diff_impedance: None,
        },
        class_rules: vec![],
        net_class_assignments: std::collections::HashMap::new(),
        edge_clearance: 0.5,
        hole_to_hole: 0.5,
        min_annular_ring: 0.15,
        min_drill: 0.2,
    };

    let pad = |num: &str, net: &str, x: f64, y: f64| Pad {
        number: num.into(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: 1.0,
            height: 1.2,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: Some(net.into()),
        layers: vec![PcbLayer::FCu],
    };
    let two_pad = |reference: &str, value: &str, x: f64| Footprint {
        reference: reference.into(),
        value: value.into(),
        footprint_name: "SLICE2-2PAD".into(),
        position: Vec2::new(x, 20.0),
        rotation: 0.0,
        front: true,
        // SIG above, GND below — 5 mm apart, clear at 0.25/0.2 rules.
        pads: vec![pad("1", "SIG", 0.0, 2.5), pad("2", "GND", 0.0, -2.5)],
        graphics: vec![],
        model_3d: None,
        properties: std::collections::HashMap::new(),
    };

    // Fixed part near the left; connector slides with connector_x. The slice-1
    // board plate is world-translated +5 in X, and the connector BODY centers on
    // world `connector_x`, so the footprint centers on board-local connector_x-5.
    // Clamp to keep both pads on the board with edge clearance, and never left of
    // the fixed part (no pad overlap).
    let fixed = two_pad("U1", "FIXED", 10.0);
    let cx = (connector_x - 5.0).clamp(16.0, 64.0);
    let connector = two_pad("J1", "CONN", cx);

    Pcb {
        outline,
        stackup,
        nets,
        rules,
        footprints: vec![fixed, connector],
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

/// One straight copper segment from the auto-router, board-LOCAL mm.
/// `layer`: 0 = FCu, 1 = BCu, 2+ = inner. `net_id`: 0 = SIG, 1 = GND.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VcadTraceLine {
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub width: f64,
    pub layer: u32,
    pub net_id: u32,
}

/// Owned result of routing the slice-2 board: the copper to draw, plus how many
/// nets failed to route (for an honest "copper routed ✓ / unrouted ✗" verdict).
pub struct VcadRouteResult {
    traces: Vec<VcadTraceLine>,
    unrouted: usize,
}

fn layer_code(l: PcbLayer) -> u32 {
    match l {
        PcbLayer::FCu => 0,
        PcbLayer::BCu => 1,
        PcbLayer::In1Cu => 2,
        PcbLayer::In2Cu => 3,
        _ => 0,
    }
}

fn net_code(name: &str) -> u32 {
    if name == "GND" {
        1
    } else {
        0
    }
}

/// Build the slice-2 board at `connector_x`, route it, and return the copper.
/// `width` ≤ 0 falls back to 0.25 mm. Caller owns the result
/// (`vcad_route_result_free`). Null only on panic.
#[no_mangle]
pub extern "C" fn vcad_route_traces(connector_x: f64, width: f64) -> *mut VcadRouteResult {
    catch_unwind(|| {
        let pcb = build_gripper_slice2_board(connector_x);
        let w = if width > 0.0 { width } else { 0.25 };
        let r = route_all(&pcb, w, &[]);
        let traces = r
            .traces
            .iter()
            .map(|t| VcadTraceLine {
                start: [t.start.x, t.start.y],
                end: [t.end.x, t.end.y],
                width: t.width,
                layer: layer_code(t.layer),
                net_id: net_code(&t.net),
            })
            .collect();
        Box::into_raw(Box::new(VcadRouteResult {
            traces,
            unrouted: r.unrouted_nets.len(),
        }))
    })
    .unwrap_or(ptr::null_mut())
}

/// Number of copper segments in a route result. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_route_result_trace_count(r: *const VcadRouteResult) -> usize {
    if r.is_null() {
        return 0;
    }
    unsafe { &*r }.traces.len()
}

/// Copper segment `idx` (board-local mm). Returns a zeroed line on null / OOB —
/// Swift must bound by `vcad_route_result_trace_count`.
#[no_mangle]
pub extern "C" fn vcad_route_result_trace(r: *const VcadRouteResult, idx: usize) -> VcadTraceLine {
    let zero = VcadTraceLine {
        start: [0.0; 2],
        end: [0.0; 2],
        width: 0.0,
        layer: 0,
        net_id: 0,
    };
    if r.is_null() {
        return zero;
    }
    unsafe { &*r }.traces.get(idx).copied().unwrap_or(zero)
}

/// How many nets could not be routed legally (0 = fully routed). 0 on null.
#[no_mangle]
pub extern "C" fn vcad_route_result_unrouted_count(r: *const VcadRouteResult) -> usize {
    if r.is_null() {
        return 0;
    }
    unsafe { &*r }.unrouted
}

/// Free a route result. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_route_result_free(r: *mut VcadRouteResult) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

// =========================================================================
// The unified cross-domain solve — ONE call returns every domain.
//
// `vcad_doc_solve` sets connector_x and returns geometry (the meshes, including
// the sheet-metal fold), copper (routed traces), AND the receipt scalars — all
// descending from the SAME resolved parameter. This is the cross-domain vision
// at the FFI boundary: the KERNEL fans connector_x out to mechanical +
// electrical + sheet-metal, not the view layer. It's the SETTLE path (full +
// expensive); the per-frame cutout still uses `vcad_doc_set_param_cheap`.
// =========================================================================

/// Rebuild the slice-1 bracket `SheetMetalModel` at a resolved flange `length`,
/// mirroring nodes 10/11 of `build_gripper_slice1`. Kept local so the receipt
/// path doesn't reach into the evaluator's private chain-walker.
fn gripper_bracket_model(
    flange_len: f64,
) -> Result<vcad_kernel::vcad_kernel_sheet::SheetMetalModel, String> {
    use vcad_kernel::vcad_kernel_sheet::edge_flange::EdgeFlangeParams;
    use vcad_kernel::vcad_kernel_sheet::{
        add_edge_flange, base_flange_rect, BendDirection, BendTable, FlangePosition,
    };
    let mut model = base_flange_rect(70.0, 18.0, 1.0).map_err(|e| e.to_string())?;
    model.material = "al-soft".into();
    add_edge_flange(
        &mut model,
        &BendTable::builtin(),
        EdgeFlangeParams {
            panel: 0,
            edge_index: 2,
            length: flange_len,
            angle: std::f64::consts::FRAC_PI_2,
            radius: 1.0,
            direction: BendDirection::Down,
            position: FlangePosition::MaterialInside,
            material: "al-soft".into(),
            manual_k: Some(0.44),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(model)
}

/// Real minimum enclosure wall: the smallest clearance from the connector cutout
/// to an enclosure outer face, in mm — the Receipt's first gating check.
///
/// This is GEOMETRY that tracks the parameter, not arithmetic keyed to the box's
/// literal extents. We evaluate the RESOLVED cutout (node 3) and enclosure
/// (node 4) to solids and read their axis-aligned bounding boxes; each face's
/// wall is `enclosure_outer − cutout_edge`, and the min-wall is the smallest over
/// the FIVE ENCLOSED faces. We gate all of them — not just the connector's slide
/// axis — because the true thinnest wall here is the fixed ±Z span (9 mm), not
/// the ±X walls (34 mm at centre); reporting only X would overstate the margin.
/// The one excluded face is the −Y front, which the cutout deliberately breaches:
/// that's the connector PORT (an opening by design, not a wall). A wall goes
/// negative when the cutout breaches the shell on a side it shouldn't → Violated.
///
/// Returns `None` when either solid can't be evaluated, so a missing enclosure
/// surfaces as "unmeasured" rather than a fake 0.
fn enclosure_min_wall(resolved: &Document) -> Option<f64> {
    let mut cache = std::collections::HashMap::new();
    // node 4 = housing (box − cutout), node 3 = the placed cutout, at their
    // resolved positions (resolve_document already patched the X binding).
    let housing = vcad_eval::evaluate_node(4, &resolved.nodes, &mut cache).ok()??;
    let cutout = vcad_eval::evaluate_node(3, &resolved.nodes, &mut cache).ok()??;
    let (h_min, h_max) = housing.bounding_box();
    let (c_min, c_max) = cutout.bounding_box();
    let neg_x = c_min[0] - h_min[0]; // left wall
    let pos_x = h_max[0] - c_max[0]; // right wall
    let pos_y = h_max[1] - c_max[1]; // back wall (−Y front is the port — excluded)
    let neg_z = c_min[2] - h_min[2]; // bottom wall
    let pos_z = h_max[2] - c_max[2]; // top wall
    Some(
        [neg_x, pos_x, pos_y, neg_z, pos_z]
            .into_iter()
            .fold(f64::INFINITY, f64::min),
    )
}

/// Enclosure CNC cost from the RESOLVED solid, in integer cents.
///
/// Real removed-volume machining model (`estimate_cnc_from_removed_volume`):
/// stock = the part's bounding-box block, removed = stock − true part volume.
/// Both are read off the actual `box − cutout` BRep, so the number tracks the
/// cutout as it moves — no hardcoded dims. Returns 0 only when there's no
/// enclosure solid to cost. The `is_estimate` flag the model sets is implicit:
/// this is a labeled CNC estimate (bbox stock, 1 feature = the pocket).
fn enclosure_cnc_cents(scene: &EvaluatedScene) -> u64 {
    use vcad_kernel::vcad_kernel_cost::{estimate_cnc_from_removed_volume, Material};
    let Some(solid) = scene.parts.iter().find_map(|p| p.solid.as_ref()) else {
        return 0;
    };
    let part_vol = solid.volume();
    let (mn, mx) = solid.bounding_box();
    let stock_vol = (mx[0] - mn[0]).abs() * (mx[1] - mn[1]).abs() * (mx[2] - mn[2]).abs();
    // 1 machined feature: the connector pocket (the only cutout in this DAG).
    let est = estimate_cnc_from_removed_volume(stock_vol, part_vol, 1, &Material::aluminum_6061());
    (est.total_usd * 100.0).round().max(0.0) as u64
}

/// PCB fabrication estimate, in integer cents.
///
/// There is NO Rust PCB cost model — this is a clearly-labeled rate that
/// mirrors the MCP `jlcpcb` placeholder (`estimatePcb`) so the native and MCP
/// quotes agree: 2-layer, area-driven.
/// `unit = round(area_cm2 · 6 · layer_factor) + 30`, plus a `200`-cent setup at
/// qty 1. The area is read off the resolved board outline's bounding box, so it
/// tracks a resized board. pricing_basis = ESTIMATE: surface it with an "est."
/// tag, never as a kernel result.
fn board_estimate_cents(pcb: Option<&Pcb>) -> u64 {
    let Some(pcb) = pcb else { return 0 };
    let verts = &pcb.outline.vertices;
    if verts.is_empty() {
        return 0;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for v in verts {
        min_x = min_x.min(v.x);
        min_y = min_y.min(v.y);
        max_x = max_x.max(v.x);
        max_y = max_y.max(v.y);
    }
    let area_mm2 = (max_x - min_x).abs() * (max_y - min_y).abs();
    let area_cm2 = (area_mm2 / 100.0).max(1.0);
    let layers = pcb.stackup.layers.len().max(2) as f64;
    let layer_factor = 1.0 + (layers - 2.0).max(0.0) * 0.4;
    let unit = (area_cm2 * 6.0 * layer_factor).round() + 30.0;
    (unit as u64) + 200
}

/// Bracket DFM verdict + a REAL bracket quote at a given flange length.
/// Returns `(bracket_ok, severity, cents, lead_days)`. `bracket_ok = 0` only on a
/// hard `Severity::Error` (a warning keeps it 1). The cost is a kernel model
/// (`estimate_cost`); the lead time is a stated heuristic (no kernel lead model).
fn gripper_bracket_verdict(flange_len: f64) -> (u8, u8, u64, u32) {
    use vcad_kernel::vcad_kernel_sheet::{
        check_manufacturability, estimate_cost, unfold, CostRates, FlatPattern, Severity,
        ShopProfile,
    };
    let Ok(mut model) = gripper_bracket_model(flange_len) else {
        return (0, 2, 0, 0); // can't build the bracket → Violated, no quote
    };
    // DFM verdict against a generic shop (min flange height = 5·t, etc.).
    let viols = check_manufacturability(&model, &ShopProfile::generic());
    let has_error = viols.iter().any(|v| v.severity() == Severity::Error);
    let severity = if has_error {
        2
    } else if viols.is_empty() {
        0
    } else {
        1
    };
    let bracket_ok = u8::from(!has_error);

    // Quote: unfold → flat pattern → real line-item cost (qty 1, generic rates).
    if unfold(&mut model).is_err() {
        return (bracket_ok, severity, 0, 0);
    }
    let flat = FlatPattern::from_model(&model);
    let cost = estimate_cost(&model, &flat, 1, &CostRates::generic());
    let cents = (cost.total_each * 100.0).round().max(0.0) as u64;
    // Lead time is a STATED HEURISTIC: 5 base days + 1 per bend, capped.
    let lead_days = (5 + cost.bends).min(15);
    (bracket_ok, severity, cents, lead_days)
}

/// Owned result of a full cross-domain solve: meshes + copper + receipt.
pub struct VcadSolve {
    scene: EvaluatedScene,
    traces: Vec<VcadTraceLine>,
    unrouted: usize,
    min_wall: f64,
    /// Bracket DFM: 1 = Held (no manufacturability Error), 0 = Violated.
    /// Warnings keep it 1 — only a hard Error kills the Make-it gate.
    bracket_ok: u8,
    /// Worst bracket finding for display: 0 = clean, 1 = Warning, 2 = Error.
    bracket_severity: u8,
    /// Total manufacturing quote in integer cents (enclosure + board + bracket,
    /// qty 1) — no float across the ABI. Per-domain breakdown below.
    quote_cost_cents: u64,
    /// Enclosure CNC cost (cents) — REAL removed-volume kernel model.
    quote_enclosure_cents: u64,
    /// PCB fab cost (cents) — LABELED ESTIMATE (no Rust PCB cost model).
    quote_board_cents: u64,
    /// Sheet-metal bracket cost (cents) — REAL kernel model (estimate_cost).
    quote_bracket_cents: u64,
    /// 1 when the total includes a labeled estimate (the board) the UI should
    /// tag "est."; 0 when every line is a kernel-real cost.
    quote_has_estimate: u8,
    /// Estimated lead time, business days. HEURISTIC — no kernel lead model;
    /// the max over participating domains (parts fabricate in parallel).
    lead_days: u32,
}

/// Borrowed mesh view over an evaluated part (normals reported only when they
/// match the vertex count; the caller synthesizes otherwise).
fn eval_mesh_view(m: &vcad_eval::EvaluatedMesh) -> VcadMeshView {
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

/// Set `connector_x` and solve EVERY domain at once: evaluate the document to
/// meshes (enclosure + board + sheet-metal bracket), route the board's copper,
/// and compute the receipt — all from the one resolved parameter. Caller owns
/// the result (`vcad_solve_free`). Null on null input / eval error / panic.
#[no_mangle]
pub extern "C" fn vcad_doc_solve(
    doc: *mut VcadDoc,
    name: *const c_char,
    value: f64,
) -> *mut VcadSolve {
    if doc.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let d: &mut VcadDoc = unsafe { &mut *doc };
        let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
            return ptr::null_mut();
        };
        d.inner
            .parameters
            .insert(name.to_string(), Parameter::literal(value));
        let Ok(scene) = evaluate_document(&d.inner, &interactive_opts()) else {
            return ptr::null_mut();
        };
        // The PCB lives in the document: resolve it (the "pcb." binding moves the
        // connector footprint) and route THAT — copper descends from the same
        // resolve_document as the geometry, not a procedural rebuild. Resolve a
        // clone since evaluate_document resolved a clone internally for the meshes
        // and didn't mutate the resident doc.
        let cx = vcad_ir::resolve_parameters(&d.inner.parameters)
            .ok()
            .and_then(|env| env.get("connector_x").copied())
            .unwrap_or(value);
        let mut resolved = d.inner.clone();
        let _ = vcad_eval::resolve_document(&mut resolved);
        let (traces, unrouted) = match &resolved.pcb {
            Some(pcb) => {
                let r = route_all(pcb, 0.25, &[]);
                let traces: Vec<VcadTraceLine> = r
                    .traces
                    .iter()
                    .map(|t| VcadTraceLine {
                        start: [t.start.x, t.start.y],
                        end: [t.end.x, t.end.y],
                        width: t.width,
                        layer: layer_code(t.layer),
                        net_id: net_code(&t.net),
                    })
                    .collect();
                (traces, r.unrouted_nets.len())
            }
            None => (Vec::new(), 0),
        };
        // Receipt: REAL minimum wall thickness of the resolved enclosure BRep
        // (DFM thickness raycast), not arithmetic on the box's literal extents.
        // Falls to a large sentinel only if there's no solid to measure, so a
        // missing enclosure can't masquerade as a violated wall.
        let _ = cx; // retained for the binding fallback below, not the wall.
        let min_wall = enclosure_min_wall(&resolved).unwrap_or(f64::INFINITY);
        // Bracket DFM + quote — read the flange length that actually folded off the
        // resolved node 11 (resolve_document patches op fields in place), so the
        // verdict matches the rendered bracket. Fall back to the binding formula.
        let flange_len = match resolved.nodes.get(&11).map(|n| &n.op) {
            Some(CsgOp::SheetMetalEdgeFlange { length, .. }) => *length,
            _ => cx * 0.25 + 4.0,
        };
        let (bracket_ok, bracket_severity, quote_bracket_cents, bracket_lead) =
            gripper_bracket_verdict(flange_len);
        // Multi-domain quote: enclosure CNC (real) + board fab (labeled est) +
        // bracket (real). Lead time = max over domains (parallel fabrication,
        // ship on the slowest); the per-domain leads are stated heuristics.
        let quote_enclosure_cents = enclosure_cnc_cents(&scene);
        let quote_board_cents = board_estimate_cents(resolved.pcb.as_ref());
        let quote_cost_cents = quote_enclosure_cents + quote_board_cents + quote_bracket_cents;
        let quote_has_estimate = u8::from(quote_board_cents > 0);
        // Per-domain lead heuristics (business days): CNC 10, PCB 7, bracket
        // from the sheet model. No kernel lead model exists.
        let lead_days = bracket_lead.max(10).max(7);
        Box::into_raw(Box::new(VcadSolve {
            scene,
            traces,
            unrouted,
            min_wall,
            bracket_ok,
            bracket_severity,
            quote_cost_cents,
            quote_enclosure_cents,
            quote_board_cents,
            quote_bracket_cents,
            quote_has_estimate,
            lead_days,
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Number of evaluated parts (mesh roots) in a solve. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_part_count(s: *const VcadSolve) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.scene.parts.len()
}

/// Borrow the `index`-th part's mesh. Empty view on null / OOB.
#[no_mangle]
pub extern "C" fn vcad_solve_part_mesh(s: *const VcadSolve, index: usize) -> VcadMeshView {
    if s.is_null() {
        return VcadMeshView::empty();
    }
    match unsafe { &*s }.scene.parts.get(index) {
        Some(part) => eval_mesh_view(&part.mesh),
        None => VcadMeshView::empty(),
    }
}

/// Number of routed copper segments in a solve. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_trace_count(s: *const VcadSolve) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.traces.len()
}

/// Copper segment `idx` (board-local mm). Zeroed line on null / OOB.
#[no_mangle]
pub extern "C" fn vcad_solve_trace(s: *const VcadSolve, idx: usize) -> VcadTraceLine {
    let zero = VcadTraceLine {
        start: [0.0; 2],
        end: [0.0; 2],
        width: 0.0,
        layer: 0,
        net_id: 0,
    };
    if s.is_null() {
        return zero;
    }
    unsafe { &*s }.traces.get(idx).copied().unwrap_or(zero)
}

/// Receipt: nets that failed to route (0 = fully routed). 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_unrouted(s: *const VcadSolve) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.unrouted
}

/// Receipt: REAL minimum wall thickness of the resolved enclosure (mm), from
/// the kernel DFM thickness raycast — not arithmetic on literal box dims.
/// Returns f64::INFINITY when there's no enclosure solid to measure.
#[no_mangle]
pub extern "C" fn vcad_solve_min_wall(s: *const VcadSolve) -> f64 {
    if s.is_null() {
        return 0.0;
    }
    unsafe { &*s }.min_wall
}

/// Receipt: bracket DFM. 1 = Held (no manufacturability Error), 0 = Violated.
#[no_mangle]
pub extern "C" fn vcad_solve_bracket_ok(s: *const VcadSolve) -> u8 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.bracket_ok
}

/// Receipt: worst bracket finding — 0 clean / 1 Warning / 2 Error. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_bracket_severity(s: *const VcadSolve) -> u8 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.bracket_severity
}

/// Receipt: TOTAL manufacturing quote in integer cents (enclosure + board +
/// bracket, qty 1). 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_quote_cost_cents(s: *const VcadSolve) -> u64 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.quote_cost_cents
}

/// Receipt: enclosure CNC cost (cents) — REAL removed-volume model. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_quote_enclosure_cents(s: *const VcadSolve) -> u64 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.quote_enclosure_cents
}

/// Receipt: PCB fab cost (cents) — LABELED ESTIMATE. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_quote_board_cents(s: *const VcadSolve) -> u64 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.quote_board_cents
}

/// Receipt: sheet-metal bracket cost (cents) — REAL kernel model. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_quote_bracket_cents(s: *const VcadSolve) -> u64 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.quote_bracket_cents
}

/// Receipt: 1 iff the quote total includes a labeled estimate (the board) the
/// UI should tag "est."; 0 when every line is kernel-real. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_quote_has_estimate(s: *const VcadSolve) -> u8 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.quote_has_estimate
}

/// Receipt: estimated lead time, business days (HEURISTIC). 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_lead_days(s: *const VcadSolve) -> u32 {
    if s.is_null() {
        return 0;
    }
    unsafe { &*s }.lead_days
}

/// The honest Make-it gate: 1 iff EVERY gating domain holds — min-wall ≥ 6 mm,
/// 0 unrouted, bracket DFM Held. The quote never gates (sourcing never gates).
/// Make-it must be disabled whenever this is 0. 0 on null.
#[no_mangle]
pub extern "C" fn vcad_solve_all_held(s: *const VcadSolve) -> u8 {
    if s.is_null() {
        return 0;
    }
    let s = unsafe { &*s };
    u8::from(s.min_wall >= 6.0 && s.unrouted == 0 && s.bracket_ok == 1)
}

/// Free a solve result. No-op on null.
#[no_mangle]
pub extern "C" fn vcad_solve_free(s: *mut VcadSolve) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
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
    let miss = VcadHit {
        hit: 0,
        point: [0.0; 3],
        normal: [0.0; 3],
        t: 0.0,
    };
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
        assert!(
            view.vertices_len > 0,
            "cube should tessellate to >0 vertices"
        );
        assert_eq!(
            view.vertices_len, view.normals_len,
            "one normal per position"
        );
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
        // A cube, not a cylinder: the cap-rim blend on a primitive
        // cylinder is refused by the kernel (it diverges), which the old
        // silent fail-soft hid — this test used to assert geometry on an
        // *unfilleted* cylinder.
        let src = "[root [fillet 2.0 [cube 10 10 30]] \"brass\"]";
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
        assert!(
            scene.is_null(),
            "garbage loon should fail to a null scene, not panic"
        );
        assert!(vcad_scene_from_loon(ptr::null(), 0).is_null());
    }

    #[test]
    fn gripper_connector_x_drives_both_domains() {
        // The coupling: one parameter must move BOTH the enclosure cutout
        // (mechanical, node 3) and the board connector (electrical, node 8).
        let mut doc = build_gripper_slice1();
        doc.parameters
            .insert("connector_x".into(), Parameter::literal(25.0));
        vcad_eval::resolve_document(&mut doc).unwrap();
        let cutout_x = match &doc.nodes[&3].op {
            CsgOp::Translate { offset, .. } => offset.x,
            _ => panic!("node 3 should be a Translate"),
        };
        let conn_x = match &doc.nodes[&8].op {
            CsgOp::Translate { offset, .. } => offset.x,
            _ => panic!("node 8 should be a Translate"),
        };
        assert_eq!(cutout_x, 19.0, "cutout follows connector_x - 6");
        assert_eq!(conn_x, 20.0, "connector follows connector_x - 5");

        // Drag it and both re-solve together.
        doc.parameters
            .insert("connector_x".into(), Parameter::literal(60.0));
        vcad_eval::resolve_document(&mut doc).unwrap();
        let cutout_x2 = match &doc.nodes[&3].op {
            CsgOp::Translate { offset, .. } => offset.x,
            _ => panic!(),
        };
        assert_eq!(cutout_x2, 54.0, "one drag moved the mechanical domain too");
    }

    #[test]
    fn gripper_doc_set_param_evaluates_two_parts() {
        let doc = vcad_doc_gripper_slice1();
        assert!(!doc.is_null());
        let name = std::ffi::CString::new("connector_x").unwrap();
        let scene = vcad_doc_set_param(doc, name.as_ptr(), 30.0);
        assert!(!scene.is_null(), "gripper should evaluate");
        assert!(vcad_scene_part_count(scene) >= 2, "enclosure + board");
        let view = vcad_scene_part_mesh(scene, 0);
        assert!(view.vertices_len > 0, "enclosure has geometry");
        vcad_scene_free(scene);
        // a second drag re-solves cleanly
        let scene2 = vcad_doc_set_param(doc, name.as_ptr(), 64.0);
        assert!(!scene2.is_null());
        vcad_scene_free(scene2);
        vcad_doc_free(doc);
    }

    #[test]
    fn gripper_doc_null_inputs_are_safe() {
        assert!(vcad_doc_set_param(ptr::null_mut(), ptr::null(), 1.0).is_null());
        vcad_doc_free(ptr::null_mut());
        assert!(vcad_doc_load(ptr::null(), 0).is_null());
    }

    #[test]
    fn slice2_board_routes_both_nets() {
        // The electrical domain: SIG and GND must both route cleanly on the
        // hand-built board, or there is nothing honest to draw.
        let pcb = build_gripper_slice2_board(40.0);
        let r = route_all(&pcb, 0.25, &[]);
        assert!(
            r.unrouted_nets.is_empty(),
            "SIG+GND must both route, got {:?}",
            r.unrouted_nets
        );
        assert!(
            r.traces.len() >= 2,
            "expect at least one segment per net, got {}",
            r.traces.len()
        );
    }

    #[test]
    fn slice2_connector_x_moves_copper() {
        // The coupling: sliding the connector right must push the copper right.
        let a = vcad_route_traces(24.0, 0.25);
        let b = vcad_route_traces(64.0, 0.25);
        assert!(!a.is_null() && !b.is_null());
        let max_x = |r: *const VcadRouteResult| {
            let rr = unsafe { &*r };
            rr.traces
                .iter()
                .flat_map(|t| [t.start[0], t.end[0]])
                .fold(f64::MIN, f64::max)
        };
        assert!(
            max_x(b) > max_x(a),
            "copper should reach further +X when the connector slides right"
        );
        vcad_route_result_free(a);
        vcad_route_result_free(b);
    }

    #[test]
    fn slice2_route_null_safe() {
        assert_eq!(vcad_route_result_trace_count(ptr::null()), 0);
        let z = vcad_route_result_trace(ptr::null(), 0);
        assert_eq!(z.width, 0.0);
        assert_eq!(vcad_route_result_unrouted_count(ptr::null()), 0);
        vcad_route_result_free(ptr::null_mut());
    }

    #[test]
    fn gripper_bracket_refolds_with_connector_x() {
        // Tier 1: the sheet-metal bracket is a node in the gripper DAG, driven by
        // the SAME connector_x binding as the cubes. One evaluate_document folds
        // it. Assert the upstand grows with connector_x, and TIME the full
        // 3-domain solve so the per-frame-vs-settle call is measured, not guessed.
        use std::time::Instant;
        let mut doc = build_gripper_slice1();
        let bracket_bbox = |doc: &Document| -> ([f64; 3], [f64; 3]) {
            // interactive path: no O(n^2) clash detection
            let opts = EvalOptions {
                skip_clash_detection: true,
                clock: None,
                root_cache: None,
                mesh_segments: 0,
            };
            let scene = evaluate_document(doc, &opts).unwrap();
            // roots in order: enclosure (4), board (9), bracket (12).
            let solid = scene.parts[2]
                .solid
                .as_ref()
                .expect("bracket should fold to a solid");
            solid.bounding_box()
        };

        doc.parameters
            .insert("connector_x".into(), Parameter::literal(8.0));
        let lo = bracket_bbox(&doc); // warmup (first solve pays kernel init)
                                     // steady-state: time three successive solves
        for i in 0..3 {
            doc.parameters.insert(
                "connector_x".into(),
                Parameter::literal(20.0 + i as f64 * 15.0),
            );
            let t = Instant::now();
            let _ = bracket_bbox(&doc);
            eprintln!(
                "[gripper] steady solve #{i} = {:.1}ms",
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
        doc.parameters
            .insert("connector_x".into(), Parameter::literal(76.0));
        let hi = bracket_bbox(&doc);

        eprintln!(
            "[gripper] lo min={:?} max={:?}  hi min={:?} max={:?}",
            lo.0, lo.1, hi.0, hi.1
        );
        // the flange grows with connector_x — assert SOME axis extent increases.
        let span = |b: &([f64; 3], [f64; 3]), ax: usize| b.1[ax] - b.0[ax];
        let grew = (0..3).any(|ax| span(&hi, ax) > span(&lo, ax) + 4.0);
        assert!(
            grew,
            "taller flange must grow the bracket bbox: lo={lo:?}, hi={hi:?}"
        );
    }

    #[test]
    fn gripper_solve_bundles_all_domains() {
        // Tier 2: ONE call returns geometry + copper + receipt, all from the same
        // resolved connector_x.
        let doc = vcad_doc_gripper_slice1();
        let name = std::ffi::CString::new("connector_x").unwrap();
        let s = vcad_doc_solve(doc, name.as_ptr(), 40.0);
        assert!(!s.is_null());
        assert_eq!(
            vcad_solve_part_count(s),
            3,
            "enclosure + board + bracket meshes"
        );
        assert!(
            vcad_solve_part_mesh(s, 2).vertices_len > 0,
            "bracket has geometry"
        );
        assert!(vcad_solve_trace_count(s) >= 2, "both nets routed to copper");
        assert_eq!(vcad_solve_unrouted(s), 0, "fully routed");
        // Real min-wall is a positive geometric thickness of the resolved
        // enclosure (not the faked 74-cx). It must be finite and positive.
        let mw = vcad_solve_min_wall(s);
        assert!(
            mw.is_finite() && mw > 0.0,
            "centered min-wall finite & positive, got {mw}"
        );
        // Multi-domain quote: enclosure (real) + board (est) + bracket (real),
        // each a positive line item, summing to the total.
        let enc = vcad_solve_quote_enclosure_cents(s);
        let brd = vcad_solve_quote_board_cents(s);
        let brk = vcad_solve_quote_bracket_cents(s);
        assert!(enc > 0, "enclosure CNC cost present");
        assert!(brd > 0, "board fab estimate present");
        assert!(brk > 0, "bracket cost present");
        assert_eq!(
            vcad_solve_quote_cost_cents(s),
            enc + brd + brk,
            "total = sum of domains"
        );
        assert_eq!(
            vcad_solve_quote_has_estimate(s),
            1,
            "board line is a labeled estimate"
        );
        vcad_solve_free(s);
        // null-safety
        assert_eq!(vcad_solve_part_count(ptr::null()), 0);
        assert_eq!(vcad_solve_trace_count(ptr::null()), 0);
        vcad_solve_free(ptr::null_mut());
        vcad_doc_free(doc);
    }

    #[test]
    fn sheet_metal_fold_is_opt_in_web_contract_preserved() {
        // The load-bearing contract behind the CI fix: under the DEFAULT
        // (non-folding) `evaluate_document` — the path the WASM/web + MCP use — a
        // sheet-metal root must evaluate EMPTY, so the TS engine's
        // `positions.length === 0` fallback routes it to evaluateSheetMetalChain.
        // Only the native opt-in (`evaluate_document_with_sheet_metal`) folds.
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::SheetMetalBaseFlangeRect {
                    width: 40.0,
                    depth: 20.0,
                    thickness: 1.5,
                    material: "al-soft".into(),
                    shop_profile: None,
                    engravings: None,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "al-soft".into(),
            visible: None,
        });

        // Web/MCP path: empty (no solid) — preserves the fallback contract.
        let web = vcad_eval::evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert!(
            web.parts[0].solid.is_none(),
            "web contract: sheet metal must evaluate empty under the default evaluator"
        );
        // Native opt-in: the same root folds to a real solid.
        let native =
            vcad_eval::evaluate_document_with_sheet_metal(&doc, &EvalOptions::default()).unwrap();
        assert!(
            native.parts[0].solid.is_some(),
            "native opt-in: sheet metal folds to a solid"
        );
    }

    #[test]
    fn sheet_metal_polygon_base_flange_evaluates() {
        // B1: the polygon base-flange eval arm (un-stubbed this pass) must fold to
        // a real solid, and the IR material must thread through (B2) — not the old
        // hardcoded "al-soft".
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::SheetMetalBaseFlangePolygon {
                    outline: vec![
                        Vec2::new(0.0, 0.0),
                        Vec2::new(40.0, 0.0),
                        Vec2::new(40.0, 20.0),
                        Vec2::new(0.0, 20.0),
                    ],
                    holes: vec![],
                    thickness: 1.5,
                    material: "steel".into(),
                    shop_profile: None,
                    engravings: None,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "steel".into(),
            visible: None,
        });
        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(
            scene.parts[0].solid.is_some(),
            "polygon base flange should fold to a solid, not blank out"
        );
    }

    #[test]
    fn receipt_gate_flips_on_min_wall() {
        // The honesty rule: the Make-it gate dies when any gating check Violates.
        // The min-wall is now REAL geometry, so the proof is behavioral — the
        // measured wall must SHRINK as the connector slides toward the side wall,
        // and the gate must shut once it drops below the 6 mm floor. No magic
        // constant keyed to the box's literal extents.
        let doc = vcad_doc_gripper_slice1();
        assert!(!doc.is_null());
        let name = std::ffi::CString::new("connector_x").unwrap();

        // Centered: every gating domain holds, real multi-domain quote present.
        let s = vcad_doc_solve(doc, name.as_ptr(), 40.0);
        assert_eq!(vcad_solve_bracket_ok(s), 1, "bracket foldable at cx=40");
        assert!(
            vcad_solve_quote_cost_cents(s) > 0,
            "real multi-domain quote (cents > 0)"
        );
        assert!(
            vcad_solve_lead_days(s) >= 7,
            "lead-time = max over domains (PCB floor 7)"
        );
        assert_eq!(vcad_solve_all_held(s), 1, "gate open when centered");
        let wall_centered = vcad_solve_min_wall(s);
        vcad_solve_free(s);

        // Push the connector toward the +X wall: the real measured wall must
        // shrink, and at the wall the gate is KILLED.
        let s2 = vcad_doc_solve(doc, name.as_ptr(), 78.0);
        let wall_at_edge = vcad_solve_min_wall(s2);
        assert!(
            wall_at_edge < wall_centered,
            "real min-wall must shrink as connector nears the wall: {wall_at_edge} !< {wall_centered}"
        );
        assert!(
            wall_at_edge < 6.0,
            "min-wall violated at the wall, got {wall_at_edge}"
        );
        assert_eq!(
            vcad_solve_all_held(s2),
            0,
            "gate KILLED when min-wall violated"
        );
        vcad_solve_free(s2);

        assert_eq!(vcad_solve_all_held(ptr::null()), 0);
        vcad_doc_free(doc);
    }

    #[test]
    fn cheap_min_wall_matches_full_solve_and_tracks_param() {
        // The per-frame `vcad_doc_min_wall` (cheap: cutout + housing only) must
        // agree with the full solve's min-wall AND move with connector_x — so the
        // live Receipt row and the settled one read the same geometry.
        let doc = vcad_doc_gripper_slice1();
        assert!(!doc.is_null());
        let name = std::ffi::CString::new("connector_x").unwrap();

        for cx in [12.0_f64, 40.0, 68.0] {
            // Drive the param through the cheap path (writes it to the doc), then
            // measure both ways.
            let scene = vcad_doc_set_param_cheap(doc, name.as_ptr(), cx);
            assert!(!scene.is_null());
            vcad_scene_free(scene);
            let cheap = vcad_doc_min_wall(doc);
            let s = vcad_doc_solve(doc, name.as_ptr(), cx);
            let full = vcad_solve_min_wall(s);
            vcad_solve_free(s);
            assert!(
                (cheap - full).abs() < 1e-6,
                "cx={cx}: cheap min-wall {cheap} must equal full-solve {full}"
            );
        }

        // Tracks the parameter: moving the connector toward +X shrinks the wall.
        vcad_scene_free(vcad_doc_set_param_cheap(doc, name.as_ptr(), 20.0));
        let near_left = vcad_doc_min_wall(doc);
        vcad_scene_free(vcad_doc_set_param_cheap(doc, name.as_ptr(), 68.0));
        let near_right = vcad_doc_min_wall(doc);
        // cx=20 leaves a fat +X wall; cx=68 nearly breaches it.
        assert!(
            near_right < near_left,
            "min-wall must shrink toward the +X wall: {near_right} !< {near_left}"
        );

        assert!(vcad_doc_min_wall(ptr::null()).is_infinite());
        vcad_doc_free(doc);
    }

    #[test]
    fn gripper_pcb_binding_moves_copper() {
        // Tier 2b: the PCB lives in doc.pcb and the "pcb." binding moves the
        // connector footprint declaratively — so copper shifts with connector_x
        // through resolve_document, no procedural board rebuild.
        let doc = vcad_doc_gripper_slice1();
        let name = std::ffi::CString::new("connector_x").unwrap();
        let max_trace_x = |cx: f64| -> f64 {
            let s = vcad_doc_solve(doc, name.as_ptr(), cx);
            let mut mx = f64::MIN;
            for i in 0..vcad_solve_trace_count(s) {
                let t = vcad_solve_trace(s, i);
                mx = mx.max(t.start[0]).max(t.end[0]);
            }
            vcad_solve_free(s);
            mx
        };
        let lo = max_trace_x(20.0);
        let hi = max_trace_x(70.0);
        assert!(
            hi > lo,
            "the pcb. binding must shift copper right with connector_x: lo={lo}, hi={hi}"
        );
        vcad_doc_free(doc);
    }

    #[test]
    fn gripper_cheap_path_drops_the_bracket() {
        // The per-frame path skips the expensive sheet-metal root: full solve
        // yields 3 parts (enclosure, board, bracket); the cheap solve yields 2.
        let doc = vcad_doc_gripper_slice1();
        assert!(!doc.is_null());
        let name = std::ffi::CString::new("connector_x").unwrap();
        let full = vcad_doc_set_param(doc, name.as_ptr(), 40.0);
        let cheap = vcad_doc_set_param_cheap(doc, name.as_ptr(), 40.0);
        assert_eq!(
            vcad_scene_part_count(full),
            3,
            "full solve: enclosure + board + bracket"
        );
        assert_eq!(
            vcad_scene_part_count(cheap),
            2,
            "cheap solve drops the sheet-metal root"
        );
        vcad_scene_free(full);
        vcad_scene_free(cheap);
        vcad_doc_free(doc);
    }

    #[test]
    fn raycast_hits_a_cube_face() {
        let cube = vcad_solid_cube(30.0, 30.0, 30.0);
        let origin = [15.0_f64, 15.0, 100.0];
        let dir = [0.0_f64, 0.0, -1.0];
        let hit = vcad_solid_raycast(cube, origin.as_ptr(), dir.as_ptr());
        assert_eq!(hit.hit, 1, "a ray down +Z should hit the top face");
        assert!(
            (hit.point[2] - 30.0).abs() < 1e-6,
            "hit z should be 30, got {}",
            hit.point[2]
        );
        assert!(hit.normal[2] > 0.9, "top-face normal should point +Z");
        vcad_solid_free(cube);
    }

    /// The native inspector's document-parameter scrub writes
    /// `parameters.<name>.value` (untagged number) into the doc JSON and
    /// re-evaluates through `vcad_scene_from_json`; bindings fan the value out
    /// to node fields. Lock that wire shape down end-to-end.
    #[test]
    fn scene_from_json_resolves_parameters_and_bindings() {
        fn doc_json(width: f64) -> String {
            format!(
                r#"{{
                    "version": "0.1",
                    "materials": {{}},
                    "part_materials": {{}},
                    "nodes": {{
                        "1": {{ "id": 1, "op": {{ "type": "Cube", "size": {{ "x": 1.0, "y": 10.0, "z": 10.0 }} }} }}
                    }},
                    "roots": [ {{ "root": 1, "material": "default" }} ],
                    "parameters": {{
                        "width": {{ "value": {width}, "min": 5.0, "max": 80.0, "unit": "mm" }}
                    }},
                    "bindings": {{ "1:size.x": "width" }}
                }}"#
            )
        }
        fn max_x(json: &str) -> f32 {
            let scene = vcad_scene_from_json(json.as_ptr(), json.len());
            assert!(!scene.is_null(), "parametric doc should evaluate");
            let view = vcad_scene_part_mesh(scene, 0);
            let verts = unsafe { std::slice::from_raw_parts(view.vertices, view.vertices_len) };
            let mx = verts.chunks(3).map(|v| v[0]).fold(f32::MIN, f32::max);
            vcad_scene_free(scene);
            mx
        }
        assert!((max_x(&doc_json(20.0)) - 20.0).abs() < 1e-4);
        assert!((max_x(&doc_json(50.0)) - 50.0).abs() < 1e-4);
    }

    /// Kinematic playback surface: an assembly document (partDefs, instances,
    /// and a revolute joint) exposes instances through the FFI, and
    /// `vcad_scene_solve_fk` re-places the child when the joint value changes.
    #[test]
    fn scene_instances_and_fk_playback() {
        let json = r#"{
            "version": "0.1",
            "materials": {},
            "part_materials": {},
            "nodes": {
                "1": { "id": 1, "op": { "type": "Cube", "size": { "x": 10.0, "y": 10.0, "z": 10.0 } } },
                "2": { "id": 2, "op": { "type": "Cube", "size": { "x": 4.0, "y": 4.0, "z": 30.0 } } }
            },
            "roots": [],
            "partDefs": {
                "base": { "id": "base", "name": "Base", "root": 1 },
                "arm":  { "id": "arm",  "name": "Arm",  "root": 2 }
            },
            "instances": [
                { "id": "base_inst", "partDefId": "base" },
                { "id": "arm_inst",  "partDefId": "arm" }
            ],
            "joints": [
                {
                    "id": "hinge",
                    "name": "Hinge",
                    "parentInstanceId": "base_inst",
                    "childInstanceId": "arm_inst",
                    "parentAnchor": { "x": 5.0, "y": 5.0, "z": 10.0 },
                    "childAnchor": { "x": 2.0, "y": 2.0, "z": 0.0 },
                    "kind": { "type": "Revolute", "axis": { "x": 0.0, "y": 1.0, "z": 0.0 } },
                    "state": 0.0
                }
            ],
            "groundInstanceId": "base_inst"
        }"#;
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null(), "assembly doc should evaluate");
        assert_eq!(vcad_scene_instance_count(scene), 2);
        let view = vcad_scene_instance_mesh(scene, 1);
        assert!(view.vertices_len > 0, "arm instance carries a mesh");
        let mut len = 0usize;
        let idp = vcad_scene_instance_id(scene, 1, &mut len);
        let id = unsafe { std::slice::from_raw_parts(idp, len) };
        assert_eq!(id, b"arm_inst");

        // Authored state (0°): the joint places the arm at parent−child anchor.
        let mut t0 = [0.0f64; 16];
        assert_eq!(vcad_scene_instance_transform(scene, 1, t0.as_mut_ptr()), 1);
        assert!((t0[12] - 3.0).abs() < 1e-9 && (t0[14] - 10.0).abs() < 1e-9);

        // Drive the hinge to 90° about +Y: rotation column 0 becomes ~(0,0,-1).
        let pose = r#"{"hinge": 90.0}"#;
        let mut out = [0.0f64; 32];
        let n = vcad_scene_solve_fk(scene, pose.as_ptr(), pose.len(), out.as_mut_ptr(), 32);
        assert_eq!(n, 2);
        let base = &out[0..16];
        assert!((base[12]).abs() < 1e-9, "grounded base stays put");
        let arm = &out[16..32];
        assert!(
            (arm[0]).abs() < 1e-9 && (arm[2] + 1.0).abs() < 1e-9,
            "arm rotated 90° about +Y (col0 = {:?})",
            &arm[0..3]
        );
        // Joint-name fallback drives the same joint.
        let by_name = r#"{"Hinge": 90.0}"#;
        let mut out2 = [0.0f64; 32];
        assert_eq!(
            vcad_scene_solve_fk(
                scene,
                by_name.as_ptr(),
                by_name.len(),
                out2.as_mut_ptr(),
                32
            ),
            2
        );
        assert!((out2[16] - arm[0]).abs() < 1e-12);
        vcad_scene_free(scene);
    }

    /// Guard the shipped parametric example: it must keep evaluating (the
    /// native app's demo doc for the document-parameter scrub), including its
    /// derived formula parameter and cross-parameter bindings.
    #[test]
    fn parametric_plate_example_evaluates() {
        let json = include_str!("../../../examples/parametric-plate.vcad");
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null(), "example should parse and evaluate");
        assert_eq!(vcad_scene_part_count(scene), 1);
        let view = vcad_scene_part_mesh(scene, 0);
        assert!(view.vertices_len > 0 && view.indices_len > 0);
        vcad_scene_free(scene);
    }

    #[test]
    fn null_inputs_are_safe() {
        assert!(vcad_solid_to_mesh(ptr::null(), 8).is_null());
        let view = vcad_mesh_view(ptr::null());
        assert_eq!(view.vertices_len, 0);
        vcad_mesh_free(ptr::null_mut());
        vcad_solid_free(ptr::null_mut());
        assert!(vcad_scene_part_edges(ptr::null(), 0, 25.0).is_null());
        assert_eq!(vcad_edges_view(ptr::null()).floats_len, 0);
        vcad_edges_free(ptr::null_mut());
    }

    #[test]
    fn scene_part_edges_returns_segments_for_the_example() {
        let json = include_str!("../../../examples/parametric-plate.vcad");
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null());
        let edges = vcad_scene_part_edges(scene, 0, 25.0);
        assert!(!edges.is_null());
        let view = vcad_edges_view(edges);
        assert!(
            view.floats_len > 0 && view.floats_len % 6 == 0,
            "edges must be 6 floats per segment, got {}",
            view.floats_len
        );
        // Out of range index → null, not a crash.
        assert!(vcad_scene_part_edges(scene, 99, 25.0).is_null());
        vcad_edges_free(edges);
        vcad_scene_free(scene);
    }

    /// Needs a real GPU adapter — run manually / on GPU hosts:
    /// `cargo test -p vcad-ffi gpu_raytrace -- --ignored`
    #[test]
    #[ignore = "requires a GPU adapter (Metal/Vulkan)"]
    fn gpu_raytrace_renders_the_example_plate() {
        let json = include_str!("../../../examples/parametric-plate.vcad");
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null());
        let cam = [40.0, -80.0, 90.0];
        let target = [40.0, 25.0, 3.0];
        let colors = [0.7f32, 0.7, 0.75];
        let img = vcad_scene_raytrace_gpu(
            scene,
            cam.as_ptr(),
            target.as_ptr(),
            35.0,
            320,
            240,
            colors.as_ptr(),
            colors.len(),
        );
        assert!(!img.is_null(), "GPU raytrace should produce a frame");
        let view = vcad_image_view(img);
        assert_eq!(view.pixels_len, 320 * 240 * 4);
        // The frame must not be uniform (sky + plate + ground all present).
        let px = unsafe { std::slice::from_raw_parts(view.pixels, view.pixels_len) };
        let first = &px[0..3];
        assert!(
            px.chunks(4).any(|p| &p[0..3] != first),
            "GPU frame is uniform — pipeline produced nothing"
        );
        vcad_image_free(img);
        vcad_scene_free(scene);
    }

    #[test]
    fn scene_raytrace_renders_the_example_plate() {
        let json = include_str!("../../../examples/parametric-plate.vcad");
        let scene = vcad_scene_from_json(json.as_ptr(), json.len());
        assert!(!scene.is_null());
        // Camera above and in front of the 80×50×6 plate, looking at its center.
        let cam = [40.0, -80.0, 90.0];
        let target = [40.0, 25.0, 3.0];
        let colors = [0.7f32, 0.7, 0.75];
        let img = vcad_scene_raytrace(
            scene,
            cam.as_ptr(),
            target.as_ptr(),
            35.0,
            160,
            120,
            colors.as_ptr(),
            colors.len(),
        );
        assert!(!img.is_null(), "raytrace should produce a frame");
        let view = vcad_image_view(img);
        assert_eq!(view.pixels_len, 160 * 120 * 4);
        // The plate must actually appear (alpha 255), over a transparent
        // background (alpha 0) so the app can composite the still.
        let px = unsafe { std::slice::from_raw_parts(view.pixels, view.pixels_len) };
        let lit = px.chunks(4).filter(|p| p[3] == 255).count();
        let clear = px.chunks(4).filter(|p| p[3] == 0).count();
        assert!(lit > 500, "expected the plate to cover pixels, lit={lit}");
        assert!(
            clear > 500,
            "expected transparent background, clear={clear}"
        );
        vcad_image_free(img);
        // Null / degenerate inputs fail closed.
        assert!(vcad_scene_raytrace(
            scene,
            ptr::null(),
            target.as_ptr(),
            35.0,
            8,
            8,
            ptr::null(),
            0
        )
        .is_null());
        vcad_scene_free(scene);
    }
}
