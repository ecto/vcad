//! Face-level B-rep queries for agent-facing inspection.
//!
//! `inspect_cad` / `measure` and friends answer questions about a part's
//! *tessellation*: bbox, volume, centre of mass, min distance. That is not
//! enough to reason mechanically about a real part — you cannot ask which
//! face is the mounting plane, what a bore's diameter is, or where a shaft
//! axis points. Worse, a bounding box actively lies about diameters: a motor
//! with an 80 mm body and an asymmetric radial connector boss reads ~102 mm
//! across its bbox.
//!
//! This module walks the B-rep directly ([`crate::Solid::brep`]) and reports,
//! per face: a stable identifier, surface type, area, bounding box, centroid,
//! and the analytic surface parameters (a cylinder's radius / axis / axial
//! extent, a plane's normal and a point on it, …).
//!
//! Accuracy notes, because the two halves differ:
//!
//! - **Analytic** (exact, tessellation-independent): surface type, plane
//!   normal and origin, cylinder radius and axis, cone half-angle, sphere and
//!   torus radii.
//! - **Tessellation-bound** (same caveat as `inspect_cad`): face area, face
//!   bounding box, face centroid, and the axial extent of a cylindrical face
//!   — all derived from the face's triangulation.
//!
//! All lengths are millimetres, areas mm², angles degrees.
//!
//! Face identity uses [`vcad_kernel_naming`] names (`n3:top`, `cube:side.1`)
//! whenever the solid carries a [`NameMap`](vcad_kernel_naming::NameMap) —
//! those survive a parameter change, unlike arena indices. Solids without
//! names (imported STEP, mesh-derived) fall back to `face_<n>`, where `n` is
//! the face's rank in quantized-centroid order — deterministic for a given
//! geometry, but *not* stable across a rebuild that moves faces.

use serde::Serialize;

use vcad_kernel_geom::{
    ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::{tessellate_brep_by_face, TessellationParams, TriangleMesh};
use vcad_kernel_topo::{FaceId, Orientation};

/// Quantization step (mm) for deterministic face ordering.
const QUANT: f64 = 1e-6;

/// Angular tolerance (radians) for calling two axes parallel.
const AXIS_TOL: f64 = 1e-6;

/// Distance tolerance (mm) for calling two parallel axes collinear.
const COAXIAL_TOL: f64 = 1e-6;

// =============================================================================
// Reported shapes
// =============================================================================

/// Analytic parameters of the surface carrying a face. Exactly one variant is
/// populated per face; unsupported kinds (BSpline, Bilinear) report `Other`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceInfo {
    /// A planar face.
    Plane {
        /// Outward unit normal (respects the face's orientation).
        normal: [f64; 3],
        /// A point lying on the plane (the surface origin).
        point: [f64; 3],
    },
    /// A cylindrical face.
    Cylinder {
        /// Cylinder radius, mm.
        radius_mm: f64,
        /// Diameter, mm (`2 × radius`) — what a caller usually wants.
        diameter_mm: f64,
        /// Unit axis direction, sign-canonicalised.
        axis: [f64; 3],
        /// A point on the axis line.
        axis_point: [f64; 3],
        /// Axial extent of *this face* along the axis, `[min, max]`,
        /// measured from `axis_point`. Tessellation-bound.
        axial_range_mm: [f64; 2],
        /// `axial_range_mm[1] - axial_range_mm[0]`, mm.
        axial_length_mm: f64,
        /// True if the face's material lies outside the cylinder (a shaft or
        /// boss); false if inside (a bore). Derived from face orientation.
        convex: bool,
    },
    /// A conical face.
    Cone {
        /// Cone apex.
        apex: [f64; 3],
        /// Unit axis direction (apex toward base).
        axis: [f64; 3],
        /// Half-angle, degrees.
        half_angle_deg: f64,
    },
    /// A spherical face.
    Sphere {
        /// Sphere centre.
        center: [f64; 3],
        /// Sphere radius, mm.
        radius_mm: f64,
    },
    /// A toroidal face.
    Torus {
        /// Torus centre.
        center: [f64; 3],
        /// Unit axis direction.
        axis: [f64; 3],
        /// Major radius, mm.
        major_radius_mm: f64,
        /// Minor radius, mm.
        minor_radius_mm: f64,
    },
    /// A free-form or otherwise unparameterised face.
    Other {
        /// Surface kind name (`bspline`, `bilinear`, …).
        surface_type: String,
    },
}

/// One face of a solid.
#[derive(Debug, Clone, Serialize)]
pub struct FaceInfo {
    /// Stable identifier: a topological name (`n3:top`) when the solid
    /// carries one, else `face_<n>` in quantized-centroid order.
    pub id: String,
    /// The topological name, when present. `None` means `id` is positional
    /// and will not survive a rebuild that moves faces.
    pub name: Option<String>,
    /// True when `id` is a topological name (survives parameter changes).
    pub stable: bool,
    /// Surface kind: `plane`, `cylinder`, `cone`, `sphere`, `torus`, …
    pub surface_type: String,
    /// Face area, mm². Tessellation-bound.
    pub area_mm2: f64,
    /// Face bounding box minimum, mm. Tessellation-bound.
    pub bbox_min_mm: [f64; 3],
    /// Face bounding box maximum, mm. Tessellation-bound.
    pub bbox_max_mm: [f64; 3],
    /// Area-weighted face centroid, mm. Tessellation-bound.
    pub centroid_mm: [f64; 3],
    /// Number of inner loops (holes) in the face.
    pub inner_loops: usize,
    /// Analytic surface parameters.
    pub surface: SurfaceInfo,
}

/// A set of cylindrical faces sharing one axis line.
#[derive(Debug, Clone, Serialize)]
pub struct CoaxialGroup {
    /// Unit axis direction, sign-canonicalised.
    pub axis: [f64; 3],
    /// A point on the shared axis line.
    pub axis_point: [f64; 3],
    /// Largest radius in the group, mm — the outer diameter of whatever this
    /// axis carries.
    pub max_radius_mm: f64,
    /// `2 × max_radius_mm`, mm.
    pub max_diameter_mm: f64,
    /// Smallest radius in the group, mm (the innermost bore on this axis).
    pub min_radius_mm: f64,
    /// Distinct radii present, ascending, mm.
    pub radii_mm: Vec<f64>,
    /// Total lateral area of the group's faces, mm². Tessellation-bound.
    pub total_area_mm2: f64,
    /// Combined axial extent of the group, `[min, max]` from `axis_point`, mm.
    pub axial_range_mm: [f64; 2],
    /// Face ids in the group.
    pub face_ids: Vec<String>,
}

/// A `(surface_type, count)` tally, or a `(radius, count)` tally for
/// cylinders — the compact answer to "what is this part made of".
#[derive(Debug, Clone, Serialize)]
pub struct FaceGroup {
    /// Surface kind.
    pub surface_type: String,
    /// Radius, mm — cylinders and spheres only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius_mm: Option<f64>,
    /// Number of faces in this group.
    pub count: usize,
    /// Total area of the group, mm². Tessellation-bound.
    pub total_area_mm2: f64,
    /// Up to eight representative face ids.
    pub example_face_ids: Vec<String>,
}

/// Everything [`inspect_faces`] knows about one solid.
#[derive(Debug, Clone, Serialize)]
pub struct FaceReport {
    /// Total face count on the outer shell.
    pub face_count: usize,
    /// True when face ids are topological names.
    pub named: bool,
    /// Every face.
    pub faces: Vec<FaceInfo>,
    /// Faces tallied by surface type (and radius, for cylinders/spheres).
    pub groups: Vec<FaceGroup>,
    /// Cylindrical faces grouped by shared axis line, largest first.
    pub coaxial_groups: Vec<CoaxialGroup>,
}

impl FaceReport {
    /// The coaxial cylinder group about `axis`, or — when `axis` is `None` —
    /// the group carrying the most cylindrical area (the part's dominant
    /// axis). This is what "true outer diameter" means on a part whose
    /// bounding box is inflated by a boss: `max_diameter_mm` of this group.
    pub fn largest_coaxial(&self, axis: Option<Vec3>) -> Option<&CoaxialGroup> {
        match axis {
            None => self
                .coaxial_groups
                .iter()
                .max_by(|a, b| a.total_area_mm2.total_cmp(&b.total_area_mm2)),
            Some(a) => {
                let a = normalize(a)?;
                self.coaxial_groups
                    .iter()
                    .filter(|g| parallel(Vec3::new(g.axis[0], g.axis[1], g.axis[2]), a))
                    .max_by(|x, y| x.max_radius_mm.total_cmp(&y.max_radius_mm))
            }
        }
    }
}

// =============================================================================
// Entry point
// =============================================================================

/// Errors from a face query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaceQueryError {
    /// The solid has no B-rep (mesh-only import, or empty).
    NoBRep,
}

impl std::fmt::Display for FaceQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBRep => write!(
                f,
                "this part has no B-rep topology (it is a mesh or empty), so face-level \
                 queries are unavailable; only mesh measurements apply"
            ),
        }
    }
}

impl std::error::Error for FaceQueryError {}

impl crate::Solid {
    /// Enumerate this solid's faces with their analytic surface parameters.
    ///
    /// Fails closed on mesh-only solids rather than silently reporting a
    /// tessellation-derived guess.
    pub fn inspect_faces(&self) -> Result<FaceReport, FaceQueryError> {
        let brep = self.brep().ok_or(FaceQueryError::NoBRep)?;
        Ok(report(brep, self.names()))
    }
}

/// Build a face report for a B-rep, optionally using a name map for stable ids.
pub fn inspect_faces(brep: &BRepSolid, names: Option<&vcad_kernel_naming::NameMap>) -> FaceReport {
    report(brep, names)
}

fn report(brep: &BRepSolid, names: Option<&vcad_kernel_naming::NameMap>) -> FaceReport {
    let params = TessellationParams::default();
    let per_face = tessellate_brep_by_face(brep, &params);

    // Deterministic positional ids: rank by quantized centroid so the
    // fallback id doesn't depend on slotmap iteration order.
    let mut ranked: Vec<([i64; 3], FaceId)> = per_face
        .iter()
        .map(|(id, _, mesh)| (quantize(mesh_centroid(mesh)), *id))
        .collect();
    ranked.sort();
    let ordinal = |face: FaceId| ranked.iter().position(|(_, id)| *id == face).unwrap_or(0);

    let named = names.is_some_and(|n| !n.faces.is_empty());

    let mut faces = Vec::with_capacity(per_face.len());
    for (face_id, kind, mesh) in &per_face {
        let face = &brep.topology.faces[*face_id];
        let surface = brep.geometry.surfaces[face.surface_index].as_ref();
        let reversed = face.orientation == Orientation::Reversed;

        let name = names
            .and_then(|n| n.faces.get(face_id))
            .map(|n| n.to_string());
        let id = name
            .clone()
            .unwrap_or_else(|| format!("face_{}", ordinal(*face_id)));

        let (bbox_min, bbox_max) = mesh_bbox(mesh);
        faces.push(FaceInfo {
            id,
            stable: name.is_some(),
            name,
            surface_type: kind_name(*kind).to_string(),
            area_mm2: mesh_area(mesh),
            bbox_min_mm: bbox_min,
            bbox_max_mm: bbox_max,
            centroid_mm: arr(mesh_centroid(mesh)),
            inner_loops: face.inner_loops.len(),
            surface: surface_info(surface, reversed, mesh),
        });
    }

    faces.sort_by(|a, b| b.area_mm2.total_cmp(&a.area_mm2));

    let groups = group_faces(&faces);
    let coaxial_groups = coaxial_groups(&faces);

    FaceReport {
        face_count: faces.len(),
        named,
        faces,
        groups,
        coaxial_groups,
    }
}

// =============================================================================
// Surface parameters
// =============================================================================

fn kind_name(kind: SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::Plane => "plane",
        SurfaceKind::Cylinder => "cylinder",
        SurfaceKind::Cone => "cone",
        SurfaceKind::Sphere => "sphere",
        SurfaceKind::Torus => "torus",
        SurfaceKind::BSpline => "bspline",
        SurfaceKind::Bilinear => "bilinear",
    }
}

fn surface_info(surface: &dyn Surface, reversed: bool, mesh: &TriangleMesh) -> SurfaceInfo {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(p) = surface.as_any().downcast_ref::<Plane>() {
                let n: Vec3 = p.normal_dir.into_inner();
                let n = if reversed { -n } else { n };
                return SurfaceInfo::Plane {
                    normal: arr_v(n),
                    point: arr(p.origin),
                };
            }
            SurfaceInfo::Other {
                surface_type: "plane".into(),
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(c) = surface.as_any().downcast_ref::<CylinderSurface>() {
                let axis_raw: Vec3 = c.axis.into_inner();
                let (axis, flipped) = canonical_axis(axis_raw);
                let (lo, hi) = axial_range(mesh, c.center, axis);
                let _ = flipped;
                return SurfaceInfo::Cylinder {
                    radius_mm: c.radius,
                    diameter_mm: 2.0 * c.radius,
                    axis: arr_v(axis),
                    axis_point: arr(c.center),
                    axial_range_mm: [lo, hi],
                    axial_length_mm: hi - lo,
                    // A forward-oriented cylindrical face has its surface
                    // normal pointing away from the axis, i.e. material
                    // inside → a shaft. Reversed → a bore.
                    convex: !reversed,
                };
            }
            SurfaceInfo::Other {
                surface_type: "cylinder".into(),
            }
        }
        SurfaceKind::Cone => {
            if let Some(c) = surface.as_any().downcast_ref::<ConeSurface>() {
                return SurfaceInfo::Cone {
                    apex: arr(c.apex),
                    axis: arr_v(c.axis.into_inner()),
                    half_angle_deg: c.half_angle.to_degrees(),
                };
            }
            SurfaceInfo::Other {
                surface_type: "cone".into(),
            }
        }
        SurfaceKind::Sphere => {
            if let Some(s) = surface.as_any().downcast_ref::<SphereSurface>() {
                return SurfaceInfo::Sphere {
                    center: arr(s.center),
                    radius_mm: s.radius,
                };
            }
            SurfaceInfo::Other {
                surface_type: "sphere".into(),
            }
        }
        SurfaceKind::Torus => {
            if let Some(t) = surface.as_any().downcast_ref::<TorusSurface>() {
                return SurfaceInfo::Torus {
                    center: arr(t.center),
                    axis: arr_v(t.axis.into_inner()),
                    major_radius_mm: t.major_radius,
                    minor_radius_mm: t.minor_radius,
                };
            }
            SurfaceInfo::Other {
                surface_type: "torus".into(),
            }
        }
        other => SurfaceInfo::Other {
            surface_type: kind_name(other).to_string(),
        },
    }
}

// =============================================================================
// Grouping
// =============================================================================

/// Quantize a radius so faces that came off the same feature land in one
/// bucket despite float noise (1 µm buckets).
fn radius_key(r: f64) -> i64 {
    (r / 1e-3).round() as i64
}

fn group_faces(faces: &[FaceInfo]) -> Vec<FaceGroup> {
    let mut keys: Vec<(String, Option<i64>)> = Vec::new();
    let mut groups: Vec<FaceGroup> = Vec::new();

    for f in faces {
        let radius = match &f.surface {
            SurfaceInfo::Cylinder { radius_mm, .. } => Some(*radius_mm),
            SurfaceInfo::Sphere { radius_mm, .. } => Some(*radius_mm),
            _ => None,
        };
        let key = (f.surface_type.clone(), radius.map(radius_key));
        let idx = match keys.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                keys.push(key);
                groups.push(FaceGroup {
                    surface_type: f.surface_type.clone(),
                    radius_mm: radius,
                    count: 0,
                    total_area_mm2: 0.0,
                    example_face_ids: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[idx].count += 1;
        groups[idx].total_area_mm2 += f.area_mm2;
        if groups[idx].example_face_ids.len() < 8 {
            groups[idx].example_face_ids.push(f.id.clone());
        }
    }

    groups.sort_by(|a, b| b.total_area_mm2.total_cmp(&a.total_area_mm2));
    groups
}

struct CoaxAccum {
    axis: Vec3,
    axis_point: Point3,
    radii: Vec<f64>,
    area: f64,
    lo: f64,
    hi: f64,
    face_ids: Vec<String>,
}

fn coaxial_groups(faces: &[FaceInfo]) -> Vec<CoaxialGroup> {
    let mut accum: Vec<CoaxAccum> = Vec::new();

    for f in faces {
        let SurfaceInfo::Cylinder {
            radius_mm,
            axis,
            axis_point,
            axial_range_mm,
            ..
        } = &f.surface
        else {
            continue;
        };
        let axis = Vec3::new(axis[0], axis[1], axis[2]);
        let point = Point3::new(axis_point[0], axis_point[1], axis_point[2]);

        let hit = accum
            .iter_mut()
            .find(|g| parallel(g.axis, axis) && collinear(g.axis, g.axis_point, point));

        match hit {
            Some(g) => {
                // Re-express this face's extent in the group's own axial
                // coordinate (its origin is the group's axis_point).
                let shift = (point - g.axis_point).dot(g.axis);
                g.lo = g.lo.min(axial_range_mm[0] + shift);
                g.hi = g.hi.max(axial_range_mm[1] + shift);
                g.radii.push(*radius_mm);
                g.area += f.area_mm2;
                g.face_ids.push(f.id.clone());
            }
            None => accum.push(CoaxAccum {
                axis,
                axis_point: point,
                radii: vec![*radius_mm],
                area: f.area_mm2,
                lo: axial_range_mm[0],
                hi: axial_range_mm[1],
                face_ids: vec![f.id.clone()],
            }),
        }
    }

    let mut out: Vec<CoaxialGroup> = accum
        .into_iter()
        .map(|g| {
            let mut radii: Vec<f64> = g.radii.clone();
            radii.sort_by(f64::total_cmp);
            radii.dedup_by(|a, b| radius_key(*a) == radius_key(*b));
            CoaxialGroup {
                axis: arr_v(g.axis),
                axis_point: arr(g.axis_point),
                max_radius_mm: g.radii.iter().copied().fold(f64::MIN, f64::max),
                max_diameter_mm: 2.0 * g.radii.iter().copied().fold(f64::MIN, f64::max),
                min_radius_mm: g.radii.iter().copied().fold(f64::MAX, f64::min),
                radii_mm: radii,
                total_area_mm2: g.area,
                axial_range_mm: [g.lo, g.hi],
                face_ids: g.face_ids,
            }
        })
        .collect();

    out.sort_by(|a, b| b.total_area_mm2.total_cmp(&a.total_area_mm2));
    out
}

// =============================================================================
// Geometry helpers
// =============================================================================

fn arr(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

fn arr_v(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn normalize(v: Vec3) -> Option<Vec3> {
    let n = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if n < 1e-12 {
        return None;
    }
    Some(Vec3::new(v.x / n, v.y / n, v.z / n))
}

/// Give an axis a canonical sign so `+Z` and `-Z` cylinders group together.
/// Returns the canonical direction and whether the input was flipped.
fn canonical_axis(v: Vec3) -> (Vec3, bool) {
    let v = normalize(v).unwrap_or(Vec3::new(0.0, 0.0, 1.0));
    let comps = [v.x, v.y, v.z];
    let dominant = comps
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(i, _)| i)
        .unwrap_or(2);
    if comps[dominant] < 0.0 {
        (Vec3::new(-v.x, -v.y, -v.z), true)
    } else {
        (v, false)
    }
}

fn parallel(a: Vec3, b: Vec3) -> bool {
    let cross = Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    );
    (cross.x * cross.x + cross.y * cross.y + cross.z * cross.z).sqrt() < AXIS_TOL
}

/// True when `point` lies on the line through `origin` along `axis`.
fn collinear(axis: Vec3, origin: Point3, point: Point3) -> bool {
    let d = point - origin;
    let along = d.dot(axis);
    let perp = Vec3::new(
        d.x - along * axis.x,
        d.y - along * axis.y,
        d.z - along * axis.z,
    );
    (perp.x * perp.x + perp.y * perp.y + perp.z * perp.z).sqrt() < COAXIAL_TOL
}

fn quantize(p: Point3) -> [i64; 3] {
    [
        (p.x / QUANT).round() as i64,
        (p.y / QUANT).round() as i64,
        (p.z / QUANT).round() as i64,
    ]
}

fn vertices(mesh: &TriangleMesh) -> impl Iterator<Item = Point3> + '_ {
    mesh.vertices
        .chunks_exact(3)
        .map(|c| Point3::new(c[0] as f64, c[1] as f64, c[2] as f64))
}

fn mesh_bbox(mesh: &TriangleMesh) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in vertices(mesh) {
        let v = [p.x, p.y, p.z];
        for i in 0..3 {
            lo[i] = lo[i].min(v[i]);
            hi[i] = hi[i].max(v[i]);
        }
    }
    if !lo[0].is_finite() {
        return ([0.0; 3], [0.0; 3]);
    }
    (lo, hi)
}

fn triangles(mesh: &TriangleMesh) -> impl Iterator<Item = (Point3, Point3, Point3)> + '_ {
    let vtx = move |i: u32| -> Point3 {
        let b = i as usize * 3;
        Point3::new(
            mesh.vertices[b] as f64,
            mesh.vertices[b + 1] as f64,
            mesh.vertices[b + 2] as f64,
        )
    };
    mesh.indices
        .chunks_exact(3)
        .map(move |t| (vtx(t[0]), vtx(t[1]), vtx(t[2])))
}

fn mesh_area(mesh: &TriangleMesh) -> f64 {
    triangles(mesh)
        .map(|(a, b, c)| {
            let u = b - a;
            let v = c - a;
            let cx = u.y * v.z - u.z * v.y;
            let cy = u.z * v.x - u.x * v.z;
            let cz = u.x * v.y - u.y * v.x;
            0.5 * (cx * cx + cy * cy + cz * cz).sqrt()
        })
        .sum()
}

/// Area-weighted centroid of a face's triangulation. Falls back to the mean
/// vertex position for a degenerate (zero-area) face.
fn mesh_centroid(mesh: &TriangleMesh) -> Point3 {
    let mut acc = Vec3::new(0.0, 0.0, 0.0);
    let mut total = 0.0;
    for (a, b, c) in triangles(mesh) {
        let u = b - a;
        let v = c - a;
        let cx = u.y * v.z - u.z * v.y;
        let cy = u.z * v.x - u.x * v.z;
        let cz = u.x * v.y - u.y * v.x;
        let area = 0.5 * (cx * cx + cy * cy + cz * cz).sqrt();
        acc += Vec3::new(
            (a.x + b.x + c.x) / 3.0 * area,
            (a.y + b.y + c.y) / 3.0 * area,
            (a.z + b.z + c.z) / 3.0 * area,
        );
        total += area;
    }
    if total > 0.0 {
        return Point3::new(acc.x / total, acc.y / total, acc.z / total);
    }
    let mut sum = Vec3::new(0.0, 0.0, 0.0);
    let mut n = 0usize;
    for p in vertices(mesh) {
        sum += Vec3::new(p.x, p.y, p.z);
        n += 1;
    }
    if n == 0 {
        return Point3::new(0.0, 0.0, 0.0);
    }
    Point3::new(sum.x / n as f64, sum.y / n as f64, sum.z / n as f64)
}

/// Project a face's triangulation onto `axis`, measured from `origin`.
fn axial_range(mesh: &TriangleMesh, origin: Point3, axis: Vec3) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in vertices(mesh) {
        let t = (p - origin).dot(axis);
        lo = lo.min(t);
        hi = hi.max(t);
    }
    if !lo.is_finite() {
        return (0.0, 0.0);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Solid;

    fn cyl_faces(report: &FaceReport) -> Vec<&FaceInfo> {
        report
            .faces
            .iter()
            .filter(|f| f.surface_type == "cylinder")
            .collect()
    }

    #[test]
    fn cube_has_six_named_planar_faces() {
        let cube = Solid::cube(10.0, 20.0, 30.0);
        let report = cube.inspect_faces().unwrap();

        assert_eq!(report.face_count, 6);
        assert!(report.named, "primitives seed a name map");
        assert!(report.faces.iter().all(|f| f.surface_type == "plane"));
        assert!(report.faces.iter().all(|f| f.stable));

        // Total area of a 10x20x30 box.
        let total: f64 = report.faces.iter().map(|f| f.area_mm2).sum();
        let expect = 2.0 * (10.0 * 20.0 + 10.0 * 30.0 + 20.0 * 30.0);
        assert!((total - expect).abs() < 1e-6, "area {total} vs {expect}");

        // The top face: outward normal +Z, area 10*20, plane at z=30.
        let top = report
            .faces
            .iter()
            .find(|f| f.id.ends_with(":top"))
            .expect("cube:top exists");
        let SurfaceInfo::Plane { normal, point } = top.surface else {
            panic!("top face is planar");
        };
        assert!((normal[2] - 1.0).abs() < 1e-9, "normal {normal:?}");
        assert!((point[2] - 30.0).abs() < 1e-9, "point {point:?}");
        assert!((top.area_mm2 - 200.0).abs() < 1e-6);
        assert!((top.centroid_mm[2] - 30.0).abs() < 1e-9);
    }

    #[test]
    fn cylinder_reports_exact_radius_and_axis() {
        // Radius 7, height 25, axis +Z.
        let cyl = Solid::cylinder(7.0, 25.0, 32);
        let report = cyl.inspect_faces().unwrap();

        let side = cyl_faces(&report);
        assert_eq!(side.len(), 1, "one lateral face");
        let SurfaceInfo::Cylinder {
            radius_mm,
            diameter_mm,
            axis,
            axial_length_mm,
            convex,
            ..
        } = side[0].surface
        else {
            panic!("cylindrical");
        };

        // Analytic, NOT tessellation-bound: exact to the bit.
        assert_eq!(radius_mm, 7.0);
        assert_eq!(diameter_mm, 14.0);
        assert!((axis[2].abs() - 1.0).abs() < 1e-12, "axis {axis:?}");
        assert!((axial_length_mm - 25.0).abs() < 1e-6);
        assert!(convex, "an outer wall is convex");

        // A 32-segment tessellation under-reads the lateral area (the
        // inscribed prism is shorter around than the circle); the analytic
        // radius does not care. That gap is why the radius is reported from
        // the surface and the area is flagged tessellation-bound.
        let exact_lateral = 2.0 * std::f64::consts::PI * 7.0 * 25.0;
        assert!(
            side[0].area_mm2 < exact_lateral,
            "tessellated {} vs exact {exact_lateral}",
            side[0].area_mm2
        );
        assert!(side[0].area_mm2 > 0.99 * exact_lateral);
    }

    #[test]
    fn bore_is_concave_and_bbox_lies_about_outer_diameter() {
        // Body OD 80 (r=40), height 30, with an off-axis boss that inflates
        // the bounding box — the MyActuator X6-60 failure mode in miniature.
        let body = Solid::cylinder(40.0, 30.0, 64);
        let boss = Solid::cylinder(6.0, 30.0, 32).translate(48.0, 0.0, 0.0);
        let part = body.union(&boss);

        let report = part.inspect_faces().unwrap();
        let group = report.largest_coaxial(None).expect("a dominant axis");

        // The bounding box reads ~108 across; the true OD is exactly 80.
        assert_eq!(group.max_diameter_mm, 80.0);
        assert!((group.axis[2].abs() - 1.0).abs() < 1e-9);

        let mesh_bbox_span: f64 = {
            let (lo, hi) = (
                report
                    .faces
                    .iter()
                    .map(|f| f.bbox_min_mm[0])
                    .fold(f64::MAX, f64::min),
                report
                    .faces
                    .iter()
                    .map(|f| f.bbox_max_mm[0])
                    .fold(f64::MIN, f64::max),
            );
            hi - lo
        };
        assert!(
            mesh_bbox_span > 90.0,
            "bbox inflated by the boss: {mesh_bbox_span}"
        );
    }

    #[test]
    fn a_drilled_hole_reads_as_a_bore() {
        // 40x40x10 plate with a 5 mm-diameter through hole on the Z axis at
        // the centre — the "what size is this hole" question.
        let plate = Solid::cube(40.0, 40.0, 10.0);
        let drill = Solid::cylinder(2.5, 30.0, 64).translate(20.0, 20.0, -10.0);
        let part = plate.difference(&drill);

        let report = part.inspect_faces().unwrap();
        let bores: Vec<&FaceInfo> = report
            .faces
            .iter()
            .filter(|f| matches!(f.surface, SurfaceInfo::Cylinder { .. }))
            .collect();
        assert!(!bores.is_empty(), "the hole leaves a cylindrical face");

        let SurfaceInfo::Cylinder {
            diameter_mm,
            axis_point,
            convex,
            ..
        } = bores[0].surface
        else {
            unreachable!()
        };
        assert!((diameter_mm - 5.0).abs() < 1e-9, "diameter {diameter_mm}");
        assert!(!convex, "a drilled hole is a bore, not a shaft");
        assert!((axis_point[0] - 20.0).abs() < 1e-9);
        assert!((axis_point[1] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn coaxial_query_respects_a_requested_axis() {
        // Two shafts on different axes: r=20 about Z, r=30 about X.
        let z_shaft = Solid::cylinder(20.0, 10.0, 64);
        let x_shaft = Solid::cylinder(30.0, 200.0, 64)
            .rotate(0.0, 90.0, 0.0)
            .translate(0.0, 0.0, 5.0);
        let part = z_shaft.union(&x_shaft);
        let report = part.inspect_faces().unwrap();

        let about_z = report
            .largest_coaxial(Some(Vec3::new(0.0, 0.0, 1.0)))
            .expect("a Z group");
        assert!((about_z.max_diameter_mm - 40.0).abs() < 1e-9);

        let about_x = report
            .largest_coaxial(Some(Vec3::new(1.0, 0.0, 0.0)))
            .expect("an X group");
        assert!((about_x.max_diameter_mm - 60.0).abs() < 1e-9);

        // Sign of the requested axis must not matter.
        let about_neg_x = report
            .largest_coaxial(Some(Vec3::new(-1.0, 0.0, 0.0)))
            .expect("an X group either way");
        assert_eq!(about_neg_x.max_diameter_mm, about_x.max_diameter_mm);
    }

    #[test]
    fn groups_collapse_repeated_features() {
        let report = Solid::cube(50.0, 50.0, 10.0).inspect_faces().unwrap();
        let planes = report
            .groups
            .iter()
            .find(|g| g.surface_type == "plane")
            .unwrap();
        assert_eq!(planes.count, 6);
        assert!(planes.example_face_ids.len() <= 8);
    }

    #[test]
    fn mesh_only_solids_fail_closed() {
        let mesh = Solid::from_mesh(Solid::cube(5.0, 5.0, 5.0).to_mesh(32));
        assert_eq!(mesh.inspect_faces().unwrap_err(), FaceQueryError::NoBRep);
    }
}
