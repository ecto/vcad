//! Spot-size figures of merit over a field × wavelength grid.
//!
//! Traces a deterministic equal-area pupil ring bundle per (field angle,
//! wavelength) to the image plane and reports RMS spot radius about the
//! centroid, plus a polychromatic RMS per field (all wavelengths pooled
//! about the pooled centroid — the "white light" spot).
//!
//! **Pupil model (M0, stated honestly):** rays are aimed at a grid on the
//! tangent plane of surface 0 — i.e. the entrance pupil is taken at the
//! front surface. For prescriptions whose stop is at (or near) the front
//! this is exact; internal-stop systems still vignette correctly at the
//! stop but the bundle is not pupil-weighted. Paraxial pupil imaging is
//! the M1 rung.
//!
//! **Determinism:** the bundle is a fixed lattice — no randomness — so
//! finite-difference gradient probes see the exact same ray set on every
//! evaluation (the freeze-the-discretization lesson from the particle
//! crate, encoded from day one).
//!
//! Every launched ray is accounted for: imaged, vignetted, TIR, or
//! missed. Fail-closed consumers (receipts) refuse to summarize a bundle
//! containing TIR or missed rays.

use serde::{Deserialize, Serialize};

use crate::prescription::Prescription;
use crate::trace::{trace_to_image, Ray, RayFate, Vec3};

/// Deterministic equal-area pupil lattice: `rings` concentric rings at
/// radii `R·√((j−½)/rings)` (each ring the centroid of an equal-area
/// annulus), 12 points per ring with a per-ring angular stagger. By
/// construction ⟨ρ²⟩ = R²/2 — the uniform-disk second moment — exactly.
pub fn pupil_points(pupil_radius_mm: f64, rings: usize) -> Vec<(f64, f64)> {
    let mut pts = Vec::with_capacity(12 * rings);
    for j in 1..=rings {
        let r = pupil_radius_mm * ((j as f64 - 0.5) / rings as f64).sqrt();
        for k in 0..12 {
            let a = std::f64::consts::TAU * (k as f64 + 0.5 * (j % 2) as f64) / 12.0;
            pts.push((r * a.cos(), r * a.sin()));
        }
    }
    pts
}

/// One (field, wavelength) bundle's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotResult {
    /// Field angle, degrees.
    pub field_deg: f64,
    /// Wavelength, µm.
    pub lambda_um: f64,
    /// RMS spot radius about the bundle centroid, µm. `None` when no ray
    /// reached the image plane.
    pub rms_um: Option<f64>,
    /// Centroid (x, y) on the image plane, mm.
    pub centroid_mm: (f64, f64),
    /// Rays imaged.
    pub n_imaged: usize,
    /// Rays vignetted (reported, excluded from RMS).
    pub n_vignetted: usize,
    /// Rays lost to total internal reflection (a hard design failure).
    pub n_tir: usize,
    /// Rays that missed a surface (a hard design failure).
    pub n_missed: usize,
}

/// Full analysis over the field × wavelength grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotAnalysis {
    /// Per-(field, wavelength) results, field-major.
    pub results: Vec<SpotResult>,
    /// Polychromatic RMS per field (all wavelengths pooled about the
    /// pooled centroid), µm. `None` when any bundle of that field had no
    /// imaged rays.
    pub poly_rms_um: Vec<Option<f64>>,
    /// Field angles, degrees.
    pub fields_deg: Vec<f64>,
    /// Wavelengths, µm.
    pub wavelengths_um: Vec<f64>,
    /// Image-plane z used, mm (global frame).
    pub image_z_mm: f64,
    /// Entrance-pupil radius sampled, mm.
    pub pupil_radius_mm: f64,
    /// Hexapolar rings per bundle.
    pub pupil_rings: usize,
    /// Worst Snell-invariant residual across all traced rays (exactness
    /// diagnostic).
    pub max_snell_residual: f64,
}

impl SpotAnalysis {
    /// Total rays across all bundles with hard failures (TIR + missed).
    pub fn hard_failures(&self) -> usize {
        self.results.iter().map(|r| r.n_tir + r.n_missed).sum()
    }

    /// Fraction of launched rays vignetted.
    pub fn vignetted_fraction(&self) -> f64 {
        let (mut v, mut total) = (0usize, 0usize);
        for r in &self.results {
            v += r.n_vignetted;
            total += r.n_imaged + r.n_vignetted + r.n_tir + r.n_missed;
        }
        if total == 0 {
            0.0
        } else {
            v as f64 / total as f64
        }
    }
}

fn bundle_rays(pupil_radius_mm: f64, rings: usize, field_deg: f64) -> Vec<Ray> {
    let th = field_deg.to_radians();
    let (s, c) = (th.sin(), th.cos());
    pupil_points(pupil_radius_mm, rings)
        .into_iter()
        .map(|(px, py)| Ray {
            // M0 pupil model: parallel rays at the field angle, each passing
            // through its pupil-grid point on the FRONT VERTEX PLANE (z = 0).
            // Launch at z = -1 and back the origin off by tanθ in y so that
            // at z = 0 the ray lands exactly on (px, py): the entrance-pupil
            // footprint is the fixed grid, independent of field. This takes
            // the pupil at the front vertex — exact for front-stop systems,
            // unweighted (but correctly vignetting) for internal stops. M1
            // replaces this with paraxial entrance-pupil imaging, at which
            // point the pupil plane, not the front vertex, sets the origin.
            p: Vec3::new(px, py - s / c, -1.0),
            d: Vec3::new(0.0, s, c),
        })
        .collect()
}

/// Analyze a prescription over `fields_deg` × `wavelengths_um` with a
/// equal-area ring bundle at the given entrance-pupil radius.
pub fn analyze(
    presc: &Prescription,
    pupil_radius_mm: f64,
    rings: usize,
    fields_deg: &[f64],
    wavelengths_um: &[f64],
    image_z_mm: f64,
) -> SpotAnalysis {
    let mut results = Vec::new();
    let mut poly = Vec::new();
    let mut max_res: f64 = 0.0;

    for &field in fields_deg {
        let rays = bundle_rays(pupil_radius_mm, rings, field);
        let mut pooled: Vec<(f64, f64)> = Vec::new();
        let mut field_ok = true;
        for &lam in wavelengths_um {
            let mut pts: Vec<(f64, f64)> = Vec::new();
            let (mut nv, mut nt, mut nm) = (0usize, 0usize, 0usize);
            for r in &rays {
                let out = trace_to_image(presc, lam, *r, image_z_mm, false);
                max_res = max_res.max(out.max_snell_residual);
                match out.fate {
                    RayFate::Imaged(p) => pts.push((p.x, p.y)),
                    RayFate::Vignetted(_) => nv += 1,
                    RayFate::TotalInternalReflection(_) => nt += 1,
                    RayFate::Missed(_) => nm += 1,
                }
            }
            let (centroid, rms) = centroid_rms(&pts);
            if pts.is_empty() {
                field_ok = false;
            }
            pooled.extend_from_slice(&pts);
            results.push(SpotResult {
                field_deg: field,
                lambda_um: lam,
                rms_um: rms,
                centroid_mm: centroid,
                n_imaged: pts.len(),
                n_vignetted: nv,
                n_tir: nt,
                n_missed: nm,
            });
        }
        let (_, prms) = centroid_rms(&pooled);
        poly.push(if field_ok { prms } else { None });
    }

    SpotAnalysis {
        results,
        poly_rms_um: poly,
        fields_deg: fields_deg.to_vec(),
        wavelengths_um: wavelengths_um.to_vec(),
        image_z_mm,
        pupil_radius_mm,
        pupil_rings: rings,
        max_snell_residual: max_res,
    }
}

/// Centroid (mm) and RMS radius (µm) of image points.
fn centroid_rms(pts: &[(f64, f64)]) -> ((f64, f64), Option<f64>) {
    if pts.is_empty() {
        return ((0.0, 0.0), None);
    }
    let n = pts.len() as f64;
    let cx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let ms = pts
        .iter()
        .map(|p| (p.0 - cx).powi(2) + (p.1 - cy).powi(2))
        .sum::<f64>()
        / n;
    ((cx, cy), Some(ms.sqrt() * 1000.0))
}

/// Airy-disk first-dark-ring radius 1.22·λ·N, in µm — the diffraction
/// context every geometric spot claim must carry (a geometric RMS below
/// this number means "diffraction-limited", not "this small").
pub fn airy_radius_um(lambda_um: f64, f_number: f64) -> f64 {
    1.22 * lambda_um * f_number
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glass::Glass;
    use crate::paraxial::first_order;
    use crate::prescription::{Prescription, Surface};

    /// The equal-area lattice has the uniform disk's second moment
    /// ⟨ρ²⟩ = R²/2 exactly, by construction.
    #[test]
    fn pupil_lattice_is_disk_uniform() {
        let pts = pupil_points(10.0, 12);
        let mean_r2 = pts.iter().map(|(x, y)| x * x + y * y).sum::<f64>() / pts.len() as f64;
        assert!(
            (mean_r2 / 50.0 - 1.0).abs() < 1e-12,
            "⟨ρ²⟩ = {mean_r2}, uniform disk gives 50"
        );
    }

    /// Geometric defocus blur: for an axial bundle defocused by δ, the
    /// RMS radius is δ·√⟨ρ²⟩/f′ exactly (similar triangles). Validates
    /// tracing, centroiding, and RMS in one closed-form check.
    #[test]
    fn defocus_blur_matches_similar_triangles() {
        let p = Prescription::new(vec![
            Surface::sphere(51.68, 3.0, 0.5, Glass::n_bk7()),
            Surface::sphere(-51.68, 3.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let fo = first_order(&p, crate::lines::D).unwrap();
        // Tiny pupil (f/100): the spherical-aberration focus shift
        // (~0.03·h² mm) must stay well under the 1 mm defocus, or the
        // similar-triangles formula picks up a real few-percent tail.
        let pupil = 0.5;
        let delta = 1.0;
        let a = analyze(
            &p,
            pupil,
            8,
            &[0.0],
            &[crate::lines::D],
            fo.image_z_mm + delta,
        );
        let rms = a.results[0].rms_um.unwrap();
        let mean_r2 = {
            let pts = pupil_points(pupil, 8);
            pts.iter().map(|(x, y)| x * x + y * y).sum::<f64>() / pts.len() as f64
        };
        let expected = delta * mean_r2.sqrt() / fo.efl_mm * 1000.0;
        assert!(
            (rms / expected - 1.0).abs() < 0.02,
            "rms {rms} µm vs similar-triangles {expected} µm"
        );
    }

    /// At the paraxial focus the axial spot collapses (only residual
    /// spherical aberration remains — orders below the defocused spot).
    #[test]
    fn spot_collapses_at_paraxial_focus() {
        let p = Prescription::new(vec![
            Surface::sphere(51.68, 3.0, 0.5, Glass::n_bk7()),
            Surface::sphere(-51.68, 3.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let fo = first_order(&p, crate::lines::D).unwrap();
        let at_focus = analyze(&p, 1.0, 8, &[0.0], &[crate::lines::D], fo.image_z_mm);
        let defocused = analyze(&p, 1.0, 8, &[0.0], &[crate::lines::D], fo.image_z_mm + 1.0);
        // At f/50 the residual spherical blur (~0.4 µm) sits well over an
        // order below the 1 mm-defocus blur (~14 µm).
        let r0 = at_focus.results[0].rms_um.unwrap();
        let r1 = defocused.results[0].rms_um.unwrap();
        assert!(r0 < r1 / 20.0, "focus {r0} µm vs defocused {r1} µm");
    }

    #[test]
    fn every_ray_is_accounted_for() {
        let p = Prescription::new(vec![
            Surface::stop(0.5, 1.0),
            Surface::sphere(51.68, 3.0, 0.5, Glass::n_bk7()),
            Surface::sphere(-51.68, 3.0, 0.0, Glass::Air),
        ])
        .unwrap();
        // Pupil sampled wider than the stop: outer rings vignette.
        let a = analyze(&p, 2.0, 6, &[0.0], &[crate::lines::D], 100.0);
        let r = &a.results[0];
        let launched = pupil_points(2.0, 6).len();
        assert_eq!(r.n_imaged + r.n_vignetted + r.n_tir + r.n_missed, launched);
        assert!(r.n_vignetted > 0);
        assert!(a.vignetted_fraction() > 0.0);
        assert_eq!(a.hard_failures(), 0);
    }

    #[test]
    fn airy_radius_reference_value() {
        // λ = 0.5876 µm at f/10: 1.22 · 0.5876 · 10 ≈ 7.17 µm.
        let r = airy_radius_um(0.5876, 10.0);
        assert!((r - 7.168).abs() < 0.01, "{r}");
    }
}
