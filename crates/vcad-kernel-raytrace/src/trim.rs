//! Trim testing for determining if a UV point lies within a face boundary.
//!
//! BRep faces are bounded by trim loops that define valid regions of the
//! underlying surface. This module tests whether intersection points are
//! within these boundaries.

use vcad_kernel_geom::Surface;
use vcad_kernel_math::Point2;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation};

/// A face's trim boundary, projected into UV once and reusable for every
/// subsequent point test.
///
/// Building this is the expensive half of [`point_in_face`]: every loop
/// vertex is inverse-projected onto the surface (Newton iteration for
/// B-spline and bilinear faces), pole vertices are repaired, and degenerate
/// caps are re-synthesised from the adjacent surface. None of it depends on
/// the query point, so a ray tracer that tests millions of hits against the
/// same face should do it once — see [`FaceTrim::build`] and
/// [`FaceTrim::contains`].
#[derive(Debug, Clone)]
pub struct FaceTrim {
    /// The face spans its whole surface: the outer loop is only a seam.
    untrimmed: bool,
    /// Outer boundary in UV. Meaningless when `untrimmed`.
    outer: Vec<Point2>,
    /// For an untrimmed cylinder or cone, the extent of the loop along the
    /// unbounded `v` parameter.
    v_range: Option<(f64, f64)>,
    /// Hole boundaries in UV.
    inners: Vec<Vec<Point2>>,
    /// Set when the loops were unwrapped across a periodic surface's seam
    /// (see [`PeriodicTrim`]); it then decides containment on its own.
    periodic: Option<PeriodicTrim>,
}

impl FaceTrim {
    /// Project a face's trim loops into UV.
    pub fn build(brep: &BRepSolid, face_id: FaceId) -> Self {
        let topo = &brep.topology;
        let face = &topo.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];

        if let Some(periodic) = PeriodicTrim::build(brep, face_id) {
            return Self {
                untrimmed: false,
                outer: Vec::new(),
                v_range: None,
                inners: Vec::new(),
                periodic: Some(periodic),
            };
        }

        // Get UV coordinates of the outer loop vertices
        let raw_uvs = loop_uv_coords(brep, face.outer_loop, surface.as_ref());

        // A closed surface covering the whole primitive (a full sphere or
        // torus) is bounded only by its seam: the outer loop projects to a
        // zero-area polygon in UV, which would reject every hit. Treat a
        // degenerate outer loop as "untrimmed" — the face spans the entire
        // surface — and still honour inner loops (holes) below.
        //
        // The degeneracy verdict is taken on the *raw* projection, before any
        // pole repair: a full sphere's seam loop passes through both poles,
        // and repairing those would hand it a non-zero area and reject every
        // hit.
        let mut untrimmed = polygon_area(&raw_uvs).abs() < 1e-9;
        let mut outer = if untrimmed {
            raw_uvs
        } else {
            repair_pole_vertices(surface.as_ref(), &raw_uvs)
        };

        // A planar cap bounded by a single closed circle edge (cylinder/cone
        // caps) projects to a degenerate UV polygon too — but "untrimmed" on
        // a plane means the infinite plane. Rebuild the circle from the
        // adjacent surface instead.
        if untrimmed {
            if let Some(poly) = synthesize_planar_cap_polygon(brep, face_id) {
                outer = poly;
                untrimmed = false;
            }
        }

        // On a cylinder or cone, v is an unbounded length parameter, so a
        // seam-degenerate loop (e.g. a full cylinder wall: only seam vertices
        // survive projection, the rim circles collapse) must still clamp v to
        // the loop's extent — otherwise the wall traces as an infinite
        // cylinder. u legitimately wraps the full turn.
        let v_range = if untrimmed {
            wall_v_range(brep, face_id, &outer)
        } else {
            None
        };

        let inners = face
            .inner_loops
            .iter()
            .map(|&inner_loop| loop_uv_coords(brep, inner_loop, surface.as_ref()))
            .collect();

        Self {
            untrimmed,
            outer,
            v_range,
            inners,
            periodic: None,
        }
    }

    /// Is `uv` inside the outer boundary and outside every hole?
    pub fn contains(&self, uv: Point2) -> bool {
        if let Some(periodic) = &self.periodic {
            return periodic.contains(uv);
        }
        if self.untrimmed {
            if let Some((v_min, v_max)) = self.v_range {
                if uv.y < v_min || uv.y > v_max {
                    return false;
                }
            }
        } else if !point_in_polygon(&wrap_u_into_polygon(uv, &self.outer), &self.outer) {
            return false;
        }

        // Inside a hole is outside the face.
        !self.inners.iter().any(|inner| point_in_polygon(&uv, inner))
    }
}

/// Trim polygons for a face on a surface that is periodic in `u` (cylinder,
/// cone, sphere) or in both `u` and `v` (torus), built on the unwrapped
/// covering of parameter space.
///
/// Projection reports `u` in `[0, 2π)`, so a loop that crosses the surface's
/// own `u = 0` line jumps a whole turn mid-polygon and folds across the
/// domain — and a STEP writer is free to put its circles' start points (and
/// its seam, if it has one) anywhere. Onshape does exactly that, and its
/// circles arrive subdivided into many vertices, so nearly every curved face
/// of an Onshape part rejected every hit. Unwrapping each loop onto one
/// continuous branch fixes contractible loops.
///
/// A seamless wall's loops are not contractible: each circle winds a whole
/// turn and never closes in UV. Two such rings bound a strip, closed here
/// into a polygon by joining the rings at matching `u`; a lone ring is closed
/// against the cone's apex (the apex `VERTEX_LOOP` carries no edges and is
/// dropped on import) or the sphere's pole.
#[derive(Debug, Clone)]
struct PeriodicTrim {
    /// Regions of the face, each a closed polygon on the unwrapped branch.
    regions: Vec<Vec<Point2>>,
    /// Holes, likewise.
    holes: Vec<Vec<Point2>>,
    /// The face covers the whole surface outside its holes: a sphere or
    /// torus whose every loop winds the hole way round.
    whole: bool,
    /// Whether `v` is periodic too (a torus).
    v_periodic: bool,
}

/// Whole-turn shifts tried when testing a query against an unwrapped
/// polygon. Unwrapping starts from the projected (`[0, 2π)`) first vertex
/// and a loop spans at most a turn, so ±2 always reaches it.
const BRANCH_SHIFTS: std::ops::RangeInclusive<i32> = -2..=2;

impl PeriodicTrim {
    fn build(brep: &BRepSolid, face_id: FaceId) -> Option<Self> {
        use vcad_kernel_geom::SurfaceKind;
        let topo = &brep.topology;
        let face = &topo.faces[face_id];
        let surface = brep.geometry.surfaces[face.surface_index].as_ref();
        let kind = surface.surface_type();
        let v_periodic = match kind {
            SurfaceKind::Cylinder | SurfaceKind::Cone | SurfaceKind::Sphere => false,
            SurfaceKind::Torus => true,
            _ => return None,
        };

        let mut contractible = Vec::new();
        let mut rings = Vec::new();
        for (i, &loop_id) in std::iter::once(&face.outer_loop)
            .chain(&face.inner_loops)
            .enumerate()
        {
            let raw = loop_uv_coords(brep, loop_id, surface);
            // Single-vertex circles and pole-touching sphere loops have no
            // usable longitude; the non-periodic path already handles them.
            if raw.len() < 2 {
                return None;
            }
            if kind == SurfaceKind::Sphere
                && raw
                    .iter()
                    .any(|p| (p.y.abs() - std::f64::consts::FRAC_PI_2).abs() < POLE_V_EPS)
            {
                return None;
            }
            let (pts, turns) = unwrap_loop(&raw, v_periodic);
            match turns {
                0 => contractible.push((i == 0, pts)),
                t if t.abs() == 1 => rings.push(if t < 0 { reverse_loop(pts) } else { pts }),
                _ => return None,
            }
        }

        // A sphere or torus is closed, so a contractible loop may bound
        // either side of itself — Onshape writes a ball with two flats as one
        // face whose two loops are both holes. Which side is the face is set
        // by the loop's winding against the face's orientation (interior to
        // the left), not by which loop STEP happened to list first.
        let closed = matches!(kind, SurfaceKind::Sphere | SurfaceKind::Torus);

        // Nothing wrapped: the plain polygon path is already right.
        let any_wrap = !rings.is_empty()
            || contractible.iter().any(|(_, pts)| {
                pts.iter()
                    .any(|p| !(0.0..std::f64::consts::TAU).contains(&p.x))
            });
        if !closed && !any_wrap {
            return None;
        }

        let sense = match face.orientation {
            Orientation::Forward => 1.0,
            Orientation::Reversed => -1.0,
        };
        let mut regions = Vec::new();
        let mut holes = Vec::new();
        for (is_outer, pts) in contractible {
            let area = polygon_area(&pts);
            if closed && area.abs() < 1e-9 {
                // A seam-only loop bounds nothing.
                continue;
            }
            let is_region = if closed { area * sense > 0.0 } else { is_outer };
            if is_region {
                regions.push(pts);
            } else {
                holes.push(pts);
            }
        }
        let whole = closed && regions.is_empty() && rings.is_empty();

        if !rings.is_empty() {
            if !regions.is_empty() {
                // A contractible outer loop *and* whole-turn rings: not a
                // shape a valid face takes.
                return None;
            }
            let mean_v = |r: &Vec<Point2>| r.iter().map(|p| p.y).sum::<f64>() / r.len() as f64;
            rings.sort_by(|a, b| mean_v(a).total_cmp(&mean_v(b)));
            if rings.len() % 2 == 1 {
                let cap = match kind {
                    SurfaceKind::Cone => 0.0,
                    SurfaceKind::Sphere => {
                        // Close toward whichever pole the lone ring is nearer.
                        let v = mean_v(&rings[0]);
                        std::f64::consts::FRAC_PI_2.copysign(v)
                    }
                    _ => return None,
                };
                let ring = if kind == SurfaceKind::Cone && mean_v(&rings[0]) < 0.0 {
                    rings.remove(0)
                } else {
                    rings.pop()?
                };
                regions.push(cap_polygon(&ring, cap));
            }
            for pair in rings.chunks(2) {
                regions.push(strip_polygon(&pair[0], &pair[1]));
            }
        }

        Some(Self {
            regions,
            holes,
            whole,
            v_periodic,
        })
    }

    fn contains(&self, uv: Point2) -> bool {
        let hit = |poly: &Vec<Point2>| self.on_any_branch(uv, poly);
        (self.whole || self.regions.iter().any(hit)) && !self.holes.iter().any(hit)
    }

    fn on_any_branch(&self, uv: Point2, poly: &[Point2]) -> bool {
        let tau = std::f64::consts::TAU;
        let v_shifts = if self.v_periodic {
            BRANCH_SHIFTS
        } else {
            0..=0
        };
        BRANCH_SHIFTS.into_iter().any(|ku| {
            v_shifts.clone().any(|kv| {
                let q = Point2::new(uv.x + ku as f64 * tau, uv.y + kv as f64 * tau);
                point_in_polygon(&q, poly)
            })
        })
    }
}

/// A periodic face's trim in the GPU tracer's shape: at most one region
/// polygon plus holes, all on the unwrapped branch.
///
/// The shader tests whole-turn shifts of the query against these (see
/// `point_in_face` in `brep.wgsl`), mirroring [`FaceTrim::contains`].
#[derive(Debug, Clone)]
pub struct PeriodicGpuTrim {
    /// The region, or `None` when the face is the whole surface outside its
    /// holes.
    pub outer: Option<Vec<Point2>>,
    /// Hole polygons.
    pub holes: Vec<Vec<Point2>>,
    /// Whether `v` wraps too (a torus).
    pub v_periodic: bool,
}

/// The [`PeriodicGpuTrim`] for a face, or `None` when the face takes the
/// ordinary (non-periodic) trim path or needs more than one region polygon.
pub fn periodic_gpu_trim(brep: &BRepSolid, face_id: FaceId) -> Option<PeriodicGpuTrim> {
    let mut t = PeriodicTrim::build(brep, face_id)?;
    let outer = match (t.whole, t.regions.len()) {
        (true, 0) => None,
        (false, 1) => t.regions.pop(),
        _ => return None,
    };
    Some(PeriodicGpuTrim {
        outer,
        holes: t.holes,
        v_periodic: t.v_periodic,
    })
}

/// Unwrap a loop's `u` (and, on a torus, `v`) onto one continuous branch,
/// each step taking the shortest turn. Returns the unwrapped points and how
/// many whole turns the closed loop winds in `u` — `0` for an ordinary
/// boundary, `±1` for a full circle around the axis.
fn unwrap_loop(raw: &[Point2], v_periodic: bool) -> (Vec<Point2>, i32) {
    let tau = std::f64::consts::TAU;
    let nearest = |x: f64, last: f64| x - ((x - last) / tau).round() * tau;
    let mut out = Vec::with_capacity(raw.len());
    let mut last = raw[0];
    out.push(last);
    for p in &raw[1..] {
        let u = nearest(p.x, last.x);
        let v = if v_periodic {
            nearest(p.y, last.y)
        } else {
            p.y
        };
        last = Point2::new(u, v);
        out.push(last);
    }
    // The closing edge back to the first vertex settles the winding.
    let closing_u = nearest(raw[0].x, last.x);
    let turns = ((closing_u - out[0].x) / tau).round() as i32;
    (out, turns)
}

/// A loop traversed backwards, so a ring winds `+u`.
fn reverse_loop(mut pts: Vec<Point2>) -> Vec<Point2> {
    pts.reverse();
    pts
}

/// Close two `+u`-winding rings into the strip polygon between them: ring
/// `a` out one full turn, across to ring `b` at the same `u`, and `b` back.
fn strip_polygon(a: &[Point2], b: &[Point2]) -> Vec<Point2> {
    let tau = std::f64::consts::TAU;
    let a0 = a[0].x;
    // Start `b` at the vertex nearest `a`'s start, shifted onto its branch.
    let offset = |x: f64| {
        let d = (x - a0).rem_euclid(tau);
        d.min(tau - d)
    };
    let start = (0..b.len())
        .min_by(|&i, &j| offset(b[i].x).total_cmp(&offset(b[j].x)))
        .unwrap_or(0);
    let raw: Vec<Point2> = (0..b.len()).map(|k| b[(start + k) % b.len()]).collect();
    let (mut b_un, _) = unwrap_loop(&raw, false);
    let shift = ((a0 - b_un[0].x) / tau).round() * tau;
    for p in &mut b_un {
        p.x += shift;
    }

    let mut poly = a.to_vec();
    poly.push(Point2::new(a0 + tau, a[0].y));
    poly.push(Point2::new(b_un[0].x + tau, b_un[0].y));
    poly.extend(b_un.iter().rev());
    poly
}

/// Close a `+u`-winding ring against the line `v = cap` (a cone's apex or a
/// sphere's pole).
fn cap_polygon(ring: &[Point2], cap: f64) -> Vec<Point2> {
    let tau = std::f64::consts::TAU;
    let a0 = ring[0].x;
    let mut poly = ring.to_vec();
    poly.push(Point2::new(a0 + tau, ring[0].y));
    poly.push(Point2::new(a0 + tau, cap));
    poly.push(Point2::new(a0, cap));
    poly
}

/// Test if a UV point is inside a face's trim boundaries.
///
/// Returns `true` if the point is inside the outer loop and outside all inner loops (holes).
///
/// This reprojects the face's loops on every call. Callers testing the same
/// face repeatedly — the ray tracer, above all — should hold a [`FaceTrim`]
/// instead.
pub fn point_in_face(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> bool {
    FaceTrim::build(brep, face_id).contains(uv)
}

/// Latitude magnitude above which a spherical loop vertex counts as sitting
/// on a pole, where longitude is indeterminate.
const POLE_V_EPS: f64 = 1e-7;

/// Repair a spherical face's UV trim polygon around pole vertices and the
/// longitude seam.
///
/// On a sphere, `u` (longitude) is meaningless at a pole: every meridian
/// meets there, and `project_to_sphere` reports `u = 0` for want of anything
/// better. Straight-line point-in-polygon then draws the boundary to
/// `u = 0` instead of up the two meridians that actually bound the patch.
///
/// The fillet corner blend is exactly this case. Each convex corner of a
/// filleted box gets a spherical octant whose three vertices are the
/// tangency points with the three faces; the first of those lands on the
/// pole (the surface's `axis` is that face's normal), so the loop projects
/// to the right triangle `(0, π/2), (0, 0), (π/2, 0)` when the true patch is
/// the *square* `[0, π/2] × [0, π/2]`. The hypotenuse cuts away half the
/// octant, and rays through the missing half pass into the solid — the
/// crescent-shaped hole visible at every corner of a ray-traced filleted
/// part, right where the three fillet cylinders meet.
///
/// The repair replaces each pole vertex with two vertices carrying the
/// longitudes of its neighbours, so the boundary runs up one meridian,
/// across the pole, and back down the other. Longitudes are also unwrapped
/// onto one continuous branch first, so a patch straddling the `u = 0` seam
/// stays a single polygon instead of folding across the whole domain.
///
/// Non-spherical surfaces are returned unchanged.
fn repair_pole_vertices(surface: &dyn Surface, uvs: &[Point2]) -> Vec<Point2> {
    use vcad_kernel_geom::SurfaceKind;
    if surface.surface_type() != SurfaceKind::Sphere || uvs.len() < 3 {
        return uvs.to_vec();
    }
    let half_pi = std::f64::consts::FRAC_PI_2;
    let is_pole = |uv: &Point2| (uv.y.abs() - half_pi).abs() < POLE_V_EPS;

    // Nothing to do when the loop avoids both poles — but still unwrap, so a
    // seam-straddling patch tests as one contiguous polygon.
    let n = uvs.len();
    let anchor = (0..n).find(|&i| !is_pole(&uvs[i]));
    let Some(anchor) = anchor else {
        return uvs.to_vec();
    };

    // Unwrap longitudes of the non-pole vertices onto a continuous branch,
    // walking the loop from the anchor so each step takes the shortest turn.
    let tau = std::f64::consts::TAU;
    let mut unwrapped: Vec<Option<f64>> = vec![None; n];
    unwrapped[anchor] = Some(uvs[anchor].x);
    let mut last = uvs[anchor].x;
    for step in 1..n {
        let i = (anchor + step) % n;
        if is_pole(&uvs[i]) {
            continue;
        }
        let mut u = uvs[i].x;
        while u - last > std::f64::consts::PI {
            u -= tau;
        }
        while last - u > std::f64::consts::PI {
            u += tau;
        }
        unwrapped[i] = Some(u);
        last = u;
    }

    // Emit, expanding each pole vertex into the two meridian endpoints that
    // bracket it in loop order.
    let prev_nonpole = |i: usize| -> f64 {
        (1..=n)
            .find_map(|k| unwrapped[(i + n - k) % n])
            .unwrap_or(0.0)
    };
    let next_nonpole =
        |i: usize| -> f64 { (1..=n).find_map(|k| unwrapped[(i + k) % n]).unwrap_or(0.0) };

    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        if let Some(u) = unwrapped[i] {
            out.push(Point2::new(u, uvs[i].y));
            continue;
        }
        let v = if uvs[i].y > 0.0 { half_pi } else { -half_pi };
        out.push(Point2::new(prev_nonpole(i), v));
        out.push(Point2::new(next_nonpole(i), v));
    }
    out
}

/// Shift a query longitude onto the branch spanned by a repaired polygon.
///
/// [`repair_pole_vertices`] unwraps longitudes, so a polygon may live on
/// `[-π/2, π/2]` while `project_to_sphere` reports the hit at `3π/2`. The
/// polygon's `u` extent is always under a full turn, so at most one whole-turn
/// shift can land inside.
fn wrap_u_into_polygon(uv: Point2, polygon: &[Point2]) -> Point2 {
    let tau = std::f64::consts::TAU;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in polygon {
        lo = lo.min(p.x);
        hi = hi.max(p.x);
    }
    if !lo.is_finite() || hi - lo >= tau {
        return uv;
    }
    // Map u into [lo, lo + τ). Since the polygon spans under a full turn,
    // that interval contains [lo, hi] and any u inside the patch lands there.
    let u = uv.x - ((uv.x - lo) / tau).floor() * tau;
    Point2::new(u, uv.y)
}

/// Signed area of a UV polygon (shoelace). Near-zero means the loop is
/// degenerate in parameter space — e.g. a closed surface's seam-only loop.
pub(crate) fn polygon_area(polygon: &[Point2]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..polygon.len() {
        let a = &polygon[i];
        let b = &polygon[(i + 1) % polygon.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area / 2.0
}

/// For surfaces whose v parameter is an unbounded length (cylinder, cone),
/// return the v-range spanned by a degenerate outer loop's vertices. Returns
/// `None` for surfaces with intrinsically bounded v (sphere, torus, plane…),
/// where the untrimmed fallback is safe as-is, or when the loop is empty.
pub(crate) fn unbounded_v_range(surface: &dyn Surface, outer_uvs: &[Point2]) -> Option<(f64, f64)> {
    use vcad_kernel_geom::SurfaceKind;
    match surface.surface_type() {
        SurfaceKind::Cylinder | SurfaceKind::Cone => {
            let mut it = outer_uvs.iter();
            let first = it.next()?;
            let (mut v_min, mut v_max) = (first.y, first.y);
            for uv in it {
                v_min = v_min.min(uv.y);
                v_max = v_max.max(uv.y);
            }
            Some((v_min, v_max))
        }
        _ => None,
    }
}

/// [`unbounded_v_range`] for an untrimmed cylinder/cone wall, taken over
/// *every* loop of the face rather than the outer one alone.
///
/// A seamless wall (as Onshape and other STEP writers emit it) has no seam
/// edge: it is bounded by two circular loops, outer plus one inner, each at a
/// single `v`. The outer loop alone spans zero `v`, which would reject every
/// hit — so the far ring's `v` comes from the inner loop. A drill-tip cone
/// has only its base ring (the apex `VERTEX_LOOP` carries no edges and is
/// dropped on import), so a cone whose rings span zero `v` extends to the
/// apex at `v = 0`.
pub(crate) fn wall_v_range(
    brep: &BRepSolid,
    face_id: FaceId,
    outer_uvs: &[Point2],
) -> Option<(f64, f64)> {
    use vcad_kernel_geom::SurfaceKind;
    let face = &brep.topology.faces[face_id];
    let surface = brep.geometry.surfaces[face.surface_index].as_ref();
    let mut uvs = outer_uvs.to_vec();
    for &inner in &face.inner_loops {
        uvs.extend(loop_uv_coords(brep, inner, surface));
    }
    let (v_min, v_max) = unbounded_v_range(surface, &uvs)?;
    if surface.surface_type() == SurfaceKind::Cone
        && v_max - v_min <= 1e-9 * v_min.abs().max(v_max.abs()).max(1.0)
    {
        return Some((v_min.min(0.0), v_max.max(0.0)));
    }
    Some((v_min, v_max))
}

/// Number of segments used when a degenerate cap loop is rebuilt as a
/// sampled circle polygon.
const CAP_CIRCLE_SEGMENTS: usize = 64;

/// Rebuild the trim polygon for a planar face whose outer loop is degenerate
/// in UV — a cap bounded by a single closed circle edge (e.g. a cylinder or
/// cone cap). Only loop *vertices* project to UV, so a full-circle edge
/// collapses to a point and the cap would either be rejected (GPU) or spill
/// to the whole infinite plane (CPU untrimmed fallback).
///
/// The circle is recovered from the adjacent surface across the loop's twin
/// half-edge: the surface's axis pierced through the cap plane gives the
/// center, and the loop vertex gives the radius. Returns a sampled polygon
/// in the plane's (orthonormal, hence isometric) UV space, or `None` when
/// the face isn't planar or no axis-bearing neighbour is found.
pub(crate) fn synthesize_planar_cap_polygon(
    brep: &BRepSolid,
    face_id: FaceId,
) -> Option<Vec<Point2>> {
    use vcad_kernel_geom::SurfaceKind;

    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;

    for he_id in topo.loop_half_edges(face.outer_loop) {
        let he = &topo.half_edges[he_id];
        let Some(twin_id) = he.twin else { continue };
        let Some(loop_id) = topo.half_edges[twin_id].loop_id else {
            continue;
        };
        let Some(nbr_face_id) = topo.loops[loop_id].face else {
            continue;
        };
        let nbr_surface = &brep.geometry.surfaces[topo.faces[nbr_face_id].surface_index];

        // Axis (point + direction) of the neighbouring surface of revolution.
        let (axis_point, axis_dir) = match nbr_surface.surface_type() {
            SurfaceKind::Cylinder => {
                let cyl = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::CylinderSurface>()?;
                (cyl.center, *cyl.axis.as_ref())
            }
            SurfaceKind::Cone => {
                let cone = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::ConeSurface>()?;
                (cone.apex, *cone.axis.as_ref())
            }
            SurfaceKind::Sphere => {
                let sph = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::SphereSurface>()?;
                (sph.center, *sph.axis.as_ref())
            }
            SurfaceKind::Torus => {
                let torus = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::TorusSurface>()?;
                (torus.center, *torus.axis.as_ref())
            }
            _ => continue,
        };

        // Pierce the axis through the cap plane to get the circle center;
        // if the axis is parallel to the plane, project the axis point.
        let n = plane.normal_dir.as_ref();
        let denom = axis_dir.dot(n);
        let center = if denom.abs() > 1e-12 {
            let t = (plane.origin - axis_point).dot(n) / denom;
            axis_point + axis_dir * t
        } else {
            axis_point - *n * (axis_point - plane.origin).dot(n)
        };

        let vertex = topo.vertices[he.origin].point;
        let radius = (vertex - center).norm();
        if radius < 1e-12 {
            continue;
        }

        let center_uv = plane.project(&center);
        let mut poly = Vec::with_capacity(CAP_CIRCLE_SEGMENTS);
        for i in 0..CAP_CIRCLE_SEGMENTS {
            let theta = std::f64::consts::TAU * (i as f64) / (CAP_CIRCLE_SEGMENTS as f64);
            poly.push(Point2::new(
                center_uv.x + radius * theta.cos(),
                center_uv.y + radius * theta.sin(),
            ));
        }
        return Some(poly);
    }

    None
}

/// Project a 3D point onto the parameter space of a face's surface.
///
/// The inverse of evaluating the face's surface — the coordinates
/// [`point_in_face`] expects. Exposed so callers can ask whether a specific
/// surface point falls inside a face's trim boundary.
pub fn project_face_uv(
    brep: &BRepSolid,
    face_id: FaceId,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    project_to_surface_uv(surface.as_ref(), point)
}

/// Get the UV coordinates of vertices in a loop by projecting 3D positions onto the surface.
fn loop_uv_coords(
    brep: &BRepSolid,
    loop_id: vcad_kernel_topo::LoopId,
    surface: &dyn Surface,
) -> Vec<Point2> {
    let topo = &brep.topology;

    topo.loop_half_edges(loop_id)
        .map(|he_id| {
            let v_id = topo.half_edges[he_id].origin;
            let point = topo.vertices[v_id].point;
            project_to_surface_uv(surface, &point)
        })
        .collect()
}

/// Project a 3D point onto a surface's UV parameter space.
///
/// This is an inverse evaluation that finds (u, v) such that surface.evaluate(u, v) ≈ point.
fn project_to_surface_uv(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    use vcad_kernel_geom::SurfaceKind;

    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                return plane.project(point);
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                return project_to_cylinder(cyl, point);
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                return project_to_sphere(sph, point);
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                return project_to_cone(cone, point);
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                return project_to_torus(torus, point);
            }
        }
        _ => {}
    }

    // Fallback: Newton iteration for general surfaces
    project_newton(surface, point)
}

fn project_to_cylinder(
    cyl: &vcad_kernel_geom::CylinderSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cyl.axis.as_ref();
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - cyl.center;
    let v = to_point.dot(axis);

    let proj = to_point - v * axis;
    let x = proj.dot(ref_dir);
    let y = proj.dot(y_dir);

    let u = y.atan2(x);
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    Point2::new(u, v)
}

fn project_to_sphere(
    sph: &vcad_kernel_geom::SphereSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = sph.axis.as_ref();
    let ref_dir = sph.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = (point - sph.center) / sph.radius;
    let z = to_point.dot(axis).clamp(-1.0, 1.0);
    let v = z.asin();

    let proj = to_point - z * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_cone(
    cone: &vcad_kernel_geom::ConeSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cone.axis.as_ref();
    let ref_dir = cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);
    let cos_a = cone.half_angle.cos();

    let to_point = point - cone.apex;
    let height = to_point.dot(axis);
    let v = height / cos_a;

    let proj = to_point - height * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_torus(
    torus: &vcad_kernel_geom::TorusSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = torus.axis.as_ref();
    let ref_dir = torus.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - torus.center;
    let h = to_point.dot(axis);

    let proj = to_point - h * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir);
        let y = proj.dot(y_dir);
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    let tube_center_dist = proj_len - torus.major_radius;
    let v = h.atan2(tube_center_dist);
    let v = if v < 0.0 { v + 2.0 * PI } else { v };

    Point2::new(u, v)
}

/// Newton iteration to find UV coordinates for a point on a surface.
fn project_newton(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let mut uv = Point2::new((u_min + u_max) / 2.0, (v_min + v_max) / 2.0);

    for _ in 0..20 {
        let p = surface.evaluate(uv);
        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        let residual = p - point;

        // Solve 2x2 system: [du, dv]^T * [delta_u, delta_v]^T = residual
        // Using least squares: (J^T J) * delta = J^T * residual
        let a11 = du.dot(du);
        let a12 = du.dot(dv);
        let a22 = dv.dot(dv);
        let b1 = du.dot(residual);
        let b2 = dv.dot(residual);

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-14 {
            break;
        }

        let delta_u = (a22 * b1 - a12 * b2) / det;
        let delta_v = (a11 * b2 - a12 * b1) / det;

        uv.x -= delta_u;
        uv.y -= delta_v;

        // Clamp to domain
        uv.x = uv.x.clamp(u_min, u_max);
        uv.y = uv.y.clamp(v_min, v_max);

        if delta_u.abs() < 1e-10 && delta_v.abs() < 1e-10 {
            break;
        }
    }

    uv
}

/// Point-in-polygon test using the winding number algorithm.
///
/// Works correctly for both convex and concave polygons.
pub fn point_in_polygon(point: &Point2, polygon: &[Point2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    let mut winding = 0i32;
    let n = polygon.len();

    for i in 0..n {
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];

        if p1.y <= point.y {
            if p2.y > point.y {
                // Upward crossing
                if is_left(&p1, &p2, point) > 0.0 {
                    winding += 1;
                }
            }
        } else if p2.y <= point.y {
            // Downward crossing
            if is_left(&p1, &p2, point) < 0.0 {
                winding -= 1;
            }
        }
    }

    winding != 0
}

/// Compute the signed area of the triangle (p0, p1, p2).
/// Positive if p2 is to the left of the line p0->p1.
#[inline]
fn is_left(p0: &Point2, p1: &Point2, p2: &Point2) -> f64 {
    (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y)
}

/// Extract the UV coordinates of a face's outer loop.
///
/// This returns the UV coordinates of the outer boundary loop vertices,
/// suitable for point-in-polygon testing.
pub fn extract_face_uv_loop(brep: &BRepSolid, face_id: FaceId) -> Vec<Point2> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    let raw = loop_uv_coords(brep, face.outer_loop, surface.as_ref());
    // Mirror `point_in_face`: repair pole longitudes on a real trim loop, but
    // leave a degenerate (seam-only) loop alone so callers can still detect
    // it and fall back to "untrimmed".
    if polygon_area(&raw).abs() < 1e-9 {
        raw
    } else {
        repair_pole_vertices(surface.as_ref(), &raw)
    }
}

/// Extract UV coordinates for all inner loops (holes) of a face.
///
/// Returns a vector of inner loops, where each inner loop is a vector of UV points.
pub fn extract_face_inner_loops(brep: &BRepSolid, face_id: FaceId) -> Vec<Vec<Point2>> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    face.inner_loops
        .iter()
        .map(|&loop_id| loop_uv_coords(brep, loop_id, surface.as_ref()))
        .collect()
}

/// Get the face normal considering orientation.
pub fn face_normal(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> vcad_kernel_math::Dir3 {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let n = surface.normal(uv);

    match face.orientation {
        Orientation::Forward => n,
        Orientation::Reversed => vcad_kernel_math::Dir3::new_normalize(-n.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_point_in_polygon_square() {
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &square));
        assert!(point_in_polygon(&Point2::new(0.1, 0.1), &square));
        assert!(!point_in_polygon(&Point2::new(1.5, 0.5), &square));
        assert!(!point_in_polygon(&Point2::new(-0.1, 0.5), &square));
    }

    #[test]
    fn test_point_in_polygon_concave() {
        // L-shaped polygon
        let l_shape = vec![
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(2.0, 1.0),
            Point2::new(1.0, 1.0),
            Point2::new(1.0, 2.0),
            Point2::new(0.0, 2.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &l_shape));
        assert!(point_in_polygon(&Point2::new(0.5, 1.5), &l_shape));
        assert!(!point_in_polygon(&Point2::new(1.5, 1.5), &l_shape)); // In the concave notch
    }

    #[test]
    fn test_point_in_cube_face() {
        let cube = make_cube(10.0, 10.0, 10.0);

        // Get the first face and test a point in the middle
        let face_id = cube.topology.faces.iter().next().unwrap().0;

        // The face should be a 10x10 square in some plane
        // UV coordinates depend on the face, but center should be valid
        let center_uv = Point2::new(5.0, 5.0);
        assert!(point_in_face(&cube, face_id, center_uv));
    }

    #[test]
    fn test_project_to_cylinder() {
        use std::f64::consts::PI;
        let cyl = vcad_kernel_geom::CylinderSurface::new(5.0);

        // Point at (5, 0, 3) should project to u=0, v=3
        let uv = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(5.0, 0.0, 3.0));
        assert!(uv.x.abs() < 1e-10);
        assert!((uv.y - 3.0).abs() < 1e-10);

        // Point at (0, 5, 7) should project to u=π/2, v=7
        let uv2 = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(0.0, 5.0, 7.0));
        assert!((uv2.x - PI / 2.0).abs() < 1e-10);
        assert!((uv2.y - 7.0).abs() < 1e-10);
    }

    /// A seamless cylinder wall as STEP writers emit it: no seam edge, one
    /// circle per loop (outer at z=0, inner at z=h). Before the fix the
    /// v-range came from the outer loop alone — zero width — and every hit on
    /// the wall was rejected, so `--exact` renders lost all curved walls.
    #[test]
    fn test_two_ring_cylinder_wall_accepts_hits() {
        use vcad_kernel_geom::{CylinderSurface, GeometryStore};
        use vcad_kernel_math::Point3;
        use vcad_kernel_topo::{ShellType, Topology};

        let (r, h) = (5.0, 8.0);
        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();
        let cyl = geom.add_surface(Box::new(CylinderSurface::new(r)));
        let v_bot = topo.add_vertex(Point3::new(r, 0.0, 0.0));
        let v_top = topo.add_vertex(Point3::new(r, 0.0, h));
        let he_bot = topo.add_half_edge(v_bot);
        let he_top = topo.add_half_edge(v_top);
        let outer = topo.add_loop(&[he_bot]);
        let inner = topo.add_loop(&[he_top]);
        let face = topo.add_face(outer, cyl, Orientation::Forward);
        topo.add_inner_loop(face, inner);
        let shell = topo.add_shell(vec![face], ShellType::Outer);
        let solid_id = topo.add_solid(shell);
        let brep = BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        };

        let trim = FaceTrim::build(&brep, face);
        assert!(trim.contains(Point2::new(1.0, h / 2.0)));
        assert!(trim.contains(Point2::new(5.0, 0.5)));
        assert!(!trim.contains(Point2::new(1.0, -0.5)));
        assert!(!trim.contains(Point2::new(1.0, h + 0.5)));
    }

    /// A drill-tip cone: base ring only, the apex VERTEX_LOOP dropped on
    /// import. The wall must still span from the apex (v = 0) to the ring.
    #[test]
    fn test_single_ring_cone_spans_to_apex() {
        use vcad_kernel_geom::{ConeSurface, GeometryStore};
        use vcad_kernel_math::Point3;
        use vcad_kernel_topo::{ShellType, Topology};

        let half_angle = std::f64::consts::FRAC_PI_4;
        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();
        let cone = ConeSurface::new(half_angle);
        let ring = Point3::new(3.0, 0.0, 3.0);
        let ring_v = project_to_cone(&cone, &ring).y;
        let cone_idx = geom.add_surface(Box::new(cone));
        let v = topo.add_vertex(ring);
        let he = topo.add_half_edge(v);
        let outer = topo.add_loop(&[he]);
        let face = topo.add_face(outer, cone_idx, Orientation::Forward);
        let shell = topo.add_shell(vec![face], ShellType::Outer);
        let solid_id = topo.add_solid(shell);
        let brep = BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        };

        let trim = FaceTrim::build(&brep, face);
        assert!(trim.contains(Point2::new(1.0, ring_v / 2.0)));
        assert!(!trim.contains(Point2::new(1.0, ring_v * 1.5)));
    }

    /// Onshape's form of the same wall: each circle subdivided into many
    /// vertices, starting away from the surface's `u = 0`, so every ring
    /// crosses the projection's wrap mid-loop and winds a full turn without
    /// closing in UV. Both rings must be stitched into one strip.
    #[test]
    fn test_subdivided_seam_crossing_rings_bound_a_strip() {
        use vcad_kernel_geom::{CylinderSurface, GeometryStore};
        use vcad_kernel_math::Point3;
        use vcad_kernel_topo::{ShellType, Topology};

        let (r, h, n) = (5.0, 8.0, 24);
        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();
        let cyl = geom.add_surface(Box::new(CylinderSurface::new(r)));
        let mut ring = |z: f64, start: f64, dir: f64| {
            let hes: Vec<_> = (0..n)
                .map(|i| {
                    let t = start + dir * std::f64::consts::TAU * i as f64 / n as f64;
                    let v = topo.add_vertex(Point3::new(r * t.cos(), r * t.sin(), z));
                    topo.add_half_edge(v)
                })
                .collect();
            topo.add_loop(&hes)
        };
        let outer = ring(0.0, 1.0, 1.0);
        let inner = ring(h, 2.5, -1.0);
        let face = topo.add_face(outer, cyl, Orientation::Forward);
        topo.add_inner_loop(face, inner);
        let shell = topo.add_shell(vec![face], ShellType::Outer);
        let solid_id = topo.add_solid(shell);
        let brep = BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        };

        let trim = FaceTrim::build(&brep, face);
        for k in 0..16 {
            let u = std::f64::consts::TAU * k as f64 / 16.0;
            assert!(trim.contains(Point2::new(u, h / 2.0)), "u={u}");
            assert!(!trim.contains(Point2::new(u, -0.5)), "u={u}");
            assert!(!trim.contains(Point2::new(u, h + 0.5)), "u={u}");
        }
    }
}
