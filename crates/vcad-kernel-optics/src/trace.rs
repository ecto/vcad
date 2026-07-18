//! Exact sequential ray tracing: closed-form conic intersection + vector
//! Snell refraction.
//!
//! Every conic-of-revolution surface is a quadric — the intersection is a
//! quadratic in the ray parameter, solved in the numerically stable Kahan
//! form (no iteration anywhere at M0). Refraction uses the vector form of
//! Snell's law, which is exact in f64 and reduces to the identity at
//! normal incidence on a plane (tested to 1e-15).
//!
//! **Fail-closed fates:** a ray is never dropped. It either reaches the
//! image plane ([`RayFate::Imaged`]) or reports exactly why not —
//! vignetted at a named surface, total internal reflection at a named
//! surface, or a geometric miss. Downstream figures of merit must account
//! for every launched ray.

use crate::prescription::Prescription;

/// Minimal 3-vector (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    /// x, mm.
    pub x: f64,
    /// y, mm.
    pub y: f64,
    /// z, mm.
    pub z: f64,
}

impl Vec3 {
    /// Construct.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    /// Dot product.
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    /// Euclidean norm.
    pub fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// Cross-product magnitude |self × o| (used for sin of the angle
    /// between unit vectors).
    pub fn cross_norm(self, o: Vec3) -> f64 {
        let cx = self.y * o.z - self.z * o.y;
        let cy = self.z * o.x - self.x * o.z;
        let cz = self.x * o.y - self.y * o.x;
        (cx * cx + cy * cy + cz * cz).sqrt()
    }

    /// Scaled copy.
    pub fn scale(self, k: f64) -> Vec3 {
        Vec3::new(self.x * k, self.y * k, self.z * k)
    }

    /// Sum.
    pub fn plus(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    /// Unit copy.
    pub fn normalized(self) -> Vec3 {
        self.scale(1.0 / self.norm())
    }
}

/// A ray: position + unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Position, mm (global frame; surface 0's vertex at z = 0).
    pub p: Vec3,
    /// Unit direction (must have positive z to enter the system).
    pub d: Vec3,
}

/// The fate of a traced ray — every launched ray gets exactly one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RayFate {
    /// Reached the image plane at this point.
    Imaged(Vec3),
    /// Blocked by the aperture of the named surface.
    Vignetted(usize),
    /// Total internal reflection at the named surface.
    TotalInternalReflection(usize),
    /// No intersection with the named surface (or, at index
    /// `n_surfaces`, the exit ray never reaches the image plane).
    Missed(usize),
}

/// One trace's full record.
#[derive(Debug, Clone, PartialEq)]
pub struct Traced {
    /// The fate.
    pub fate: RayFate,
    /// Surface hit points (global frame), one per surface reached, plus
    /// the image point when imaged. Populated only when `record` is set.
    pub hits: Vec<Vec3>,
    /// Worst per-refraction Snell-invariant residual
    /// |n₁ sinθ₁ − n₂ sinθ₂| over the trace — the exactness diagnostic
    /// (the analog of the particle crate's energy-drift column).
    pub max_snell_residual: f64,
}

const T_MIN: f64 = 1e-9;

/// Intersect a ray (in surface-local coordinates: vertex at origin) with
/// the conic `c(s² + (1+κ)z²) − 2z = 0`. Returns the parameter `t` of the
/// physical (near-vertex-sheet) intersection.
fn intersect_conic(p: Vec3, d: Vec3, c: f64, kappa: f64) -> Option<(f64, Vec3)> {
    let k1 = 1.0 + kappa;
    let a = c * (d.x * d.x + d.y * d.y + k1 * d.z * d.z);
    let b = 2.0 * c * (p.x * d.x + p.y * d.y + k1 * p.z * d.z) - 2.0 * d.z;
    let c0 = c * (p.x * p.x + p.y * p.y + k1 * p.z * p.z) - 2.0 * p.z;

    let mut candidates: [Option<f64>; 2] = [None, None];
    if a.abs() < 1e-14 {
        // Plane (or tangential degenerate): linear.
        if b.abs() < 1e-14 {
            return None;
        }
        candidates[0] = Some(-c0 / b);
    } else {
        let disc = b * b - 4.0 * a * c0;
        if disc < 0.0 {
            return None;
        }
        // Kahan-stable quadratic roots.
        let q = -0.5 * (b + b.signum() * disc.sqrt());
        if q.abs() > 0.0 {
            candidates[0] = Some(q / a);
            candidates[1] = Some(c0 / q);
        } else {
            // b = disc = 0: double root at t = 0.
            candidates[0] = Some(0.0);
        }
    }

    // Among forward intersections, the physical hit is the one on the
    // near-vertex sheet: smallest |z_local|. (For a convex cap that is
    // the first hit; for a concave cap the far sphere wall comes first
    // along the ray and must be rejected.)
    let mut best: Option<(f64, Vec3)> = None;
    for t in candidates.into_iter().flatten() {
        if t < T_MIN {
            continue;
        }
        let hit = p.plus(d.scale(t));
        if let Some((_, bh)) = best {
            if hit.z.abs() < bh.z.abs() {
                best = Some((t, hit));
            }
        } else {
            best = Some((t, hit));
        }
    }
    best
}

/// Trace one ray through the prescription at wavelength `lambda_um`,
/// then to the plane `z = image_z_mm`.
///
/// Set `record` to collect per-surface hit points (for ray diagrams).
pub fn trace_to_image(
    presc: &Prescription,
    lambda_um: f64,
    ray: Ray,
    image_z_mm: f64,
    record: bool,
) -> Traced {
    let mut p = ray.p;
    let mut d = ray.d.normalized();
    let mut hits = Vec::new();
    let mut max_res: f64 = 0.0;
    let n_surf = presc.surfaces.len();

    for (i, surf) in presc.surfaces.iter().enumerate() {
        let vz = presc.vertex_z(i);
        let local = Vec3::new(p.x, p.y, p.z - vz);
        let c = surf.curvature();

        let Some((_, hit_local)) = intersect_conic(local, d, c, surf.conic) else {
            return Traced {
                fate: RayFate::Missed(i),
                hits,
                max_snell_residual: max_res,
            };
        };

        // Branch check: the intersection must lie on the sag sheet the
        // prescription defines (fail-closed against the wrong quadric
        // branch on strongly-curved or hyperbolic surfaces).
        let s2 = hit_local.x * hit_local.x + hit_local.y * hit_local.y;
        match surf.sag(s2) {
            Some(sag) if (sag - hit_local.z).abs() <= 1e-6 * (1.0 + hit_local.z.abs()) => {}
            _ => {
                return Traced {
                    fate: RayFate::Missed(i),
                    hits,
                    max_snell_residual: max_res,
                };
            }
        }

        if s2 > surf.semi_diameter_mm * surf.semi_diameter_mm {
            return Traced {
                fate: RayFate::Vignetted(i),
                hits,
                max_snell_residual: max_res,
            };
        }

        let hit_global = Vec3::new(hit_local.x, hit_local.y, hit_local.z + vz);
        if record {
            hits.push(hit_global);
        }

        // Surface normal: ∇[c(s² + (1+κ)z²) − 2z], oriented against the ray.
        let k1 = 1.0 + surf.conic;
        let mut n = Vec3::new(
            2.0 * c * hit_local.x,
            2.0 * c * hit_local.y,
            2.0 * c * k1 * hit_local.z - 2.0,
        )
        .normalized();
        if d.dot(n) > 0.0 {
            n = n.scale(-1.0);
        }

        let n1 = presc.index_before(i, lambda_um);
        let n2 = presc.index_after(i, lambda_um);
        p = hit_global;
        if n1 != n2 {
            let eta = n1 / n2;
            let cos_i = -d.dot(n);
            let radicand = 1.0 - eta * eta * (1.0 - cos_i * cos_i);
            if radicand < 0.0 {
                return Traced {
                    fate: RayFate::TotalInternalReflection(i),
                    hits,
                    max_snell_residual: max_res,
                };
            }
            let cos_t = radicand.sqrt();
            let t_dir = d.scale(eta).plus(n.scale(eta * cos_i - cos_t));
            let refracted = t_dir.normalized();
            // Snell invariant diagnostic: n₁ sinθ₁ = n₂ sinθ₂.
            let res = (n1 * d.cross_norm(n) - n2 * refracted.cross_norm(n)).abs();
            max_res = max_res.max(res);
            d = refracted;
        }
    }

    if d.z <= 0.0 {
        return Traced {
            fate: RayFate::Missed(n_surf),
            hits,
            max_snell_residual: max_res,
        };
    }
    let t = (image_z_mm - p.z) / d.z;
    if t < 0.0 {
        return Traced {
            fate: RayFate::Missed(n_surf),
            hits,
            max_snell_residual: max_res,
        };
    }
    let img = p.plus(d.scale(t));
    if record {
        hits.push(img);
    }
    Traced {
        fate: RayFate::Imaged(img),
        hits,
        max_snell_residual: max_res,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glass::Glass;
    use crate::prescription::Surface;

    fn axial_ray(x: f64, y: f64) -> Ray {
        Ray {
            p: Vec3::new(x, y, -10.0),
            d: Vec3::new(0.0, 0.0, 1.0),
        }
    }

    /// Mission-mandated exactness gate: a ray through a plane surface at
    /// normal incidence is unchanged to 1e-15 (position and direction).
    #[test]
    fn plane_normal_incidence_is_exact() {
        let p = Prescription::new(vec![Surface::sphere(
            f64::INFINITY,
            10.0,
            0.0,
            Glass::n_bk7(),
        )])
        .unwrap();
        let r = axial_ray(3.0, -2.0);
        let out = trace_to_image(&p, crate::lines::D, r, 50.0, true);
        let RayFate::Imaged(img) = out.fate else {
            panic!("fate = {:?}", out.fate)
        };
        assert!((img.x - 3.0).abs() < 1e-15);
        assert!((img.y + 2.0).abs() < 1e-15);
        assert!((img.z - 50.0).abs() < 1e-12);
        assert!(out.max_snell_residual < 1e-15);
    }

    /// Snell invariant holds to near machine precision at steep incidence
    /// through curved glass.
    #[test]
    fn snell_invariant_at_steep_incidence() {
        let p = Prescription::new(vec![
            Surface::sphere(30.0, 14.0, 8.0, Glass::n_sf11()),
            Surface::sphere(-30.0, 14.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let out = trace_to_image(&p, crate::lines::D, axial_ray(11.0, 0.0), 40.0, false);
        assert!(matches!(out.fate, RayFate::Imaged(_)), "{:?}", out.fate);
        assert!(
            out.max_snell_residual < 1e-13,
            "residual {}",
            out.max_snell_residual
        );
    }

    #[test]
    fn tir_is_reported_not_dropped() {
        // Steep exit from dense flint into air: past the critical angle
        // (~34° for n = 1.78) the flat exit face must report TIR.
        let p = Prescription::new(vec![
            Surface::sphere(15.0, 14.0, 10.0, Glass::n_sf11()),
            Surface::sphere(f64::INFINITY, 14.0, 0.0, Glass::Air),
        ])
        .unwrap();
        // A marginal ray bent hard by the strong first surface: incidence
        // asin(13.9/15) ≈ 68° refracts to ≈31° inside, leaving the ray
        // ≈37° off the exit-face normal — past the ≈34° critical angle.
        let out = trace_to_image(&p, crate::lines::D, axial_ray(13.9, 0.0), 40.0, false);
        assert_eq!(out.fate, RayFate::TotalInternalReflection(1));
    }

    #[test]
    fn vignetting_names_the_surface() {
        let p = Prescription::new(vec![
            Surface::stop(5.0, 5.0),
            Surface::sphere(50.0, 12.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let out = trace_to_image(&p, crate::lines::D, axial_ray(6.0, 0.0), 40.0, false);
        assert_eq!(out.fate, RayFate::Vignetted(0));
    }

    /// Concave-first-surface regression: the physical hit is the cap near
    /// the vertex, not the far wall of the underlying sphere.
    #[test]
    fn concave_surface_hits_near_sheet() {
        let p = Prescription::new(vec![Surface::sphere(-40.0, 10.0, 0.0, Glass::n_bk7())]).unwrap();
        let out = trace_to_image(&p, crate::lines::D, axial_ray(8.0, 0.0), 30.0, true);
        assert!(matches!(out.fate, RayFate::Imaged(_)), "{:?}", out.fate);
        // The cap at s = 8 on R = −40 sags to −(40 − √(1600−64)) ≈ −0.81.
        let hit = out.hits[0];
        assert!(
            (hit.z + 0.808).abs() < 5e-3,
            "hit.z = {} (far-wall pick would be ≈ −79)",
            hit.z
        );
    }

    /// A spherical lens focuses a paraxial ray at the thick-lens focus.
    #[test]
    fn biconvex_bends_ray_toward_axis() {
        let p = Prescription::new(vec![
            Surface::sphere(50.0, 12.0, 3.0, Glass::n_bk7()),
            Surface::sphere(-50.0, 12.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let out = trace_to_image(&p, crate::lines::D, axial_ray(1.0, 0.0), 20.0, false);
        let RayFate::Imaged(img) = out.fate else {
            panic!("{:?}", out.fate)
        };
        assert!(img.x < 1.0 && img.x > 0.0, "x = {}", img.x);
    }
}
