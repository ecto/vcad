//! Swept volume of a solid rotated about an axis.
//!
//! The clearance-cut primitive: `shell − sweep_rotate(link)` carves the path
//! a neighbouring link swings through over its joint range (knee fold, hip,
//! elbow). The sweep is the union of `n` rotated copies of the child's
//! tessellation, taken in one flat pass by
//! [`vcad_kernel_booleans::mesh_union_all`]: folding pairwise unions over
//! heavily overlapping copies re-splits already-split fragments at every
//! step and blows up super-linearly.
//!
//! Between two samples the true envelope bulges past the chord by at most
//! `R·(1 − cos(Δθ/2))` for a point at radius `R` from the axis; the default
//! step keeps that under [`DEFAULT_SAG_MM`], and `clearance` pads every copy
//! outward so the cut also covers a fit gap.

use crate::provenance::{DegradeEvent, LossKind};
use crate::{resolve_segments, Solid, SolidRepr};
use std::collections::HashMap;
use vcad_kernel_booleans::{mesh_union_all, DegradeReason};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_tessellate::TriangleMesh;

/// Default worst-case chord sag between adjacent sweep samples, in mm.
pub const DEFAULT_SAG_MM: f64 = 0.25;
/// Default worst-case spacing between adjacent copies at the furthest
/// point, in mm. The sag bound alone keeps the *outer* envelope smooth but
/// lets copies of a part thinner than the arc spacing leave wedge-shaped
/// gaps between them; features thinner than this need an explicit step.
pub const DEFAULT_COVER_MM: f64 = 15.0;
/// Upper bound on samples, so a careless step can't explode the union.
pub const MAX_SWEEP_SAMPLES: u32 = 361;

impl Solid {
    /// Union of this solid rotated about the axis through `origin` along
    /// `axis`, for angles from `deg_lo` to `deg_hi` inclusive.
    ///
    /// `step_deg` fixes the sampling; `None` picks the step that keeps the
    /// inter-sample chord sag under [`DEFAULT_SAG_MM`] at the solid's
    /// furthest point from the axis. `clearance` (mm, ≥ 0) dilates each copy
    /// outward along its vertex normals before the union.
    ///
    /// The result is a triangle-soup B-rep (correct geometry, recorded as a
    /// [`LossKind::BooleanFallback`]), so it subtracts cleanly via the mesh
    /// boolean path.
    pub fn sweep_rotate(
        &self,
        origin: Point3,
        axis: Vec3,
        deg_lo: f64,
        deg_hi: f64,
        step_deg: Option<f64>,
        clearance: f64,
    ) -> Solid {
        if matches!(self.repr, SolidRepr::Empty) || axis.norm() < 1e-12 {
            return self.clone();
        }
        let axis = axis.normalize();
        let segments = resolve_segments(self.segments);
        let mut base = self.to_mesh(segments);
        if clearance > 0.0 {
            dilate(&mut base, clearance);
        }

        let (lo, hi) = if deg_lo <= deg_hi {
            (deg_lo, deg_hi)
        } else {
            (deg_hi, deg_lo)
        };
        let span = hi - lo;
        let step = step_deg
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| auto_step_deg(&base, origin, axis));
        let n = if span <= 0.0 {
            1
        } else {
            ((span / step).ceil() as u32 + 1).clamp(2, MAX_SWEEP_SAMPLES)
        };

        let layer: Vec<TriangleMesh> = (0..n)
            .map(|i| {
                let t = if n == 1 {
                    0.0
                } else {
                    i as f64 / (n - 1) as f64
                };
                rotated(&base, origin, axis, (lo + span * t).to_radians())
            })
            .collect();
        let mesh = mesh_union_all(&layer);

        let mut provenance = self.provenance.clone();
        provenance.push(DegradeEvent::new(
            "sweep_rotate",
            LossKind::BooleanFallback(DegradeReason::SoupOperand),
        ));
        Solid {
            names: None,
            provenance,
            repr: SolidRepr::BRep(Box::new(vcad_kernel_booleans::mesh::mesh_to_brep(&mesh))),
            segments,
        }
    }
}

/// Step (degrees) whose chord sag at the furthest vertex stays under
/// [`DEFAULT_SAG_MM`] (`Δθ = 2·acos(1 − sag/R)`) and whose arc spacing stays
/// under [`DEFAULT_COVER_MM`], clamped to [0.5°, 15°].
fn auto_step_deg(mesh: &TriangleMesh, origin: Point3, axis: Vec3) -> f64 {
    let r_max = mesh
        .vertices
        .chunks(3)
        .map(|p| {
            let d = Vec3::new(
                p[0] as f64 - origin.x,
                p[1] as f64 - origin.y,
                p[2] as f64 - origin.z,
            );
            (d - axis * d.dot(axis)).norm()
        })
        .fold(0.0_f64, f64::max);
    if r_max <= DEFAULT_SAG_MM {
        return 15.0;
    }
    let sag = 2.0 * (1.0 - DEFAULT_SAG_MM / r_max).acos();
    let cover = DEFAULT_COVER_MM / r_max;
    sag.min(cover).to_degrees().clamp(0.5, 15.0)
}

/// Rodrigues rotation of every vertex (and normal) by `angle` about the axis.
fn rotated(mesh: &TriangleMesh, origin: Point3, axis: Vec3, angle: f64) -> TriangleMesh {
    let (s, c) = angle.sin_cos();
    let rot = |v: Vec3| v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c));
    let mut out = mesh.clone();
    for p in out.vertices.chunks_mut(3) {
        let d = Vec3::new(
            p[0] as f64 - origin.x,
            p[1] as f64 - origin.y,
            p[2] as f64 - origin.z,
        );
        let r = rot(d);
        p[0] = (r.x + origin.x) as f32;
        p[1] = (r.y + origin.y) as f32;
        p[2] = (r.z + origin.z) as f32;
    }
    for n in out.normals.chunks_mut(3) {
        let r = rot(Vec3::new(n[0] as f64, n[1] as f64, n[2] as f64));
        n[0] = r.x as f32;
        n[1] = r.y as f32;
        n[2] = r.z as f32;
    }
    out
}

/// Offset the surface outward by `d`. Vertices are grouped by position
/// (tessellations split vertices along sharp edges, and moving the copies
/// differently would tear the shell); each group moves along its
/// angle-weighted normal, stretched so every incident face plane moves by at
/// least `d` (a mitred corner), capped at `3d` for needle-sharp vertices.
fn dilate(mesh: &mut TriangleMesh, d: f64) {
    let nv = mesh.vertices.len() / 3;
    let pos = |i: usize| {
        Vec3::new(
            mesh.vertices[3 * i] as f64,
            mesh.vertices[3 * i + 1] as f64,
            mesh.vertices[3 * i + 2] as f64,
        )
    };
    let q = 1e4_f64;
    let key = |v: Vec3| {
        (
            (v.x * q).round() as i64,
            (v.y * q).round() as i64,
            (v.z * q).round() as i64,
        )
    };
    let mut group_of = vec![0usize; nv];
    let mut groups: HashMap<(i64, i64, i64), usize> = HashMap::new();
    for (i, g) in group_of.iter_mut().enumerate() {
        let len = groups.len();
        *g = *groups.entry(key(pos(i))).or_insert(len);
    }
    let ng = groups.len();
    let mut nsum = vec![Vec3::zeros(); ng];
    let mut faces: Vec<Vec<Vec3>> = vec![Vec::new(); ng];
    for tri in mesh.indices.chunks(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (pa, pb, pc) = (pos(a), pos(b), pos(c));
        let fnrm = (pb - pa).cross(pc - pa);
        if fnrm.norm() < 1e-18 {
            continue;
        }
        let fnrm = fnrm.normalize();
        for (v, e1, e2) in [
            (a, pb - pa, pc - pa),
            (b, pc - pb, pa - pb),
            (c, pa - pc, pb - pc),
        ] {
            let ang = e1.cross(e2).norm().atan2(e1.dot(e2));
            let g = group_of[v];
            nsum[g] += fnrm * ang;
            faces[g].push(fnrm);
        }
    }
    let disp: Vec<Vec3> = (0..ng)
        .map(|g| {
            if nsum[g].norm() < 1e-12 {
                return Vec3::zeros();
            }
            let n = nsum[g].normalize();
            let min_dot = faces[g]
                .iter()
                .map(|f| f.dot(n))
                .fold(1.0_f64, f64::min)
                .max(1.0 / 3.0);
            n * (d / min_dot)
        })
        .collect();
    let moved: Vec<Vec3> = (0..nv).map(|i| pos(i) + disp[group_of[i]]).collect();
    for (i, p) in moved.into_iter().enumerate() {
        mesh.vertices[3 * i] = p.x as f32;
        mesh.vertices[3 * i + 1] = p.y as f32;
        mesh.vertices[3 * i + 2] = p.z as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(s: &Solid) -> f64 {
        vcad_kernel_booleans::mesh_signed_volume(&s.to_mesh(32))
    }

    #[test]
    fn quarter_sweep_of_box_about_edge_is_quarter_cylinder() {
        // 10×4×1 bar along +X, swept 0..90° about Z through the origin:
        // the swept area in XY covers the quarter disc of radius 10 and
        // stays inside the quarter disc through the far corner plus the
        // bar's own footprint.
        let bar = Solid::cube(10.0, 4.0, 1.0);
        let swept = bar.sweep_rotate(
            Point3::origin(),
            Vec3::new(0.0, 0.0, 1.0),
            0.0,
            90.0,
            None,
            0.0,
        );
        let quarter = std::f64::consts::PI * 100.0 / 4.0;
        let v = volume(&swept);
        let upper = std::f64::consts::PI * 116.0 / 4.0 + 40.0;
        assert!(v > quarter * 0.98 && v < upper, "v={v}");
    }

    #[test]
    fn clearance_grows_the_sweep() {
        let bar = Solid::cube(10.0, 2.0, 2.0);
        let o = Point3::origin();
        let z = Vec3::new(0.0, 0.0, 1.0);
        let a = volume(&bar.sweep_rotate(o, z, 0.0, 30.0, Some(5.0), 0.0));
        let b = volume(&bar.sweep_rotate(o, z, 0.0, 30.0, Some(5.0), 0.5));
        assert!(b > a * 1.1, "a={a} b={b}");
    }

    #[test]
    fn zero_span_is_the_part_itself() {
        let bar = Solid::cube(4.0, 3.0, 2.0);
        let s = bar.sweep_rotate(
            Point3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            20.0,
            20.0,
            None,
            0.0,
        );
        assert!((volume(&s) - 24.0).abs() < 0.1);
    }
}
