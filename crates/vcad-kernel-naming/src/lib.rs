#![warn(missing_docs)]

//! Persistent topological naming for the vcad BRep kernel.
//!
//! The classic "topological naming problem": downstream features (fillets,
//! sketches-on-faces) reference topology by ephemeral arena keys, and when an
//! upstream parameter changes and the model rebuilds, those keys mean nothing
//! — the feature breaks or, worse, silently attaches to the wrong entity.
//!
//! This crate derives **stable names from generating operations** instead of
//! arena indices:
//!
//! - A primitive's faces are named by their role in the generating op
//!   (`cube:top`, `cylinder:side`, …) — see [`seed_names`]. Seeding is a pure
//!   function of the face geometry, so two rebuilds of the same primitive (at
//!   the same or a smoothly-changed parameter) produce the same names.
//! - A boolean's result faces inherit the name of the input face whose
//!   surface carries them ([`propagate_boolean`]) — the boolean splitter trims
//!   faces but every kept face still lies on exactly one input surface. Faces
//!   split into several pieces get a deterministic sibling ordinal appended
//!   (`cube:top.0`, `cube:top.1`, ordered by quantized centroid).
//! - Edges are named by their two adjacent faces ([`EdgeRef`]) — an edge
//!   reference survives a rebuild as long as both faces resolve.
//!
//! Resolution ([`resolve_edge`]) is **fail-closed**: an edge reference
//! resolves to exactly one edge by name, or falls back to geometric matching
//! against a recorded [`EdgeHint`] (direction / midpoint / length), and every
//! other outcome is reported explicitly as [`EdgeResolution::Ambiguous`] or
//! [`EdgeResolution::Lost`] — never a silent rebind.

use std::collections::HashMap;
use std::fmt;

use vcad_kernel_geom::{
    ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId};

/// Absolute geometric tolerance (mm) for surface-identity matching.
const SURF_TOL: f64 = 1e-6;

/// Quantization step (mm) for deterministic centroid ordering.
const QUANT: f64 = 1e-6;

// =============================================================================
// Names
// =============================================================================

/// A stable, human-readable face name derived from the generating operation.
///
/// Rendered as `scope:tag` with an optional dot-separated split path, e.g.
/// `cube:top`, `n3:side.1`, `cube:top.0.2`. The `scope` identifies the
/// operation that created the base face (primitive kind by default; callers
/// evaluating a DAG should [`NameMap::rescope`] it to the node id so names
/// stay unique across a multi-primitive document). The `path` records one
/// sibling ordinal per boolean split generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceName {
    /// Operation scope (primitive kind, or a caller-assigned node id).
    pub scope: String,
    /// Face role within the generating operation (`top`, `side`, …).
    pub tag: String,
    /// Split lineage: one deterministic sibling ordinal per boolean that
    /// split the face. Empty for an unsplit face.
    pub path: Vec<u32>,
}

impl FaceName {
    /// A fresh (unsplit) face name.
    pub fn new(scope: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            tag: tag.into(),
            path: Vec::new(),
        }
    }

    /// Parse the canonical `scope:tag[.n]*` form produced by `Display`.
    ///
    /// Returns `None` when the string is not in canonical form.
    pub fn parse(s: &str) -> Option<Self> {
        let (scope, rest) = s.split_once(':')?;
        if scope.is_empty() || rest.is_empty() {
            return None;
        }
        let mut segs = rest.split('.');
        let tag = segs.next()?.to_string();
        if tag.is_empty() {
            return None;
        }
        let mut path = Vec::new();
        for seg in segs {
            path.push(seg.parse::<u32>().ok()?);
        }
        Some(Self {
            scope: scope.to_string(),
            tag,
            path,
        })
    }
}

impl fmt::Display for FaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.scope, self.tag)?;
        for p in &self.path {
            write!(f, ".{p}")?;
        }
        Ok(())
    }
}

/// Face-name side table for one `BRepSolid`.
///
/// Keys are that solid's `FaceId`s; the map is only meaningful next to the
/// solid it was built for. Faces without an entry are anonymous (their
/// provenance was lost fail-closed rather than guessed).
#[derive(Debug, Clone, Default)]
pub struct NameMap {
    /// Face id → stable name.
    pub faces: HashMap<FaceId, FaceName>,
}

impl NameMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the face carrying `name`, if exactly one does.
    pub fn face_named(&self, name: &FaceName) -> Option<FaceId> {
        let mut hits = self.faces.iter().filter(|(_, n)| *n == name);
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        Some(*first.0)
    }

    /// Rewrite every name's scope — used by DAG evaluators to replace the
    /// default primitive-kind scope with a document node id so names stay
    /// unique when several primitives combine.
    pub fn rescope(&mut self, scope: &str) {
        for name in self.faces.values_mut() {
            name.scope = scope.to_string();
        }
    }
}

// =============================================================================
// Seeding (M0): names from the generating primitive
// =============================================================================

/// Name the faces of a freshly constructed solid from its geometry.
///
/// Tags are derived per face from the carrying surface, so the result is a
/// pure function of the geometry — deterministic across rebuilds and stable
/// under smooth parameter changes (a resized cube keeps its axis-aligned
/// normals, so `cube:top` stays `cube:top`):
///
/// - axis-aligned planes → `bottom` / `top` / `front` / `back` / `left` /
///   `right` (Z-up convention);
/// - other planes → `plane`;
/// - cylinders → `side`; cones → `cone`; spheres → `sphere`; tori → `torus`;
/// - anything else → `face`.
///
/// When several faces share a tag (a prism's `side` walls, a segmented
/// sphere's bands) they are disambiguated with a trailing ordinal assigned in
/// quantized-centroid order (`side.0`, `side.1`, …), which is deterministic
/// regardless of arena iteration order.
pub fn seed_names(brep: &BRepSolid, scope: &str) -> NameMap {
    let topo = &brep.topology;
    let solid = &topo.solids[brep.solid_id];
    let shell = &topo.shells[solid.outer_shell];

    // (tag, quantized centroid, face) per shell face.
    let mut entries: Vec<(String, [i64; 3], FaceId)> = Vec::new();
    for &face_id in &shell.faces {
        let face = &topo.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];
        let tag = surface_tag(surface.as_ref());
        let c = face_centroid(brep, face_id);
        entries.push((tag, quantize(c), face_id));
    }

    let mut map = NameMap::new();
    let mut by_tag: HashMap<String, Vec<([i64; 3], FaceId)>> = HashMap::new();
    for (tag, qc, id) in entries {
        by_tag.entry(tag).or_default().push((qc, id));
    }
    for (tag, mut group) in by_tag {
        group.sort();
        let solo = group.len() == 1;
        for (i, (_, id)) in group.into_iter().enumerate() {
            let mut name = FaceName::new(scope, tag.clone());
            if !solo {
                name.path.push(i as u32);
            }
            map.faces.insert(id, name);
        }
    }
    map
}

/// Semantic tag for the surface carrying a face.
fn surface_tag(surface: &dyn Surface) -> String {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            let Some(plane) = surface.as_any().downcast_ref::<Plane>() else {
                return "plane".to_string();
            };
            let n: Vec3 = plane.normal_dir.into_inner();
            let axes: [(Vec3, &str, &str); 3] = [
                (Vec3::new(1.0, 0.0, 0.0), "right", "left"),
                (Vec3::new(0.0, 1.0, 0.0), "back", "front"),
                (Vec3::new(0.0, 0.0, 1.0), "top", "bottom"),
            ];
            for (axis, pos, neg) in axes {
                let d = n.dot(axis);
                if d > 1.0 - 1e-9 {
                    return pos.to_string();
                }
                if d < -(1.0 - 1e-9) {
                    return neg.to_string();
                }
            }
            "plane".to_string()
        }
        SurfaceKind::Cylinder => "side".to_string(),
        SurfaceKind::Cone => "cone".to_string(),
        SurfaceKind::Sphere => "sphere".to_string(),
        SurfaceKind::Torus => "torus".to_string(),
        _ => "face".to_string(),
    }
}

// =============================================================================
// Boolean propagation (M0)
// =============================================================================

/// Derive names for a boolean result from its two named inputs.
///
/// Every face the boolean keeps still lies on exactly one input surface (the
/// splitter trims loops but reuses the carrying surfaces), so each result
/// face is matched to the input faces whose surface is geometrically
/// identical (within [`SURF_TOL`]). Fail-closed rules:
///
/// - candidates carrying more than one distinct input name (e.g. two flush
///   coplanar faces from A and B) → the result face stays anonymous;
/// - no candidate (a genuinely new face, or an unnamed input face) → stays
///   anonymous;
/// - several result faces inheriting the same input name (a face split into
///   pieces) → each gets a sibling ordinal appended, in quantized-centroid
///   order, which is deterministic across rebuilds.
pub fn propagate_boolean(
    a: &BRepSolid,
    names_a: &NameMap,
    b: &BRepSolid,
    names_b: &NameMap,
    out: &BRepSolid,
) -> NameMap {
    // Collect (surface, name) for every named input face.
    let mut inputs: Vec<(&dyn Surface, &FaceName)> = Vec::new();
    for (brep, names) in [(a, names_a), (b, names_b)] {
        for (face_id, name) in &names.faces {
            let Some(face) = brep.topology.faces.get(*face_id) else {
                continue;
            };
            inputs.push((brep.geometry.surfaces[face.surface_index].as_ref(), name));
        }
    }

    // Inherit: result face → the unique input name on the same surface.
    let mut inherited: Vec<(FaceName, [i64; 3], FaceId)> = Vec::new();
    for (face_id, face) in &out.topology.faces {
        let surface = out.geometry.surfaces[face.surface_index].as_ref();
        let mut names = inputs
            .iter()
            .filter(|(s, _)| same_surface(surface, *s))
            .map(|(_, n)| *n)
            .collect::<Vec<_>>();
        names.dedup();
        names.sort();
        names.dedup();
        if let [name] = names.as_slice() {
            let c = face_centroid(out, face_id);
            inherited.push(((*name).clone(), quantize(c), face_id));
        }
    }

    // Disambiguate split siblings with a deterministic ordinal.
    let mut by_name: HashMap<FaceName, Vec<([i64; 3], FaceId)>> = HashMap::new();
    for (name, qc, id) in inherited {
        by_name.entry(name).or_default().push((qc, id));
    }
    let mut map = NameMap::new();
    for (name, mut group) in by_name {
        group.sort();
        let solo = group.len() == 1;
        for (i, (_, id)) in group.into_iter().enumerate() {
            let mut n = name.clone();
            if !solo {
                n.path.push(i as u32);
            }
            map.faces.insert(id, n);
        }
    }
    map
}

/// Geometric identity of two surfaces within [`SURF_TOL`].
///
/// Kind-wise comparison of the analytic parameters that determine the point
/// set (parameterization details like `ref_dir` are ignored; axis sign is
/// ignored where the point set is symmetric). Unsupported kinds compare
/// not-equal — fail-closed.
fn same_surface(x: &dyn Surface, y: &dyn Surface) -> bool {
    if x.surface_type() != y.surface_type() {
        return false;
    }
    match x.surface_type() {
        SurfaceKind::Plane => {
            let (Some(p), Some(q)) = (
                x.as_any().downcast_ref::<Plane>(),
                y.as_any().downcast_ref::<Plane>(),
            ) else {
                return false;
            };
            let np: Vec3 = p.normal_dir.into_inner();
            let nq: Vec3 = q.normal_dir.into_inner();
            // Same oriented plane: parallel normals (same sign — a boolean
            // keeps the material side), coincident up to origin shift.
            np.dot(nq) > 1.0 - SURF_TOL && (q.origin - p.origin).dot(np).abs() < SURF_TOL
        }
        SurfaceKind::Cylinder => {
            let (Some(p), Some(q)) = (
                x.as_any().downcast_ref::<CylinderSurface>(),
                y.as_any().downcast_ref::<CylinderSurface>(),
            ) else {
                return false;
            };
            let ap: Vec3 = p.axis.into_inner();
            let aq: Vec3 = q.axis.into_inner();
            let d = q.center - p.center;
            ap.dot(aq).abs() > 1.0 - SURF_TOL
                && (p.radius - q.radius).abs() < SURF_TOL
                && (d - ap * d.dot(ap)).norm() < SURF_TOL
        }
        SurfaceKind::Sphere => {
            let (Some(p), Some(q)) = (
                x.as_any().downcast_ref::<SphereSurface>(),
                y.as_any().downcast_ref::<SphereSurface>(),
            ) else {
                return false;
            };
            (q.center - p.center).norm() < SURF_TOL && (p.radius - q.radius).abs() < SURF_TOL
        }
        SurfaceKind::Cone => {
            let (Some(p), Some(q)) = (
                x.as_any().downcast_ref::<ConeSurface>(),
                y.as_any().downcast_ref::<ConeSurface>(),
            ) else {
                return false;
            };
            let ap: Vec3 = p.axis.into_inner();
            let aq: Vec3 = q.axis.into_inner();
            (q.apex - p.apex).norm() < SURF_TOL
                && ap.dot(aq) > 1.0 - SURF_TOL
                && (p.half_angle - q.half_angle).abs() < SURF_TOL
        }
        SurfaceKind::Torus => {
            let (Some(p), Some(q)) = (
                x.as_any().downcast_ref::<TorusSurface>(),
                y.as_any().downcast_ref::<TorusSurface>(),
            ) else {
                return false;
            };
            let ap: Vec3 = p.axis.into_inner();
            let aq: Vec3 = q.axis.into_inner();
            (q.center - p.center).norm() < SURF_TOL
                && ap.dot(aq).abs() > 1.0 - SURF_TOL
                && (p.major_radius - q.major_radius).abs() < SURF_TOL
                && (p.minor_radius - q.minor_radius).abs() < SURF_TOL
        }
        _ => false,
    }
}

// =============================================================================
// Edge references + resolution (M1)
// =============================================================================

/// Geometric snapshot of an edge, recorded when a reference is created.
///
/// Used as the fallback matcher when name-based resolution breaks across an
/// edit (e.g. a face's provenance was lost in a boolean). Tolerances scale
/// with the recorded length, so hints work at any model scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeHint {
    /// Edge midpoint at record time.
    pub midpoint: Point3,
    /// Unit edge direction at record time (sign-insensitive on match).
    pub direction: Vec3,
    /// Edge length at record time.
    pub length: f64,
}

/// A persistent edge reference: the names of its two adjacent faces, stored
/// in canonical (sorted) order, plus an optional geometric hint.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRef {
    /// Name of one adjacent face (lexicographically smaller).
    pub face_a: FaceName,
    /// Name of the other adjacent face.
    pub face_b: FaceName,
    /// Geometric snapshot for fallback matching.
    pub hint: Option<EdgeHint>,
}

impl EdgeRef {
    /// Build a reference to the edge between the two named faces, canonically
    /// ordered, without a geometric hint.
    pub fn new(a: FaceName, b: FaceName) -> Self {
        let (face_a, face_b) = if a <= b { (a, b) } else { (b, a) };
        Self {
            face_a,
            face_b,
            hint: None,
        }
    }

    /// Capture a reference to an existing edge of `brep`: its adjacent-face
    /// names from `names` plus a geometric hint. Returns `None` when the
    /// edge is degenerate or either adjacent face is anonymous.
    pub fn capture(brep: &BRepSolid, names: &NameMap, edge: EdgeId) -> Option<Self> {
        let (fa, fb) = edge_faces(brep, edge)?;
        let na = names.faces.get(&fa)?.clone();
        let nb = names.faces.get(&fb)?.clone();
        let (a, b) = edge_endpoints(brep, edge)?;
        let ev = b - a;
        let len = ev.norm();
        if len < 1e-12 {
            return None;
        }
        let mut r = Self::new(na, nb);
        r.hint = Some(EdgeHint {
            midpoint: a + ev * 0.5,
            direction: ev / len,
            length: len,
        });
        Some(r)
    }
}

/// How a resolved edge was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveMethod {
    /// Both face names matched exactly — provenance intact.
    ByName,
    /// Name matching failed; the geometric hint matched exactly one edge.
    ByGeometry,
}

/// Outcome of resolving an [`EdgeRef`] against a (re)built solid.
///
/// Fail-closed: only `Resolved` may be acted on; `Ambiguous` and `Lost`
/// carry enough context to report, and must never be silently rebound.
#[derive(Debug, Clone)]
pub enum EdgeResolution {
    /// Exactly one edge matched.
    Resolved {
        /// The matched edge in the current topology.
        edge: EdgeId,
        /// Current endpoint positions.
        endpoints: (Point3, Point3),
        /// Whether the match came from names or the geometric fallback.
        method: ResolveMethod,
    },
    /// The geometric fallback matched more than one edge.
    Ambiguous {
        /// All candidate edges, for reporting.
        candidates: Vec<EdgeId>,
    },
    /// No edge matched by name or geometry.
    Lost {
        /// Human-readable reason for the failure.
        reason: String,
    },
}

/// Resolve an edge reference against `brep` + its name map.
///
/// Name resolution first: find the unique faces carrying `face_a` / `face_b`
/// and the unique manifold edge adjacent to both. When that fails and the
/// reference carries an [`EdgeHint`], fall back to geometric matching:
/// candidates must agree with the hint in direction (within ~18°), length
/// (±25%), and midpoint (within 25% of the recorded length). Exactly one
/// candidate resolves; zero is [`EdgeResolution::Lost`]; several are
/// [`EdgeResolution::Ambiguous`].
pub fn resolve_edge(brep: &BRepSolid, names: &NameMap, edge_ref: &EdgeRef) -> EdgeResolution {
    // --- primary: by name -------------------------------------------------
    let fa = names.face_named(&edge_ref.face_a);
    let fb = names.face_named(&edge_ref.face_b);
    if let (Some(fa), Some(fb)) = (fa, fb) {
        let mut hits: Vec<EdgeId> = Vec::new();
        for (edge_id, _) in &brep.topology.edges {
            if let Some((x, y)) = edge_faces(brep, edge_id) {
                if (x == fa && y == fb) || (x == fb && y == fa) {
                    hits.push(edge_id);
                }
            }
        }
        match hits.as_slice() {
            [edge] => {
                if let Some(endpoints) = edge_endpoints(brep, *edge) {
                    return EdgeResolution::Resolved {
                        edge: *edge,
                        endpoints,
                        method: ResolveMethod::ByName,
                    };
                }
            }
            [] => {} // faces exist but no longer share an edge — try fallback
            _ => {
                // Two faces sharing several edges (e.g. through a slot):
                // names alone cannot pick one — fall through to the hint.
                if edge_ref.hint.is_none() {
                    return EdgeResolution::Ambiguous { candidates: hits };
                }
            }
        }
    }

    // --- fallback: by geometry -------------------------------------------
    let Some(hint) = &edge_ref.hint else {
        return EdgeResolution::Lost {
            reason: format!(
                "no edge between faces '{}' and '{}', and the reference has no geometric hint",
                edge_ref.face_a, edge_ref.face_b
            ),
        };
    };
    let tol = hint.length * 0.25;
    let mut candidates: Vec<EdgeId> = Vec::new();
    for (edge_id, _) in &brep.topology.edges {
        let Some((a, b)) = edge_endpoints(brep, edge_id) else {
            continue;
        };
        let ev = b - a;
        let len = ev.norm();
        if len < 1e-12 || (len - hint.length).abs() > tol {
            continue;
        }
        if (ev / len).dot(hint.direction).abs() < 0.95 {
            continue;
        }
        if (a + ev * 0.5 - hint.midpoint).norm() > tol {
            continue;
        }
        candidates.push(edge_id);
    }
    match candidates.as_slice() {
        [edge] => {
            let endpoints = edge_endpoints(brep, *edge).expect("candidate had endpoints");
            EdgeResolution::Resolved {
                edge: *edge,
                endpoints,
                method: ResolveMethod::ByGeometry,
            }
        }
        [] => EdgeResolution::Lost {
            reason: format!(
                "no edge between faces '{}' and '{}', and no edge matches the geometric hint",
                edge_ref.face_a, edge_ref.face_b
            ),
        },
        _ => EdgeResolution::Ambiguous { candidates },
    }
}

// =============================================================================
// Topology helpers
// =============================================================================

/// The two faces adjacent to a manifold edge, or `None` at a non-manifold /
/// un-twinned edge (boolean seams may leave those).
pub fn edge_faces(brep: &BRepSolid, edge: EdgeId) -> Option<(FaceId, FaceId)> {
    let topo = &brep.topology;
    let he = topo.edges.get(edge)?.half_edge;
    let h = topo.half_edges.get(he)?;
    let fa = topo.loops.get(h.loop_id?)?.face?;
    let twin = topo.half_edges.get(h.twin?)?;
    let fb = topo.loops.get(twin.loop_id?)?.face?;
    Some((fa, fb))
}

/// Endpoint positions of an edge (origin of the primary half-edge first).
pub fn edge_endpoints(brep: &BRepSolid, edge: EdgeId) -> Option<(Point3, Point3)> {
    let topo = &brep.topology;
    let he = topo.edges.get(edge)?.half_edge;
    let h = topo.half_edges.get(he)?;
    let a = topo.vertices.get(h.origin)?.point;
    let b = topo
        .vertices
        .get(topo.half_edge_dest(he))
        .map(|v| v.point)?;
    Some((a, b))
}

/// Centroid of a face's outer-loop vertices.
fn face_centroid(brep: &BRepSolid, face: FaceId) -> Point3 {
    let topo = &brep.topology;
    let mut sum = Vec3::new(0.0, 0.0, 0.0);
    let mut n = 0usize;
    let outer = topo.faces[face].outer_loop;
    for he in topo.loop_half_edges(outer) {
        let p = topo.vertices[topo.half_edges[he].origin].point;
        sum += Vec3::new(p.x, p.y, p.z);
        n += 1;
    }
    if n == 0 {
        return Point3::new(0.0, 0.0, 0.0);
    }
    let inv = 1.0 / n as f64;
    Point3::new(sum.x * inv, sum.y * inv, sum.z * inv)
}

/// Quantize a point for deterministic ordering.
fn quantize(p: Point3) -> [i64; 3] {
    [
        (p.x / QUANT).round() as i64,
        (p.y / QUANT).round() as i64,
        (p.z / QUANT).round() as i64,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::{make_cube, make_cylinder, make_prism};

    fn names_of(map: &NameMap) -> Vec<String> {
        let mut v: Vec<String> = map.faces.values().map(|n| n.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn face_name_roundtrip() {
        for s in ["cube:top", "n3:side.1", "cube:top.0.2"] {
            let n = FaceName::parse(s).expect("parses");
            assert_eq!(n.to_string(), s);
        }
        assert!(FaceName::parse("noscope").is_none());
        assert!(FaceName::parse(":tag").is_none());
        assert!(FaceName::parse("scope:").is_none());
        assert!(FaceName::parse("scope:tag.x").is_none());
    }

    #[test]
    fn cube_seeding_is_semantic_and_deterministic() {
        let c = make_cube(10.0, 20.0, 30.0);
        let m = seed_names(&c, "cube");
        assert_eq!(
            names_of(&m),
            vec![
                "cube:back",
                "cube:bottom",
                "cube:front",
                "cube:left",
                "cube:right",
                "cube:top"
            ]
        );
        // A rebuild at a different parameter keeps every tag.
        let c2 = make_cube(14.0, 20.0, 30.0);
        let m2 = seed_names(&c2, "cube");
        assert_eq!(names_of(&m), names_of(&m2));
    }

    #[test]
    fn cylinder_seeding_names_caps_and_side() {
        let c = make_cylinder(5.0, 10.0, 16);
        let m = seed_names(&c, "cyl");
        assert_eq!(names_of(&m), vec!["cyl:bottom", "cyl:side", "cyl:top"]);
    }

    #[test]
    fn prism_sides_get_deterministic_ordinals() {
        let p = make_prism(6, 5.0, 10.0);
        let m1 = seed_names(&p, "prism");
        let m2 = seed_names(&make_prism(6, 5.0, 12.0), "prism");
        let n1 = names_of(&m1);
        // 6 lateral walls + 2 caps, all uniquely named (axis-aligned walls
        // get semantic tags, the rest get plane.N ordinals).
        assert_eq!(n1.len(), 8);
        assert_eq!(n1.iter().collect::<std::collections::HashSet<_>>().len(), 8);
        assert!(n1.iter().any(|n| n.contains(":plane.")));
        assert_eq!(n1, names_of(&m2));
    }

    #[test]
    fn edge_resolution_by_name_survives_a_rebuild() {
        let c = make_cube(10.0, 10.0, 10.0);
        let names = seed_names(&c, "cube");
        // The top-right edge: between "cube:top" and "cube:right".
        let r = EdgeRef::new(FaceName::new("cube", "top"), FaceName::new("cube", "right"));
        let EdgeResolution::Resolved {
            endpoints, method, ..
        } = resolve_edge(&c, &names, &r)
        else {
            panic!("expected resolution");
        };
        assert_eq!(method, ResolveMethod::ByName);
        for p in [endpoints.0, endpoints.1] {
            assert!((p.x - 10.0).abs() < 1e-9 && (p.z - 10.0).abs() < 1e-9);
        }
        // Rebuild wider: the same reference resolves to the moved edge.
        let c2 = make_cube(14.0, 10.0, 10.0);
        let names2 = seed_names(&c2, "cube");
        let EdgeResolution::Resolved { endpoints, .. } = resolve_edge(&c2, &names2, &r) else {
            panic!("expected resolution after rebuild");
        };
        for p in [endpoints.0, endpoints.1] {
            assert!((p.x - 14.0).abs() < 1e-9 && (p.z - 10.0).abs() < 1e-9);
        }
    }

    #[test]
    fn resolution_falls_back_to_geometry_and_reports_lost() {
        let c = make_cube(10.0, 10.0, 10.0);
        let names = seed_names(&c, "cube");
        // Capture a real edge, then break its names (simulating lost
        // provenance): the hint must still find it.
        let top_right = EdgeRef::new(FaceName::new("cube", "top"), FaceName::new("cube", "right"));
        let EdgeResolution::Resolved { edge, .. } = resolve_edge(&c, &names, &top_right) else {
            panic!("expected resolution");
        };
        let mut captured = EdgeRef::capture(&c, &names, edge).expect("capturable");
        captured.face_a = FaceName::new("gone", "top");
        captured.face_b = FaceName::new("gone", "right");
        let EdgeResolution::Resolved { method, .. } = resolve_edge(&c, &names, &captured) else {
            panic!("expected geometric fallback");
        };
        assert_eq!(method, ResolveMethod::ByGeometry);

        // Without a hint, broken names are Lost — never rebound.
        captured.hint = None;
        assert!(matches!(
            resolve_edge(&c, &names, &captured),
            EdgeResolution::Lost { .. }
        ));
    }

    #[test]
    fn stale_hint_after_a_large_edit_is_lost_not_rebound() {
        let c = make_cube(10.0, 10.0, 10.0);
        let names = seed_names(&c, "cube");
        let top_right = EdgeRef::new(FaceName::new("cube", "top"), FaceName::new("cube", "right"));
        let EdgeResolution::Resolved { edge, .. } = resolve_edge(&c, &names, &top_right) else {
            panic!("expected resolution");
        };
        let mut captured = EdgeRef::capture(&c, &names, edge).expect("capturable");
        captured.face_a = FaceName::new("gone", "top");
        captured.face_b = FaceName::new("gone", "right");
        // Rebuild far away: the stale hint matches nothing.
        let c2 = make_cube(100.0, 100.0, 100.0);
        let names2 = seed_names(&c2, "cube");
        assert!(matches!(
            resolve_edge(&c2, &names2, &captured),
            EdgeResolution::Lost { .. }
        ));
    }
}
