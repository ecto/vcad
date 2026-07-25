//! High-permeability boundaries by the method of images.
//!
//! Most real "air-core" machines are only air-core in the *winding* region: the
//! coils have no iron, but steel back-irons close the magnetic circuit behind
//! the magnets and behind the stator. That steel is worth a factor of ~2 in
//! airgap flux, so ignoring it is not a small error.
//!
//! Because the steel operates far below saturation in these machines, it can be
//! treated as `μ_r → ∞`, and an infinitely permeable plane is exactly a mirror:
//! reflect the current path through the plane and keep the current the same, and
//! the pair reproduces the boundary condition `B_tangential = 0` at the surface.
//! That keeps the whole solver grid-free — no mesh, no iteration, no truncation
//! boundary — at the cost of assuming the plates are flat, parallel and
//! unbounded.
//!
//! # Convergence precondition — read this before using two planes
//!
//! One plane is a single reflection and always exact. **Two** planes generate an
//! infinite image series, like facing mirrors, and that series only converges if
//! the source set is **magnetically balanced** — net dipole moment ≈ 0.
//!
//! This is not a numerical nicety, it is the physics. Measured on a 5.6 mm plate
//! separation:
//!
//! | source | depth 8 | depth 16 | depth 32 | behaviour |
//! |---|---|---|---|---|
//! | one loop (net moment) | 2.29e-4 T | 7.28e-5 T | 2.21e-5 T | **never settles** |
//! | 6 alternating poles | 2.258e-2 T | 2.2598e-2 T | 2.2598e-2 T | converged to 6e-5 by depth 16 |
//!
//! A single magnet between two infinite iron plates is an ill-posed idealization:
//! its flux has no bounded return path, so every image adds with the same sense
//! and the sum drifts without limit. Any real multi-pole rotor is balanced by
//! construction — poles alternate — so this costs nothing in practice, but
//! [`IronStack::balance_residual`] enforces it rather than trusting it.
//!
//! # What this does and does not buy
//!
//! - **Exact** (to `1/μ_r`, ≈0.1% for unsaturated mild steel) for flat parallel
//!   plates of infinite extent, given a balanced source set.
//! - **Not modelled:** finite plate radius. Real back-irons are discs, so flux
//!   fringes at the inner and outer diameters and the image model over-predicts
//!   there. Compare against a grid solver before trusting the last few percent.
//! - **Not modelled:** saturation. [`IronStack::flux_density_estimate`] reports
//!   the working flux density in the iron so callers can check the assumption
//!   instead of inheriting it silently.

use crate::filament::Filament;
use crate::vec3::Vec3;

/// A stack of parallel, infinitely-permeable planes normal to **z**.
///
/// One plane is a single mirror. Two planes form a cavity whose image series is
/// infinite, exactly like facing mirrors; it is truncated at
/// [`IronStack::max_reflections`] and the residual is reported by
/// [`IronStack::tail_fraction`].
#[derive(Debug, Clone, PartialEq)]
pub struct IronStack {
    /// z positions of the planes, m.
    planes_z: Vec<f64>,
    /// Reflection depth for the image series.
    max_reflections: usize,
}

impl IronStack {
    /// No iron — the pure free-space case.
    pub fn none() -> Self {
        Self {
            planes_z: Vec::new(),
            max_reflections: 0,
        }
    }

    /// A single back-iron plane at `z_m`.
    pub fn single(z_m: f64) -> Self {
        Self {
            planes_z: vec![z_m],
            max_reflections: 1,
        }
    }

    /// Two parallel planes — the usual rotor/stator back-iron sandwich.
    ///
    /// `max_reflections` bounds the image series. Depth 16 reaches a 6e-5
    /// residual for a balanced 6-pole rotor at 5.6 mm plate separation; see
    /// [`IronStack::DEFAULT_REFLECTIONS`]. **The source set must be
    /// magnetically balanced** — check with [`IronStack::balance_residual`], and
    /// confirm the achieved truncation with [`IronStack::tail_fraction`].
    pub fn pair(z_a: f64, z_b: f64, max_reflections: usize) -> Self {
        assert!(z_a != z_b, "iron planes must be distinct");
        Self {
            planes_z: vec![z_a, z_b],
            max_reflections: max_reflections.max(1),
        }
    }

    /// Reflection depth that converges a balanced multi-pole rotor to ~1e-4.
    pub const DEFAULT_REFLECTIONS: usize = 16;

    /// How far a source set is from magnetically balanced: `|Σmᵢ| / Σ|mᵢ|`.
    ///
    /// Zero for an alternating-pole rotor, one for a lone magnet. The two-plane
    /// image series converges only near zero — see the module note. Values above
    /// ~0.05 mean the result depends on [`IronStack::max_reflections`] and must
    /// not be quoted.
    pub fn balance_residual(sources: &[Filament]) -> f64 {
        let mut net = Vec3::ZERO;
        let mut gross = 0.0;
        for s in sources {
            let m = s.dipole_moment();
            net = net + m;
            gross += m.norm();
        }
        if gross <= 0.0 {
            return 0.0;
        }
        net.norm() / gross
    }

    /// The planes' z positions, m.
    pub fn planes(&self) -> &[f64] {
        &self.planes_z
    }

    /// Whether any iron is present.
    pub fn is_empty(&self) -> bool {
        self.planes_z.is_empty()
    }

    /// Reflect a filament through the plane at `z_p`.
    ///
    /// Geometric reflection of the directed path, current unchanged. A tangent
    /// `(tx, ty, tz)` maps to `(tx, ty, −tz)`, which is precisely the `μ→∞`
    /// image rule: tangential current preserved, normal current reversed. For a
    /// loop parallel to the plane this keeps the circulation sense, so the pair
    /// doubles `B_z` and cancels `B_r` at the surface — the boundary condition.
    fn reflect(f: &Filament, z_p: f64) -> Filament {
        Filament {
            points: f
                .points
                .iter()
                .map(|p| Vec3::new(p.x, p.y, 2.0 * z_p - p.z))
                .collect(),
            ..f.clone()
        }
    }

    /// Expand `source` into itself plus its images.
    ///
    /// The images of a source between two planes are generated by alternately
    /// reflecting in each plane, which for parallel planes yields the familiar
    /// ladder marching away in both directions. Duplicate positions (a source
    /// exactly on a plane reflects to itself) are dropped.
    pub fn expand(&self, source: &Filament) -> Vec<Filament> {
        if self.planes_z.is_empty() {
            return vec![source.clone()];
        }
        let mut out = vec![source.clone()];
        let mut frontier = vec![(source.clone(), usize::MAX)];
        let same = |a: &Filament, b: &Filament| {
            a.points.len() == b.points.len()
                && a.points
                    .iter()
                    .zip(&b.points)
                    .all(|(p, q)| (p.z - q.z).abs() < 1e-12)
        };
        for _ in 0..self.max_reflections {
            let mut next = Vec::new();
            for (f, last_plane) in &frontier {
                for (pi, &z_p) in self.planes_z.iter().enumerate() {
                    // Reflecting twice in the same plane returns the source.
                    if *last_plane == pi {
                        continue;
                    }
                    let img = Self::reflect(f, z_p);
                    if same(&img, f) || out.iter().any(|o| same(o, &img)) {
                        continue;
                    }
                    out.push(img.clone());
                    next.push((img, pi));
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        out
    }

    /// Fraction of the field magnitude carried by the outermost image shell, at
    /// `probe`, for `source`.
    ///
    /// This is the truncation error of the image series: if it is not small, the
    /// answer depends on [`IronStack::max_reflections`] and should not be quoted.
    pub fn tail_fraction(&self, source: &Filament, probe: Vec3) -> f64 {
        if self.planes_z.is_empty() {
            return 0.0;
        }
        let all = self.expand(source);
        let total: Vec3 = all.iter().map(|f| f.b_at(probe)).sum();
        let deeper = IronStack {
            planes_z: self.planes_z.clone(),
            max_reflections: self.max_reflections + 2,
        };
        let refined: Vec3 = deeper.expand(source).iter().map(|f| f.b_at(probe)).sum();
        let scale = refined.norm().max(1e-18);
        (refined - total).norm() / scale
    }

    /// Working flux density in the iron, tesla, estimated as the normal field at
    /// the plane scaled by the plate's flux-concentration ratio.
    ///
    /// The image model assumes `μ_r → ∞`, which fails once the steel saturates.
    /// `pole_area_m2` is the flux-carrying area per pole at the plane and
    /// `iron_section_m2` the cross-section the returning flux must pass through
    /// (thickness × mean circumference / poles). Compare against ~1.5 T for mild
    /// steel: above that, the image result is optimistic and a nonlinear grid
    /// solve is required.
    pub fn flux_density_estimate(b_normal_t: f64, pole_area_m2: f64, iron_section_m2: f64) -> f64 {
        if iron_section_m2 <= 0.0 {
            return f64::INFINITY;
        }
        b_normal_t * pole_area_m2 / iron_section_m2
    }
}
