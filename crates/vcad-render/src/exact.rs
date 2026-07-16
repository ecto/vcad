//! BRep-exact linework (`--exact-edges`).
//!
//! The mesh pipeline approximates every curved edge as a polyline at
//! `TESSELLATION_SEGMENTS` chords, which facets visibly at high `--scale`.
//! This module recovers the underlying analytic curves straight from the
//! evaluated BRep and lets the SVG path emit them as mathematically exact
//! elliptical arcs instead:
//!
//!   - **Rim circles** of cylinder and cone faces (a bore's mouth, a boss's
//!     cap edge): the face's loop vertices bound the axial extent, the
//!     surface gives axis + radius.
//!   - **View-outline circles** of spheres: for an orthographic camera the
//!     silhouette of a sphere is the great circle in the plane
//!     perpendicular to the view direction.
//!
//! Rather than trusting the (possibly boolean-trimmed) topology for arc
//! extents, each candidate circle is *matched against the mesh linework
//! that would otherwise be drawn*: a kept mesh edge whose endpoints both
//! lie on the circle contributes its angular interval and is suppressed
//! from polyline output. Merged intervals become exact arcs. Trimmed rims
//! therefore keep exactly the coverage the polyline render had, curves the
//! matcher doesn't recognise (tori, NURBS, intersection seams) fall back to
//! polylines untouched, and cylinder/cone silhouette rulings stay as the
//! mesh's straight lines (which are already exact).

use std::collections::HashMap;

use vcad_kernel::vcad_kernel_geom::{ConeSurface, CylinderSurface, SphereSurface};
use vcad_kernel::vcad_kernel_topo::{LoopId, Topology};
use vcad_kernel::Solid;

use crate::{cross, dot, normalize, sub, EdgeKind};

/// An analytic circle in 3D: `center + radius·(u·cosθ + v·sinθ)`.
pub(crate) struct CircleCurve {
    pub center: [f64; 3],
    /// Unit vector in the circle's plane (θ = 0 direction).
    pub u: [f64; 3],
    /// Unit vector in the circle's plane, perpendicular to `u`.
    pub v: [f64; 3],
    pub radius: f64,
    /// Distance tolerance (mm) for a mesh vertex to count as "on" this circle.
    tol: f64,
    /// Angular gap (radians) across which adjacent matched intervals merge.
    gap: f64,
    /// Match only smooth-silhouette mesh edges (sphere view outlines — their
    /// mesh counterpart zigzags across facet rings, so the tolerance is loose
    /// and must not swallow unrelated hard edges).
    smooth_only: bool,
}

impl CircleCurve {
    /// Point on the circle at parameter `th` (radians).
    pub fn point(&self, th: f64) -> [f64; 3] {
        let (c, s) = (th.cos(), th.sin());
        [
            self.center[0] + self.radius * (self.u[0] * c + self.v[0] * s),
            self.center[1] + self.radius * (self.u[1] * c + self.v[1] * s),
            self.center[2] + self.radius * (self.u[2] * c + self.v[2] * s),
        ]
    }

    /// Distance from `p` to the circle (the curve, not the disc).
    fn dist(&self, p: [f64; 3]) -> f64 {
        let d = sub(p, self.center);
        let n = cross(self.u, self.v);
        let axial = dot(d, n);
        let du = dot(d, self.u);
        let dv = dot(d, self.v);
        let radial = (du * du + dv * dv).sqrt() - self.radius;
        (axial * axial + radial * radial).sqrt()
    }

    /// Angular parameter of `p` projected onto the circle's plane.
    fn angle_of(&self, p: [f64; 3]) -> f64 {
        let d = sub(p, self.center);
        dot(d, self.v).atan2(dot(d, self.u))
    }
}

/// One exact arc to draw: circle index, parameter interval (end > start,
/// spans ≤ 2π), and the line-weight class inherited from the mesh edges it
/// replaces.
pub(crate) struct ArcSpan {
    pub circle: usize,
    pub start: f64,
    pub end: f64,
    pub kind: EdgeKind,
}

/// The exact linework recovered for one solid.
pub(crate) struct ExactCurves {
    pub circles: Vec<CircleCurve>,
    pub arcs: Vec<ArcSpan>,
    /// Per-`art.edges` index: true when that mesh edge is replaced by an arc
    /// and must be dropped from polyline output.
    pub suppressed: Vec<bool>,
}

/// Collect the 3D positions of every vertex on a face loop. Bounded by the
/// half-edge count so corrupt topology can't spin forever.
fn loop_points(topo: &Topology, lp: LoopId, out: &mut Vec<[f64; 3]>) {
    let Some(l) = topo.loops.get(lp) else { return };
    let start = l.half_edge;
    let mut he = start;
    for _ in 0..topo.half_edges.len() {
        let Some(h) = topo.half_edges.get(he) else {
            return;
        };
        if let Some(vx) = topo.vertices.get(h.origin) {
            let p = vx.point;
            out.push([p.x, p.y, p.z]);
        }
        match h.next {
            Some(n) if n != start => he = n,
            _ => return,
        }
    }
}

/// Candidate analytic circles derived from a solid's BRep faces for the
/// current view direction `cam`. Deduplicated; extents are *not* trusted —
/// the caller matches them against mesh edges for coverage.
fn candidate_circles(solid: &Solid, cam: [f64; 3], segments: u32) -> Vec<CircleCurve> {
    let Some(brep) = solid.as_brep() else {
        return Vec::new();
    };
    let topo = &brep.topology;
    let geom = &brep.geometry;
    let seg = segments.max(3) as f64;
    // Chord sag of an inscribed `segments`-gon — how far the polyline rim
    // can sit from the true circle — plus the vertex-weld tolerance.
    let sag = |r: f64| r * (1.0 - (std::f64::consts::PI / seg).cos());

    let mut out: Vec<CircleCurve> = Vec::new();
    // Dedup key: quantized center, plane normal (canonical sign), radius.
    let mut seen: HashMap<([i64; 3], [i64; 3], i64), ()> = HashMap::new();
    let mut push = |out: &mut Vec<CircleCurve>, c: CircleCurve| {
        if !(c.radius.is_finite() && c.radius > 1e-6) {
            return;
        }
        let q = |x: f64| (x * 1e4).round() as i64;
        let mut n = cross(c.u, c.v);
        // Canonical normal sign so the same circle found from two faces
        // (a boolean-split barrel) dedups regardless of winding.
        if n[0] < -1e-12
            || (n[0].abs() <= 1e-12 && (n[1] < -1e-12 || (n[1].abs() <= 1e-12 && n[2] < 0.0)))
        {
            n = [-n[0], -n[1], -n[2]];
        }
        let key = (
            [q(c.center[0]), q(c.center[1]), q(c.center[2])],
            [q(n[0]), q(n[1]), q(n[2])],
            q(c.radius),
        );
        if seen.insert(key, ()).is_none() {
            out.push(c);
        }
    };

    for (_fid, face) in &topo.faces {
        let Some(surface) = geom.surfaces.get(face.surface_index) else {
            continue;
        };
        let any = surface.as_any();

        // Loop vertices bound the face's extent along the surface axis.
        let mut pts: Vec<[f64; 3]> = Vec::new();
        loop_points(topo, face.outer_loop, &mut pts);
        for &il in &face.inner_loops {
            loop_points(topo, il, &mut pts);
        }

        if let Some(cyl) = any.downcast_ref::<CylinderSurface>() {
            let c0 = [cyl.center.x, cyl.center.y, cyl.center.z];
            let n = normalize([
                cyl.axis.as_ref().x,
                cyl.axis.as_ref().y,
                cyl.axis.as_ref().z,
            ]);
            if n == [0.0; 3] || pts.is_empty() {
                continue;
            }
            let ts: Vec<f64> = pts.iter().map(|p| dot(sub(*p, c0), n)).collect();
            let tmin = ts.iter().cloned().fold(f64::INFINITY, f64::min);
            let tmax = ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let u = plane_basis_u(n);
            let v = cross(n, u);
            for t in [tmin, tmax] {
                push(
                    &mut out,
                    CircleCurve {
                        center: [c0[0] + n[0] * t, c0[1] + n[1] * t, c0[2] + n[2] * t],
                        u,
                        v,
                        radius: cyl.radius,
                        tol: (4.0 * sag(cyl.radius)).max(0.02),
                        gap: 1e-3,
                        smooth_only: false,
                    },
                );
            }
        } else if let Some(cone) = any.downcast_ref::<ConeSurface>() {
            let apex = [cone.apex.x, cone.apex.y, cone.apex.z];
            let n = normalize([
                cone.axis.as_ref().x,
                cone.axis.as_ref().y,
                cone.axis.as_ref().z,
            ]);
            if n == [0.0; 3] || pts.is_empty() {
                continue;
            }
            let ts: Vec<f64> = pts.iter().map(|p| dot(sub(*p, apex), n)).collect();
            let tmin = ts.iter().cloned().fold(f64::INFINITY, f64::min);
            let tmax = ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let u = plane_basis_u(n);
            let v = cross(n, u);
            let tan_a = cone.half_angle.tan();
            for t in [tmin, tmax] {
                let r = t * tan_a;
                push(
                    &mut out,
                    CircleCurve {
                        center: [apex[0] + n[0] * t, apex[1] + n[1] * t, apex[2] + n[2] * t],
                        u,
                        v,
                        radius: r,
                        tol: (4.0 * sag(r.abs())).max(0.02),
                        gap: 1e-3,
                        smooth_only: false,
                    },
                );
            }
        } else if let Some(sph) = any.downcast_ref::<SphereSurface>() {
            // View outline: the great circle in the plane ⟂ cam. The mesh
            // counterpart (smooth-silhouette edges) zigzags across facet
            // rings, so both the distance tolerance and the merge gap are a
            // facet-scale loose fit — restricted to Smooth edges so nothing
            // else is swallowed.
            let center = [sph.center.x, sph.center.y, sph.center.z];
            let u = plane_basis_u(cam);
            let v = cross(cam, u);
            let facet = std::f64::consts::TAU / seg;
            push(
                &mut out,
                CircleCurve {
                    center,
                    u,
                    v,
                    radius: sph.radius,
                    tol: 2.5 * sph.radius * facet,
                    gap: 3.0 * facet,
                    smooth_only: true,
                },
            );
        }
    }
    out
}

/// Any unit vector perpendicular to `n`.
fn plane_basis_u(n: [f64; 3]) -> [f64; 3] {
    let pick = if n[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    normalize(cross(n, pick))
}

/// Merge sorted angular intervals, bridging gaps up to `gap` and joining
/// across the 0/2π seam. Input intervals are (start ∈ [0, 2π), end > start).
fn merge_intervals(mut ivals: Vec<(f64, f64)>, gap: f64) -> Vec<(f64, f64)> {
    use std::f64::consts::TAU;
    if ivals.is_empty() {
        return ivals;
    }
    ivals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for iv in ivals {
        match merged.last_mut() {
            Some(last) if iv.0 <= last.1 + gap => last.1 = last.1.max(iv.1),
            _ => merged.push(iv),
        }
    }
    // Wraparound: the first interval may continue where the last ends.
    if merged.len() > 1 {
        let first = merged[0];
        let last = merged.last_mut().unwrap();
        if first.0 + TAU <= last.1 + gap {
            last.1 = last.1.max(first.1 + TAU);
            merged.remove(0);
        }
    }
    // A fully-covered circle collapses to one revolution.
    for iv in &mut merged {
        if iv.1 - iv.0 >= TAU {
            iv.1 = iv.0 + TAU;
        }
    }
    merged
}

/// Extract the exact linework for one solid: derive candidate circles from
/// its BRep, match the mesh's kept `edges` (canonical-vertex index pairs
/// into `verts`) against them, and return arcs + the suppression mask.
pub(crate) fn extract(
    solid: &Solid,
    verts: &[[f64; 3]],
    edges: &[(usize, usize, EdgeKind)],
    cam: [f64; 3],
    segments: u32,
) -> ExactCurves {
    use std::f64::consts::{PI, TAU};
    let circles = candidate_circles(solid, cam, segments);
    let mut suppressed = vec![false; edges.len()];
    // Per (circle, kind) angular intervals contributed by matched edges.
    let mut buckets: HashMap<(usize, EdgeKind), Vec<(f64, f64)>> = HashMap::new();

    for (ei, &(a, b, kind)) in edges.iter().enumerate() {
        let (pa, pb) = (verts[a], verts[b]);
        for (ci, c) in circles.iter().enumerate() {
            if c.smooth_only && kind != EdgeKind::Smooth {
                continue;
            }
            if c.dist(pa) > c.tol || c.dist(pb) > c.tol {
                continue;
            }
            let ta = c.angle_of(pa);
            let tb = c.angle_of(pb);
            // Shorter way around; a chord spanning more than a quarter turn
            // is not one tessellation facet of this circle — reject it.
            let mut d = (tb - ta).rem_euclid(TAU);
            let (mut start, len) = if d <= PI { (ta, d) } else { (tb, TAU - d) };
            d = len;
            if d > PI / 2.0 || d <= 0.0 {
                continue;
            }
            start = start.rem_euclid(TAU);
            buckets
                .entry((ci, kind))
                .or_default()
                .push((start, start + d));
            suppressed[ei] = true;
            break;
        }
    }

    let mut arcs = Vec::new();
    for ((ci, kind), ivals) in buckets {
        for (start, end) in merge_intervals(ivals, circles[ci].gap) {
            arcs.push(ArcSpan {
                circle: ci,
                start,
                end,
                kind,
            });
        }
    }
    ExactCurves {
        circles,
        arcs,
        suppressed,
    }
}
