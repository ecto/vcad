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
use vcad_eval::{evaluate_document, EvalOptions, EvaluatedScene};
use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, Net, NetClassRules, Pad, PadShape, PadType,
    Pcb, PcbLayer, StackupLayer,
};
use vcad_ir::{
    BindingKey, CsgOp, Document, Expr, MaterialDef, Node, Parameter, SceneEntry, Vec2,
    Vec3 as IrVec3,
};
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
    4
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
    eval_mesh_view(&part.mesh)
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
    catch_unwind(|| Box::into_raw(Box::new(VcadDoc { inner: build_gripper_slice1() })))
        .unwrap_or(ptr::null_mut())
}

/// Interactive eval options: skip the O(n^2) clash pass the native app doesn't render.
fn interactive_opts() -> EvalOptions {
    EvalOptions { skip_clash_detection: true, ..Default::default() }
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
        d.inner.parameters.insert(name.to_string(), Parameter::literal(value));
        match evaluate_document(&d.inner, &interactive_opts()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene { inner: scene })),
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
        d.inner.parameters.insert(name.to_string(), Parameter::literal(value));
        // Evaluate a clone whose expensive (sheet-metal) roots are dropped.
        let mut fast = d.inner.clone();
        let nodes = fast.nodes.clone();
        fast.roots.retain(|e| !subtree_has_sheet_metal(&nodes, e.root));
        match evaluate_document(&fast, &interactive_opts()) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene { inner: scene })),
            Err(_) => ptr::null_mut(),
        }
    }))
    .unwrap_or(ptr::null_mut())
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
    doc.nodes.insert(1, Node { id: 1, name: Some("enclosure".into()),
        op: CsgOp::Cube { size: v(80.0, 50.0, 30.0) } });
    doc.nodes.insert(2, Node { id: 2, name: None,
        op: CsgOp::Cube { size: v(12.0, 8.0, 12.0) } });
    // offset.x is bound below; y/z fixed (straddles the front wall at y≈0).
    doc.nodes.insert(3, Node { id: 3, name: None,
        op: CsgOp::Translate { child: 2, offset: v(0.0, -2.0, 9.0) } });
    doc.nodes.insert(4, Node { id: 4, name: Some("housing".into()),
        op: CsgOp::Difference { left: 1, right: 3 } });

    // --- electrical: board = plate + connector body ---
    doc.nodes.insert(5, Node { id: 5, name: None,
        op: CsgOp::Cube { size: v(70.0, 40.0, 2.0) } });
    doc.nodes.insert(6, Node { id: 6, name: None,
        op: CsgOp::Translate { child: 5, offset: v(5.0, 5.0, 5.0) } });
    doc.nodes.insert(7, Node { id: 7, name: None,
        op: CsgOp::Cube { size: v(10.0, 14.0, 6.0) } });
    doc.nodes.insert(8, Node { id: 8, name: None,
        op: CsgOp::Translate { child: 7, offset: v(0.0, 2.0, 6.0) } });
    doc.nodes.insert(9, Node { id: 9, name: Some("board".into()),
        op: CsgOp::Union { left: 6, right: 8 } });

    // --- sheet metal: an L-bracket whose upstand height tracks connector_x ---
    // Foundation-tier sheet metal is now a first-class evaluator (vcad-eval), so
    // the bracket is just more nodes in this DAG — driven by the SAME binding
    // mechanism as the cubes, re-solved by the same evaluate_document. No
    // separate fold FFI: the cross-domain coupling rides one resolve.
    doc.nodes.insert(10, Node { id: 10, name: Some("bracket-base".into()),
        op: CsgOp::SheetMetalBaseFlangeRect {
            width: 70.0, depth: 18.0, thickness: 1.0,
            material: "al-soft".into(), shop_profile: None } });
    doc.nodes.insert(11, Node { id: 11, name: Some("bracket".into()),
        op: CsgOp::SheetMetalEdgeFlange {
            parent: 10, panel_id: 0, edge_index: 0,
            length: 12.0, angle: std::f64::consts::FRAC_PI_2,
            radius: Some(1.0), direction: vcad_ir::SheetMetalDirection::Down,
            manual_k: Some(0.44) } });
    doc.nodes.insert(12, Node { id: 12, name: None,
        op: CsgOp::Translate { child: 11, offset: v(5.0, 1.0, 3.5) } });

    // --- the coupling: ONE parameter drives THREE domains ---
    doc.parameters.insert("connector_x".into(), Parameter::literal(40.0));
    doc.bindings.bind(BindingKey::new(3, "offset.x"), Expr::formula("connector_x - 6"));
    doc.bindings.bind(BindingKey::new(8, "offset.x"), Expr::formula("connector_x - 5"));
    // the bracket upstand grows toward the connector (length stays > 0 across the
    // 4..76 drag range: 5..23 mm).
    doc.bindings.bind(BindingKey::new(11, "length"), Expr::formula("connector_x * 0.25 + 4"));

    // --- the PCB lives IN the document: a "pcb." binding moves the connector
    // footprint declaratively, so the board's copper re-routes from the SAME
    // resolve_document as the geometry. (node id 0 is a placeholder — the "pcb."
    // prefix routes the binding to the PCB, not a node.) The slope keeps J1 on
    // the 70 mm board (board-local ~10..61) across the 4..76 drag range.
    doc.pcb = Some(build_gripper_slice2_board(40.0));
    doc.bindings.bind(BindingKey::new(0, "pcb.J1.position.x"), Expr::formula("connector_x * 0.7 + 8"));

    doc.roots.push(SceneEntry { root: 4, material: "aluminum".into(), visible: None });
    doc.roots.push(SceneEntry { root: 9, material: "board".into(), visible: None });
    doc.roots.push(SceneEntry { root: 12, material: "bracket".into(), visible: None });
    doc.materials.insert("aluminum".into(), MaterialDef {
        name: "aluminum".into(), color: [0.72, 0.74, 0.78],
        metallic: 0.6, roughness: 0.34, density: None, friction: None, ..Default::default() });
    doc.materials.insert("board".into(), MaterialDef {
        name: "board".into(), color: [0.12, 0.42, 0.18],
        metallic: 0.0, roughness: 0.6, density: None, friction: None, ..Default::default() });
    doc.materials.insert("bracket".into(), MaterialDef {
        name: "bracket".into(), color: [0.66, 0.68, 0.72],
        metallic: 0.85, roughness: 0.3, density: None, friction: None, ..Default::default() });
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
        Net { id: "SIG".into(), name: "SIG".into() },
        Net { id: "GND".into(), name: "GND".into() },
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
        shape: PadShape::Rect { width: 1.0, height: 1.2 },
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
        Box::into_raw(Box::new(VcadRouteResult { traces, unrouted: r.unrouted_nets.len() }))
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
    let zero = VcadTraceLine { start: [0.0; 2], end: [0.0; 2], width: 0.0, layer: 0, net_id: 0 };
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

/// Owned result of a full cross-domain solve: meshes + copper + receipt.
pub struct VcadSolve {
    scene: EvaluatedScene,
    traces: Vec<VcadTraceLine>,
    unrouted: usize,
    min_wall: f64,
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
        d.inner.parameters.insert(name.to_string(), Parameter::literal(value));
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
        // Receipt: connector cutout clearance to the nearest enclosure wall (mm).
        let min_wall = (cx - 6.0).min(74.0 - cx);
        Box::into_raw(Box::new(VcadSolve {
            scene,
            traces,
            unrouted,
            min_wall,
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
    let zero = VcadTraceLine { start: [0.0; 2], end: [0.0; 2], width: 0.0, layer: 0, net_id: 0 };
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

/// Receipt: connector-to-wall clearance (mm; the slice-1 min-wall verdict).
#[no_mangle]
pub extern "C" fn vcad_solve_min_wall(s: *const VcadSolve) -> f64 {
    if s.is_null() {
        return 0.0;
    }
    unsafe { &*s }.min_wall
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
    fn gripper_connector_x_drives_both_domains() {
        // The coupling: one parameter must move BOTH the enclosure cutout
        // (mechanical, node 3) and the board connector (electrical, node 8).
        let mut doc = build_gripper_slice1();
        doc.parameters.insert("connector_x".into(), Parameter::literal(25.0));
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
        doc.parameters.insert("connector_x".into(), Parameter::literal(60.0));
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
        assert!(r.unrouted_nets.is_empty(), "SIG+GND must both route, got {:?}", r.unrouted_nets);
        assert!(r.traces.len() >= 2, "expect at least one segment per net, got {}", r.traces.len());
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
        assert!(max_x(b) > max_x(a), "copper should reach further +X when the connector slides right");
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
            let opts = EvalOptions { skip_clash_detection: true, clock: None };
            let scene = evaluate_document(doc, &opts).unwrap();
            // roots in order: enclosure (4), board (9), bracket (12).
            let solid = scene.parts[2].solid.as_ref().expect("bracket should fold to a solid");
            solid.bounding_box()
        };

        doc.parameters.insert("connector_x".into(), Parameter::literal(8.0));
        let lo = bracket_bbox(&doc); // warmup (first solve pays kernel init)
        // steady-state: time three successive solves
        for i in 0..3 {
            doc.parameters.insert("connector_x".into(), Parameter::literal(20.0 + i as f64 * 15.0));
            let t = Instant::now();
            let _ = bracket_bbox(&doc);
            eprintln!("[gripper] steady solve #{i} = {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
        }
        doc.parameters.insert("connector_x".into(), Parameter::literal(76.0));
        let hi = bracket_bbox(&doc);

        eprintln!("[gripper] lo min={:?} max={:?}  hi min={:?} max={:?}",
            lo.0, lo.1, hi.0, hi.1);
        // the flange grows with connector_x — assert SOME axis extent increases.
        let span = |b: &([f64; 3], [f64; 3]), ax: usize| b.1[ax] - b.0[ax];
        let grew = (0..3).any(|ax| span(&hi, ax) > span(&lo, ax) + 4.0);
        assert!(grew, "taller flange must grow the bracket bbox: lo={lo:?}, hi={hi:?}");
    }

    #[test]
    fn gripper_solve_bundles_all_domains() {
        // Tier 2: ONE call returns geometry + copper + receipt, all from the same
        // resolved connector_x.
        let doc = vcad_doc_gripper_slice1();
        let name = std::ffi::CString::new("connector_x").unwrap();
        let s = vcad_doc_solve(doc, name.as_ptr(), 40.0);
        assert!(!s.is_null());
        assert_eq!(vcad_solve_part_count(s), 3, "enclosure + board + bracket meshes");
        assert!(vcad_solve_part_mesh(s, 2).vertices_len > 0, "bracket has geometry");
        assert!(vcad_solve_trace_count(s) >= 2, "both nets routed to copper");
        assert_eq!(vcad_solve_unrouted(s), 0, "fully routed");
        assert!((vcad_solve_min_wall(s) - 34.0).abs() < 1e-6, "min-wall at cx=40 is 34mm");
        vcad_solve_free(s);
        // null-safety
        assert_eq!(vcad_solve_part_count(ptr::null()), 0);
        assert_eq!(vcad_solve_trace_count(ptr::null()), 0);
        vcad_solve_free(ptr::null_mut());
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
        assert!(hi > lo, "the pcb. binding must shift copper right with connector_x: lo={lo}, hi={hi}");
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
        assert_eq!(vcad_scene_part_count(full), 3, "full solve: enclosure + board + bracket");
        assert_eq!(vcad_scene_part_count(cheap), 2, "cheap solve drops the sheet-metal root");
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
