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
}

impl TangencyLine {
    /// Perpendicular distance from `p` to the line.
    pub fn distance(&self, p: &Point3) -> f64 {
        let d = *p - self.point;
        (d - self.dir * d.dot(self.dir)).norm()
    }
}

/// Is `p` inside a tangency's ill-conditioned neighbourhood, but not on the
/// touch line itself?
pub(crate) fn in_tangency_zone(lines: &[TangencyLine], p: &Point3) -> bool {
    lines.iter().any(|t| {
        let d = t.distance(p);
        d > TANGENCY_EPS && d < TANGENCY_ZONE
    })
}

/// Every line along which a cylindrical carrier of `a` touches one of `b`.
///
/// Parallel axes only — two cylinders whose axes are skew or crossing touch
/// at a point, not a line, and no arrangement in this crate's corpus needs
/// that yet. Both senses of tangency count: externally (centre distance
/// R + r, the stator's tab-root fillets against the OD) and internally
/// (|R − r|, its post-root fillets inside the bore).
pub(crate) fn cylinder_tangencies(a: &BRepSolid, b: &BRepSolid) -> Vec<TangencyLine> {
    let cylinders = |s: &BRepSolid| -> Vec<(Point3, Vec3, f64)> {
        s.geometry
            .surfaces
            .iter()
            .filter_map(|surf| {
                surf.as_any()
                    .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
                    .map(|c| (c.center, c.axis.into_inner(), c.radius))
            })
            .collect()
    };
    let (ca, cb) = (cylinders(a), cylinders(b));
    let mut out: Vec<TangencyLine> = Vec::new();
    for &(pa, axa, ra) in &ca {
        for &(pb, axb, rb) in &cb {
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
            let line = TangencyLine {
                point: pa + toward * ra,
                dir: axa,
            };
            if !out.iter().any(|t| {
                t.dir.cross(line.dir).norm() < 1e-9 && t.distance(&line.point) < TANGENCY_EPS
            }) {
                out.push(line);
            }
        }
    }
    out
}
