//! Where two operands' curved carriers TOUCH rather than cross.
//!
//! A fillet is built tangent to what it blends into, so in a filleted part
//! tangency is the rule and not the exception: the rana-60 stator's root
//! fillet is an r 1.05 cylinder internally tangent to the r 24 bore, and its
//! tab-root fillets are externally tangent to the r 28.75 OD.
//!
//! Near such a touch the two carriers stay within microns of each other over
//! a tenth of a millimetre, and every downstream question of the form "which
//! curve is this vertex on" becomes ill-conditioned. Answering it by
//! proximity — which is what the splitters and the T-junction heal did — gave
//! three different corners 6.8e-3 mm apart and imprinted one curve's vertices
//! onto the other's edges. The result was a union with the right volume,
//! `Analytic` fidelity, and 642 unpaired edges
//! (`docs/boolean-multilump-union-diagnosis.md`).
//!
//! So the touch is found ONCE, analytically, before anything is split, and
//! the answer is shared: the splitters pin their crossing to it and the
//! repair pass refuses to imprint anything else inside its neighbourhood.

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;

/// How close two coplanar circles must be to touching before the arrangement
/// is declared TANGENT rather than crossing.
///
/// Read off the geometry this exists for. The stator's root fillet is meant
/// to be internally tangent to the bore, but its centre is authored to four
/// decimals, so the built centre distance is 22.9500263 against an exact
/// 22.95 — the arrangement misses tangency by **2.6e-5 mm** purely from
/// rounding the coordinates. The value has to clear that.
///
/// It also has to stay well under the smallest separation at which two arcs
/// are genuinely DIFFERENT features. `split_planar_face_by_arc`'s arc guard
/// puts that at "a few µm" (~3e-3 mm) for tangent cylindrical stadium
/// cutters, and `repair`'s seam tolerances sit at 1.5e-3 mm. 1e-4 mm is 4×
/// above the rounding residue it must catch and 30× below the separation it
/// must not eat — and 100× below the 1e-2 mm near miss pinned by
/// `a_near_miss_fillet_is_not_fused_into_a_tangency`.
pub(crate) const TANGENCY_EPS: f64 = 1e-4;

/// Radius around a tangency inside which a vertex's curve cannot be told
/// from its neighbour's, so proximity must not be used to decide anything.
///
/// Near an internal tangency of radii r and R the radial gap grows as
/// s²·(1/r − 1/R)/2 with arc distance s, so a polyline carrying the usual
/// ~1e-3 mm of chord sag first pokes through the other curve a long way from
/// the touch point: for the stator's 1.05/24 pair that is s ≈ 0.1 mm.
/// Measured displacements inside that zone: 6.8e-3 mm for the cap splitter's
/// crossing, and arc vertices sitting 1e-6 mm from the seam chord out to
/// 6.9e-3 mm from the corner. 0.02 mm covers those with 3× headroom and
/// stays five times inside the 0.1 mm the sag geometry allows.
pub(crate) const TANGENCY_ZONE: f64 = 0.02;

/// A line along which a carrier of A and a carrier of B touch.
///
/// Two cylinders with parallel axes touch along a line parallel to both, so
/// the tangency is stored as a line rather than a point and the same object
/// serves every cross-section of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TangencyLine {
    /// A point on the line.
    pub point: Point3,
    /// Unit direction along the line.
    pub dir: Vec3,
    /// How wide the seam is — the distance between the two generators
    /// `ssi::parallel_cylinders` would have emitted had it not merged them.
    ///
    /// This is the span over which the two carriers are indistinguishable,
    /// measured off the carriers rather than chosen. It is at most
    /// `MERGE_GENERATORS` by construction (a wider pair is not a tangency and
    /// gets a real intersection curve) and never below `TANGENCY_EPS`: a pair
    /// that genuinely only touches has one seam point and nothing to collapse.
    pub width: f64,
    /// Are BOTH carriers curved?
    ///
    /// A cylinder-plane touch is bounded along the axis like any other, but
    /// the plane is unbounded ACROSS it, so `span` cannot say where the touch
    /// stops in the other direction. On a Z-up part every face shares the
    /// same Z range, which makes that bound vacuous.
    pub both_curved: bool,
    /// Where the touch actually EXISTS: the parameter range along `dir`,
    /// measured from `point`, over which both carriers are really present.
    ///
    /// A `CylinderSurface` is an unbounded carrier, so a tangency derived
    /// from two of them is an infinite line — and a line through a 1 mm
    /// fillet at the rim of a 57-stage part runs the whole length of it,
    /// passing near features that have nothing to do with the touch. Treating
    /// those as "on the seam" is not a small error: snapping them onto the
    /// line cost one of the rana-60 stator's post-pair unions **4.8 mm³**
    /// (707.3 → 702.5), and the union referee correctly rejected the result
    /// as `VolumeDisagreement`.
    ///
    /// `f64::NEG_INFINITY..=f64::INFINITY` means "not bounded" — what a
    /// carrier-derived tangency knows before any face is consulted.
    pub span: (f64, f64),
}

impl TangencyLine {
    /// Perpendicular distance from `p` to the line, **if `p` is beside the
    /// stretch where the touch exists**; `f64::INFINITY` past either end.
    ///
    /// Callers ask "is this vertex on the seam", and a point level with the
    /// line but 30 mm past the end of both faces is not, however close it
    /// lies to the infinite extension.
    pub fn distance(&self, p: &Point3) -> f64 {
        let d = *p - self.point;
        let t = d.dot(self.dir);
        if t < self.span.0 || t > self.span.1 {
            return f64::INFINITY;
        }
        (d - self.dir * t).norm()
    }

    /// Perpendicular distance to the infinite line, ignoring `span`. Used
    /// when building the set, where two lines are compared as carriers.
    fn distance_unbounded(&self, p: &Point3) -> f64 {
        let d = *p - self.point;
        (d - self.dir * d.dot(self.dir)).norm()
    }
}

/// A cylindrical FACE: its carrier, plus the stretch of the axis the face
/// actually occupies, as `[min, max]` of `vertex · axis`.
///
/// Faces rather than surfaces because a `CylinderSurface` is unbounded, and
/// an unbounded tangency is the bug documented on [`TangencyLine::span`].
struct CylFace {
    center: Point3,
    axis: Vec3,
    radius: f64,
    extent: (f64, f64),
}

fn cylinder_faces(s: &BRepSolid) -> Vec<CylFace> {
    let mut out = Vec::new();
    for (_, face) in &s.topology.faces {
        let Some(c) = s.geometry.surfaces[face.surface_index]
            .as_any()
            .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
        else {
            continue;
        };
        let axis = c.axis.into_inner();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for loop_id in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            for he in s.topology.loop_half_edges(loop_id) {
                let t = s.topology.vertices[s.topology.half_edges[he].origin]
                    .point
                    .coords()
                    .dot(axis);
                lo = lo.min(t);
                hi = hi.max(t);
            }
        }
        if lo <= hi {
            out.push(CylFace {
                center: c.center,
                axis,
                radius: c.radius,
                extent: (lo, hi),
            });
        }
    }
    out
}

/// The same, for planar faces: origin, normal, and the extent along `axis`
/// that the face covers (a plane has no axis of its own, so the caller's is
/// used).
struct PlaneFace {
    origin: Point3,
    normal: Vec3,
    points: Vec<Point3>,
}

fn plane_faces(s: &BRepSolid) -> Vec<PlaneFace> {
    let mut out = Vec::new();
    for (_, face) in &s.topology.faces {
        let Some(p) = s.geometry.surfaces[face.surface_index]
            .as_any()
            .downcast_ref::<vcad_kernel_geom::Plane>()
        else {
            continue;
        };
        let mut points = Vec::new();
        for loop_id in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            for he in s.topology.loop_half_edges(loop_id) {
                points.push(s.topology.vertices[s.topology.half_edges[he].origin].point);
            }
        }
        if !points.is_empty() {
            out.push(PlaneFace {
                origin: p.origin,
                normal: p.normal_dir.into_inner(),
                points,
            });
        }
    }
    out
}

/// The overlap of two axial extents, both already expressed as `x · axis`.
/// `None` when the two faces do not share any of the axis.
fn overlap(a: (f64, f64), b: (f64, f64)) -> Option<(f64, f64)> {
    let lo = a.0.max(b.0);
    let hi = a.1.min(b.1);
    (lo <= hi).then_some((lo, hi))
}

/// Is `p` inside a tangency's ill-conditioned neighbourhood, but not on the
/// touch line itself?
pub(crate) fn in_tangency_zone(lines: &[TangencyLine], p: &Point3) -> bool {
    lines.iter().any(|t| {
        let d = t.distance(p);
        d > TANGENCY_EPS && d < TANGENCY_ZONE
    })
}

/// Every line along which a cylinder of one solid touches a PLANE of the
/// other.
///
/// The second tangency family, and in a filleted part the commoner one: a
/// fillet blending a wall into another wall is a cylinder laid against a
/// plane, tangent by construction. The rana-60 stator has two —
///
///   * the stadium tab's round end (r 3.1 on the tab's centre line) against
///     the tab cube's side planes at y = ±3.1, which leaves a 0.015 mm crack
///     at the corner of two of the three tabs;
///   * the lead notch's R1.05 fillet, centred on one notch wall and tangent
///     to the other at y = 2.0.
///
/// A plane parallel to the axis meets the cylinder in one line when its
/// distance from the axis equals the radius. Planes that cut across the axis
/// meet it in an ellipse and are somebody else's problem.
pub(crate) fn cylinder_plane_tangencies(a: &BRepSolid, b: &BRepSolid) -> Vec<TangencyLine> {
    let mut out: Vec<TangencyLine> = Vec::new();
    let mut pairs = |cyls: &[CylFace], pls: &[PlaneFace]| {
        for cyl in cyls {
            let (c, axis, r) = (cyl.center, cyl.axis, cyl.radius);
            for pl in pls {
                let n = pl.normal;
                // Only a plane PARALLEL to the axis touches along a line.
                if n.dot(axis).abs() > 1e-9 {
                    continue;
                }
                let signed = (c - pl.origin).dot(n);
                if (signed.abs() - r).abs() > TANGENCY_EPS {
                    continue;
                }
                // A plane cuts a cylinder in two generators separated by
                // 2*sqrt(r^2 - signed^2); at a designed tangency the plane
                // sits `r` from the axis, so that collapses to the miss.
                let h_sq = r * r - signed * signed;
                let width = if h_sq > 0.0 {
                    (2.0 * h_sq.sqrt()).max(TANGENCY_EPS)
                } else {
                    TANGENCY_EPS
                };
                // The planar face has no axis of its own, so its stretch of
                // this one is read off its own boundary.
                let mut plo = f64::INFINITY;
                let mut phi = f64::NEG_INFINITY;
                for p in &pl.points {
                    let t = p.coords().dot(axis);
                    plo = plo.min(t);
                    phi = phi.max(t);
                }
                let Some((lo, hi)) = overlap(cyl.extent, (plo, phi)) else {
                    continue;
                };
                let point = c - n * signed;
                let base = point.coords().dot(axis);
                let line = TangencyLine {
                    point,
                    dir: axis,
                    width,
                    span: (lo - base, hi - base),
                    both_curved: false,
                };
                if !out.iter().any(|t| {
                    t.dir.cross(line.dir).norm() < 1e-9
                        && t.distance_unbounded(&line.point) < TANGENCY_EPS
                }) {
                    out.push(line);
                }
            }
        }
    };
    pairs(&cylinder_faces(a), &plane_faces(b));
    pairs(&cylinder_faces(b), &plane_faces(a));
    out
}

/// Every line along which a cylindrical carrier of `a` touches one of `b`.
///
/// Parallel axes only — two cylinders whose axes are skew or crossing touch
/// at a point, not a line, and no arrangement in this crate's corpus needs
/// that yet. Both senses of tangency count: externally (centre distance
/// R + r, the stator's tab-root fillets against the OD) and internally
/// (|R − r|, its post-root fillets inside the bore).
pub(crate) fn cylinder_tangencies(a: &BRepSolid, b: &BRepSolid) -> Vec<TangencyLine> {
    let (ca, cb) = (cylinder_faces(a), cylinder_faces(b));
    let mut out: Vec<TangencyLine> = Vec::new();
    for fa in &ca {
        let (pa, axa, ra) = (fa.center, fa.axis, fa.radius);
        for fb in &cb {
            let (pb, axb, rb) = (fb.center, fb.axis, fb.radius);
            if axa.cross(axb).norm() > 1e-9 {
                continue;
            }
            // Separation measured perpendicular to the shared axis.
            let d = pb - pa;
            let perp = d - axa * d.dot(axa);
            let sep = perp.norm();
            if sep < 1e-12 {
                continue; // coaxial: never tangent
            }
            let external = (sep - (ra + rb)).abs() <= TANGENCY_EPS;
            let internal = (sep - (ra - rb).abs()).abs() <= TANGENCY_EPS;
            if !external && !internal {
                continue;
            }
            // The touch sits on the line of centres, `ra` from A's axis. That
            // is towards B except when A is the SMALLER of an internal pair,
            // where A sits inside B and the touch is on A's far side.
            let u = perp / sep;
            let toward = if internal && !external && ra < rb {
                -u
            } else {
                u
            };
            // The seam's width: the chord `ssi::parallel_cylinders` would
            // have cut had the pair not been merged. Same algebra, so the two
            // always agree on how wide "indistinguishable" is here.
            let t_c = (sep * sep + ra * ra - rb * rb) / (2.0 * sep);
            let h_sq = ra * ra - t_c * t_c;
            let width = if h_sq > 0.0 {
                (2.0 * h_sq.sqrt()).max(TANGENCY_EPS)
            } else {
                TANGENCY_EPS
            };
            // Both faces' extents in ONE frame — `x · axa`. B's own axis may
            // be antiparallel, in which case its projections negate and the
            // ends swap.
            let b_extent = if axb.dot(axa) > 0.0 {
                fb.extent
            } else {
                (-fb.extent.1, -fb.extent.0)
            };
            let Some((lo, hi)) = overlap(fa.extent, b_extent) else {
                continue; // the two faces never share this stretch of axis
            };
            let point = pa + toward * ra;
            let base = point.coords().dot(axa);
            let line = TangencyLine {
                point,
                dir: axa,
                width,
                span: (lo - base, hi - base),
                both_curved: true,
            };
            if !out.iter().any(|t| {
                t.dir.cross(line.dir).norm() < 1e-9
                    && t.distance_unbounded(&line.point) < TANGENCY_EPS
            }) {
                out.push(line);
            }
        }
    }
    out
}

thread_local! {
    /// The tangency lines of the boolean currently being evaluated.
    ///
    /// Scoped state rather than a parameter because the splitters that need
    /// it sit five frames and two recursions below the pipeline, and the
    /// alternative — re-deriving the lines from the geometry store on every
    /// arc split — is what it replaces: the rana-60 stator grows to ~500
    /// surfaces over its 57 stages, and re-scanning them per split cost the
    /// part 79 s against 28.8 s without it.
    static CURRENT: std::cell::RefCell<Vec<TangencyLine>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Publishes `lines` as the current boolean's tangencies until dropped,
/// restoring whatever was there before — booleans nest (a band split runs its
/// own), and an inner one must not leave the outer one's view changed.
pub(crate) struct Scope(Vec<TangencyLine>);

impl Drop for Scope {
    fn drop(&mut self) {
        let previous = std::mem::take(&mut self.0);
        CURRENT.with(|c| *c.borrow_mut() = previous);
    }
}

/// Publish `lines` for the lifetime of the returned guard.
pub(crate) fn scoped(lines: Vec<TangencyLine>) -> Scope {
    let previous = CURRENT.with(|c| std::mem::replace(&mut *c.borrow_mut(), lines));
    Scope(previous)
}

/// The tangency lines of the boolean in progress, if any.
pub(crate) fn with_current<R>(f: impl FnOnce(&[TangencyLine]) -> R) -> R {
    CURRENT.with(|c| f(&c.borrow()))
}
