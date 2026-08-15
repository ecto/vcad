#![warn(missing_docs)]

//! Feature recognition over B-rep solids.
//!
//! Vendor CAD arrives as a wall of faces. Designing a mount for a purchased
//! actuator starts by reading its bolt pattern off that wall: "eight M4 holes
//! on a Ø98 bolt circle at 22.5° + 45n". This crate answers that question.
//!
//! The pipeline is deliberately boring:
//!
//! 1. Collect every cylindrical face — radius, axis direction, a point on the
//!    axis, axial extent, and whether the material is inside (a boss) or
//!    outside (a hole).
//! 2. Merge faces that share the same *axis line* and radius. A single hole is
//!    usually two half-cylinder faces split at the surface seam.
//! 3. Group by axis direction, then cluster by radius. Equal-radius coaxial
//!    holes are the candidates for one pattern.
//! 4. Project the axis positions onto the plane normal to the group axis and
//!    fit a circle. A good fit with n ≥ 3 is a bolt circle; a collinear,
//!    evenly-spaced run is a linear pattern; a lone big bore is a bore.
//! 5. Report every angle *relative to the pattern* — the first hole is 0° and
//!    the rest follow — plus the relation between concentric patterns (e.g.
//!    "these three dowels bisect adjacent holes of that six-hole circle").
//!
//! The relative reporting is not a nicety. Reading the RobStride RS03's three
//! output dowels as absolute angles off global X gives two different-looking
//! answers for the same part depending on how the STEP happened to be placed;
//! read against the output bolt circle they are always "bisecting adjacent
//! holes".
//!
//! ```no_run
//! # use vcad_kernel_features::recognize;
//! # fn f(brep: &vcad_kernel_primitives::BRepSolid) {
//! let report = recognize(brep);
//! for pattern in &report.patterns {
//!     println!("{}", pattern.describe());
//! }
//! # }
//! ```

use serde::Serialize;
use vcad_kernel_geom::{CylinderSurface, SurfaceKind};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{HalfEdgeId, Orientation};

mod tolerance {
    /// Two axis directions are parallel when their angle is under this (radians, ≈0.5°).
    pub const AXIS_ANGLE: f64 = 0.0087;
    /// Two axis lines are the same line when their lateral offset is under this (mm).
    pub const AXIS_OFFSET: f64 = 1e-3;
    /// Radii within this (mm) belong to the same cluster.
    pub const RADIUS: f64 = 1e-3;
    /// Circle-fit residual under this fraction of the fitted radius is "on the circle".
    pub const CIRCLE_FIT_REL: f64 = 5e-3;
    /// Absolute floor for the circle-fit residual (mm), for small bolt circles.
    pub const CIRCLE_FIT_ABS: f64 = 5e-3;
    /// Angular spacings within this (degrees) count as uniform.
    pub const SPACING_DEG: f64 = 0.05;
    /// Pattern centres within this (mm) are concentric.
    pub const CONCENTRIC: f64 = 1e-2;
}

// =============================================================================
// Cylindrical faces
// =============================================================================

/// Which side of a cylindrical face the material sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Concavity {
    /// Material is outside the cylinder — a hole, bore, or pocket.
    Internal,
    /// Material is inside the cylinder — a boss, shaft, or the body OD.
    External,
}

/// One cylindrical feature: a full hole or boss, with its split faces merged.
#[derive(Debug, Clone, Serialize)]
pub struct CylindricalFeature {
    /// Faces (by index into `topology.faces` iteration order) that were merged.
    pub faces: Vec<usize>,
    /// Radius in mm.
    pub radius_mm: f64,
    /// Unit axis direction, sign-canonicalised (first significant component positive).
    #[serde(serialize_with = "ser_vec3")]
    pub axis: Vec3,
    /// A point on the axis: the foot of the axis line closest to the origin.
    #[serde(serialize_with = "ser_point3")]
    pub axis_point: Point3,
    /// Extent along `axis`, measured as a signed coordinate from `axis_point`.
    pub axial_start: f64,
    /// Upper end of the axial extent, same frame as `axial_start`.
    pub axial_end: f64,
    /// Hole or boss.
    pub concavity: Concavity,
    /// Number of separate axial runs merged onto this axis line. More than one
    /// means the cylinder is interrupted — by a counterbore, groove, or chamfer.
    pub segments: usize,
    /// Whether every merged face belongs to the solid's outer shell.
    pub on_outer_shell: bool,
}

impl CylindricalFeature {
    /// Diameter in mm.
    pub fn diameter_mm(&self) -> f64 {
        self.radius_mm * 2.0
    }

    /// Axial length in mm.
    pub fn length_mm(&self) -> f64 {
        self.axial_end - self.axial_start
    }
}

fn ser_vec3<S: serde::Serializer>(v: &Vec3, s: S) -> Result<S::Ok, S::Error> {
    [v.x, v.y, v.z].serialize(s)
}

fn ser_point3<S: serde::Serializer>(p: &Point3, s: S) -> Result<S::Ok, S::Error> {
    [p.x, p.y, p.z].serialize(s)
}

/// Canonicalise an axis direction so that ±d hash to the same representative.
fn canonical_axis(d: Vec3) -> Vec3 {
    let n = d.norm();
    let d = if n > 0.0 { d / n } else { Vec3::z() };
    let eps = 1e-9;
    let flip = if d.x.abs() > eps {
        d.x < 0.0
    } else if d.y.abs() > eps {
        d.y < 0.0
    } else {
        d.z < 0.0
    };
    if flip {
        -d
    } else {
        d
    }
}

/// Foot of the perpendicular from the origin onto the line `(p, dir)`.
fn axis_foot(p: Point3, dir: Vec3) -> Point3 {
    let v = p.coords();
    p - dir * v.dot(dir)
}

/// Walk a loop's half-edge ring, yielding origin vertices.
fn loop_vertices(brep: &BRepSolid, start: HalfEdgeId, out: &mut Vec<Point3>) {
    let mut he = start;
    for _ in 0..4096 {
        let Some(h) = brep.topology.half_edges.get(he) else {
            return;
        };
        if let Some(v) = brep.topology.vertices.get(h.origin) {
            out.push(v.point);
        }
        match h.next {
            Some(next) if next != start => he = next,
            _ => return,
        }
    }
}

/// Every cylindrical face of `brep`, before merging.
fn raw_cylindrical_faces(brep: &BRepSolid) -> Vec<CylindricalFeature> {
    let outer_shell = brep
        .topology
        .solids
        .get(brep.solid_id)
        .map(|s| s.outer_shell);

    let mut out = Vec::new();
    for (idx, (face_id, face)) in brep.topology.faces.iter().enumerate() {
        let Some(surface) = brep.geometry.surfaces.get(face.surface_index) else {
            continue;
        };
        if surface.surface_type() != SurfaceKind::Cylinder {
            continue;
        }
        let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() else {
            continue;
        };

        let axis = canonical_axis(*cyl.axis.as_ref());
        let axis_point = axis_foot(cyl.center, axis);

        let mut pts = Vec::new();
        if let Some(l) = brep.topology.loops.get(face.outer_loop) {
            loop_vertices(brep, l.half_edge, &mut pts);
        }
        for lid in &face.inner_loops {
            if let Some(l) = brep.topology.loops.get(*lid) {
                loop_vertices(brep, l.half_edge, &mut pts);
            }
        }
        if pts.is_empty() {
            continue;
        }
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &pts {
            let t = (p - axis_point).dot(axis);
            lo = lo.min(t);
            hi = hi.max(t);
        }

        // A cylinder surface's normal points radially outward. A face whose
        // normal is reversed therefore has material on the outside: a hole.
        let concavity = match face.orientation {
            Orientation::Reversed => Concavity::Internal,
            Orientation::Forward => Concavity::External,
        };
        let on_outer_shell = match (face.shell, outer_shell) {
            (Some(s), Some(o)) => s == o,
            // No shell bookkeeping — don't claim knowledge we don't have.
            _ => true,
        };
        let _ = face_id;

        out.push(CylindricalFeature {
            faces: vec![idx],
            radius_mm: cyl.radius,
            axis,
            axis_point,
            axial_start: lo,
            axial_end: hi,
            concavity,
            segments: 1,
            on_outer_shell,
        });
    }
    out
}

fn same_axis_line(a: &CylindricalFeature, b: &CylindricalFeature) -> bool {
    if a.axis.dot(b.axis).abs() < tolerance::AXIS_ANGLE.cos() {
        return false;
    }
    let d = b.axis_point - a.axis_point;
    let lateral = d - a.axis * d.dot(a.axis);
    lateral.norm() <= tolerance::AXIS_OFFSET
}

/// Merge faces that are the same cylinder split at a seam (or by an intersecting
/// feature) into one hole/boss.
fn merge_faces(raw: Vec<CylindricalFeature>) -> Vec<CylindricalFeature> {
    let mut merged: Vec<CylindricalFeature> = Vec::new();
    'outer: for f in raw {
        for m in merged.iter_mut() {
            if (m.radius_mm - f.radius_mm).abs() > tolerance::RADIUS
                || m.concavity != f.concavity
                || !same_axis_line(m, &f)
            {
                continue;
            }
            // Re-express f's extent in m's frame (axis directions agree up to
            // sign after canonicalisation, and axis_point is canonical too).
            let base = (f.axis_point - m.axis_point).dot(m.axis);
            let (lo, hi) = (base + f.axial_start, base + f.axial_end);
            // Merge even when the two runs don't overlap: a hole interrupted by
            // a counterbore, a groove, or a chamfer arrives as several faces on
            // one axis line, and a caller asking "where are the holes" wants one
            // answer per axis, not one per face. `segments` keeps the count.
            m.axial_start = m.axial_start.min(lo);
            m.axial_end = m.axial_end.max(hi);
            m.segments += f.segments;
            m.faces.extend_from_slice(&f.faces);
            m.on_outer_shell &= f.on_outer_shell;
            continue 'outer;
        }
        merged.push(f);
    }
    merged
}

/// Collect every cylindrical hole and boss in `brep`, seam-split faces merged.
pub fn cylindrical_features(brep: &BRepSolid) -> Vec<CylindricalFeature> {
    merge_faces(raw_cylindrical_faces(brep))
}

/// Collect cylindrical features across several solids.
///
/// Face indices are offset per solid, so they run continuously across the
/// slice in the order the solids were given.
pub fn cylindrical_features_many(breps: &[&BRepSolid]) -> Vec<CylindricalFeature> {
    let mut raw = Vec::new();
    let mut offset = 0usize;
    for brep in breps {
        for mut f in raw_cylindrical_faces(brep) {
            for idx in &mut f.faces {
                *idx += offset;
            }
            raw.push(f);
        }
        offset += brep.topology.faces.len();
    }
    merge_faces(raw)
}

// =============================================================================
// Patterns
// =============================================================================

/// One hole (or boss) within a pattern.
#[derive(Debug, Clone, Serialize)]
pub struct PatternMember {
    /// Angle about the pattern centre, measured from the first member (degrees,
    /// CCW about the pattern axis, in `[0, 360)`). Zero for non-circular patterns.
    pub angle_deg: f64,
    /// Distance from the pattern centre in mm (the bolt-circle radius, per hole).
    pub radius_from_center_mm: f64,
    /// Position of the hole axis on the pattern plane.
    #[serde(serialize_with = "ser_point3")]
    pub center: Point3,
    /// Start of the hole's axial extent, in the pattern's axial coordinate.
    pub axial_start: f64,
    /// End of the hole's axial extent, in the pattern's axial coordinate.
    pub axial_end: f64,
    /// Whether this member sits on the solid's outer shell.
    pub on_outer_shell: bool,
    /// Faces that produced this member.
    pub faces: Vec<usize>,
}

/// What shape the members form.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternKind {
    /// Three or more equal-radius holes on a common circle.
    BoltCircle {
        /// Bolt-circle diameter (BCD) in mm.
        bolt_circle_diameter_mm: f64,
        /// Uniform angular spacing in degrees, if the holes are evenly spaced.
        spacing_deg: Option<f64>,
    },
    /// A single feature — a bore, a shaft, or the body OD.
    Single,
    /// Two or more collinear, evenly-spaced holes.
    Linear {
        /// Centre-to-centre spacing in mm.
        spacing_mm: f64,
        /// Unit direction of the run, on the pattern plane.
        #[serde(serialize_with = "ser_vec3")]
        direction: Vec3,
    },
    /// Coaxial holes that fit none of the above (an irregular cluster).
    Cluster,
}

/// A recognised group of equal-radius, parallel-axis cylindrical features.
#[derive(Debug, Clone, Serialize)]
pub struct HolePattern {
    /// Shape of the group.
    pub kind: PatternKind,
    /// Hole or boss.
    pub concavity: Concavity,
    /// Hole diameter in mm (every member shares it).
    pub hole_diameter_mm: f64,
    /// Number of members.
    pub count: usize,
    /// Common axis direction.
    #[serde(serialize_with = "ser_vec3")]
    pub axis: Vec3,
    /// Pattern centre: the fitted circle centre, or the centroid.
    #[serde(serialize_with = "ser_point3")]
    pub center: Point3,
    /// Members, ordered by angle (circular) or along the run (linear).
    pub members: Vec<PatternMember>,
    /// Absolute angle of the first member about the pattern centre, measured
    /// from global +X (or +Y when the axis is X) in degrees. Present for
    /// reference only — placement-dependent, so prefer the relative angles.
    pub first_member_absolute_deg: Option<f64>,
    /// Whether every member is on the outer shell.
    pub on_outer_shell: bool,
}

impl HolePattern {
    /// A one-line human summary, in the language a mechanical designer uses.
    pub fn describe(&self) -> String {
        let (noun, plural) = match self.concavity {
            Concavity::Internal => ("hole", "holes"),
            Concavity::External => ("boss", "bosses"),
        };
        match &self.kind {
            PatternKind::BoltCircle {
                bolt_circle_diameter_mm,
                spacing_deg,
            } => {
                let spacing = match spacing_deg {
                    Some(s) => format!("{s:.4} deg spacing"),
                    None => "uneven spacing".to_string(),
                };
                format!(
                    "{} x Ø{:.3} {} on BCD {:.3}, {}",
                    self.count, self.hole_diameter_mm, plural, bolt_circle_diameter_mm, spacing
                )
            }
            PatternKind::Single => format!(
                "1 x Ø{:.3} {} (length {:.3})",
                self.hole_diameter_mm,
                noun,
                self.members
                    .first()
                    .map(|m| m.axial_end - m.axial_start)
                    .unwrap_or(0.0)
            ),
            PatternKind::Linear { spacing_mm, .. } => format!(
                "{} x Ø{:.3} {} in a line, pitch {:.3}",
                self.count, self.hole_diameter_mm, plural, spacing_mm
            ),
            PatternKind::Cluster => format!(
                "{} x Ø{:.3} parallel {} (irregular arrangement)",
                self.count, self.hole_diameter_mm, plural
            ),
        }
    }
}

/// How one pattern is clocked against another concentric, coaxial pattern.
#[derive(Debug, Clone, Serialize)]
pub struct PatternRelation {
    /// Index into [`FeatureReport::patterns`] of the reference pattern.
    pub reference: usize,
    /// Index into [`FeatureReport::patterns`] of the pattern being described.
    pub subject: usize,
    /// Smallest angle (degrees) from any reference hole to any subject hole.
    pub phase_deg: f64,
    /// True when `phase_deg` is half the reference's spacing: the subject sits
    /// midway between adjacent reference holes.
    pub bisects_adjacent: bool,
}

/// The body envelope, measured from coaxial cylinders rather than the bbox.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    /// Axis carrying the most cylindrical surface area — the part's main axis.
    #[serde(serialize_with = "ser_vec3")]
    pub dominant_axis: Vec3,
    /// A point on the dominant axis line.
    #[serde(serialize_with = "ser_point3")]
    pub dominant_axis_point: Point3,
    /// Diameter of the largest external cylinder coaxial with the dominant axis.
    ///
    /// This is the number a designer means by "body OD". The bounding box
    /// overstates it whenever the part carries an asymmetric boss or connector.
    pub body_od_mm: Option<f64>,
    /// Extent along the dominant axis, over every B-rep vertex, in mm.
    pub axial_length_mm: f64,
    /// Largest bounding-box dimension across the dominant axis, for contrast
    /// with `body_od_mm`.
    pub bbox_across_axis_mm: f64,
}

/// Everything the recogniser found.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureReport {
    /// Recognised patterns, largest count first.
    pub patterns: Vec<HolePattern>,
    /// Clocking relations between concentric coaxial patterns.
    pub relations: Vec<PatternRelation>,
    /// Body envelope.
    pub envelope: Envelope,
}

impl FeatureReport {
    /// Patterns whose members all sit on the solid's outer shell.
    ///
    /// Vendor STEP assemblies bundle internal fasteners and bearing races; their
    /// cylinders pollute a naive scrape. This is the cheap filter; the axial
    /// extents on each member let a caller apply a stricter one.
    pub fn outer_patterns(&self) -> impl Iterator<Item = &HolePattern> {
        self.patterns.iter().filter(|p| p.on_outer_shell)
    }

    /// Bolt circles only, largest count first.
    pub fn bolt_circles(&self) -> impl Iterator<Item = &HolePattern> {
        self.patterns
            .iter()
            .filter(|p| matches!(p.kind, PatternKind::BoltCircle { .. }))
    }
}

// =============================================================================
// Recognition
// =============================================================================

/// An orthonormal (u, v) basis for the plane normal to `axis`.
fn plane_basis(axis: Vec3) -> (Vec3, Vec3) {
    let seed = if axis.x.abs() < 0.9 {
        Vec3::x()
    } else {
        Vec3::y()
    };
    let u = (seed - axis * seed.dot(axis)).normalize();
    let v = axis.cross(u);
    (u, v)
}

/// Kåsa algebraic circle fit. Returns `(center_u, center_v, radius, max_residual)`.
fn fit_circle(pts: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let (mu, mv) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (mu, mv) = (mu / n, mv / n);
    let (mut suu, mut suv, mut svv, mut suz, mut svz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (pu, pv) in pts {
        let (u, v) = (pu - mu, pv - mv);
        let z = u * u + v * v;
        suu += u * u;
        suv += u * v;
        svv += v * v;
        suz += u * z;
        svz += v * z;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return None; // collinear
    }
    let cu = 0.5 * (suz * svv - svz * suv) / det;
    let cv = 0.5 * (svz * suu - suz * suv) / det;
    let (cu, cv) = (cu + mu, cv + mv);
    let radii: Vec<f64> = pts
        .iter()
        .map(|(u, v)| ((u - cu).powi(2) + (v - cv).powi(2)).sqrt())
        .collect();
    let r = radii.iter().sum::<f64>() / n;
    let resid = radii.iter().fold(0.0f64, |a, x| a.max((x - r).abs()));
    Some((cu, cv, r, resid))
}

fn wrap360(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    a
}

/// Recognise holes, bolt circles, and the body envelope in a B-rep solid.
pub fn recognize(brep: &BRepSolid) -> FeatureReport {
    recognize_many(&[brep])
}

/// Recognise features over several solids treated as one part.
///
/// This is what an assembly wants: place every component into a common frame
/// first, then recognise, so a bolt circle spanning two components (a housing
/// and its cover) is read as one pattern.
pub fn recognize_many(breps: &[&BRepSolid]) -> FeatureReport {
    let features = cylindrical_features_many(breps);

    // --- group by axis direction, then radius + concavity -------------------
    let mut axis_groups: Vec<(Vec3, Vec<usize>)> = Vec::new();
    for (i, f) in features.iter().enumerate() {
        match axis_groups
            .iter_mut()
            .find(|(a, _)| a.dot(f.axis).abs() >= tolerance::AXIS_ANGLE.cos())
        {
            Some((_, members)) => members.push(i),
            None => axis_groups.push((f.axis, vec![i])),
        }
    }

    let mut patterns = Vec::new();
    for (axis, group) in &axis_groups {
        let mut clusters: Vec<(f64, Concavity, Vec<usize>)> = Vec::new();
        for &i in group {
            let f = &features[i];
            match clusters
                .iter_mut()
                .find(|(r, c, _)| (r - f.radius_mm).abs() <= tolerance::RADIUS && *c == f.concavity)
            {
                Some((_, _, m)) => m.push(i),
                None => clusters.push((f.radius_mm, f.concavity, vec![i])),
            }
        }
        for (radius, concavity, members) in clusters {
            patterns.push(build_pattern(&features, *axis, radius, concavity, &members));
        }
    }

    patterns.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(b.hole_diameter_mm.total_cmp(&a.hole_diameter_mm))
    });

    let relations = relate(&patterns);
    let envelope = envelope(breps, &features);

    FeatureReport {
        patterns,
        relations,
        envelope,
    }
}

fn build_pattern(
    features: &[CylindricalFeature],
    axis: Vec3,
    radius: f64,
    concavity: Concavity,
    members: &[usize],
) -> HolePattern {
    let (u, v) = plane_basis(axis);
    // Common origin so the 2D coordinates and the axial coordinates of every
    // member share one frame.
    let origin = features[members[0]].axis_point;
    let proj: Vec<(f64, f64)> = members
        .iter()
        .map(|&i| {
            let d = features[i].axis_point - origin;
            (d.dot(u), d.dot(v))
        })
        .collect();

    // Widest separation in the group — used to reject the degenerate fit where
    // three nearly-collinear holes sit on a circle of enormous radius. That fit
    // has a tiny residual and is arithmetically fine; it just isn't a bolt
    // circle, and calling it one would hand a caller a meaningless BCD.
    let spread = proj
        .iter()
        .flat_map(|a| proj.iter().map(move |b| (a.0 - b.0).hypot(a.1 - b.1)))
        .fold(0.0f64, f64::max);
    let fit = fit_circle(&proj);
    let is_circle = matches!(fit, Some((_, _, r, resid))
        if members.len() >= 3
            && r <= spread * 2.0
            && resid <= (r * tolerance::CIRCLE_FIT_REL).max(tolerance::CIRCLE_FIT_ABS));

    let (center_2d, kind_center_radius) = match (is_circle, fit) {
        (true, Some((cu, cv, r, _))) => ((cu, cv), Some(r)),
        _ => {
            let n = proj.len() as f64;
            let c = proj.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
            ((c.0 / n, c.1 / n), None)
        }
    };
    let center = origin + u * center_2d.0 + v * center_2d.1;

    // Order + angles, always relative to the first member.
    let mut indexed: Vec<(usize, f64, f64)> = members
        .iter()
        .zip(&proj)
        .map(|(&i, (pu, pv))| {
            let (du, dv) = (pu - center_2d.0, pv - center_2d.1);
            (i, dv.atan2(du).to_degrees(), du.hypot(dv))
        })
        .collect();

    let kind;
    if let Some(r) = kind_center_radius {
        indexed.sort_by(|a, b| a.1.total_cmp(&b.1));
        let base = indexed[0].1;
        for e in indexed.iter_mut() {
            e.1 = wrap360(e.1 - base);
        }
        let n = indexed.len();
        let expected = 360.0 / n as f64;
        let uniform = indexed
            .iter()
            .enumerate()
            .all(|(k, e)| (e.1 - expected * k as f64).abs() <= tolerance::SPACING_DEG);
        kind = PatternKind::BoltCircle {
            bolt_circle_diameter_mm: r * 2.0,
            spacing_deg: uniform.then_some(expected),
        };
    } else if indexed.len() == 1 {
        indexed[0].1 = 0.0;
        kind = PatternKind::Single;
    } else {
        // Collinear + evenly spaced → linear pattern.
        let dir_raw = {
            let a = proj[0];
            let b = proj
                .iter()
                .max_by(|x, y| {
                    let d = |p: &(f64, f64)| (p.0 - a.0).hypot(p.1 - a.1);
                    d(x).total_cmp(&d(y))
                })
                .copied()
                .unwrap_or(a);
            (b.0 - a.0, b.1 - a.1)
        };
        let len = dir_raw.0.hypot(dir_raw.1);
        let dir = if len > 0.0 {
            (dir_raw.0 / len, dir_raw.1 / len)
        } else {
            (1.0, 0.0)
        };
        let mut ts: Vec<(usize, f64, f64)> = members
            .iter()
            .zip(&proj)
            .map(|(&i, p)| {
                let (du, dv) = (p.0 - proj[0].0, p.1 - proj[0].1);
                let t = du * dir.0 + dv * dir.1;
                let off = (du * -dir.1 + dv * dir.0).abs();
                (i, t, off)
            })
            .collect();
        ts.sort_by(|a, b| a.1.total_cmp(&b.1));
        let collinear = ts.iter().all(|e| e.2 <= tolerance::AXIS_OFFSET.max(1e-3));
        let steps: Vec<f64> = ts.windows(2).map(|w| w[1].1 - w[0].1).collect();
        let pitch = steps.iter().sum::<f64>() / steps.len() as f64;
        let even = steps.iter().all(|s| (s - pitch).abs() <= 1e-3);
        kind = if collinear && even && pitch.abs() > 1e-6 {
            PatternKind::Linear {
                spacing_mm: pitch,
                direction: u * dir.0 + v * dir.1,
            }
        } else {
            PatternKind::Cluster
        };
        indexed = ts
            .into_iter()
            .map(|(i, _, _)| {
                let d = features[i].axis_point - center;
                (i, 0.0, (d - axis * d.dot(axis)).norm())
            })
            .collect();
    }

    let first_absolute = matches!(kind, PatternKind::BoltCircle { .. }).then(|| {
        let d = features[indexed[0].0].axis_point - center;
        wrap360(d.dot(v).atan2(d.dot(u)).to_degrees())
    });

    let members_out: Vec<PatternMember> = indexed
        .iter()
        .map(|&(i, ang, rad)| {
            let f = &features[i];
            let base = (f.axis_point - center).dot(axis);
            PatternMember {
                angle_deg: ang,
                radius_from_center_mm: rad,
                center: f.axis_point,
                axial_start: base + f.axial_start,
                axial_end: base + f.axial_end,
                on_outer_shell: f.on_outer_shell,
                faces: f.faces.clone(),
            }
        })
        .collect();

    HolePattern {
        kind,
        concavity,
        hole_diameter_mm: radius * 2.0,
        count: members_out.len(),
        axis,
        center,
        on_outer_shell: members_out.iter().all(|m| m.on_outer_shell),
        members: members_out,
        first_member_absolute_deg: first_absolute,
    }
}

/// Clocking between concentric coaxial bolt circles.
fn relate(patterns: &[HolePattern]) -> Vec<PatternRelation> {
    let mut out = Vec::new();
    for (i, reference) in patterns.iter().enumerate() {
        let PatternKind::BoltCircle { spacing_deg, .. } = reference.kind else {
            continue;
        };
        for (j, subject) in patterns.iter().enumerate() {
            if i == j || !matches!(subject.kind, PatternKind::BoltCircle { .. }) {
                continue;
            }
            if reference.axis.dot(subject.axis).abs() < tolerance::AXIS_ANGLE.cos() {
                continue;
            }
            let d = subject.center - reference.center;
            let lateral = d - reference.axis * d.dot(reference.axis);
            if lateral.norm() > tolerance::CONCENTRIC {
                continue;
            }
            // Absolute angles cancel: measure each pattern's holes in the same
            // basis and take the smallest reference→subject step.
            let (u, v) = plane_basis(reference.axis);
            let ang = |m: &PatternMember| {
                let d = m.center - reference.center;
                wrap360(d.dot(v).atan2(d.dot(u)).to_degrees())
            };
            let refs: Vec<f64> = reference.members.iter().map(ang).collect();
            let mut phase = f64::INFINITY;
            for m in &subject.members {
                let a = ang(m);
                for r in &refs {
                    let mut d = wrap360(a - r);
                    if d > 180.0 {
                        d = 360.0 - d;
                    }
                    phase = phase.min(d);
                }
            }
            let bisects = match spacing_deg {
                Some(s) => (phase - s / 2.0).abs() <= 0.5,
                None => false,
            };
            out.push(PatternRelation {
                reference: i,
                subject: j,
                phase_deg: phase,
                bisects_adjacent: bisects,
            });
        }
    }
    out
}

fn envelope(breps: &[&BRepSolid], features: &[CylindricalFeature]) -> Envelope {
    // Dominant axis LINE: the one carrying the most cylindrical lateral area.
    let mut lines: Vec<(Vec3, Point3, f64)> = Vec::new();
    for f in features {
        let area = 2.0 * std::f64::consts::PI * f.radius_mm * f.length_mm().abs();
        match lines.iter_mut().find(|(a, p, _)| {
            a.dot(f.axis).abs() >= tolerance::AXIS_ANGLE.cos() && {
                let d = f.axis_point - *p;
                (d - *a * d.dot(a)).norm() <= tolerance::AXIS_OFFSET
            }
        }) {
            Some((_, _, acc)) => *acc += area,
            None => lines.push((f.axis, f.axis_point, area)),
        }
    }
    let (axis, axis_point) = lines
        .iter()
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(a, p, _)| (*a, *p))
        .unwrap_or((Vec3::z(), Point3::origin()));

    let coaxial = |f: &&CylindricalFeature| {
        f.axis.dot(axis).abs() >= tolerance::AXIS_ANGLE.cos() && {
            let d = f.axis_point - axis_point;
            (d - axis * d.dot(axis)).norm() <= tolerance::AXIS_OFFSET
        }
    };
    let body_od_mm = features
        .iter()
        .filter(coaxial)
        .filter(|f| f.concavity == Concavity::External)
        .map(|f| f.diameter_mm())
        .fold(None, |acc: Option<f64>, d| {
            Some(acc.map_or(d, |a| a.max(d)))
        });

    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut across = 0.0f64;
    for vtx in breps
        .iter()
        .flat_map(|b| b.topology.vertices.iter().map(|(_, v)| v))
    {
        let d = vtx.point - axis_point;
        let t = d.dot(axis);
        lo = lo.min(t);
        hi = hi.max(t);
        across = across.max((d - axis * t).norm());
    }
    let axial_length_mm = if lo.is_finite() { hi - lo } else { 0.0 };

    Envelope {
        dominant_axis: axis,
        dominant_axis_point: axis_point,
        body_od_mm,
        axial_length_mm,
        bbox_across_axis_mm: across * 2.0,
    }
}

/// Recognise features in each solid of a STEP assembly.
///
/// Faces carry no cross-solid identity, so each solid gets its own report;
/// the caller decides which one is the part it cares about.
pub fn recognize_all(breps: &[&BRepSolid]) -> Vec<FeatureReport> {
    breps.iter().map(|b| recognize(b)).collect()
}

#[cfg(feature = "step")]
pub mod step;

#[cfg(test)]
mod tests;
