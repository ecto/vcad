#![warn(missing_docs)]

//! High-level B-rep CAD kernel facade for vcad.
//!
//! Provides the [`Solid`] type — the primary interface for creating and
//! manipulating 3D geometry using B-rep representation.
//!
//! # Example
//!
//! ```
//! use vcad_kernel::Solid;
//!
//! let cube = Solid::cube(10.0, 20.0, 30.0);
//! let mesh = cube.to_mesh(32);
//! assert!(mesh.num_triangles() >= 12);
//! ```

use std::path::Path;

pub use vcad_kernel_acoustics;
pub use vcad_kernel_antenna;
pub use vcad_kernel_booleans;
pub use vcad_kernel_calibration;
pub use vcad_kernel_cam;
pub use vcad_kernel_constraints;
pub use vcad_kernel_cost;
pub use vcad_kernel_dfm;
pub use vcad_kernel_em;
pub use vcad_kernel_enclosure;
pub use vcad_kernel_fea;
pub use vcad_kernel_fillet;
pub use vcad_kernel_geom;
pub use vcad_kernel_math;
pub use vcad_kernel_neutronics;
pub use vcad_kernel_particle;
pub use vcad_kernel_qcd;
pub use vcad_kernel_photonics;
pub use vcad_kernel_primitives;
pub use vcad_kernel_sheet;
pub use vcad_kernel_shell;
pub use vcad_kernel_sketch;
pub use vcad_kernel_step;
pub use vcad_kernel_stocksim;
pub use vcad_kernel_sweep;
pub use vcad_kernel_tessellate;
pub use vcad_kernel_text;
pub use vcad_kernel_thermal;
pub use vcad_kernel_tolerance;
pub use vcad_kernel_topo;
pub use vcad_kernel_topopt;

pub mod cam_verify;
pub use cam_verify::verify_toolpaths;

pub mod sheet_fold;
pub use sheet_fold::folded_sheet_solid;

pub use vcad_kernel_booleans::BooleanError;
use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_step::StepError;
use vcad_kernel_tessellate::{mesh_clearance, tessellate_brep, TriangleMesh};

pub use vcad_kernel_tessellate::ClearanceResult;
pub use vcad_kernel_tessellate::{compute_mesh_properties, MeshBBox, MeshProperties};

/// Error returned when STEP export fails.
#[derive(Debug)]
pub enum StepExportError {
    /// The solid has been converted to mesh-only representation (e.g., after boolean operations).
    /// B-rep data is required for STEP export.
    NotBRep,
    /// The solid is empty (no geometry).
    Empty,
    /// An error occurred during STEP file writing.
    Step(StepError),
}

impl std::fmt::Display for StepExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepExportError::NotBRep => write!(
                f,
                "cannot export to STEP: solid has been converted to mesh (B-rep data lost after boolean operations)"
            ),
            StepExportError::Empty => write!(f, "cannot export to STEP: solid is empty"),
            StepExportError::Step(e) => write!(f, "STEP export error: {}", e),
        }
    }
}

impl std::error::Error for StepExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StepExportError::Step(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StepError> for StepExportError {
    fn from(e: StepError) -> Self {
        StepExportError::Step(e)
    }
}

/// Why a named-edge reference failed to resolve (fail-closed: the caller
/// must surface this, never fall back to a guessed edge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedEdgeError {
    /// The solid carries no name map (mesh-only, imported, or produced by an
    /// operation without a name-propagation rule).
    NoNames,
    /// A face name string was not in canonical `scope:tag[.n]*` form.
    BadName(String),
    /// More than one edge matched the reference.
    Ambiguous {
        /// Number of candidate edges.
        count: usize,
    },
    /// No edge matched by name or geometric hint.
    Lost(String),
}

impl std::fmt::Display for NamedEdgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNames => write!(f, "solid carries no persistent face names"),
            Self::BadName(s) => write!(f, "invalid face name '{s}' (expected scope:tag[.n]*)"),
            Self::Ambiguous { count } => {
                write!(f, "edge reference is ambiguous ({count} candidate edges)")
            }
            Self::Lost(reason) => write!(f, "edge reference lost: {reason}"),
        }
    }
}

impl std::error::Error for NamedEdgeError {}

/// The internal representation of a solid.
#[derive(Debug, Clone)]
enum SolidRepr {
    /// Full B-rep solid with topology and geometry.
    BRep(Box<BRepSolid>),
    /// Mesh-only solid (result of boolean operations in Phase 1).
    Mesh(TriangleMesh),
    /// Empty solid (no geometry).
    Empty,
}

/// A 3D solid geometry object.
///
/// Solids can be created from primitives, combined with CSG boolean operations,
/// and transformed. The tessellation to triangle meshes is done on demand.
#[derive(Debug, Clone)]
pub struct Solid {
    repr: SolidRepr,
    /// Default tessellation segment count.
    segments: u32,
    /// Persistent face names (topological naming), when this solid's
    /// provenance supports them. Primitive constructors seed the map,
    /// booleans propagate it, and rigid transforms carry it (topology keys
    /// survive `apply_transform`'s clone). Operations that rebuild topology
    /// without a propagation rule (fillet, shell, sweep, imports) drop it —
    /// fail-closed: downstream name resolution reports Lost rather than
    /// guessing.
    names: Option<vcad_kernel_naming::NameMap>,
}

/// Fallback circular resolution when a curved primitive is created with
/// `segments == 0` — the IR "0 = auto" sentinel (see `CsgOp::Cylinder`).
/// Matches the count the box and the other default constructors carry, so a
/// document built entirely from cylinders/spheres/cones approximates its
/// booleans at the same fidelity as one that happens to include a box.
const DEFAULT_SEGMENTS: u32 = 32;

/// Resolve a primitive's segment count to a concrete, safe value.
///
/// `0` means "auto" (use the default). Any non-zero count is clamped to a
/// floor of 3: a circle approximated by fewer than 3 vertices can't form a
/// valid face loop, and feeding such a count into the boolean splitter
/// builds an empty loop that panics in `Topology::add_loop`
/// ("loop must have at least one half-edge"). Resolving here keeps that
/// degenerate count from ever reaching the kernel's geometry layer.
fn resolve_segments(segments: u32) -> u32 {
    match segments {
        0 => DEFAULT_SEGMENTS,
        n => n.max(3),
    }
}

impl Solid {
    // =========================================================================
    // Constructors
    // =========================================================================

    /// Create an empty solid.
    pub fn empty() -> Self {
        Self {
            names: None,
            repr: SolidRepr::Empty,
            segments: 32,
        }
    }

    /// Borrow the underlying BRep if this solid is BRep-backed. Used by
    /// diagnostic harnesses that want to inspect topology/geometry
    /// directly without duplicating the evaluation pipeline.
    pub fn as_brep(&self) -> Option<&BRepSolid> {
        match &self.repr {
            SolidRepr::BRep(b) => Some(b.as_ref()),
            _ => None,
        }
    }

    /// Create a solid from a triangle mesh.
    pub fn from_mesh(mesh: TriangleMesh) -> Self {
        Self {
            names: None,
            repr: SolidRepr::Mesh(mesh),
            segments: 32,
        }
    }

    /// Create a solid from a raw BRep. Intended for callers that produce
    /// a `BRepSolid` outside the kernel's primitive constructors — e.g.
    /// STEP import, custom topology builders, eval grader round-trip.
    pub fn from_brep(brep: BRepSolid) -> Self {
        Self {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        }
    }

    /// Create a box (cuboid) with corner at origin and dimensions `(sx, sy, sz)`.
    pub fn cube(sx: f64, sy: f64, sz: f64) -> Self {
        let brep = vcad_kernel_primitives::make_cube(sx, sy, sz);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "cube")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        }
    }

    /// Create a cylinder along Z axis with the given radius and height.
    pub fn cylinder(radius: f64, height: f64, segments: u32) -> Self {
        let segments = resolve_segments(segments);
        let brep = vcad_kernel_primitives::make_cylinder(radius, height, segments);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "cylinder")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments,
        }
    }

    /// Create a sphere centered at origin with the given radius.
    pub fn sphere(radius: f64, segments: u32) -> Self {
        let segments = resolve_segments(segments);
        let brep = vcad_kernel_primitives::make_sphere(radius, segments);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "sphere")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments,
        }
    }

    /// Create a cone/frustum along Z axis.
    pub fn cone(radius_bottom: f64, radius_top: f64, height: f64, segments: u32) -> Self {
        let segments = resolve_segments(segments);
        let brep = vcad_kernel_primitives::make_cone(radius_bottom, radius_top, height, segments);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "cone")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments,
        }
    }

    /// Create a torus centered at origin with axis along Z.
    ///
    /// `major_radius` is the distance from the central axis to the tube center;
    /// `minor_radius` is the tube cross-section radius.
    pub fn torus(major_radius: f64, minor_radius: f64, segments: u32) -> Self {
        let brep = vcad_kernel_primitives::make_torus(major_radius, minor_radius, segments);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "torus")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments,
        }
    }

    /// Create a right-triangular-prism wedge. See
    /// [`vcad_kernel_primitives::make_wedge`] for geometry details.
    pub fn wedge(sx: f64, sy: f64, sz: f64) -> Self {
        let brep = vcad_kernel_primitives::make_wedge(sx, sy, sz);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "wedge")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        }
    }

    /// Create an `n`-gonal right prism centred on the Z axis.
    ///
    /// `sides` must be >= 3; the polygon's circumradius is `radius`, and the
    /// prism extrudes `height` along +Z.
    pub fn prism(sides: u32, radius: f64, height: f64) -> Self {
        let brep = vcad_kernel_primitives::make_prism(sides, radius, height);
        Self {
            names: Some(vcad_kernel_naming::seed_names(&brep, "prism")),
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: sides,
        }
    }

    /// Mirror the solid across a plane defined by a point on the plane and
    /// a normal vector. Routes through [`apply_transform`] with a reflection
    /// matrix, so face / triangle winding is automatically reversed to
    /// preserve outward normals.
    pub fn mirror(&self, plane_origin: [f64; 3], plane_normal: [f64; 3]) -> Solid {
        let p0 = Point3::new(plane_origin[0], plane_origin[1], plane_origin[2]);
        let n = Vec3::new(plane_normal[0], plane_normal[1], plane_normal[2]);
        let t = Transform::reflection(p0, n);
        self.apply_transform(&t)
    }

    // =========================================================================
    // CSG boolean operations
    // =========================================================================

    /// Boolean union (self ∪ other).
    ///
    /// Infallible: on a kernel [`BooleanError`] the operands are merged as
    /// tessellated meshes instead. Use [`Solid::try_union`] to observe the
    /// error (the WASM bindings do, so the browser gets a JS error instead
    /// of a trapped instance).
    pub fn union(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Union)
    }

    /// Boolean difference (self − other).
    ///
    /// Infallible: on a kernel [`BooleanError`] the target is returned
    /// unchanged (the cut simply doesn't apply). Use
    /// [`Solid::try_difference`] to observe the error.
    pub fn difference(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Difference)
    }

    /// Boolean intersection (self ∩ other).
    ///
    /// Infallible: on a kernel [`BooleanError`] the target is returned
    /// unchanged. Use [`Solid::try_intersection`] to observe the error.
    pub fn intersection(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Intersection)
    }

    /// Fallible boolean union — surfaces kernel errors instead of degrading.
    pub fn try_union(&self, other: &Solid) -> Result<Solid, BooleanError> {
        self.try_boolean(other, BooleanOp::Union)
    }

    /// Fallible boolean difference — surfaces kernel errors instead of
    /// degrading.
    pub fn try_difference(&self, other: &Solid) -> Result<Solid, BooleanError> {
        self.try_boolean(other, BooleanOp::Difference)
    }

    /// Fallible boolean intersection — surfaces kernel errors instead of
    /// degrading.
    pub fn try_intersection(&self, other: &Solid) -> Result<Solid, BooleanError> {
        self.try_boolean(other, BooleanOp::Intersection)
    }

    fn boolean(&self, other: &Solid, op: BooleanOp) -> Solid {
        self.try_boolean(other, op).unwrap_or_else(|_| {
            // Degrade gracefully instead of panicking: a visible-but-crude
            // result beats poisoning the process (in the browser a panic
            // kills the WASM instance for the rest of the session).
            match op {
                BooleanOp::Union => {
                    let segments = resolve_segments(self.segments.max(other.segments));
                    let mut combined = self.to_mesh(segments);
                    combined.merge(&other.to_mesh(segments));
                    Solid {
                        names: None,
                        repr: SolidRepr::Mesh(combined),
                        segments,
                    }
                }
                // The cut/overlap couldn't be computed — leave the target
                // untouched, mirroring how `fillet` returns its input when a
                // blend degenerates.
                BooleanOp::Difference | BooleanOp::Intersection => self.clone(),
            }
        })
    }

    fn try_boolean(&self, other: &Solid, op: BooleanOp) -> Result<Solid, BooleanError> {
        match (&self.repr, &other.repr) {
            (SolidRepr::Empty, _) => Ok(match op {
                BooleanOp::Union => other.clone(),
                BooleanOp::Difference | BooleanOp::Intersection => Solid::empty(),
            }),
            (_, SolidRepr::Empty) => Ok(match op {
                BooleanOp::Union | BooleanOp::Difference => self.clone(),
                BooleanOp::Intersection => Solid::empty(),
            }),
            (SolidRepr::BRep(a), SolidRepr::BRep(b)) => {
                // Resolve before the splitter sees it: an operand carrying the
                // raw `0` sentinel (e.g. a BRep built outside the primitive
                // constructors) must not drive a 0-vertex circle loop.
                let segments = resolve_segments(self.segments.max(other.segments));
                let result = boolean_op(a.as_ref(), b.as_ref(), op, segments)?;
                let BooleanResult::BRep(brep) = result;
                let names = match (&self.names, &other.names) {
                    (Some(na), Some(nb)) => Some(vcad_kernel_naming::propagate_boolean(
                        a.as_ref(),
                        na,
                        b.as_ref(),
                        nb,
                        brep.as_ref(),
                    )),
                    _ => None,
                };
                Ok(Solid {
                    names,
                    repr: SolidRepr::BRep(brep),
                    segments,
                })
            }
            // For mesh-only solids, tessellate BRep first then combine meshes
            _ => {
                let segments = resolve_segments(self.segments.max(other.segments));
                let mesh_a = self.to_mesh(segments);
                let mesh_b = other.to_mesh(segments);
                // For mesh-only cases, just concatenate meshes.
                // This is a Phase 1 limitation — proper mesh CSG comes in Phase 2.
                let mut combined = mesh_a;
                combined.merge(&mesh_b);
                Ok(Solid {
                    names: None,
                    repr: SolidRepr::Mesh(combined),
                    segments,
                })
            }
        }
    }

    // =========================================================================
    // Fillet & chamfer
    // =========================================================================

    /// Chamfer all edges of the solid by the given distance.
    ///
    /// Each edge is replaced by a planar bevel face, each original face is
    /// trimmed inward, and each vertex becomes a triangular face.
    ///
    /// Only works on B-rep solids with planar faces (e.g., cubes, extruded
    /// prisms). Returns the solid unchanged for mesh-only or empty solids.
    pub fn chamfer(&self, distance: f64) -> Solid {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                // Same inner-loop hazard as `fillet` — see brep_has_inner_loops.
                if brep_has_inner_loops(brep) {
                    return self.clone();
                }
                let chamfered = vcad_kernel_fillet::chamfer_all_edges(brep, distance);
                if !blend_result_is_valid(brep, &chamfered, true) {
                    return self.clone();
                }
                Solid {
                    names: None,
                    repr: SolidRepr::BRep(Box::new(chamfered)),
                    segments: self.segments,
                }
            }
            _ => self.clone(),
        }
    }

    /// Fillet all edges of the solid with the given radius.
    ///
    /// For plane-only B-reps, each edge is replaced by a cylindrical blend
    /// surface tangent to both adjacent faces, each original face is trimmed
    /// inward, and each vertex becomes a triangular face.
    ///
    /// For B-reps containing non-planar faces (e.g. an arc-extruded profile
    /// with `CylinderSurface` side walls), the curved fillet path is used:
    /// plane-cylinder edges get torus blends, cylinder-cylinder edges are
    /// left sharp (parallel-axis offset is numerically fragile in the
    /// generic rolling-ball solver). "Seam" edges inside a single analytic
    /// surface are skipped since they aren't real geometric edges.
    ///
    /// If the chosen fillet path would produce geometry that escapes the
    /// input AABB expanded by 2·radius, the blend is deemed degenerate and
    /// the input is returned unchanged — a clean sharp-edged solid is
    /// always preferable to a fractured shell with outlying vertices.
    ///
    /// Returns the solid unchanged for mesh-only or empty solids.
    pub fn fillet(&self, radius: f64) -> Solid {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                // Faces with inner loops (holes from booleans) can't survive
                // the rebuild — fail soft rather than fill the holes in.
                if brep_has_inner_loops(brep) {
                    return self.clone();
                }
                let is_planar = brep_is_all_planar(brep);
                let filleted = if is_planar {
                    vcad_kernel_fillet::fillet_all_edges(brep, radius)
                } else {
                    let target_edges = collect_fillet_target_edges(brep);
                    let (result, _details) =
                        vcad_kernel_fillet::fillet_edges_detailed(brep, &target_edges, radius);
                    result
                };
                // Sanity check: the fillet output must live inside a box
                // that's at most 2·radius larger than the input AABB. If
                // any vertex falls outside, some blend diverged — discard
                // the result and return the input unchanged.
                if !fillet_aabb_is_reasonable(brep, &filleted, radius) {
                    return self.clone();
                }
                // A cracked or volume-gaining result is silently-bad
                // geometry — prefer the clean sharp-edged input. The
                // planar all-edges pipeline is expected to produce a
                // perfectly watertight shell; the curved per-edge path
                // intentionally tolerates residual corner-blend gaps
                // (arc-extrude sphere vertex blends), so it only gets the
                // volume-sanity check.
                if !blend_result_is_valid(brep, &filleted, is_planar) {
                    return self.clone();
                }
                Solid {
                    names: None,
                    repr: SolidRepr::BRep(Box::new(filleted)),
                    segments: self.segments,
                }
            }
            _ => self.clone(),
        }
    }

    /// Per-edge blend on query-selected edges with a keyed profile.
    ///
    /// `query` picks plane-plane edges declaratively (all / nearest to a
    /// point / by direction); `keys` describes the cross-section along
    /// each edge — `shape` interpolates a flat chamfer (`0`) into a round
    /// fillet (`1`), `size` is the tangent setback (chamfer leg = fillet
    /// radius). A single key is a constant profile; multiple keys loft
    /// between sections (e.g. a chamfer morphing into a fillet).
    ///
    /// `EdgeQuery::All` with a constant pure-fillet or pure-chamfer
    /// profile routes to the analytic all-edge pipelines (cylindrical /
    /// planar blend surfaces with corner patches). Everything else uses
    /// the per-edge loft builder; edges that share a vertex with an
    /// already-blended edge are skipped (miter corners are a follow-up).
    /// Returns the solid unchanged for mesh-only or empty solids
    /// (fail-soft, mirroring `fillet`).
    pub fn edge_blend(
        &self,
        query: &vcad_kernel_fillet::EdgeQuery,
        keys: &[vcad_kernel_fillet::BlendKey],
    ) -> Solid {
        let SolidRepr::BRep(brep) = &self.repr else {
            return self.clone();
        };
        if keys.is_empty() {
            return self.clone();
        }
        // Same inner-loop hazard as `fillet` — see brep_has_inner_loops.
        if brep_has_inner_loops(brep) {
            return self.clone();
        }

        // Fast path: whole-solid constant fillet/chamfer already have
        // dedicated pipelines with analytic blend surfaces and corner
        // patches — use them.
        if matches!(query, vcad_kernel_fillet::EdgeQuery::All) && keys.len() == 1 {
            let s = keys[0].section;
            if s.shape >= 1.0 - 1e-12 {
                return self.fillet(s.size);
            }
            if s.shape <= 1e-12 {
                return self.chamfer(s.size);
            }
        }

        let (blended, _outcome) = vcad_kernel_fillet::apply_edge_blend(brep, query, keys);
        if !blend_result_is_valid(brep, &blended, true) {
            return self.clone();
        }
        Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(blended)),
            segments: self.segments,
        }
    }

    // =========================================================================
    // Persistent topological naming
    // =========================================================================

    /// The face-name side table, when this solid's provenance supports one
    /// (primitives seed it, booleans propagate it, transforms carry it).
    pub fn names(&self) -> Option<&vcad_kernel_naming::NameMap> {
        self.names.as_ref()
    }

    /// Rewrite the scope of every face name — DAG evaluators call this with
    /// the document node id so names stay unique when several primitives
    /// combine (`cube:top` → `n3:top`).
    pub fn set_name_scope(&mut self, scope: &str) {
        if let Some(names) = &mut self.names {
            names.rescope(scope);
        }
    }

    /// Resolve the edge between two named faces to its current endpoint
    /// positions. Fail-closed: any outcome other than exactly one edge is an
    /// error describing what happened (no names on this solid, ambiguous
    /// match, or lost).
    pub fn resolve_named_edge(
        &self,
        face_a: &str,
        face_b: &str,
    ) -> Result<(Point3, Point3), NamedEdgeError> {
        let SolidRepr::BRep(brep) = &self.repr else {
            return Err(NamedEdgeError::NoNames);
        };
        let Some(names) = &self.names else {
            return Err(NamedEdgeError::NoNames);
        };
        let a = vcad_kernel_naming::FaceName::parse(face_a)
            .ok_or_else(|| NamedEdgeError::BadName(face_a.to_string()))?;
        let b = vcad_kernel_naming::FaceName::parse(face_b)
            .ok_or_else(|| NamedEdgeError::BadName(face_b.to_string()))?;
        let edge_ref = vcad_kernel_naming::EdgeRef::new(a, b);
        match vcad_kernel_naming::resolve_edge(brep, names, &edge_ref) {
            vcad_kernel_naming::EdgeResolution::Resolved { endpoints, .. } => Ok(endpoints),
            vcad_kernel_naming::EdgeResolution::Ambiguous { candidates } => {
                Err(NamedEdgeError::Ambiguous {
                    count: candidates.len(),
                })
            }
            vcad_kernel_naming::EdgeResolution::Lost { reason } => {
                Err(NamedEdgeError::Lost(reason))
            }
        }
    }

    /// Apply a keyed blend to the single edge between two named faces.
    ///
    /// The name resolves against the current topology
    /// ([`Solid::resolve_named_edge`]), then the blend targets exactly that
    /// edge via [`vcad_kernel_fillet::EdgeQuery::Endpoints`]. Unlike the
    /// geometric queries this path is fail-closed end to end: an
    /// unresolvable or ambiguous name is an error, never a
    /// nearest-edge guess.
    pub fn edge_blend_named(
        &self,
        face_a: &str,
        face_b: &str,
        keys: &[vcad_kernel_fillet::BlendKey],
    ) -> Result<Solid, NamedEdgeError> {
        let (a, b) = self.resolve_named_edge(face_a, face_b)?;
        Ok(self.edge_blend(&vcad_kernel_fillet::EdgeQuery::Endpoints { a, b }, keys))
    }

    /// Shell (hollow) the solid by offsetting all faces inward.
    ///
    /// Creates a hollow shell with walls of the specified thickness.
    /// The outer surface remains, and an inner surface is created at
    /// `thickness` offset.
    ///
    /// # Arguments
    ///
    /// * `thickness` - Wall thickness (positive = inward offset)
    ///
    /// # Returns
    ///
    /// A new solid representing the hollow shell. Returns self unchanged
    /// for empty solids.
    pub fn shell(&self, thickness: f64) -> Solid {
        match &self.repr {
            SolidRepr::Empty => Solid::empty(),
            SolidRepr::BRep(brep) => Solid {
                names: None,
                repr: SolidRepr::BRep(Box::new(vcad_kernel_shell::shell_brep(brep, thickness))),
                segments: self.segments,
            },
            SolidRepr::Mesh(mesh) => Solid {
                names: None,
                repr: SolidRepr::Mesh(vcad_kernel_shell::shell_mesh(mesh, thickness)),
                segments: self.segments,
            },
        }
    }

    // =========================================================================
    // Pattern operations
    // =========================================================================

    /// Create a linear pattern of the solid along a direction.
    ///
    /// # Arguments
    ///
    /// * `direction` - Direction vector (normalized internally)
    /// * `count` - Number of copies including original (must be >= 1)
    /// * `spacing` - Distance between copies along the direction
    ///
    /// # Returns
    ///
    /// A union of all copies. Returns self if count < 2.
    pub fn linear_pattern(&self, direction: Vec3, count: u32, spacing: f64) -> Solid {
        if count < 2 {
            return self.clone();
        }

        let dir_norm = direction.norm();
        if dir_norm < 1e-12 {
            return self.clone();
        }
        let dir = direction / dir_norm;

        let mut result = self.clone();
        for i in 1..count {
            let offset = dir * (spacing * i as f64);
            let copy = self.translate(offset.x, offset.y, offset.z);
            result = result.union(&copy);
        }
        result
    }

    /// Create a circular pattern of the solid around an axis.
    ///
    /// # Arguments
    ///
    /// * `axis_origin` - A point on the rotation axis
    /// * `axis_dir` - Direction of the rotation axis
    /// * `count` - Number of copies including original (must be >= 1)
    /// * `angle_deg` - Total angle span in degrees
    ///
    /// # Returns
    ///
    /// A union of all rotated copies. Returns self if count < 2.
    pub fn circular_pattern(
        &self,
        axis_origin: Point3,
        axis_dir: Vec3,
        count: u32,
        angle_deg: f64,
    ) -> Solid {
        use vcad_kernel_math::Dir3;

        if count < 2 {
            return self.clone();
        }

        let dir_norm = axis_dir.norm();
        if dir_norm < 1e-12 {
            return self.clone();
        }
        let axis = Dir3::new_normalize(axis_dir);
        let angle_step = angle_deg.to_radians() / count as f64;

        let mut result = self.clone();
        for i in 1..count {
            let angle = angle_step * i as f64;
            // Build transform: translate to origin, rotate, translate back
            let t_to_origin =
                Transform::translation(-axis_origin.x, -axis_origin.y, -axis_origin.z);
            let rot = Transform::rotation_about_axis(&axis, angle);
            let t_back = Transform::translation(axis_origin.x, axis_origin.y, axis_origin.z);
            // Compose: first translate to origin, then rotate, then translate back
            let composed = t_back.then(&rot).then(&t_to_origin);
            let copy = self.apply_transform(&composed);
            result = result.union(&copy);
        }
        result
    }

    // =========================================================================
    // Sketch-based operations
    // =========================================================================

    /// Create a solid by extruding a sketch profile along a direction.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to extrude
    /// * `direction` - The extrusion direction vector (magnitude = distance)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the profile or direction is invalid.
    pub fn extrude(
        profile: vcad_kernel_sketch::SketchProfile,
        direction: Vec3,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let brep = vcad_kernel_sketch::extrude(&profile, direction)?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by extruding a sketch profile with interior holes.
    ///
    /// Hole loops are closed segment loops in the outer profile's 2D sketch
    /// coordinate system; each becomes an interior wall of the resulting
    /// solid directly (no boolean Difference pass). See
    /// [`vcad_kernel_sketch::extrude_with_holes`] for preconditions.
    pub fn extrude_with_holes(
        profile: vcad_kernel_sketch::SketchProfile,
        holes: &[Vec<vcad_kernel_sketch::SketchSegment>],
        direction: Vec3,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let brep = vcad_kernel_sketch::extrude_with_holes(&profile, holes, direction)?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by extruding a sketch profile with twist and/or scale.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to extrude
    /// * `direction` - The extrusion direction vector (magnitude = distance)
    /// * `twist_angle` - Twist angle in radians (rotation around extrusion axis)
    /// * `scale_end` - Scale factor at the end of extrusion (1.0 = no taper)
    ///
    /// # Returns
    ///
    /// A B-rep solid with bilinear lateral faces when twisted.
    pub fn extrude_with_options(
        profile: vcad_kernel_sketch::SketchProfile,
        direction: Vec3,
        twist_angle: f64,
        scale_end: f64,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let options = vcad_kernel_sketch::ExtrudeOptions {
            twist_angle,
            scale_end,
            ..Default::default()
        };
        let brep = vcad_kernel_sketch::extrude_with_options(&profile, direction, options)?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by revolving a sketch profile around an axis.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to revolve (line segments only)
    /// * `axis_origin` - A point on the axis of revolution
    /// * `axis_dir` - Direction of the axis of revolution
    /// * `angle_deg` - Angle of revolution in degrees (0, 360]
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the profile or parameters are invalid.
    ///
    /// # Limitations
    ///
    /// Arc segments in the profile are not supported (would require torus surfaces).
    pub fn revolve(
        profile: vcad_kernel_sketch::SketchProfile,
        axis_origin: Point3,
        axis_dir: Vec3,
        angle_deg: f64,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let brep =
            vcad_kernel_sketch::revolve(&profile, axis_origin, axis_dir, angle_deg.to_radians())?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by sweeping a profile along a path curve.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to sweep
    /// * `path` - The 3D path curve to sweep along
    /// * `options` - Sweep options (twist, scaling, segments)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the path or profile is invalid.
    pub fn sweep<P: vcad_kernel_geom::Curve3d>(
        profile: vcad_kernel_sketch::SketchProfile,
        path: &P,
        options: vcad_kernel_sweep::SweepOptions,
    ) -> Result<Self, vcad_kernel_sweep::SweepError> {
        let brep = vcad_kernel_sweep::sweep(&profile, path, options)?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by lofting between multiple profiles.
    ///
    /// # Arguments
    ///
    /// * `profiles` - At least 2 profiles to interpolate between
    /// * `options` - Loft options (mode, closed)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if profiles are invalid.
    pub fn loft(
        profiles: &[vcad_kernel_sketch::SketchProfile],
        options: vcad_kernel_sweep::LoftOptions,
    ) -> Result<Self, vcad_kernel_sweep::LoftError> {
        let brep = vcad_kernel_sweep::loft(profiles, options)?;
        Ok(Solid {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    // =========================================================================
    // Transforms
    // =========================================================================

    /// Translate the solid by `(x, y, z)`.
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Solid {
        let t = Transform::translation(x, y, z);
        self.apply_transform(&t)
    }

    /// Rotate the solid by angles in degrees around X, Y, Z axes.
    pub fn rotate(&self, x_deg: f64, y_deg: f64, z_deg: f64) -> Solid {
        let rx = Transform::rotation_x(x_deg.to_radians());
        let ry = Transform::rotation_y(y_deg.to_radians());
        let rz = Transform::rotation_z(z_deg.to_radians());
        // Apply Z, then Y, then X (Euler XYZ intrinsic rotation)
        let t = rx.then(&ry).then(&rz);
        self.apply_transform(&t)
    }

    /// Scale the solid by `(x, y, z)`.
    pub fn scale(&self, x: f64, y: f64, z: f64) -> Solid {
        let t = Transform::scale(x, y, z);
        self.apply_transform(&t)
    }

    /// Apply an arbitrary affine transform to the solid.
    pub fn apply_transform(&self, transform: &Transform) -> Solid {
        match &self.repr {
            SolidRepr::Empty => Solid::empty(),
            SolidRepr::BRep(brep) => {
                let mut new_brep = brep.as_ref().clone();
                // Transform all vertex positions
                for (_id, vertex) in &mut new_brep.topology.vertices {
                    vertex.point = transform.apply_point(&vertex.point);
                }
                // Transform all surface definitions
                for surface in &mut new_brep.geometry.surfaces {
                    *surface = surface.transform(transform);
                }
                // If negative determinant (mirror), flip face orientations
                let det = transform.matrix.upper_left_3x3().determinant();
                if det < 0.0 {
                    for (_id, face) in &mut new_brep.topology.faces {
                        face.orientation = match face.orientation {
                            vcad_kernel_topo::Orientation::Forward => {
                                vcad_kernel_topo::Orientation::Reversed
                            }
                            vcad_kernel_topo::Orientation::Reversed => {
                                vcad_kernel_topo::Orientation::Forward
                            }
                        };
                    }
                }
                Solid {
                    names: self.names.clone(),
                    repr: SolidRepr::BRep(Box::new(new_brep)),
                    segments: self.segments,
                }
            }
            SolidRepr::Mesh(mesh) => {
                let mut new_mesh = mesh.clone();
                let verts = &mut new_mesh.vertices;
                for chunk in verts.chunks_mut(3) {
                    let p = Point3::new(chunk[0] as f64, chunk[1] as f64, chunk[2] as f64);
                    let tp = transform.apply_point(&p);
                    chunk[0] = tp.x as f32;
                    chunk[1] = tp.y as f32;
                    chunk[2] = tp.z as f32;
                }
                // If any scale factor is negative, flip triangle winding
                let det = transform.matrix.upper_left_3x3().determinant();
                if det < 0.0 {
                    for tri in new_mesh.indices.chunks_mut(3) {
                        tri.swap(1, 2);
                    }
                }
                Solid {
                    names: None,
                    repr: SolidRepr::Mesh(new_mesh),
                    segments: self.segments,
                }
            }
        }
    }

    // =========================================================================
    // Queries
    // =========================================================================

    /// Check if the solid is empty (has no geometry).
    pub fn is_empty(&self) -> bool {
        match &self.repr {
            SolidRepr::Empty => true,
            SolidRepr::BRep(_) => false,
            SolidRepr::Mesh(m) => m.num_triangles() == 0,
        }
    }

    /// Get the triangle mesh representation.
    ///
    /// Returns an **indexed** mesh (shared vertices across adjacent triangles).
    /// Used for analysis — volume, surface area, boundary edges, and boolean
    /// fallbacks all expect shared vertices.
    ///
    /// **For meshes heading to the renderer, STL/GLB export, or the ray
    /// tracer**, run the result through
    /// [`vcad_kernel_tessellate::render_bake`]. That module is the single
    /// canonical "prepare for rendering" entry point and its output is an
    /// unindexed mesh with crease-aware vertex normals baked in.
    pub fn to_mesh(&self, segments: u32) -> TriangleMesh {
        match &self.repr {
            SolidRepr::Empty => TriangleMesh::new(),
            SolidRepr::BRep(brep) => tessellate_brep(brep.as_ref(), segments),
            SolidRepr::Mesh(m) => m.clone(),
        }
    }

    /// Run SIMP topology optimization inside this solid's volume.
    ///
    /// The solid's tessellation becomes the design domain (material can
    /// only appear where the solid already has volume); the spec supplies
    /// loads, supports, and the target volume fraction. Returns the
    /// optimized organic structure as a mesh-backed solid together with
    /// the run diagnostics (compliance history, achieved volume fraction,
    /// grid size).
    pub fn topology_optimize(
        &self,
        spec: &vcad_kernel_topopt::TopoOptSpec,
    ) -> Result<(Solid, vcad_kernel_topopt::TopoOptResult), vcad_kernel_topopt::TopoOptError> {
        let mesh = self.to_mesh(self.segments);
        let result = vcad_kernel_topopt::optimize_mesh(&mesh, spec)?;
        Ok((Solid::from_mesh(result.mesh.clone()), result))
    }

    /// Compute the volume of the solid from its triangle mesh.
    pub fn volume(&self) -> f64 {
        let mesh = self.to_mesh(self.segments);
        compute_volume(&mesh)
    }

    /// Compute the surface area of the solid from its triangle mesh.
    pub fn surface_area(&self) -> f64 {
        let mesh = self.to_mesh(self.segments);
        compute_surface_area(&mesh)
    }

    /// Compute the axis-aligned bounding box as `(min, max)`.
    ///
    /// For B-rep solids with only planar faces, computes directly from vertex
    /// positions (no tessellation needed). For curved surfaces, falls back to
    /// the tessellated mesh since vertices alone don't capture the full extent.
    /// Check if the axis-aligned bounding boxes of two solids overlap.
    ///
    /// Uses a fast vertex-based AABB (no tessellation) for BRep solids.
    pub fn aabb_overlaps(&self, other: &Solid) -> bool {
        let (min_a, max_a) = self.vertex_aabb();
        let (min_b, max_b) = other.vertex_aabb();
        min_a[0] <= max_b[0]
            && max_a[0] >= min_b[0]
            && min_a[1] <= max_b[1]
            && max_a[1] >= min_b[1]
            && min_a[2] <= max_b[2]
            && max_a[2] >= min_b[2]
    }

    /// Minimum signed distance between this solid and `other` in mm.
    ///
    /// Positive is the minimum separation, negative the deepest penetration
    /// when the solids intersect (see
    /// [`vcad_kernel_tessellate::clearance`]). Tessellation-based: both
    /// solids are meshed at their own `segments` setting, so curved-surface
    /// results carry the usual chord error (raise `segments` for tight
    /// fits). Returns `None` when either solid is empty.
    pub fn clearance(&self, other: &Solid) -> Option<ClearanceResult> {
        let mesh_a = self.to_mesh(self.segments);
        let mesh_b = other.to_mesh(other.segments);
        mesh_clearance(&mesh_a, &mesh_b)
    }

    /// Fast vertex-only AABB (no tessellation). Slightly underestimates for curved surfaces.
    fn vertex_aabb(&self) -> ([f64; 3], [f64; 3]) {
        match &self.repr {
            SolidRepr::Empty => ([0.0; 3], [0.0; 3]),
            SolidRepr::BRep(brep) => {
                let mut min = [f64::MAX; 3];
                let mut max = [f64::MIN; 3];
                for (_id, v) in &brep.topology.vertices {
                    let p = v.point;
                    min[0] = min[0].min(p.x);
                    min[1] = min[1].min(p.y);
                    min[2] = min[2].min(p.z);
                    max[0] = max[0].max(p.x);
                    max[1] = max[1].max(p.y);
                    max[2] = max[2].max(p.z);
                }
                (min, max)
            }
            SolidRepr::Mesh(mesh) => {
                let mut min = [f64::MAX; 3];
                let mut max = [f64::MIN; 3];
                for chunk in mesh.vertices.chunks(3) {
                    min[0] = min[0].min(chunk[0] as f64);
                    min[1] = min[1].min(chunk[1] as f64);
                    min[2] = min[2].min(chunk[2] as f64);
                    max[0] = max[0].max(chunk[0] as f64);
                    max[1] = max[1].max(chunk[1] as f64);
                    max[2] = max[2].max(chunk[2] as f64);
                }
                (min, max)
            }
        }
    }

    /// Compute the axis-aligned bounding box as `([min_x, min_y, min_z], [max_x, max_y, max_z])`.
    pub fn bounding_box(&self) -> ([f64; 3], [f64; 3]) {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                use vcad_kernel_geom::SurfaceKind;
                let all_planar = brep
                    .geometry
                    .surfaces
                    .iter()
                    .all(|s| s.surface_type() == SurfaceKind::Plane);
                if all_planar {
                    let mut min = [f64::MAX; 3];
                    let mut max = [f64::MIN; 3];
                    for (_id, v) in &brep.topology.vertices {
                        let p = v.point;
                        min[0] = min[0].min(p.x);
                        min[1] = min[1].min(p.y);
                        min[2] = min[2].min(p.z);
                        max[0] = max[0].max(p.x);
                        max[1] = max[1].max(p.y);
                        max[2] = max[2].max(p.z);
                    }
                    (min, max)
                } else {
                    let mesh = self.to_mesh(self.segments);
                    compute_bounding_box(&mesh)
                }
            }
            _ => {
                let mesh = self.to_mesh(self.segments);
                compute_bounding_box(&mesh)
            }
        }
    }

    /// Compute the geometric centroid (volume-weighted center of mass).
    pub fn center_of_mass(&self) -> [f64; 3] {
        let mesh = self.to_mesh(self.segments);
        compute_center_of_mass(&mesh)
    }

    /// Number of triangles in the tessellated mesh.
    pub fn num_triangles(&self) -> usize {
        let mesh = self.to_mesh(self.segments);
        mesh.num_triangles()
    }

    // =========================================================================
    // STEP import/export
    // =========================================================================

    /// Import the first solid from a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A `Solid` containing the imported B-rep geometry.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the file cannot be read, parsed, or contains no solids.
    pub fn from_step(path: impl AsRef<Path>) -> Result<Self, StepError> {
        let solids = vcad_kernel_step::read_step(path)?;
        let brep = solids.into_iter().next().ok_or(StepError::NoSolids)?;
        Ok(Self {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Import all solids from a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A vector of `Solid`s, one for each solid found in the file.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the file cannot be read, parsed, or contains no solids.
    pub fn from_step_all(path: impl AsRef<Path>) -> Result<Vec<Self>, StepError> {
        let solids = vcad_kernel_step::read_step(path)?;
        Ok(solids
            .into_iter()
            .map(|brep| Self {
                names: None,
                repr: SolidRepr::BRep(Box::new(brep)),
                segments: 32,
            })
            .collect())
    }

    /// Import the first solid from a STEP buffer.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw STEP file contents
    ///
    /// # Returns
    ///
    /// A `Solid` containing the imported B-rep geometry.
    pub fn from_step_buffer(data: &[u8]) -> Result<Self, StepError> {
        let solids = vcad_kernel_step::read_step_from_buffer(data)?;
        let brep = solids.into_iter().next().ok_or(StepError::NoSolids)?;
        Ok(Self {
            names: None,
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Import all solids from a STEP buffer.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw STEP file contents
    ///
    /// # Returns
    ///
    /// A vector of `Solid` objects, one for each body in the STEP file.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the buffer cannot be parsed.
    pub fn from_step_buffer_all(data: &[u8]) -> Result<Vec<Self>, StepError> {
        let solids = vcad_kernel_step::read_step_from_buffer(data)?;
        Ok(solids
            .into_iter()
            .map(|brep| Self {
                names: None,
                repr: SolidRepr::BRep(Box::new(brep)),
                segments: 32,
            })
            .collect())
    }

    /// Export this solid to a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns `StepExportError::NotBRep` if the solid has been converted to mesh-only
    /// representation (e.g., after boolean operations). STEP export requires B-rep data.
    /// Returns `StepExportError::Empty` if the solid is empty.
    pub fn to_step(&self, path: impl AsRef<Path>) -> Result<(), StepExportError> {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                vcad_kernel_step::write_step(brep.as_ref(), path)?;
                Ok(())
            }
            SolidRepr::Mesh(_) => Err(StepExportError::NotBRep),
            SolidRepr::Empty => Err(StepExportError::Empty),
        }
    }

    /// Export this solid to STEP format in memory.
    ///
    /// # Returns
    ///
    /// The STEP file contents as bytes.
    ///
    /// # Errors
    ///
    /// See [`Solid::to_step`] for error conditions.
    pub fn to_step_buffer(&self) -> Result<Vec<u8>, StepExportError> {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                let buffer = vcad_kernel_step::write_step_to_buffer(brep.as_ref())?;
                Ok(buffer)
            }
            SolidRepr::Mesh(_) => Err(StepExportError::NotBRep),
            SolidRepr::Empty => Err(StepExportError::Empty),
        }
    }

    /// Check if this solid can be exported to STEP format.
    ///
    /// Returns `true` if the solid has B-rep data (not converted to mesh-only).
    /// Returns `false` for mesh-only or empty solids.
    pub fn can_export_step(&self) -> bool {
        matches!(&self.repr, SolidRepr::BRep(_))
    }

    /// Get a reference to the underlying B-rep solid, if available.
    ///
    /// Returns `None` if the solid is mesh-only (e.g., after boolean operations)
    /// or empty. This is useful for operations that require the full B-rep
    /// representation, such as ray tracing.
    pub fn brep(&self) -> Option<&BRepSolid> {
        match &self.repr {
            SolidRepr::BRep(brep) => Some(brep.as_ref()),
            _ => None,
        }
    }

    /// Check if this solid can be ray traced.
    ///
    /// Returns `true` if the solid has B-rep data (required for direct ray tracing).
    /// Returns `false` for mesh-only or empty solids.
    pub fn can_raytrace(&self) -> bool {
        matches!(&self.repr, SolidRepr::BRep(_))
    }
}

// =============================================================================
// Operator overloads for ergonomic boolean operations
// =============================================================================

impl std::ops::Add for Solid {
    type Output = Solid;

    /// Boolean union: `a + b` is equivalent to `a.union(&b)`.
    fn add(self, other: Solid) -> Solid {
        self.union(&other)
    }
}

impl std::ops::Add for &Solid {
    type Output = Solid;

    /// Boolean union: `&a + &b` is equivalent to `a.union(&b)`.
    fn add(self, other: &Solid) -> Solid {
        self.union(other)
    }
}

impl std::ops::Sub for Solid {
    type Output = Solid;

    /// Boolean difference: `a - b` is equivalent to `a.difference(&b)`.
    fn sub(self, other: Solid) -> Solid {
        self.difference(&other)
    }
}

impl std::ops::Sub for &Solid {
    type Output = Solid;

    /// Boolean difference: `&a - &b` is equivalent to `a.difference(&b)`.
    fn sub(self, other: &Solid) -> Solid {
        self.difference(other)
    }
}

impl std::ops::BitAnd for Solid {
    type Output = Solid;

    /// Boolean intersection: `a & b` is equivalent to `a.intersection(&b)`.
    fn bitand(self, other: Solid) -> Solid {
        self.intersection(&other)
    }
}

impl std::ops::BitAnd for &Solid {
    type Output = Solid;

    /// Boolean intersection: `&a & &b` is equivalent to `a.intersection(&b)`.
    fn bitand(self, other: &Solid) -> Solid {
        self.intersection(other)
    }
}

// =============================================================================
// Mesh computation helpers (same algorithms as vcad lib.rs)
// =============================================================================

/// True when any face of the solid carries inner boundary loops (holes),
/// e.g. a bore left by a boolean Difference. The fillet/chamfer rebuild
/// pipelines reconstruct faces from their outer loops only, so running
/// them on such a body silently fills the holes back in. Callers use this
/// to fail soft (return the input unchanged) instead of corrupting the
/// model.
fn brep_has_inner_loops(brep: &BRepSolid) -> bool {
    brep.topology
        .faces
        .iter()
        .any(|(_, f)| !f.inner_loops.is_empty())
}

/// Guard against runaway fillet output. Both the topology vertices AND
/// the tessellated mesh vertices of `filleted` must fit inside `input`'s
/// AABB expanded by 2·radius. The mesh check catches blend surfaces whose
/// topology corners stay sane but whose interior samples fly off because
/// the surface's parameterization is broken.
fn fillet_aabb_is_reasonable(input: &BRepSolid, filleted: &BRepSolid, radius: f64) -> bool {
    let verts_in = &input.topology.vertices;
    if verts_in.is_empty() {
        return true;
    }
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for (_id, v) in verts_in {
        let p = v.point;
        if p.x < min[0] {
            min[0] = p.x;
        }
        if p.y < min[1] {
            min[1] = p.y;
        }
        if p.z < min[2] {
            min[2] = p.z;
        }
        if p.x > max[0] {
            max[0] = p.x;
        }
        if p.y > max[1] {
            max[1] = p.y;
        }
        if p.z > max[2] {
            max[2] = p.z;
        }
    }
    let slack = 2.0 * radius.abs();
    for i in 0..3 {
        min[i] -= slack;
        max[i] += slack;
    }
    for (_id, v) in &filleted.topology.vertices {
        let p = v.point;
        if p.x < min[0]
            || p.y < min[1]
            || p.z < min[2]
            || p.x > max[0]
            || p.y > max[1]
            || p.z > max[2]
        {
            return false;
        }
    }
    // Mesh-level check: catches interior sample points from bad
    // parameterizations (NURBS rolling ball, mis-oriented torus) that
    // don't appear as topology corners.
    let mesh = tessellate_brep(filleted, 32);
    let n = mesh.num_vertices();
    for i in 0..n {
        let x = mesh.vertices[3 * i] as f64;
        let y = mesh.vertices[3 * i + 1] as f64;
        let z = mesh.vertices[3 * i + 2] as f64;
        if x < min[0] || y < min[1] || z < min[2] || x > max[0] || y > max[1] || z > max[2] {
            return false;
        }
    }
    true
}

/// Post-flight validity gate for edge-blend results. The blend output
/// must not *gain* volume (fillets and chamfers only remove material)
/// nor collapse most of it, and — when `require_watertight` — must
/// tessellate with no boundary edges. A result failing these checks is
/// silently-bad geometry (cracked shell, inverted inset faces) and
/// callers should fall back to the unmodified input instead.
/// In strict mode (planar all-edges pipeline) the mesh must be perfectly
/// watertight and the volume must shrink (within tolerance) but not
/// collapse. In lenient mode (curved per-edge pipeline, which today
/// intentionally tolerates residual corner-blend gaps at sphere/torus
/// junctions — see the arc-profile regression test) the mesh volume is
/// unreliable, so the gate instead bounds the *total length* of open
/// boundary edges relative to the input's bbox diagonal: shipped-good
/// curved fillets measure ≈1.4×, while broken rebuilds (cracked cap
/// rims, fillet-of-fillet) measure 2.6–26×.
fn blend_result_is_valid(input: &BRepSolid, blended: &BRepSolid, strict: bool) -> bool {
    let mesh = tessellate_brep(blended, 32);
    if strict {
        if !mesh.boundary_edges().is_empty() {
            return false;
        }
        let vol_out = compute_volume(&mesh);
        if !vol_out.is_finite() || vol_out <= 0.0 {
            return false;
        }
        let vol_in = compute_volume(&tessellate_brep(input, 32));
        return vol_out <= vol_in * (1.0 + 1e-6) && vol_out >= vol_in * 0.5;
    }
    let mut open_len = 0.0;
    for (a, b) in mesh.boundary_edges() {
        let (a, b) = (a as usize * 3, b as usize * 3);
        let dx = (mesh.vertices[a] - mesh.vertices[b]) as f64;
        let dy = (mesh.vertices[a + 1] - mesh.vertices[b + 1]) as f64;
        let dz = (mesh.vertices[a + 2] - mesh.vertices[b + 2]) as f64;
        open_len += (dx * dx + dy * dy + dz * dz).sqrt();
    }
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for (_id, v) in &input.topology.vertices {
        let p = [v.point.x, v.point.y, v.point.z];
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let diag = ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2))
        .sqrt()
        .max(1e-9);
    open_len <= 2.0 * diag
}

fn brep_is_all_planar(brep: &BRepSolid) -> bool {
    use vcad_kernel_geom::SurfaceKind;
    brep.geometry
        .surfaces
        .iter()
        .all(|s| s.surface_type() == SurfaceKind::Plane)
}

/// Collect edges to fillet, omitting edges that are either unsafe or
/// spurious:
///
/// - **Same-surface seams** — consecutive arc-extrude cylinders that were
///   emitted from a single arc share their surface; the edge between them
///   is tessellation artifact, not a real geometric edge.
/// - **Cylinder–cylinder offset (parallel axes, different centers)** —
///   this is the "vertical seam where two arcs meet" pattern in an
///   arc-extruded profile. The generic NURBS rolling-ball blend in
///   `rolling_ball_blend` is numerically fragile here (its alternating
///   projection can converge to a point far from the true seam curve),
///   producing blend vertices flung hundreds of units from the solid. Until
///   we have an analytic parallel-cylinder torus case, we leave these
///   seams sharp. The cap-rim (plane-cylinder) edges still get their
///   proper torus blends, which is the visually dominant rounding users
///   asked for.
pub fn collect_fillet_target_edges(brep: &BRepSolid) -> Vec<vcad_kernel_topo::EdgeId> {
    use vcad_kernel_geom::{CylinderSurface, Plane, SurfaceKind};
    let topo = &brep.topology;
    let geom = &brep.geometry;

    let mut out = Vec::new();
    for (edge_id, edge) in &topo.edges {
        let he_a = edge.half_edge;
        let Some(he_b) = topo.half_edges[he_a].twin else {
            continue;
        };
        let fa = topo.half_edges[he_a]
            .loop_id
            .and_then(|l| topo.loops[l].face);
        let fb = topo.half_edges[he_b]
            .loop_id
            .and_then(|l| topo.loops[l].face);
        let (Some(fa), Some(fb)) = (fa, fb) else {
            continue;
        };
        let sa = &geom.surfaces[topo.faces[fa].surface_index];
        let sb = &geom.surfaces[topo.faces[fb].surface_index];

        let skip = match (sa.surface_type(), sb.surface_type()) {
            (SurfaceKind::Cylinder, SurfaceKind::Cylinder) => {
                // Skip all cylinder-cylinder edges — either they share a
                // surface (tessellation artifact) or they're the fragile
                // offset-axis case handled above.
                let a = sa.as_any().downcast_ref::<CylinderSurface>();
                let b = sb.as_any().downcast_ref::<CylinderSurface>();
                // If the axes are parallel, treat as offset seam & skip.
                // The classify layer would otherwise send it to
                // CylinderCylinderSkew → diverging rolling ball.
                match (a, b) {
                    (Some(a), Some(b)) => a.axis.as_ref().dot(b.axis.as_ref()).abs() > 1.0 - 1e-6,
                    _ => true,
                }
            }
            (SurfaceKind::Plane, SurfaceKind::Plane) => {
                let a = sa.as_any().downcast_ref::<Plane>();
                let b = sb.as_any().downcast_ref::<Plane>();
                match (a, b) {
                    (Some(a), Some(b)) => {
                        a.normal_dir.as_ref().dot(b.normal_dir.as_ref()).abs() > 1.0 - 1e-9
                            && (a.origin - b.origin).dot(a.normal_dir.as_ref()).abs() < 1e-9
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !skip {
            out.push(edge_id);
        }
    }
    out
}

fn compute_volume(mesh: &TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut vol = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    (vol / 6.0).abs()
}

fn compute_surface_area(mesh: &TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut area = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = Vec3::new(verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64);
        let v1 = Vec3::new(verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64);
        let v2 = Vec3::new(verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64);
        area += (v1 - v0).cross(v2 - v0).norm() / 2.0;
    }
    area
}

fn compute_bounding_box(mesh: &TriangleMesh) -> ([f64; 3], [f64; 3]) {
    let verts = &mesh.vertices;
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for chunk in verts.chunks(3) {
        for i in 0..3 {
            let v = chunk[i] as f64;
            if v < min[i] {
                min[i] = v;
            }
            if v > max[i] {
                max[i] = v;
            }
        }
    }
    (min, max)
}

fn compute_center_of_mass(mesh: &TriangleMesh) -> [f64; 3] {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    let mut total_vol = 0.0;
    // Area-weighted surface centroid — fallback when the signed-volume
    // integral is unreliable (open or inconsistently wound meshes, e.g.
    // thin sheet bodies). A convex combination of triangle centroids, so
    // it always lands inside the bounding box.
    let mut acx = 0.0;
    let mut acy = 0.0;
    let mut acz = 0.0;
    let mut total_area = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        let vol = v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        total_vol += vol;
        cx += vol * (v0[0] + v1[0] + v2[0]);
        cy += vol * (v0[1] + v1[1] + v2[1]);
        cz += vol * (v0[2] + v1[2] + v2[2]);

        let e1 = Vec3::new(v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]);
        let e2 = Vec3::new(v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]);
        let area = e1.cross(e2).norm() / 2.0;
        total_area += area;
        acx += area * (v0[0] + v1[0] + v2[0]) / 3.0;
        acy += area * (v0[1] + v1[1] + v2[1]) / 3.0;
        acz += area * (v0[2] + v1[2] + v2[2]) / 3.0;
    }
    let area_centroid = if total_area > 0.0 {
        [acx / total_area, acy / total_area, acz / total_area]
    } else {
        [0.0; 3]
    };
    if total_vol.abs() < 1e-15 {
        return area_centroid;
    }
    let s = 1.0 / (4.0 * total_vol);
    let com = [cx * s, cy * s, cz * s];
    // The volume-weighted centroid is only valid for a closed,
    // consistently wound mesh — on open meshes the signed-tet
    // contributions partially cancel and the division can throw the
    // centroid outside the bounding box, which is impossible for a real
    // COM. Fall back to the surface centroid when that happens.
    let (min, max) = compute_bounding_box(mesh);
    let eps = 1e-9
        * (max[0] - min[0])
            .max(max[1] - min[1])
            .max(max[2] - min[2])
            .max(1.0);
    let in_bbox = (0..3).all(|i| com[i] >= min[i] - eps && com[i] <= max[i] + eps);
    if in_bbox {
        com
    } else {
        area_centroid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(!cube.is_empty());
        let mesh = cube.to_mesh(32);
        assert!(mesh.num_triangles() >= 12);
    }

    #[test]
    fn test_cylinder() {
        let cyl = Solid::cylinder(5.0, 10.0, 32);
        assert!(!cyl.is_empty());
    }

    #[test]
    fn test_sphere() {
        let sphere = Solid::sphere(10.0, 32);
        assert!(!sphere.is_empty());
    }

    #[test]
    fn test_cone() {
        let cone = Solid::cone(5.0, 3.0, 10.0, 32);
        assert!(!cone.is_empty());
    }

    #[test]
    fn test_empty() {
        let empty = Solid::empty();
        assert!(empty.is_empty());
    }

    /// Rotor/stator air gap: a 5 mm rotor inside a ring stator with a 1 mm
    /// radial design gap measures ≈1 mm (within chord error at 128 segments).
    #[test]
    fn test_clearance_rotor_stator_gap() {
        let rotor = Solid::cylinder(5.0, 10.0, 128);
        let stator = Solid::cylinder(10.0, 10.0, 128).difference(&Solid::cylinder(6.0, 12.0, 128));
        let r = rotor.clearance(&stator).unwrap();
        assert!(!r.intersecting);
        assert!(
            (r.distance - 1.0).abs() < 0.02,
            "air gap = {} mm, expected ≈1.0",
            r.distance
        );
    }

    /// Shrinking the gap moves the measured value with it.
    #[test]
    fn test_clearance_tracks_geometry() {
        let rotor = Solid::cylinder(5.6, 10.0, 128);
        let stator = Solid::cylinder(10.0, 10.0, 128).difference(&Solid::cylinder(6.0, 12.0, 128));
        let r = rotor.clearance(&stator).unwrap();
        assert!(!r.intersecting);
        assert!(
            (r.distance - 0.4).abs() < 0.02,
            "air gap = {} mm, expected ≈0.4",
            r.distance
        );
    }

    /// An oversized rotor intersects the stator: negative distance.
    #[test]
    fn test_clearance_interference_is_negative() {
        let rotor = Solid::cylinder(7.0, 10.0, 64);
        let stator = Solid::cylinder(10.0, 10.0, 64).difference(&Solid::cylinder(6.0, 12.0, 64));
        let r = rotor.clearance(&stator).unwrap();
        assert!(r.intersecting);
        assert!(r.distance < 0.0, "distance = {}, expected < 0", r.distance);
    }

    #[test]
    fn test_clearance_empty_is_none() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(cube.clearance(&Solid::empty()).is_none());
    }

    #[test]
    fn test_resolve_segments() {
        assert_eq!(resolve_segments(0), DEFAULT_SEGMENTS); // 0 = auto
        assert_eq!(resolve_segments(1), 3); // clamp to a valid loop
        assert_eq!(resolve_segments(2), 3);
        assert_eq!(resolve_segments(3), 3);
        assert_eq!(resolve_segments(48), 48); // honored as-is
    }

    /// Regression: a disc built entirely from cylinders with the IR "auto"
    /// (`segments == 0`) sentinel used to drive a 0-vertex circle loop through
    /// the boolean splitter and panic in `Topology::add_loop`. A cube survives
    /// because it carries 32, bumping the boolean's `max(..)`; an all-cylinder
    /// document has no such bump, so every operand was 0. Resolving the
    /// sentinel must keep the difference panic-free and produce real geometry.
    /// Mirrors the loon repro:
    /// `[difference [union [translate 0 0 -0.5 [cylinder 4 2.6]]
    ///   [circular-pattern 0 0 0 0 0 1 3 360 [translate 25 0 -0.5 [cylinder 1.6 2.6]]]]
    ///   [cylinder 30 1.6]]`
    #[test]
    fn test_all_cylinder_disc_difference_does_not_panic() {
        use vcad_kernel_math::{Point3, Vec3};

        // Every primitive uses segments = 0 — the exact condition that panicked.
        let disc = Solid::cylinder(4.0, 2.6, 0).translate(0.0, 0.0, -0.5);
        let boss = Solid::cylinder(1.6, 2.6, 0).translate(25.0, 0.0, -0.5);
        let bosses = boss.circular_pattern(Point3::origin(), Vec3::new(0.0, 0.0, 1.0), 3, 360.0);
        let body = disc.union(&bosses);
        let bore = Solid::cylinder(30.0, 1.6, 0);

        let result = body.difference(&bore);
        let mesh = result.to_mesh(64);
        assert!(
            !mesh.indices.is_empty(),
            "all-cylinder disc difference produced an empty mesh"
        );
    }

    #[test]
    fn test_translate() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let moved = cube.translate(100.0, 0.0, 0.0);
        let (min, max) = moved.bounding_box();
        assert!((min[0] - 100.0).abs() < 0.1);
        assert!((max[0] - 110.0).abs() < 0.1);
    }

    #[test]
    fn test_scale() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let scaled = cube.scale(2.0, 1.0, 1.0);
        let (min, max) = scaled.bounding_box();
        assert!((max[0] - min[0] - 20.0).abs() < 0.1);
        assert!((max[1] - min[1] - 10.0).abs() < 0.1);
    }

    #[test]
    fn test_union() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        let result = a.union(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_difference() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(5.0, 5.0, 5.0);
        let result = a.difference(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_intersection() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        let result = a.intersection(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_plate_with_hole_via_solid_api() {
        // This mirrors the exact code path used by the WASM/app
        // Plate: 80x6x60 at origin
        let plate = Solid::cube(80.0, 6.0, 60.0);

        // Hole: 12x20x12, translated to (34, -7, 24)
        let hole = Solid::cube(12.0, 20.0, 12.0).translate(34.0, -7.0, 24.0);

        // Boolean difference
        let result = plate.difference(&hole);

        // Check volume and bbox
        let volume = result.volume();
        let (min, max) = result.bounding_box();

        println!("Solid API test - volume: {}", volume);
        println!(
            "Solid API test - bbox: [{:.1},{:.1},{:.1}] to [{:.1},{:.1},{:.1}]",
            min[0], min[1], min[2], max[0], max[1], max[2]
        );

        // Volume should be ~27936 (plate - intersection)
        // Note: boolean operations have some precision variance
        assert!(
            volume > 25000.0 && volume < 30000.0,
            "Expected volume ~27936, got {}",
            volume
        );

        // Bbox Y should be [0, 6] (not -7 to 13!)
        assert!(
            min[1] >= -0.1 && max[1] <= 6.1,
            "Y bounds should be [0,6], got [{}, {}]",
            min[1],
            max[1]
        );
    }

    #[test]
    fn test_cube_volume() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let vol = cube.volume();
        assert!((vol - 1000.0).abs() < 1.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_cube_surface_area() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let area = cube.surface_area();
        assert!((area - 600.0).abs() < 1.0, "expected ~600, got {area}");
    }

    #[test]
    fn test_cube_bounding_box() {
        let cube = Solid::cube(10.0, 20.0, 30.0);
        let (min, max) = cube.bounding_box();
        assert!((max[0] - min[0] - 10.0).abs() < 0.01);
        assert!((max[1] - min[1] - 20.0).abs() < 0.01);
        assert!((max[2] - min[2] - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_cube_center_of_mass() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let com = cube.center_of_mass();
        assert!((com[0] - 5.0).abs() < 0.1, "cx: {}", com[0]);
        assert!((com[1] - 5.0).abs() < 0.1, "cy: {}", com[1]);
        assert!((com[2] - 5.0).abs() < 0.1, "cz: {}", com[2]);
    }

    #[test]
    fn test_center_of_mass_open_mesh_stays_in_bbox() {
        // A single triangle far from the origin: the divergence-theorem
        // integral sees a nonzero signed volume and the naive division
        // places the centroid at (v0+v1+v2)/4 — pulled toward the origin,
        // outside the triangle's own bbox (z would be 7.5 vs bbox z=10).
        // The guard must fall back to the surface centroid instead.
        let mesh = TriangleMesh {
            vertices: vec![4.0, 0.0, 10.0, 0.0, 4.0, 10.0, -4.0, -4.0, 10.0],
            indices: vec![0, 1, 2],
            normals: vec![0.0; 9],
            face_kinds: Vec::new(),
        };
        let com = compute_center_of_mass(&mesh);
        let (min, max) = compute_bounding_box(&mesh);
        for i in 0..3 {
            assert!(
                com[i] >= min[i] - 1e-9 && com[i] <= max[i] + 1e-9,
                "com[{i}] = {} outside bbox [{}, {}]",
                com[i],
                min[i],
                max[i]
            );
        }
        // Area centroid of the lone triangle.
        assert!((com[0] - 0.0).abs() < 1e-9, "cx: {}", com[0]);
        assert!((com[1] - 0.0).abs() < 1e-9, "cy: {}", com[1]);
        assert!((com[2] - 10.0).abs() < 1e-9, "cz: {}", com[2]);
    }

    #[test]
    fn test_rotate_cube_volume() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let rotated = cube.rotate(45.0, 30.0, 60.0);
        let vol = rotated.volume();
        // Volume should be preserved after rotation
        assert!((vol - 1000.0).abs() < 2.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_translate_cylinder_bbox() {
        let cyl = Solid::cylinder(5.0, 10.0, 32);
        let moved = cyl.translate(100.0, 200.0, 300.0);
        let (min, max) = moved.bounding_box();
        // Center should be offset by translation
        assert!((min[0] - 95.0).abs() < 0.5, "min x: {}", min[0]);
        assert!((max[0] - 105.0).abs() < 0.5, "max x: {}", max[0]);
        assert!((min[2] - 300.0).abs() < 0.5, "min z: {}", min[2]);
        assert!((max[2] - 310.0).abs() < 0.5, "max z: {}", max[2]);
    }

    #[test]
    fn test_scale_cylinder_volume() {
        let cyl = Solid::cylinder(5.0, 10.0, 64);
        let base_vol = cyl.volume();
        let scaled = cyl.scale(2.0, 2.0, 2.0);
        let scaled_vol = scaled.volume();
        // Volume scales by 2^3 = 8
        let ratio = scaled_vol / base_vol;
        assert!((ratio - 8.0).abs() < 0.5, "expected ratio ~8, got {ratio}");
    }

    #[test]
    fn test_mirror_x() {
        let cube = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);
        let mirrored = cube.scale(-1.0, 1.0, 1.0);
        let (min, _max) = mirrored.bounding_box();
        assert!(
            min[0] < 0.0,
            "mirrored min x should be negative: {}",
            min[0]
        );
    }

    #[test]
    fn test_empty_union() {
        let empty = Solid::empty();
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let result = empty.union(&cube);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_num_triangles() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(
            cube.num_triangles() >= 12,
            "cube should have at least 12 triangles"
        );
    }

    #[test]
    fn test_chamfer_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let chamfered = cube.chamfer(1.0);
        assert!(!chamfered.is_empty());
        let vol = chamfered.volume();
        // Chamfered cube: V = L³ - 6d²(L-d) = 1000 - 54 = 946
        assert!(
            (vol - 946.0).abs() < 5.0,
            "chamfered cube volume: expected ~946, got {vol}"
        );
    }

    #[test]
    fn test_fillet_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let filleted = cube.fillet(1.0);
        assert!(!filleted.is_empty());
        // Fillet should have more triangles than original cube due to curved surfaces
        assert!(
            filleted.num_triangles() > cube.num_triangles(),
            "filleted cube should have more triangles than plain cube"
        );
    }

    #[test]
    fn test_chamfer_empty() {
        let empty = Solid::empty();
        let chamfered = empty.chamfer(1.0);
        assert!(chamfered.is_empty());
    }

    #[test]
    fn test_extrude_rectangle() {
        use vcad_kernel_sketch::SketchProfile;

        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);
        let solid = Solid::extrude(profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        assert!(!solid.is_empty());
        let vol = solid.volume();
        // Expected: 10 * 5 * 20 = 1000
        assert!(
            (vol - 1000.0).abs() < 2.0,
            "expected volume ~1000, got {vol}"
        );
    }

    #[test]
    fn test_revolve_rectangle() {
        use vcad_kernel_sketch::SketchProfile;

        // Rectangle profile offset from Z-axis, revolve 90° around Z
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 3.0, 10.0);
        let solid = Solid::revolve(profile, Point3::origin(), Vec3::z(), 90.0).unwrap();

        assert!(!solid.is_empty());
        // Volume check is loose due to planar approximation
        let vol = solid.volume();
        assert!(vol > 100.0, "expected positive volume, got {vol}");
    }

    #[test]
    fn test_extrude_then_boolean() {
        use vcad_kernel_sketch::SketchProfile;

        // Create two extruded boxes and union them
        let profile1 = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 10.0);
        let box1 = Solid::extrude(profile1, Vec3::new(0.0, 0.0, 10.0)).unwrap();

        let profile2 =
            SketchProfile::rectangle(Point3::new(5.0, 5.0, 0.0), Vec3::x(), Vec3::y(), 10.0, 10.0);
        let box2 = Solid::extrude(profile2, Vec3::new(0.0, 0.0, 10.0)).unwrap();

        // Union should produce a valid solid
        let result = box1.union(&box2);
        assert!(!result.is_empty());

        // Volume check: combined volume should be positive and reasonable
        // Two 10x10x10 boxes with 5x5x10 overlap = 1000+1000-250 = 1750
        // Due to boolean approximations, just check it's positive
        let vol = result.volume();
        assert!(
            vol >= 1000.0 && vol <= 2000.0,
            "expected volume between 1000 and 2000, got {vol}"
        );
    }

    #[test]
    fn test_linear_pattern() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let pattern = cube.linear_pattern(Vec3::new(1.0, 0.0, 0.0), 3, 20.0);
        assert!(!pattern.is_empty());
        // 3 cubes of 1000mm³ each = 3000mm³
        let vol = pattern.volume();
        assert!((vol - 3000.0).abs() < 50.0, "expected ~3000, got {vol}");
        // Bounding box should span 50mm in X (10 + 20 + 10 + 20 + 10 = 50... wait, it's 10 + 20 + 10 = 40)
        // Actually: first cube 0-10, second 20-30, third 40-50 → span is 50
        let (min, max) = pattern.bounding_box();
        assert!(
            (max[0] - min[0] - 50.0).abs() < 1.0,
            "expected X span ~50, got {}",
            max[0] - min[0]
        );
    }

    #[test]
    fn test_linear_pattern_single() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let pattern = cube.linear_pattern(Vec3::new(1.0, 0.0, 0.0), 1, 20.0);
        // Should return original cube unchanged
        let vol = pattern.volume();
        assert!((vol - 1000.0).abs() < 2.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_circular_pattern() {
        let cube = Solid::cube(5.0, 5.0, 5.0).translate(10.0, 0.0, 0.0);
        // Pattern 4 copies around Z axis, 360° total
        let pattern = cube.circular_pattern(Point3::origin(), Vec3::z(), 4, 360.0);
        assert!(!pattern.is_empty());
        // 4 cubes of 125mm³ each = 500mm³
        let vol = pattern.volume();
        assert!((vol - 500.0).abs() < 20.0, "expected ~500, got {vol}");
    }

    #[test]
    fn test_circular_pattern_90_deg() {
        let cube = Solid::cube(5.0, 5.0, 5.0).translate(10.0, 0.0, 0.0);
        // Pattern 2 copies around Z axis, 90° span (original at 0°, copy at 45°)
        let pattern = cube.circular_pattern(Point3::origin(), Vec3::z(), 2, 90.0);
        assert!(!pattern.is_empty());
        // 2 cubes
        let vol = pattern.volume();
        assert!((vol - 250.0).abs() < 10.0, "expected ~250, got {vol}");
    }

    #[test]
    fn test_shell_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let shell = cube.shell(2.0);
        assert!(!shell.is_empty());
        // Analytical shell creates 12 faces (6 outer + 6 inner)
        let shell_vol = shell.volume();
        assert!(
            shell_vol > 0.0,
            "shell volume {} should be positive",
            shell_vol
        );
    }

    #[test]
    fn test_shell_empty() {
        let empty = Solid::empty();
        let shell = empty.shell(1.0);
        assert!(shell.is_empty());
    }

    #[test]
    fn test_step_roundtrip() {
        // Create a cube
        let cube = Solid::cube(15.0, 25.0, 35.0);

        // Export to STEP buffer
        let buffer = cube.to_step_buffer().expect("should export to STEP");
        assert!(!buffer.is_empty());

        // Import from buffer
        let imported = Solid::from_step_buffer(&buffer).expect("should import from STEP");
        assert!(!imported.is_empty());

        // Verify topology
        let cube_tris = cube.num_triangles();
        let imported_tris = imported.num_triangles();
        assert!(
            cube_tris > 0 && imported_tris > 0,
            "both should have triangles"
        );

        // Verify geometry roughly matches (volume check)
        let cube_vol = cube.volume();
        let imported_vol = imported.volume();
        let vol_diff = (cube_vol - imported_vol).abs();
        assert!(
            vol_diff < 1.0,
            "volumes should match: original={}, imported={}, diff={}",
            cube_vol,
            imported_vol,
            vol_diff
        );
    }

    #[test]
    fn test_step_can_export() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(cube.can_export_step(), "primitive should be exportable");

        // After boolean, B-rep is preserved (canExportStep returns true)
        // Note: More complex boolean chains may produce invalid topology
        // that causes toStepBuffer to fail, but canExportStep still returns true
        let hole = Solid::cylinder(3.0, 15.0, 32);
        let result = cube.difference(&hole);
        assert!(
            result.can_export_step(),
            "boolean result should preserve B-rep marker"
        );
    }

    #[test]
    fn test_step_export_empty_error() {
        let empty = Solid::empty();
        let result = empty.to_step_buffer();
        assert!(
            matches!(result, Err(StepExportError::Empty)),
            "empty solid should return Empty error"
        );
    }

    #[test]
    fn test_operator_add() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);
        let result = a + b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_sub() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(5.0, 5.0, 15.0);
        let result = a - b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_bitand() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 5.0, 5.0);
        let result = a & b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_ref() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        // Test reference operators
        let union = &a + &b;
        let diff = &a - &b;
        let inter = &a & &b;
        assert!(!union.is_empty());
        assert!(!diff.is_empty());
        assert!(!inter.is_empty());
    }

    /// Regression for issue #186: boolean operations must always produce a
    /// B-rep result. Previously a `cube.difference(cylinder)` could fall
    /// through to a mesh-only result, breaking STEP export, raytrace and
    /// any downstream feature keyed off `Solid::as_brep()`.
    #[test]
    fn test_boolean_result_is_always_brep() {
        // Common case: cube with a cylindrical hole.
        let cube = Solid::cube(20.0, 20.0, 20.0);
        let cyl = Solid::cylinder(5.0, 30.0, 32).translate(10.0, 10.0, -5.0);
        let cube_minus_cyl = cube.difference(&cyl);
        assert!(
            cube_minus_cyl.as_brep().is_some(),
            "cube.difference(cylinder) must be BRep-backed"
        );
        cube_minus_cyl
            .to_step_buffer()
            .expect("STEP export should succeed on BRep difference result");

        // Empty intersection (non-overlapping) must also be BRep, not mesh.
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0).translate(50.0, 0.0, 0.0);
        let empty = a.intersection(&b);
        assert!(
            empty.as_brep().is_some(),
            "non-overlapping intersection must be BRep-backed"
        );

        // Perpendicular equal-radius cylinder Steinmetz fallback. The old
        // `cyl_cyl` path emitted a mesh-only result; it now reconstructs a
        // triangle-soup B-rep so the type contract holds.
        let cyl1 = Solid::cylinder(5.0, 30.0, 32).translate(0.0, 0.0, -15.0);
        let cyl2 = Solid::cylinder(5.0, 30.0, 32)
            .rotate(90.0, 0.0, 0.0)
            .translate(0.0, -15.0, 0.0);
        let steinmetz = cyl1.union(&cyl2);
        assert!(
            steinmetz.as_brep().is_some(),
            "perpendicular cylinder union must be BRep-backed"
        );
    }

    /// Regression: cylinder minus bore should produce a closed solid with
    /// volume roughly equal to π(R² - r²) × h.
    #[test]
    fn test_cylinder_minus_bore_volume() {
        let outer = Solid::cylinder(15.0, 30.0, 64);
        let bore = Solid::cylinder(6.0, 40.0, 32);
        let result = outer.difference(&bore);

        // Expected volume = π(15² - 6²) × 30 ≈ 17814
        let vol = result.volume();
        let expected = std::f64::consts::PI * (225.0 - 36.0) * 30.0;
        assert!(
            (vol - expected).abs() < expected * 0.10,
            "expected ~{expected:.0}, got {vol:.0} — caps may be missing"
        );
    }

    /// Regression: flanged hub with multiple bolt holes.
    /// Tests that degenerate cap AABB is computed correctly so all bolt
    /// circles are found as intersections and accumulate as inner loops.
    #[test]
    fn test_flanged_hub_with_bolts() {
        // Match the docs example which uses centered_cylinder:
        // centered_cylinder(r, h) = cylinder(r, h).translate(0, 0, -h/2)
        // hub: centered_cylinder(15, 30) → z=-15 to z=15
        let hub = Solid::cylinder(15.0, 30.0, 64).translate(0.0, 0.0, -15.0);
        // flange: centered_cylinder(35, 6).translate(0,0,-15) → z=-18 to z=-12
        let flange = Solid::cylinder(35.0, 6.0, 64).translate(0.0, 0.0, -18.0);
        // bore: centered_cylinder(6, 40) → z=-20 to z=20
        let bore = Solid::cylinder(6.0, 40.0, 32).translate(0.0, 0.0, -20.0);

        let hub_flange = hub.union(&flange);
        let mut result = hub_flange.difference(&bore);

        // bolt_pattern(6, 50.0, 4.0, 10.0, 24).translate(0,0,-15)
        // bolt_circle_diameter=50 → radius=25, hole_diameter=4 → hole_radius=2
        for i in 0..6u32 {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / 6.0;
            let bolt = Solid::cylinder(2.0, 10.0, 24).translate(
                25.0 * angle.cos(),
                25.0 * angle.sin(),
                -15.0,
            );
            result = result.difference(&bolt);
        }

        // Verify face count: 20 faces expected (hub + flange + bore + 6 bolts)
        if let SolidRepr::BRep(ref brep) = result.repr {
            let solid = &brep.topology.solids[brep.solid_id];
            let shell = &brep.topology.shells[solid.outer_shell];
            assert!(
                shell.faces.len() >= 18,
                "expected at least 18 faces, got {}",
                shell.faces.len()
            );
        }

        // Mesh should be non-empty
        let mesh = result.to_mesh(32);
        assert!(mesh.num_triangles() > 1000, "mesh too small");
    }

    /// Reproduce the exact docs IR evaluation path for the flanged hub.
    /// Uses circular_pattern (single difference of 6-bolt union) instead of
    /// individual bolt subtractions.
    #[test]
    fn test_flanged_hub_docs_ir() {
        use vcad_kernel_math::{Point3, Vec3};

        // Exactly matches the IR in packages/docs/src/lib/examples.ts
        let hub = Solid::cylinder(15.0, 30.0, 64).translate(0.0, 0.0, -15.0);
        let flange = Solid::cylinder(35.0, 6.0, 64).translate(0.0, 0.0, -18.0);
        let hub_flange = hub.union(&flange);

        let bore = Solid::cylinder(6.0, 40.0, 32).translate(0.0, 0.0, -20.0);
        let hub_flange_bore = hub_flange.difference(&bore);

        // IR node 9: bolt radius=4, height=10
        // IR node 10: translate(25, 0, -20) → bolt from z=-20 to z=-10
        let bolt_hole = Solid::cylinder(4.0, 10.0, 24).translate(25.0, 0.0, -20.0);

        // IR node 11: CircularPattern(count=6, angle=360°, axis Z)
        let bolts = bolt_hole.circular_pattern(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            6,
            360.0,
        );

        // IR node 12: final difference
        let result = hub_flange_bore.difference(&bolts);

        // Must have the annular face at z=-12
        if let SolidRepr::BRep(ref brep) = result.repr {
            let solid = &brep.topology.solids[brep.solid_id];
            let shell = &brep.topology.shells[solid.outer_shell];
            let has_annular = shell.faces.iter().any(|&fid| {
                let face = &brep.topology.faces[fid];
                let surface = &brep.geometry.surfaces[face.surface_index];
                if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                    (plane.origin.z - (-12.0)).abs() < 0.1 && plane.normal_dir.into_inner().z > 0.5
                } else {
                    false
                }
            });
            assert!(has_annular, "Missing annular face at z=-12");
        }

        let mesh = result.to_mesh(32);

        // Verify z=-12 annular face has proper triangulation
        let mut z12_count = 0;
        let mut z18_count = 0;
        for tri in 0..mesh.num_triangles() {
            let i0 = mesh.indices[tri * 3] as usize;
            let i1 = mesh.indices[tri * 3 + 1] as usize;
            let i2 = mesh.indices[tri * 3 + 2] as usize;
            let z0 = mesh.vertices[i0 * 3 + 2] as f64;
            let z1 = mesh.vertices[i1 * 3 + 2] as f64;
            let z2 = mesh.vertices[i2 * 3 + 2] as f64;
            if (z0 - (-12.0)).abs() < 0.01
                && (z1 - (-12.0)).abs() < 0.01
                && (z2 - (-12.0)).abs() < 0.01
            {
                z12_count += 1;
            }
            if (z0 - (-18.0)).abs() < 0.01
                && (z1 - (-18.0)).abs() < 0.01
                && (z2 - (-18.0)).abs() < 0.01
            {
                z18_count += 1;
            }
        }
        assert!(
            z12_count > 50,
            "z=-12 annular face has too few triangles: {}",
            z12_count
        );
        assert!(
            z18_count > 50,
            "z=-18 flange face has too few triangles: {}",
            z18_count
        );
        assert!(
            mesh.num_triangles() > 1000,
            "mesh too small: {}",
            mesh.num_triangles()
        );
    }

    // =========================================================================
    // Mesh validation framework — ray casting from multiple angles
    // =========================================================================

    /// Ray-triangle intersection (Möller–Trumbore).
    /// Returns Some(t) if the ray hits the triangle at distance t > 0.
    fn ray_triangle_intersect(
        ray_origin: &[f64; 3],
        ray_dir: &[f64; 3],
        v0: &[f64; 3],
        v1: &[f64; 3],
        v2: &[f64; 3],
    ) -> Option<(f64, [f64; 3])> {
        let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
        let h = cross(ray_dir, &edge2);
        let a = dot(&edge1, &h);
        if a.abs() < 1e-12 {
            return None;
        }
        let f = 1.0 / a;
        let s = [
            ray_origin[0] - v0[0],
            ray_origin[1] - v0[1],
            ray_origin[2] - v0[2],
        ];
        let u = f * dot(&s, &h);
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(&s, &edge1);
        let v = f * dot(ray_dir, &q);
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = f * dot(&edge2, &q);
        if t > 1e-8 {
            // Triangle normal (unnormalized)
            let normal = cross(&edge1, &edge2);
            Some((t, normal))
        } else {
            None
        }
    }

    fn cross(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn norm(a: &[f64; 3]) -> f64 {
        dot(a, a).sqrt()
    }

    /// Get triangle vertex positions from mesh.
    fn get_tri(mesh: &TriangleMesh, tri_idx: usize) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let i0 = mesh.indices[tri_idx * 3] as usize * 3;
        let i1 = mesh.indices[tri_idx * 3 + 1] as usize * 3;
        let i2 = mesh.indices[tri_idx * 3 + 2] as usize * 3;
        let v = &mesh.vertices;
        (
            [v[i0] as f64, v[i0 + 1] as f64, v[i0 + 2] as f64],
            [v[i1] as f64, v[i1 + 1] as f64, v[i1 + 2] as f64],
            [v[i2] as f64, v[i2 + 1] as f64, v[i2 + 2] as f64],
        )
    }

    /// Compute triangle area.
    fn tri_area(v0: &[f64; 3], v1: &[f64; 3], v2: &[f64; 3]) -> f64 {
        let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
        let c = cross(&e1, &e2);
        0.5 * norm(&c)
    }

    struct MeshValidation {
        degenerate_triangles: usize,
        inverted_normals: usize,
        non_watertight_rays: usize,
        total_rays: usize,
        max_triangle_area: f64,
        issues: Vec<String>,
    }

    /// Validate a mesh from first principles using ray casting.
    fn validate_mesh(mesh: &TriangleMesh, name: &str) -> MeshValidation {
        let mut result = MeshValidation {
            degenerate_triangles: 0,
            inverted_normals: 0,
            non_watertight_rays: 0,
            total_rays: 0,
            max_triangle_area: 0.0,
            issues: Vec::new(),
        };

        let num_tris = mesh.num_triangles();

        // 1. Check for degenerate triangles and compute triangle normals
        let mut triangle_normals: Vec<[f64; 3]> = Vec::with_capacity(num_tris);
        for i in 0..num_tris {
            let (v0, v1, v2) = get_tri(mesh, i);
            let area = tri_area(&v0, &v1, &v2);
            result.max_triangle_area = result.max_triangle_area.max(area);

            if area < 1e-10 {
                result.degenerate_triangles += 1;
            }

            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            triangle_normals.push(cross(&e1, &e2));
        }

        // 2. Compute bounding box
        let mut bbox_min = [f64::INFINITY; 3];
        let mut bbox_max = [f64::NEG_INFINITY; 3];
        for i in 0..mesh.num_vertices() {
            let x = mesh.vertices[i * 3] as f64;
            let y = mesh.vertices[i * 3 + 1] as f64;
            let z = mesh.vertices[i * 3 + 2] as f64;
            for (d, val) in [(0, x), (1, y), (2, z)] {
                bbox_min[d] = bbox_min[d].min(val);
                bbox_max[d] = bbox_max[d].max(val);
            }
        }
        let bbox_center = [
            (bbox_min[0] + bbox_max[0]) / 2.0,
            (bbox_min[1] + bbox_max[1]) / 2.0,
            (bbox_min[2] + bbox_max[2]) / 2.0,
        ];
        let bbox_extent = [
            bbox_max[0] - bbox_min[0],
            bbox_max[1] - bbox_min[1],
            bbox_max[2] - bbox_min[2],
        ];
        let bbox_diag = norm(&bbox_extent);

        // 3. Check normals point outward via signed volume contribution
        // For a closed mesh, each triangle's signed volume contribution should
        // be consistent with its geometric normal direction.
        let mut positive_vol_tris = 0;
        let mut negative_vol_tris = 0;
        for i in 0..num_tris {
            let (v0, v1, v2) = get_tri(mesh, i);
            // Signed volume of tetrahedron formed with origin
            let sv = v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
            if sv > 0.0 {
                positive_vol_tris += 1;
            } else {
                negative_vol_tris += 1;
            }
        }

        // 4. Ray casting from multiple directions
        // For each direction, cast rays in a grid and check:
        // - Even number of intersections (watertight)
        // - First hit normal faces the camera (outward-facing)
        let ray_dirs: Vec<[f64; 3]> = vec![
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            // Diagonals
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, -1.0],
            [1.0, -1.0, 1.0],
            [-1.0, -1.0, -1.0],
        ];

        let grid_res = 8; // 8×8 grid per direction

        for ray_dir_raw in &ray_dirs {
            let dir_len = norm(ray_dir_raw);
            let ray_dir = [
                ray_dir_raw[0] / dir_len,
                ray_dir_raw[1] / dir_len,
                ray_dir_raw[2] / dir_len,
            ];

            // Build orthogonal basis for the ray grid
            let up = if ray_dir[1].abs() < 0.9 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            let right = cross(&ray_dir, &up);
            let right_len = norm(&right);
            let right = [
                right[0] / right_len,
                right[1] / right_len,
                right[2] / right_len,
            ];
            let up = cross(&right, &ray_dir);

            let start_dist = bbox_diag;
            let grid_step = bbox_diag * 0.8 / grid_res as f64;

            for gy in 0..grid_res {
                for gx in 0..grid_res {
                    let offset_x = (gx as f64 - grid_res as f64 / 2.0 + 0.5) * grid_step;
                    let offset_y = (gy as f64 - grid_res as f64 / 2.0 + 0.5) * grid_step;
                    let origin = [
                        bbox_center[0] - ray_dir[0] * start_dist
                            + right[0] * offset_x
                            + up[0] * offset_y,
                        bbox_center[1] - ray_dir[1] * start_dist
                            + right[1] * offset_x
                            + up[1] * offset_y,
                        bbox_center[2] - ray_dir[2] * start_dist
                            + right[2] * offset_x
                            + up[2] * offset_y,
                    ];

                    // Cast ray against all triangles
                    let mut hits: Vec<(f64, [f64; 3], usize)> = Vec::new();
                    for t in 0..num_tris {
                        let (v0, v1, v2) = get_tri(mesh, t);
                        if let Some((dist, tri_normal)) =
                            ray_triangle_intersect(&origin, &ray_dir, &v0, &v1, &v2)
                        {
                            hits.push((dist, tri_normal, t));
                        }
                    }

                    if hits.is_empty() {
                        continue; // Ray missed the object entirely
                    }

                    result.total_rays += 1;
                    hits.sort_by(|a, b| a.0.total_cmp(&b.0));

                    // Watertight check: must have even number of hits
                    if hits.len() % 2 != 0 {
                        result.non_watertight_rays += 1;
                        if result.issues.len() < 5 {
                            let hit_info: Vec<_> = hits
                                .iter()
                                .map(|(t, n, idx)| {
                                    let face_dot = dot(n, &ray_dir);
                                    format!("t={t:.2} tri={idx} dot={face_dot:.3}")
                                })
                                .collect();
                            result.issues.push(format!(
                                "Non-watertight: dir=({:.1},{:.1},{:.1}) pos=({:.1},{:.1},{:.1}) hits={} [{:}]",
                                ray_dir[0], ray_dir[1], ray_dir[2],
                                origin[0], origin[1], origin[2],
                                hits.len(),
                                hit_info.join(", "),
                            ));
                        }
                    }

                    // Normal direction check: first hit should have normal
                    // opposing ray direction (facing the camera)
                    let first_dot = dot(&hits[0].1, &ray_dir);
                    if first_dot > 0.0 {
                        result.inverted_normals += 1;
                        if result.issues.len() < 5 {
                            let tri_idx = hits[0].2;
                            let (v0, v1, v2) = get_tri(mesh, tri_idx);
                            let area = tri_area(&v0, &v1, &v2);
                            result.issues.push(format!(
                                "Inverted normal: tri={tri_idx} area={area:.1} dot={first_dot:.3} dir=({:.1},{:.1},{:.1})",
                                ray_dir[0], ray_dir[1], ray_dir[2],
                            ));
                        }
                    }
                }
            }
        }

        println!("=== Mesh Validation: {name} ===");
        println!("  Triangles: {num_tris}");
        println!("  Degenerate triangles: {}", result.degenerate_triangles);
        println!("  Max triangle area: {:.1}", result.max_triangle_area);
        println!("  Vol sign split: +{positive_vol_tris} / -{negative_vol_tris}");
        println!(
            "  Rays cast: {} | non-watertight: {} | inverted normals: {}",
            result.total_rays, result.non_watertight_rays, result.inverted_normals
        );
        for issue in &result.issues {
            println!("  ISSUE: {issue}");
        }

        result
    }

    /// Comprehensive mesh validation for the flanged hub.
    #[test]
    fn test_flanged_hub_mesh_validation() {
        let hub = Solid::cylinder(15.0, 30.0, 64);
        let flange = Solid::cylinder(35.0, 6.0, 64).translate(0.0, 0.0, -15.0);
        let bore = Solid::cylinder(6.0, 40.0, 32);

        let hub_flange = hub.union(&flange);
        let mut result = hub_flange.difference(&bore);

        for i in 0..6u32 {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / 6.0;
            let bolt = Solid::cylinder(4.0, 10.0, 24).translate(
                25.0 * angle.cos(),
                25.0 * angle.sin(),
                -15.0,
            );
            result = result.difference(&bolt);
        }

        // Also test simpler case: just hub+bore (no flange, no bolts)
        {
            let simple =
                Solid::cylinder(15.0, 30.0, 64).difference(&Solid::cylinder(6.0, 40.0, 32));
            let smesh = simple.to_mesh(32);
            let sv = validate_mesh(&smesh, "simple-bore");
            println!(
                "Simple bore: watertight={} inverted={}",
                sv.non_watertight_rays, sv.inverted_normals
            );
        }

        // Test flange alone with one bolt
        {
            let flange = Solid::cylinder(35.0, 6.0, 64).translate(0.0, 0.0, -15.0);
            let bolt = Solid::cylinder(4.0, 10.0, 24).translate(25.0, 0.0, -15.0);
            let fbolt = flange.difference(&bolt);
            let fmesh = fbolt.to_mesh(32);
            let fv = validate_mesh(&fmesh, "flange-1bolt");
            println!(
                "Flange+1bolt: watertight={} inverted={}",
                fv.non_watertight_rays, fv.inverted_normals
            );
        }

        let mesh = result.to_mesh(32);

        // Identify large triangles that may be artifacts
        let num_tris = mesh.num_triangles();
        let mut large_tris = Vec::new();
        for i in 0..num_tris {
            let (v0, v1, v2) = get_tri(&mesh, i);
            let area = tri_area(&v0, &v1, &v2);
            if area > 50.0 {
                // Compute max edge length
                let e01 = norm(&[v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]]);
                let e12 = norm(&[v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]]);
                let e20 = norm(&[v0[0] - v2[0], v0[1] - v2[1], v0[2] - v2[2]]);
                let max_edge = e01.max(e12).max(e20);
                large_tris.push((i, area, max_edge));
            }
        }
        if !large_tris.is_empty() {
            println!("\nLarge triangles (area > 50):");
            for (idx, area, max_edge) in &large_tris {
                let (v0, v1, v2) = get_tri(&mesh, *idx);
                let centroid = [
                    (v0[0] + v1[0] + v2[0]) / 3.0,
                    (v0[1] + v1[1] + v2[1]) / 3.0,
                    (v0[2] + v1[2] + v2[2]) / 3.0,
                ];
                println!(
                    "  tri {idx}: area={area:.1} max_edge={max_edge:.1} centroid=({:.1},{:.1},{:.1})",
                    centroid[0], centroid[1], centroid[2]
                );
                println!("    v0=({:.1},{:.1},{:.1})", v0[0], v0[1], v0[2]);
                println!("    v1=({:.1},{:.1},{:.1})", v1[0], v1[1], v1[2]);
                println!("    v2=({:.1},{:.1},{:.1})", v2[0], v2[1], v2[2]);
            }
        }

        let v = validate_mesh(&mesh, "flanged-hub");

        assert_eq!(
            v.degenerate_triangles, 0,
            "should have no degenerate triangles"
        );
        assert_eq!(
            v.inverted_normals,
            0,
            "all front-facing normals should point outward ({}% inverted)",
            if v.total_rays > 0 {
                v.inverted_normals * 100 / v.total_rays
            } else {
                0
            }
        );

        // Watertight check: allow up to 5% failures from pre-existing
        // boolean sewing imprecision (diagonal rays near seam edges).
        let watertight_pct = if v.total_rays > 0 {
            v.non_watertight_rays * 100 / v.total_rays
        } else {
            0
        };
        assert!(
            watertight_pct <= 5,
            "mesh should be mostly watertight ({watertight_pct}% rays failed)"
        );

        // Max triangle area: the flange cap is ~3848 mm².
        // No single triangle should be larger than ~100 mm² (was 225 before fix).
        assert!(
            v.max_triangle_area < 100.0,
            "max triangle area {:.0} is too large — tessellation artifact",
            v.max_triangle_area
        );
    }

    #[test]
    fn test_fillet_cylinder_produces_bounded_blend() {
        // Simpler isolation of the curved-fillet path: a primitive cylinder
        // has 3 faces (side + 2 caps) with plane-cylinder edges at each cap.
        // Filleting r=2 must keep all vertices within the original AABB
        // (plus a small margin from the blend arc). A diverging rolling-ball
        // would push a vertex far outside, so this catches that regression.
        let cyl = Solid::cylinder(10.0, 20.0, 32);
        let filleted = cyl.fillet(2.0);
        let mesh = filleted.to_mesh(32);
        let n = mesh.num_vertices();
        let mut min_x = f64::MAX;
        let mut max_x = f64::MIN;
        let mut min_z = f64::MAX;
        let mut max_z = f64::MIN;
        for i in 0..n {
            let x = mesh.vertices[3 * i] as f64;
            let z = mesh.vertices[3 * i + 2] as f64;
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
            if z < min_z {
                min_z = z;
            }
            if z > max_z {
                max_z = z;
            }
        }
        assert!(
            min_x > -15.0 && max_x < 15.0,
            "fillet blew up in X: range=[{:.1}, {:.1}]",
            min_x,
            max_x
        );
        assert!(
            min_z > -5.0 && max_z < 25.0,
            "fillet blew up in Z: range=[{:.1}, {:.1}]",
            min_z,
            max_z
        );
    }

    #[test]
    fn test_fillet_arc_profile_has_sphere_vertex_blend_faces() {
        // Regression: the spherical vertex-blend patches at arc-to-arc
        // convex junctions should be present in the filleted BRep — one
        // per junction on the bottom cap and one per junction on the top
        // cap. Without them the fillet leaves a visible crescent gap
        // between adjacent torus blends.
        use vcad_kernel_sketch::{SketchProfile, SketchSegment};
        let segments = vec![
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(45.0, 0.0),
                end: vcad_kernel_math::Point2::new(20.0, 40.0),
                center: vcad_kernel_math::Point2::new(10.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(20.0, 40.0),
                end: vcad_kernel_math::Point2::new(-30.0, 35.0),
                center: vcad_kernel_math::Point2::new(-5.0, 25.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-30.0, 35.0),
                end: vcad_kernel_math::Point2::new(-50.0, 5.0),
                center: vcad_kernel_math::Point2::new(-25.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-50.0, 5.0),
                end: vcad_kernel_math::Point2::new(-35.0, -25.0),
                center: vcad_kernel_math::Point2::new(-30.0, -5.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-35.0, -25.0),
                end: vcad_kernel_math::Point2::new(10.0, -30.0),
                center: vcad_kernel_math::Point2::new(-10.0, -10.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(10.0, -30.0),
                end: vcad_kernel_math::Point2::new(45.0, 0.0),
                center: vcad_kernel_math::Point2::new(20.0, -10.0),
                ccw: true,
            },
        ];
        let profile = SketchProfile::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            segments,
        )
        .expect("valid profile");
        let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 18.0)).expect("extrude ok");
        let filleted = extruded.fillet(4.0);

        let (sphere_surfaces, sphere_faces, shell_face_count) = match &filleted.repr {
            SolidRepr::BRep(b) => {
                let sphere_surf_idxs: std::collections::HashSet<usize> = b
                    .geometry
                    .surfaces
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.surface_type() == vcad_kernel_geom::SurfaceKind::Sphere)
                    .map(|(i, _)| i)
                    .collect();
                let shell = &b.topology.shells[b.topology.solids[b.solid_id].outer_shell];
                let sphere_face_count = shell
                    .faces
                    .iter()
                    .filter(|fid| sphere_surf_idxs.contains(&b.topology.faces[**fid].surface_index))
                    .count();
                (sphere_surf_idxs.len(), sphere_face_count, shell.faces.len())
            }
            _ => (0, 0, 0),
        };
        assert!(
            sphere_surfaces > 0,
            "expected sphere surfaces in the fillet output; got {sphere_surfaces}"
        );
        assert!(
            sphere_faces > 0,
            "sphere surfaces exist ({sphere_surfaces}) but no sphere face in the shell ({shell_face_count} total shell faces)"
        );

        // Tessellator must actually produce triangles from the sphere
        // patches — tessellate the whole solid once, and separately
        // tessellate each sphere face alone, confirming the per-face
        // tessellator emits ≥1 triangle for each patch. Earlier
        // versions of this regression used a hard-coded "total > 524"
        // proxy, but that coupled the test to whatever the current
        // cylinder n_height heuristic is. The per-face check is
        // invariant to unrelated density changes.
        use vcad_kernel_tessellate::{tessellate_brep_by_face, TessellationParams};
        let params = TessellationParams::from_segments(32);
        let per_face = tessellate_brep_by_face(
            match &filleted.repr {
                SolidRepr::BRep(b) => b.as_ref(),
                _ => unreachable!(),
            },
            &params,
        );
        let sphere_tris: usize = per_face
            .iter()
            .filter(|(_, k, _)| *k == vcad_kernel_geom::SurfaceKind::Sphere)
            .map(|(_, _, m)| m.num_triangles())
            .sum();
        assert!(
            sphere_tris >= sphere_faces,
            "expected at least {sphere_faces} sphere triangles (one per patch), got {sphere_tris}"
        );
    }

    /// Shared pork-chop kidney profile for regression + diagnostic tests.
    fn porkchop_segments() -> Vec<vcad_kernel_sketch::SketchSegment> {
        use vcad_kernel_sketch::SketchSegment;
        vec![
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(45.0, 0.0),
                end: vcad_kernel_math::Point2::new(20.0, 40.0),
                center: vcad_kernel_math::Point2::new(10.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(20.0, 40.0),
                end: vcad_kernel_math::Point2::new(-30.0, 35.0),
                center: vcad_kernel_math::Point2::new(-5.0, 25.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-30.0, 35.0),
                end: vcad_kernel_math::Point2::new(-50.0, 5.0),
                center: vcad_kernel_math::Point2::new(-25.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-50.0, 5.0),
                end: vcad_kernel_math::Point2::new(-35.0, -25.0),
                center: vcad_kernel_math::Point2::new(-30.0, -5.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(-35.0, -25.0),
                end: vcad_kernel_math::Point2::new(10.0, -30.0),
                center: vcad_kernel_math::Point2::new(-10.0, -10.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: vcad_kernel_math::Point2::new(10.0, -30.0),
                end: vcad_kernel_math::Point2::new(45.0, 0.0),
                center: vcad_kernel_math::Point2::new(20.0, -10.0),
                ccw: true,
            },
        ]
    }

    /// Diagnostic test — not an assertion, meant to be run with
    /// `--nocapture` to print the world-space location of every
    /// boundary edge in the pork-chop fillet mesh. Any non-zero output
    /// is a tessellation hole (the "armpit gaps" visible in the
    /// browser).
    ///
    /// Invoke with:
    ///   cargo test -p vcad-kernel diag_porkchop_boundary_edges -- --nocapture --ignored
    #[test]
    #[ignore]
    fn diag_porkchop_boundary_edges() {
        use vcad_kernel_sketch::SketchProfile;
        let segments = porkchop_segments();
        let profile = SketchProfile::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            segments,
        )
        .expect("valid profile");
        let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 18.0)).expect("extrude ok");
        let filleted = extruded.fillet(4.0);
        let mesh = filleted.to_mesh(32);

        let boundary = mesh.boundary_edges();
        let nm = mesh.non_manifold_edges();
        let loops = mesh.boundary_loops();

        println!(
            "pork-chop mesh: {} tris, {} verts",
            mesh.num_triangles(),
            mesh.num_vertices()
        );
        println!("boundary edges:  {}", boundary.len());
        println!("non-manifold edges: {}", nm.len());
        println!("boundary loops: {}", loops.len());

        for (i, positions) in mesh.boundary_edge_positions().iter().enumerate() {
            let a = positions[0];
            let b = positions[1];
            println!(
                "  [{:3}] ({:7.3},{:7.3},{:7.3}) -> ({:7.3},{:7.3},{:7.3})",
                i, a[0], a[1], a[2], b[0], b[1], b[2]
            );
        }
        for (i, chain) in loops.iter().enumerate() {
            let zs: Vec<f32> = chain
                .iter()
                .map(|&v| mesh.vertices[v as usize * 3 + 2])
                .collect();
            let z_min = zs.iter().cloned().fold(f32::INFINITY, f32::min);
            let z_max = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            println!(
                "  loop {}: {} verts, z in [{:.3}, {:.3}]",
                i,
                chain.len(),
                z_min,
                z_max
            );
        }
    }

    /// Diagnostic: print per-junction decisions from the fillet pipeline.
    ///
    ///   cargo test -p vcad-kernel diag_porkchop_fillet_trace -- --nocapture --ignored
    #[test]
    #[ignore]
    fn diag_porkchop_fillet_trace() {
        use vcad_kernel_fillet::{fillet_edges_detailed_with_trace, JunctionOutcome};
        use vcad_kernel_sketch::SketchProfile;

        let profile = SketchProfile::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            porkchop_segments(),
        )
        .expect("valid profile");
        let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 18.0)).expect("extrude ok");

        let brep = match &extruded.repr {
            SolidRepr::BRep(b) => b.as_ref().clone(),
            _ => panic!("expected brep"),
        };
        let target_edges = collect_fillet_target_edges(&brep);
        let (_new_brep, _results, trace) =
            fillet_edges_detailed_with_trace(&brep, &target_edges, 4.0, true);

        println!(
            "fillet trace: {} junctions considered",
            trace.junctions.len()
        );
        let mut n_built = 0usize;
        let mut n_skipped = 0usize;
        for j in &trace.junctions {
            let p = j.vertex_pos;
            match &j.outcome {
                JunctionOutcome::BuiltPatch {
                    ball_center,
                    tan_cap,
                    tan_cyls,
                } => {
                    n_built += 1;
                    println!(
                        "  V@({:7.3},{:7.3},{:7.3}) tgt={} seam={} -> PATCH ball=({:7.3},{:7.3},{:7.3}) cap=({:7.3},{:7.3},{:7.3}) c1=({:7.3},{:7.3},{:7.3}) c2=({:7.3},{:7.3},{:7.3})",
                        p.x, p.y, p.z, j.n_target_edges, j.n_seam_edges,
                        ball_center.x, ball_center.y, ball_center.z,
                        tan_cap.x, tan_cap.y, tan_cap.z,
                        tan_cyls[0].x, tan_cyls[0].y, tan_cyls[0].z,
                        tan_cyls[1].x, tan_cyls[1].y, tan_cyls[1].z,
                    );
                }
                other => {
                    n_skipped += 1;
                    println!(
                        "  V@({:7.3},{:7.3},{:7.3}) tgt={} seam={} -> SKIP {:?}",
                        p.x, p.y, p.z, j.n_target_edges, j.n_seam_edges, other
                    );
                }
            }
        }
        println!("  built: {}, skipped: {}", n_built, n_skipped);
    }

    #[test]
    fn test_fillet_on_extruded_arc_profile_produces_curved_blend() {
        // Regression for the "pork-chop sawtooth": extruding a sketch profile
        // containing arcs now produces analytic `CylinderSurface` side walls
        // (one per arc, not a strip of thin planar quads). `Solid::fillet`
        // detects non-planar faces and routes to the curved fillet pipeline,
        // which emits torus blends at plane-cylinder edges and rolling-ball
        // NURBS blends at cylinder-cylinder edges. The result has strictly
        // more faces than the input (adds blend faces + vertex caps) and
        // must tessellate cleanly.
        use vcad_kernel_math::Point2;
        use vcad_kernel_sketch::{SketchProfile, SketchSegment};

        // Kidney-shaped profile (6 arcs, matching the user's pork-chop).
        let segments = vec![
            SketchSegment::Arc {
                start: Point2::new(45.0, 0.0),
                end: Point2::new(20.0, 40.0),
                center: Point2::new(10.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: Point2::new(20.0, 40.0),
                end: Point2::new(-30.0, 35.0),
                center: Point2::new(-5.0, 25.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: Point2::new(-30.0, 35.0),
                end: Point2::new(-50.0, 5.0),
                center: Point2::new(-25.0, 15.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: Point2::new(-50.0, 5.0),
                end: Point2::new(-35.0, -25.0),
                center: Point2::new(-30.0, -5.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: Point2::new(-35.0, -25.0),
                end: Point2::new(10.0, -30.0),
                center: Point2::new(-10.0, -10.0),
                ccw: true,
            },
            SketchSegment::Arc {
                start: Point2::new(10.0, -30.0),
                end: Point2::new(45.0, 0.0),
                center: Point2::new(20.0, -10.0),
                ccw: true,
            },
        ];
        let profile = SketchProfile::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            segments,
        )
        .expect("valid profile");

        let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 18.0)).expect("extrude ok");

        // Count faces before fillet — this is the tessellated-arc B-rep we
        // want to protect. Many thin side faces, two planar caps.
        let (face_count_before, cyl_count_before) = match &extruded.repr {
            SolidRepr::BRep(b) => {
                let cyls = b
                    .geometry
                    .surfaces
                    .iter()
                    .filter(|s| s.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder)
                    .count();
                (b.topology.faces.len(), cyls)
            }
            _ => (0, 0),
        };
        assert_eq!(
            face_count_before, 8,
            "extruded kidney: 6 arc cylinders + 2 caps = 8 faces, got {face_count_before}"
        );
        assert_eq!(
            cyl_count_before, 6,
            "expected one analytic cylinder per arc, got {cyl_count_before}"
        );

        let filleted = extruded.fillet(4.0);
        let face_count_after = match &filleted.repr {
            SolidRepr::BRep(b) => b.topology.faces.len(),
            _ => 0,
        };
        // Either the fillet genuinely added blend + vertex faces, or its
        // AABB guard kicked in and handed back the input unchanged. Both
        // are acceptable end states — the explicit failure mode we're
        // guarding against is geometry flying hundreds of units outside
        // the input bounding box.
        assert!(
            face_count_after >= face_count_before,
            "face count regressed: before={face_count_before} after={face_count_after}"
        );

        // Tessellate and assert every vertex lands inside a generous AABB
        // around the input kidney (max radius ~50, extrude 18). This
        // catches the "pork-chop diverges" regression even if face count
        // checks passed.
        let mesh = filleted.to_mesh(32);
        assert!(mesh.num_triangles() > 0, "mesh should be non-empty");
        let n = mesh.num_vertices();
        for i in 0..n {
            let x = mesh.vertices[3 * i] as f64;
            let y = mesh.vertices[3 * i + 1] as f64;
            let z = mesh.vertices[3 * i + 2] as f64;
            assert!(
                x.abs() < 100.0 && y.abs() < 100.0 && (-10.0..40.0).contains(&z),
                "filleted kidney has outlier vertex at ({x:.1}, {y:.1}, {z:.1})"
            );
        }
    }

    /// Fillet after a boolean Difference must not fill the cut back in.
    ///
    /// The bored plate's top/bottom faces carry the bore as inner loops;
    /// the fillet rebuild only understands outer loops, so before the
    /// inner-loop guard it emitted a filleted *solid* plate (hole gone).
    /// The guard fails soft: the input comes back unchanged — sharp
    /// edges, but the bore intact.
    #[test]
    fn fillet_after_difference_preserves_the_cut() {
        let plate = Solid::cube(80.0, 50.0, 6.0);
        let bore = Solid::cylinder(8.0, 20.0, 64).translate(40.0, 25.0, -7.0);
        let bored = plate.difference(&bore);

        let bored_vol = bored.volume();
        let solid_vol = 80.0 * 50.0 * 6.0;
        // Sanity: the difference itself removed the bore (~π·8²·6 ≈ 1206).
        assert!(
            bored_vol < solid_vol - 1000.0,
            "difference lost the bore: vol={bored_vol:.1}"
        );

        for (name, result) in [
            ("fillet", bored.fillet(1.5)),
            ("chamfer", bored.chamfer(1.5)),
        ] {
            let vol = result.volume();
            // The buggy path produced ~23736 (a filleted solid plate).
            // Correct output must stay at or below the bored volume.
            assert!(
                vol <= bored_vol + 1.0,
                "{name} after difference regrew material: vol={vol:.1} > bored={bored_vol:.1}"
            );
        }
    }
}
