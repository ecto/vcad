//! Frozen tessellation for the differentiable seam.
//!
//! A derivative dx/dθ of tessellated node positions with respect to a CAD
//! parameter is only meaningful if the mesh connectivity **and** the sample
//! pattern are byte-identical at θ and θ ± h, with identical vertex ordering,
//! so that node `i` corresponds to node `i` across evaluations. The stock
//! tessellator cannot guarantee this: its discrete choices (fan-center
//! selection, radius-dependent segment counts, full-vs-partial cylinder
//! detection) are functions of θ and may flip under perturbation, permuting
//! vertices and reseeding samples.
//!
//! This module splits tessellation into two phases:
//!
//! 1. **Capture** ([`capture_plan`]): run the stock tessellator once at the
//!    base parameter value and record a [`FrozenPlan`] — for every mesh node
//!    a [`NodeRecipe`] saying *how* to recompute its position from a B-rep
//!    (read a topology vertex, or evaluate a face's surface at a frozen
//!    `(u, v)`), plus the frozen triangle connectivity and a
//!    [`TopologySignature`] of the B-rep the plan was captured from.
//! 2. **Evaluate** ([`evaluate_plan`]): given a *rebuilt* B-rep (same model,
//!    perturbed parameter), first assert the topology signature is unchanged
//!    — a changed signature means the perturbation crossed a topology change
//!    (a subgradient) and node correspondence is meaningless, so this is a
//!    hard error, never a silent wrong answer — then re-emit node positions
//!    under the frozen recipes in `f64` (the stock [`TriangleMesh`] is `f32`,
//!    which is far too coarse for a central-difference oracle at `h ≈ 1e-6`).
//!
//! Node recipes reference topology entities by *canonical traversal index*
//! (position in a deterministic walk of shells → faces → loops → vertices),
//! not by slotmap key, so correspondence survives rebuilding the model from
//! scratch as long as the construction code takes the same branches.

use std::collections::HashMap;

use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind};
use vcad_kernel_math::{Point2, Point3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation, VertexId};

use crate::TessellationParams;

/// Absolute tolerance (mm) for matching a tessellated `f32` vertex back to a
/// topology vertex during capture. Matches the stock weld quantum: tight
/// enough to only capture true coincidences, loose enough to absorb `f32`
/// rounding of the stock tessellator's output.
const VERTEX_MATCH_TOL: f64 = 1e-3;

/// Quantization step (mm) for deduplicating coincident nodes across faces
/// during capture (same value the stock `weld_coincident_vertices` uses).
const DEDUP_QUANTUM: f64 = 1e-3;

/// Tolerance (mm) for geometric correspondence when a plan is evaluated
/// against a rebuilt B-rep: an entity must lie within this distance of the
/// node's capture-time position. Far above the O(h · velocity) motion of a
/// derivative step, far below feature separation.
const MATCH_TOL: f64 = 1e-3;

/// Cheap structural fingerprint of a B-rep's combinatorics.
///
/// Two B-reps with equal signatures have the same entity counts and the
/// same *multiset* of per-face descriptors (surface kind, orientation, loop
/// lengths). The hash is deliberately order-invariant: the boolean pipeline
/// is internally order-nondeterministic (hash-map iteration decides e.g.
/// which cap face is sewn first), so an isomorphic rebuild may enumerate
/// faces in a different order — that is not a topology change. Entity
/// *correspondence* across rebuilds is recovered geometrically by
/// [`evaluate_plan`], not from traversal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologySignature {
    /// Number of vertices.
    pub vertices: usize,
    /// Number of edges.
    pub edges: usize,
    /// Number of faces (across all shells of the solid).
    pub faces: usize,
    /// Number of loops.
    pub loops: usize,
    /// Order-invariant FNV-1a hash over the sorted multiset of per-face
    /// descriptors (surface kind, orientation, outer/inner loop lengths).
    pub connectivity_hash: u64,
}

/// Canonical traversal of a B-rep solid: faces in shell order (outer shell
/// first, then void shells), vertices in first-visit order along that walk.
///
/// This is the index space [`NodeRecipe`] uses to survive rebuilds.
#[derive(Debug, Clone)]
pub struct CanonicalIndex {
    /// Faces in traversal order.
    pub faces: Vec<FaceId>,
    /// Vertices in first-visit order.
    pub vertices: Vec<VertexId>,
    vertex_index: HashMap<VertexId, u32>,
}

impl CanonicalIndex {
    /// Canonical index of a topology vertex, if visited by the walk.
    pub fn vertex_position(&self, v: VertexId) -> Option<u32> {
        self.vertex_index.get(&v).copied()
    }
}

/// How to recompute one mesh node's position from a (rebuilt) B-rep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeRecipe {
    /// The node coincides with a topology vertex; read its position.
    /// `vertex` is a canonical first-visit index into [`CanonicalIndex`].
    TopoVertex {
        /// Canonical vertex index.
        vertex: u32,
    },
    /// The node is a surface sample at frozen parameters: evaluate the
    /// surface of the face at canonical traversal index `face` at `(u, v)`.
    SurfaceUv {
        /// Canonical face traversal index.
        face: u32,
        /// Frozen u parameter.
        u: f64,
        /// Frozen v parameter.
        v: f64,
    },
    /// The node lies on the intersection of two faces' surfaces (a trim
    /// boundary that is not carried by a topology vertex, e.g. a cap-rim
    /// ring). Its position is the Newton solution of
    /// `{g_a(x) = 0, g_b(x) = 0, t·(x − anchor) = 0}` where `t` is the
    /// intersection tangent — the frozen-parameter branch choice — so the
    /// node tracks the moving trim no matter *which* of the two surfaces θ
    /// moves. The per-face `(u, v)` are the capture-time samples, kept for
    /// geometric face matching across rebuilds.
    Boundary {
        /// Canonical traversal index of the first face.
        face_a: u32,
        /// Capture-time u on face a.
        ua: f64,
        /// Capture-time v on face a.
        va: f64,
        /// Canonical traversal index of the second face.
        face_b: u32,
        /// Capture-time u on face b.
        ub: f64,
        /// Capture-time v on face b.
        vb: f64,
    },
}

/// A frozen tessellation: recipes for every node, frozen connectivity, and
/// the topology signature of the B-rep the plan was captured from.
#[derive(Debug, Clone)]
pub struct FrozenPlan {
    /// Signature of the capture-time B-rep; enforced on every evaluation.
    pub signature: TopologySignature,
    /// Per-node recompute recipes.
    pub nodes: Vec<NodeRecipe>,
    /// Node positions at the capture-time parameter value. Used to recover
    /// entity correspondence geometrically when a plan is evaluated against
    /// a rebuilt B-rep whose enumeration order differs (the boolean
    /// pipeline is order-nondeterministic), and as the anchor that
    /// perturbed positions must stay near.
    pub base_positions: Vec<Point3>,
    /// Triangles as indices into `nodes`. Winding is inherited from the
    /// stock tessellator (outward-facing).
    pub triangles: Vec<[u32; 3]>,
}

/// A frozen tessellation evaluated against a concrete B-rep: `f64` node
/// positions plus the plan's frozen connectivity.
#[derive(Debug, Clone)]
pub struct FrozenMesh {
    /// Node positions (`f64`, same order as the plan's `nodes`).
    pub positions: Vec<Point3>,
    /// Triangles (copied from the plan).
    pub triangles: Vec<[u32; 3]>,
}

/// Errors from frozen tessellation.
#[derive(Debug, Clone)]
pub enum FrozenError {
    /// The B-rep's topology signature does not match the plan's: the
    /// parameter perturbation crossed a topology change and node
    /// correspondence is lost. This is deliberately an error — returning a
    /// mesh here would silently produce garbage derivatives.
    TopologyChanged {
        /// Signature recorded in the plan.
        expected: TopologySignature,
        /// Signature of the B-rep being evaluated.
        actual: TopologySignature,
    },
    /// A face's surface kind has no frozen-capture support yet (its
    /// tessellation emits samples we cannot invert to `(u, v)`).
    UnsupportedSurface {
        /// The offending surface kind.
        kind: SurfaceKind,
    },
    /// A face's topology has no frozen-capture support yet and dropping it
    /// would silently freeze a non-watertight mesh with a wrong volume.
    UnsupportedFace {
        /// What made the face unsupported.
        reason: &'static str,
    },
    /// A node recipe referenced an entity missing from the B-rep (internal
    /// inconsistency; should be prevented by the signature check).
    RecipeOutOfRange,
    /// No entity of the rebuilt B-rep lies within matching tolerance of a
    /// node's capture-time position: the perturbation moved geometry too
    /// far (or rebuilt something else entirely), so node correspondence is
    /// lost. Like a signature mismatch, this is a hard error.
    CorrespondenceLost {
        /// Index of the node whose match failed.
        node: usize,
        /// Distance (mm) to the nearest candidate.
        distance: f64,
    },
    /// The Newton solve for a [`NodeRecipe::Boundary`] node failed to
    /// converge (tangent degenerate, singular system, or divergence): the
    /// two surfaces no longer intersect cleanly near the anchor.
    BoundarySolveFailed {
        /// Index of the offending node.
        node: usize,
    },
}

impl std::fmt::Display for FrozenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrozenError::TopologyChanged { expected, actual } => write!(
                f,
                "topology changed across parameter perturbation (expected {expected:?}, got {actual:?}); \
                 the derivative step crossed a subgradient"
            ),
            FrozenError::UnsupportedSurface { kind } => {
                write!(f, "frozen capture unsupported for surface kind {kind:?}")
            }
            FrozenError::UnsupportedFace { reason } => {
                write!(f, "frozen capture unsupported for face: {reason}")
            }
            FrozenError::RecipeOutOfRange => write!(f, "node recipe out of range for this B-rep"),
            FrozenError::CorrespondenceLost { node, distance } => write!(
                f,
                "no entity within matching tolerance of node {node}'s capture-time position \
                 (nearest at {distance:.3e} mm); node correspondence across the perturbation is lost"
            ),
            FrozenError::BoundarySolveFailed { node } => write!(
                f,
                "boundary node {node}: Newton solve on the two-surface intersection failed to \
                 converge near the capture-time anchor"
            ),
        }
    }
}

impl std::error::Error for FrozenError {}

/// Build the canonical traversal for a B-rep solid.
pub fn canonical_index(brep: &BRepSolid) -> CanonicalIndex {
    let topo = &brep.topology;
    let solid = &topo.solids[brep.solid_id];

    let mut faces = Vec::new();
    let mut vertices = Vec::new();
    let mut vertex_index: HashMap<VertexId, u32> = HashMap::new();

    let shells = std::iter::once(solid.outer_shell).chain(solid.void_shells.iter().copied());
    for shell_id in shells {
        for &face_id in &topo.shells[shell_id].faces {
            faces.push(face_id);
            let face = &topo.faces[face_id];
            let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loops {
                for he in topo.loop_half_edges(loop_id) {
                    let v = topo.half_edges[he].origin;
                    if let std::collections::hash_map::Entry::Vacant(e) = vertex_index.entry(v) {
                        e.insert(vertices.len() as u32);
                        vertices.push(v);
                    }
                }
            }
        }
    }

    CanonicalIndex {
        faces,
        vertices,
        vertex_index,
    }
}

fn kind_tag(kind: SurfaceKind) -> u8 {
    match kind {
        SurfaceKind::Plane => 0,
        SurfaceKind::Cylinder => 1,
        SurfaceKind::Cone => 2,
        SurfaceKind::Sphere => 3,
        SurfaceKind::Torus => 4,
        SurfaceKind::BSpline => 5,
        SurfaceKind::Bilinear => 6,
    }
}

/// Compute the topology signature of a B-rep solid.
pub fn topology_signature(brep: &BRepSolid) -> TopologySignature {
    let ci = canonical_index(brep);
    signature_with_index(brep, &ci)
}

/// Compute the topology signature using an existing canonical traversal.
pub fn signature_with_index(brep: &BRepSolid, ci: &CanonicalIndex) -> TopologySignature {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    fn fnv(hash: &mut u64, bytes: &[u8]) {
        for &b in bytes {
            *hash ^= b as u64;
            *hash = hash.wrapping_mul(FNV_PRIME);
        }
    }

    let topo = &brep.topology;
    let mut loop_count = 0usize;
    // Count edges reachable from the canonical walk, not the whole slotmap
    // arena: the boolean pipeline can leave orphaned arena entities behind
    // in rebuild-order-dependent numbers, which must not perturb the
    // signature of the (isomorphic) solid.
    let mut reachable_edges: std::collections::HashSet<vcad_kernel_topo::EdgeId> =
        std::collections::HashSet::new();
    let mut face_hashes: Vec<u64> = Vec::with_capacity(ci.faces.len());
    for &face_id in &ci.faces {
        let face = &topo.faces[face_id];
        let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for loop_id in loops {
            for he in topo.loop_half_edges(loop_id) {
                if let Some(e) = topo.half_edges[he].edge {
                    reachable_edges.insert(e);
                }
            }
        }
        let surface = &brep.geometry.surfaces[face.surface_index];
        let mut h = FNV_OFFSET;
        fnv(
            &mut h,
            &[
                kind_tag(surface.surface_type()),
                matches!(face.orientation, Orientation::Reversed) as u8,
            ],
        );
        loop_count += 1 + face.inner_loops.len();
        fnv(
            &mut h,
            &(topo.loop_len(face.outer_loop) as u32).to_le_bytes(),
        );
        let mut inner_lens: Vec<u32> = face
            .inner_loops
            .iter()
            .map(|&l| topo.loop_len(l) as u32)
            .collect();
        inner_lens.sort_unstable();
        fnv(&mut h, &(inner_lens.len() as u32).to_le_bytes());
        for len in inner_lens {
            fnv(&mut h, &len.to_le_bytes());
        }
        face_hashes.push(h);
    }
    // Sorting makes the combined hash independent of face enumeration order.
    face_hashes.sort_unstable();
    let mut hash = FNV_OFFSET;
    for fh in face_hashes {
        fnv(&mut hash, &fh.to_le_bytes());
    }

    TopologySignature {
        vertices: ci.vertices.len(),
        edges: reachable_edges.len(),
        faces: ci.faces.len(),
        loops: loop_count,
        connectivity_hash: hash,
    }
}

/// Invert a point known to lie on `surface` back to `(u, v)` parameters.
///
/// Only the surface kinds needed by the M0–M2 differentiable-seam milestones
/// are supported; others return [`FrozenError::UnsupportedSurface`].
fn invert_uv(surface: &dyn Surface, p: &Point3) -> Result<Point2, FrozenError> {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            let plane = surface.as_any().downcast_ref::<Plane>().ok_or(
                FrozenError::UnsupportedSurface {
                    kind: SurfaceKind::Plane,
                },
            )?;
            Ok(plane.project(p))
        }
        SurfaceKind::Cylinder => {
            let cyl = surface.as_any().downcast_ref::<CylinderSurface>().ok_or(
                FrozenError::UnsupportedSurface {
                    kind: SurfaceKind::Cylinder,
                },
            )?;
            let d = *p - cyl.center;
            let v = d.dot(cyl.axis.as_ref());
            let y = cyl.y_dir();
            let u = d.dot(y).atan2(d.dot(cyl.ref_dir.as_ref()));
            Ok(Point2::new(u.rem_euclid(2.0 * std::f64::consts::PI), v))
        }
        SurfaceKind::Sphere => {
            let sph = surface.as_any().downcast_ref::<SphereSurface>().ok_or(
                FrozenError::UnsupportedSurface {
                    kind: SurfaceKind::Sphere,
                },
            )?;
            let d = *p - sph.center;
            let r = sph.radius.abs().max(f64::MIN_POSITIVE);
            let v = (d.dot(sph.axis.as_ref()) / r).clamp(-1.0, 1.0).asin();
            let y = sph.y_dir();
            let u = d.dot(y).atan2(d.dot(sph.ref_dir.as_ref()));
            Ok(Point2::new(u.rem_euclid(2.0 * std::f64::consts::PI), v))
        }
        kind => Err(FrozenError::UnsupportedSurface { kind }),
    }
}

/// Preference between two same-surface `SurfaceUv` recipes for a coincident
/// node: a sample on a curved surface pins two position DOF, a sample on a
/// plane pins one. (Coincident nodes on *distinct* surfaces are upgraded to
/// [`NodeRecipe::Boundary`] instead, which tracks the moving trim no matter
/// which surface carries θ; a matched topology vertex always wins.)
fn kind_rank(kind: SurfaceKind) -> u8 {
    match kind {
        SurfaceKind::Plane => 0,
        _ => 1,
    }
}

/// Solve a 3×3 linear system by Cramer's rule; `None` when near-singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| -> f64 {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    let scale = m
        .iter()
        .map(|row| row.iter().map(|v| v.abs()).fold(0.0, f64::max))
        .fold(0.0, f64::max);
    // Relative singularity threshold: the determinant's roundoff floor is
    // ~ε·scale³, and a legitimately independent system (the tangent
    // pre-check already rejected near-parallel gradients) has |det| far
    // above 1e-12·scale³ — so this margin rejects ill-conditioned systems
    // early instead of letting Newton amplify noise and diverge.
    if d.abs() < 1e-12 * scale.powi(3).max(f64::MIN_POSITIVE) {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut mk = m;
        for row in 0..3 {
            mk[row][k] = b[row];
        }
        *o = det(&mk) / d;
    }
    Some(out)
}

/// Refine a boundary node onto the intersection of two surfaces: Newton on
/// `{g_a(x) = 0, g_b(x) = 0, t·(x − anchor) = 0}`, where `t` is the
/// intersection tangent (`∇g_a × ∇g_b`) at the anchor — the frozen-branch
/// convention that pins the node's curve parameter.
///
/// Returns `None` if either surface lacks an implicit form, the tangent is
/// degenerate, the system is singular, or Newton fails to converge.
pub fn refine_boundary_point(a: &dyn Surface, b: &dyn Surface, anchor: &Point3) -> Option<Point3> {
    let (_, ga0) = vcad_kernel_geom::implicit_form(a, anchor)?;
    let (_, gb0) = vcad_kernel_geom::implicit_form(b, anchor)?;
    let t = ga0.cross(gb0);
    let t_norm = t.norm();
    if t_norm < 1e-12 * (ga0.norm() * gb0.norm()).max(f64::MIN_POSITIVE) {
        return None; // (near-)tangent surfaces: no well-defined curve
    }
    let t = t / t_norm;

    let mut x = *anchor;
    for _ in 0..30 {
        let (ga, na) = vcad_kernel_geom::implicit_form(a, &x)?;
        let (gb, nb) = vcad_kernel_geom::implicit_form(b, &x)?;
        let gt = t.dot(x - *anchor);
        // Distance-like residuals (quadric g is not a signed distance).
        let ra = ga / na.norm().max(f64::MIN_POSITIVE);
        let rb = gb / nb.norm().max(f64::MIN_POSITIVE);
        if ra.abs().max(rb.abs()).max(gt.abs()) < 1e-12 * (1.0 + x.coords_norm()) {
            return Some(x);
        }
        let delta = solve3(
            [[na.x, na.y, na.z], [nb.x, nb.y, nb.z], [t.x, t.y, t.z]],
            [-ga, -gb, -gt],
        )?;
        x = Point3::new(x.x + delta[0], x.y + delta[1], x.z + delta[2]);
    }
    None
}

/// Small helper: Euclidean norm of a point's coordinates (for relative
/// convergence thresholds).
trait CoordsNorm {
    fn coords_norm(&self) -> f64;
}
impl CoordsNorm for Point3 {
    fn coords_norm(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

/// Per-face tessellation used by capture. Mirrors the face dispatch of the
/// primary `tessellate_brep` entry point: degenerate single-vertex circular
/// cap loops (primitive cylinder caps, boolean-cut caps with holes) go
/// through the shared `tessellate_degenerate_cap` helper; everything else
/// through the stock per-face tessellator.
fn tessellate_face_for_capture(
    brep: &BRepSolid,
    face_id: FaceId,
    params: &TessellationParams,
) -> crate::TriangleMesh {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let reversed = face.orientation == Orientation::Reversed;

    if surface.surface_type() == SurfaceKind::Plane && topo.loop_len(face.outer_loop) <= 1 {
        return crate::tessellate_degenerate_cap(brep, face_id, params, reversed);
    }

    crate::tessellate_face(topo, &brep.geometry, face_id, params)
}

/// Capture a frozen tessellation plan from a B-rep at the base parameter
/// value.
///
/// Runs the stock per-face tessellator once, then classifies every emitted
/// vertex: nodes coinciding with a topology vertex become
/// [`NodeRecipe::TopoVertex`]; all others are inverted to frozen `(u, v)`
/// samples on their face's surface ([`NodeRecipe::SurfaceUv`]). Coincident
/// nodes from adjacent faces are merged (same quantum as the stock weld) so
/// the frozen mesh shares vertices across face seams.
pub fn capture_plan(
    brep: &BRepSolid,
    params: &TessellationParams,
) -> Result<FrozenPlan, FrozenError> {
    let topo = &brep.topology;
    let ci = canonical_index(brep);
    let signature = signature_with_index(brep, &ci);

    let mut nodes: Vec<NodeRecipe> = Vec::new();
    let mut base_positions: Vec<Point3> = Vec::new();
    let mut node_kinds: Vec<SurfaceKind> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    // Spatial hash for cross-face dedup. Lookup probes the 27 neighboring
    // cells and merges on true distance, so two computations of the same
    // logical node that straddle a cell boundary (f32 drift between two
    // faces' paths) still merge — a straddle-split node would silently
    // dodge the recipe-priority promotion below.
    let mut dedup: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let cell = |p: &Point3| -> [i64; 3] {
        [
            (p.x / DEDUP_QUANTUM).round() as i64,
            (p.y / DEDUP_QUANTUM).round() as i64,
            (p.z / DEDUP_QUANTUM).round() as i64,
        ]
    };
    let find_near =
        |dedup: &HashMap<[i64; 3], Vec<u32>>, positions: &[Point3], p: &Point3| -> Option<u32> {
            let c = cell(p);
            let mut best: Option<(u32, f64)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let key = [c[0] + dx, c[1] + dy, c[2] + dz];
                        for &idx in dedup.get(&key).into_iter().flatten() {
                            let d2 = (positions[idx as usize] - *p).norm_squared();
                            if d2 < DEDUP_QUANTUM * DEDUP_QUANTUM
                                && best.map(|(_, bd)| d2 < bd).unwrap_or(true)
                            {
                                best = Some((idx, d2));
                            }
                        }
                    }
                }
            }
            best.map(|(idx, _)| idx)
        };

    for (fi, &face_id) in ci.faces.iter().enumerate() {
        let face = &topo.faces[face_id];
        let surface = brep.geometry.surfaces[face.surface_index].as_ref();
        let kind = surface.surface_type();
        let mesh = tessellate_face_for_capture(brep, face_id, params);
        if mesh.indices.is_empty() {
            // A degenerate-cap loop that carries holes needs the CDT path
            // `tessellate_brep` has inline but the per-face tessellator does
            // not; silently dropping the face would freeze a non-watertight
            // mesh with a plausibly wrong volume. Fail loudly instead.
            if topo.loop_len(face.outer_loop) <= 1 && !face.inner_loops.is_empty() {
                return Err(FrozenError::UnsupportedFace {
                    reason: "degenerate single-vertex cap loop with inner loops (holes)",
                });
            }
            continue;
        }

        // Topology vertices bounding this face, with canonical indices.
        let mut loop_verts: Vec<(u32, Point3)> = Vec::new();
        let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for loop_id in loops {
            for he in topo.loop_half_edges(loop_id) {
                let vid = topo.half_edges[he].origin;
                let idx = ci
                    .vertex_position(vid)
                    .expect("loop vertex visited by canonical walk");
                loop_verts.push((idx, topo.vertices[vid].point));
            }
        }

        // Only classify vertices actually referenced by triangles.
        let mut used = vec![false; mesh.num_vertices()];
        for &i in &mesh.indices {
            used[i as usize] = true;
        }

        let mut local_map: Vec<u32> = vec![u32::MAX; mesh.num_vertices()];
        for (v, mapped) in local_map.iter_mut().enumerate() {
            if !used[v] {
                continue;
            }
            let p = Point3::new(
                mesh.vertices[3 * v] as f64,
                mesh.vertices[3 * v + 1] as f64,
                mesh.vertices[3 * v + 2] as f64,
            );

            // Classify: coincident topology vertex, else frozen (u, v) sample.
            let mut best: Option<(u32, f64)> = None;
            for &(idx, q) in &loop_verts {
                let d2 = (p - q).norm_squared();
                if d2 < VERTEX_MATCH_TOL * VERTEX_MATCH_TOL
                    && best.map(|(_, bd)| d2 < bd).unwrap_or(true)
                {
                    best = Some((idx, d2));
                }
            }
            let (recipe, position) = match best {
                Some((idx, _)) => (
                    NodeRecipe::TopoVertex { vertex: idx },
                    topo.vertices[ci.vertices[idx as usize]].point,
                ),
                None => {
                    let uv = invert_uv(surface, &p)?;
                    // Canonical base position: the exact f64 surface
                    // evaluation at the frozen (u, v), not the f32 point
                    // the stock tessellator emitted — future evaluations
                    // reproduce it bit-for-bit.
                    (
                        NodeRecipe::SurfaceUv {
                            face: fi as u32,
                            u: uv.x,
                            v: uv.y,
                        },
                        surface.evaluate(uv),
                    )
                }
            };

            let idx = match find_near(&dedup, &base_positions, &position) {
                Some(existing) => {
                    // Merge coincident contributions. A matched topology
                    // vertex always wins; two samples on *distinct*
                    // surfaces mark a trim-boundary node and upgrade to a
                    // `Boundary` recipe (Newton-tracked intersection, so
                    // the node follows the moving trim regardless of which
                    // surface carries θ); same-surface duplicates keep the
                    // higher-ranked sample.
                    let e = existing as usize;
                    let merged: Option<(NodeRecipe, Point3, SurfaceKind)> = match (nodes[e], recipe)
                    {
                        (NodeRecipe::TopoVertex { .. }, _) => None,
                        (_, NodeRecipe::TopoVertex { .. }) => Some((recipe, position, kind)),
                        (NodeRecipe::Boundary { .. }, _) | (_, NodeRecipe::Boundary { .. }) => None,
                        (
                            NodeRecipe::SurfaceUv {
                                face: fa,
                                u: ua,
                                v: va,
                            },
                            NodeRecipe::SurfaceUv {
                                face: fb,
                                u: ub,
                                v: vb,
                            },
                        ) => {
                            let sidx_a = topo.faces[ci.faces[fa as usize]].surface_index;
                            let sidx_b = topo.faces[ci.faces[fb as usize]].surface_index;
                            let refined = (sidx_a != sidx_b)
                                .then(|| {
                                    let sa = brep.geometry.surfaces[sidx_a].as_ref();
                                    let sb = brep.geometry.surfaces[sidx_b].as_ref();
                                    refine_boundary_point(sa, sb, &base_positions[e])
                                })
                                .flatten();
                            match refined {
                                Some(p) => Some((
                                    NodeRecipe::Boundary {
                                        face_a: fa,
                                        ua,
                                        va,
                                        face_b: fb,
                                        ub,
                                        vb,
                                    },
                                    p,
                                    node_kinds[e],
                                )),
                                // Same surface, tangent surfaces, or no
                                // implicit form: keep the higher rank.
                                None if kind_rank(kind) > kind_rank(node_kinds[e]) => {
                                    Some((recipe, position, kind))
                                }
                                None => None,
                            }
                        }
                    };
                    if let Some((r, p, k)) = merged {
                        nodes[e] = r;
                        base_positions[e] = p;
                        node_kinds[e] = k;
                    }
                    existing
                }
                None => {
                    let new_idx = nodes.len() as u32;
                    nodes.push(recipe);
                    base_positions.push(position);
                    node_kinds.push(kind);
                    dedup.entry(cell(&position)).or_default().push(new_idx);
                    new_idx
                }
            };
            *mapped = idx;
        }

        for tri in mesh.indices.as_chunks::<3>().0 {
            let a = local_map[tri[0] as usize];
            let b = local_map[tri[1] as usize];
            let c = local_map[tri[2] as usize];
            if a == b || b == c || c == a {
                continue; // degenerate after cross-face dedup
            }
            triangles.push([a, b, c]);
        }
    }

    Ok(FrozenPlan {
        signature,
        nodes,
        base_positions,
        triangles,
    })
}

/// Evaluate a frozen plan against a (rebuilt) B-rep, producing `f64` node
/// positions under the frozen recipes.
///
/// Errors with [`FrozenError::TopologyChanged`] if the B-rep's signature
/// differs from the plan's — the invariant that separates a correct seam
/// from a plausible wrong one.
///
/// Entity correspondence is *geometric*: the boolean pipeline enumerates an
/// isomorphic result in a nondeterministic order (its cap faces can swap
/// between two builds), so traversal indices recorded at capture cannot be
/// trusted across rebuilds. Instead, each `TopoVertex` node resolves to the
/// rebuilt vertex nearest its capture-time position, and each face slot
/// used by `SurfaceUv` nodes resolves to the rebuilt face whose surface,
/// evaluated at the frozen `(u, v)`, lands nearest the capture-time
/// position. A nearest match farther than the tolerance is
/// [`FrozenError::CorrespondenceLost`].
pub fn evaluate_plan(brep: &BRepSolid, plan: &FrozenPlan) -> Result<FrozenMesh, FrozenError> {
    let ci = canonical_index(brep);
    let actual = signature_with_index(brep, &ci);
    if actual != plan.signature {
        return Err(FrozenError::TopologyChanged {
            expected: plan.signature,
            actual,
        });
    }
    if plan.base_positions.len() != plan.nodes.len() {
        return Err(FrozenError::RecipeOutOfRange);
    }

    let topo = &brep.topology;
    // Face slot (capture-time traversal index) → matched rebuilt face.
    let mut face_match: HashMap<u32, FaceId> = HashMap::new();
    // Resolve a capture-time face slot to a rebuilt face: the face whose
    // surface, evaluated at the frozen (u, v), lands nearest the node's
    // capture-time anchor.
    fn resolve_face(
        brep: &BRepSolid,
        ci: &CanonicalIndex,
        face_match: &mut HashMap<u32, FaceId>,
        slot: u32,
        uv: Point2,
        anchor: &Point3,
        node: usize,
    ) -> Result<FaceId, FrozenError> {
        if let Some(&f) = face_match.get(&slot) {
            return Ok(f);
        }
        let topo = &brep.topology;
        let mut best: Option<(FaceId, f64)> = None;
        for &fid in &ci.faces {
            let surface = &brep.geometry.surfaces[topo.faces[fid].surface_index];
            let d2 = (surface.evaluate(uv) - *anchor).norm_squared();
            if best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                best = Some((fid, d2));
            }
        }
        let (fid, d2) = best.ok_or(FrozenError::RecipeOutOfRange)?;
        if d2 > MATCH_TOL * MATCH_TOL {
            return Err(FrozenError::CorrespondenceLost {
                node,
                distance: d2.sqrt(),
            });
        }
        face_match.insert(slot, fid);
        Ok(fid)
    }

    let mut positions = Vec::with_capacity(plan.nodes.len());
    for (i, recipe) in plan.nodes.iter().enumerate() {
        let anchor = plan.base_positions[i];
        let p = match *recipe {
            NodeRecipe::TopoVertex { .. } => {
                let mut best: Option<(Point3, f64)> = None;
                let mut second_d2 = f64::INFINITY;
                for &vid in &ci.vertices {
                    let q = topo.vertices[vid].point;
                    let d2 = (q - anchor).norm_squared();
                    match best {
                        Some((_, bd)) if d2 < bd => {
                            second_d2 = bd;
                            best = Some((q, d2));
                        }
                        Some(_) => second_d2 = second_d2.min(d2),
                        None => best = Some((q, d2)),
                    }
                }
                let (q, d2) = best.ok_or(FrozenError::RecipeOutOfRange)?;
                if d2 > MATCH_TOL * MATCH_TOL {
                    return Err(FrozenError::CorrespondenceLost {
                        node: i,
                        distance: d2.sqrt(),
                    });
                }
                // Ambiguous match: a second distinct vertex also inside the
                // tolerance (a near-tangency pair) could bind differently at
                // θ+h vs θ−h and silently corrupt the FD oracle. Refuse.
                if second_d2 <= MATCH_TOL * MATCH_TOL && second_d2 > d2 {
                    return Err(FrozenError::CorrespondenceLost {
                        node: i,
                        distance: second_d2.sqrt(),
                    });
                }
                q
            }
            NodeRecipe::SurfaceUv { face, u, v } => {
                let uv = Point2::new(u, v);
                let face_id = resolve_face(brep, &ci, &mut face_match, face, uv, &anchor, i)?;
                let surface = &brep.geometry.surfaces[topo.faces[face_id].surface_index];
                let p = surface.evaluate(uv);
                // Verify EVERY node against its anchor, not just the slot's
                // first: a wrong face match that agrees near one sample but
                // diverges elsewhere (tangent surfaces, duplicated boolean
                // surface copies, a rebuilt surface with a different frame)
                // must error, not silently evaluate the wrong geometry.
                let d2 = (p - anchor).norm_squared();
                if d2 > MATCH_TOL * MATCH_TOL {
                    return Err(FrozenError::CorrespondenceLost {
                        node: i,
                        distance: d2.sqrt(),
                    });
                }
                p
            }
            NodeRecipe::Boundary {
                face_a,
                ua,
                va,
                face_b,
                ub,
                vb,
            } => {
                let fa = resolve_face(
                    brep,
                    &ci,
                    &mut face_match,
                    face_a,
                    Point2::new(ua, va),
                    &anchor,
                    i,
                )?;
                let fb = resolve_face(
                    brep,
                    &ci,
                    &mut face_match,
                    face_b,
                    Point2::new(ub, vb),
                    &anchor,
                    i,
                )?;
                let sa = brep.geometry.surfaces[topo.faces[fa].surface_index].as_ref();
                let sb = brep.geometry.surfaces[topo.faces[fb].surface_index].as_ref();
                let p = refine_boundary_point(sa, sb, &anchor)
                    .ok_or(FrozenError::BoundarySolveFailed { node: i })?;
                let d2 = (p - anchor).norm_squared();
                if d2 > MATCH_TOL * MATCH_TOL {
                    return Err(FrozenError::CorrespondenceLost {
                        node: i,
                        distance: d2.sqrt(),
                    });
                }
                p
            }
        };
        positions.push(p);
    }

    Ok(FrozenMesh {
        positions,
        triangles: plan.triangles.clone(),
    })
}

/// Signed mesh volume via the divergence theorem (outward winding →
/// positive), generic over the scalar type. This is the single volume
/// integrator for the differentiable seam: the FD oracle evaluates it at
/// `f64` and the analytic side at `Dual<f64>` (velocity-seeded points), so
/// both sides of every FD-vs-analytic gate integrate with the same formula
/// by construction.
pub fn mesh_volume<S: tang::Scalar>(positions: &[tang::Point3<S>], triangles: &[[u32; 3]]) -> S {
    let mut six_v = S::ZERO;
    for t in triangles {
        let a = &positions[t[0] as usize];
        let b = &positions[t[1] as usize];
        let c = &positions[t[2] as usize];
        six_v += a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x);
    }
    six_v / S::from_f64(6.0)
}

impl FrozenMesh {
    /// Signed volume via the divergence theorem (outward winding → positive).
    pub fn volume(&self) -> f64 {
        mesh_volume(&self.positions, &self.triangles)
    }

    /// Count of boundary (odd-parity) undirected edges. Zero for a
    /// watertight mesh.
    pub fn open_edge_count(&self) -> usize {
        let mut counts: HashMap<(u32, u32), i64> = HashMap::new();
        for t in &self.triangles {
            for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = (i.min(j), i.max(j));
                let delta = if i < j { 1 } else { -1 };
                *counts.entry(key).or_insert(0) += delta;
            }
        }
        counts.values().filter(|&&c| c != 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::{make_cube, make_cylinder};

    #[test]
    fn cube_plan_captures_topo_vertices() {
        let cube = make_cube(4.0, 3.0, 2.0);
        let plan = capture_plan(&cube, &TessellationParams::default()).unwrap();
        assert_eq!(plan.nodes.len(), 8, "cube has 8 shared corner nodes");
        assert_eq!(plan.triangles.len(), 12, "cube has 2 triangles per face");
        assert!(plan
            .nodes
            .iter()
            .all(|n| matches!(n, NodeRecipe::TopoVertex { .. })));
    }

    #[test]
    fn cube_plan_reevaluates_to_same_positions_and_volume() {
        let cube = make_cube(4.0, 3.0, 2.0);
        let plan = capture_plan(&cube, &TessellationParams::default()).unwrap();
        let mesh = evaluate_plan(&cube, &plan).unwrap();
        assert_eq!(mesh.open_edge_count(), 0, "cube mesh must be watertight");
        assert!((mesh.volume() - 24.0).abs() < 1e-12);

        // Rebuild with a perturbed height: same signature, moved positions.
        let cube2 = make_cube(4.0, 3.0, 2.0005);
        let mesh2 = evaluate_plan(&cube2, &plan).unwrap();
        assert!((mesh2.volume() - 4.0 * 3.0 * 2.0005).abs() < 1e-12);
    }

    #[test]
    fn signature_flags_topology_change() {
        let cube = make_cube(4.0, 3.0, 2.0);
        let plan = capture_plan(&cube, &TessellationParams::default()).unwrap();
        let cyl = make_cylinder(2.0, 5.0, 32);
        match evaluate_plan(&cyl, &plan) {
            Err(FrozenError::TopologyChanged { .. }) => {}
            other => panic!("expected TopologyChanged, got {other:?}"),
        }
    }

    #[test]
    fn cylinder_plan_is_watertight_and_frozen() {
        let params = TessellationParams {
            circle_segments: 24,
            height_segments: 4,
            ..Default::default()
        };
        let cyl = make_cylinder(5.0, 8.0, 24);
        let plan = capture_plan(&cyl, &params).unwrap();
        let mesh = evaluate_plan(&cyl, &plan).unwrap();
        assert_eq!(
            mesh.open_edge_count(),
            0,
            "frozen cylinder mesh must be watertight"
        );
        // Interior lateral samples exist (height_segments > 1).
        let interior = plan
            .nodes
            .iter()
            .filter(|n| matches!(n, NodeRecipe::SurfaceUv { .. }))
            .count();
        assert!(interior > 0, "expected frozen (u,v) samples");

        // Volume of the frozen mesh matches the inscribed prism closed form.
        let n = 24.0_f64;
        let expected = 0.5 * n * (2.0 * std::f64::consts::PI / n).sin() * 25.0 * 8.0;
        assert!(
            (mesh.volume() - expected).abs() / expected < 1e-9,
            "volume {} vs expected {}",
            mesh.volume(),
            expected
        );

        // Re-evaluating at a perturbed radius keeps the pattern frozen and
        // scales the volume exactly like the closed form.
        let cyl2 = make_cylinder(5.0 + 1e-4, 8.0, 24);
        let mesh2 = evaluate_plan(&cyl2, &plan).unwrap();
        let expected2 =
            0.5 * n * (2.0 * std::f64::consts::PI / n).sin() * (5.0001_f64).powi(2) * 8.0;
        assert!(
            (mesh2.volume() - expected2).abs() / expected2 < 1e-9,
            "volume {} vs expected {}",
            mesh2.volume(),
            expected2
        );
        assert_eq!(mesh2.open_edge_count(), 0);
    }
}
