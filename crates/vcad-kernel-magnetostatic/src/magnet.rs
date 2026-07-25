//! Permanent magnets as equivalent bound surface currents.
//!
//! A uniformly magnetized body is exactly equivalent to a current distribution:
//! `J_b = ∇×M` inside (zero for uniform `M`) and `K_b = M × n̂` on the surface.
//! For a prism magnetized along **z**, `M = M ẑ` gives `K = 0` on the flat faces
//! (`ẑ × ±ẑ = 0`) and `K = M (ẑ × n̂)` on the side walls — which, traced around
//! the footprint, is simply a **closed loop following the outline**.
//!
//! So any axially-magnetized prism — disc, arc segment, rectangle — becomes a
//! stack of closed loops around its footprint, each carrying `M·Δz`. One
//! representation covers every magnet shape a machine uses, and it feeds the
//! same segment integrator as the coils.
//!
//! # Fidelity
//!
//! Exact for uniform magnetization in a linear medium with recoil permeability
//! `μ_rec = 1`. Real sintered magnets have `μ_rec ≈ 1.05` (NdFeB) to `1.1`
//! (ferrite), so the model neglects the magnet's own reluctance — a 5–10%
//! over-prediction of working flux, and the dominant magnet-side error here.
//! Grade against a grid solve before quoting better than that.

use serde::{Deserialize, Serialize};

use crate::filament::Filament;
use crate::vec3::Vec3;
use crate::MU_0;

use std::f64::consts::PI;

/// Which way a magnet's north pole faces along **z**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Polarity {
    /// North toward +z.
    North,
    /// North toward −z.
    South,
}

impl Polarity {
    /// `+1` for [`Polarity::North`], `−1` for [`Polarity::South`].
    pub fn sign(self) -> f64 {
        match self {
            Polarity::North => 1.0,
            Polarity::South => -1.0,
        }
    }

    /// Alternating polarity for pole index `k`.
    pub fn alternating(k: usize) -> Self {
        if k % 2 == 0 {
            Polarity::North
        } else {
            Polarity::South
        }
    }
}

/// An axially-magnetized prism, described by its footprint outline.
///
/// All lengths in **metres** (the machine layer converts from millimetres).
#[derive(Debug, Clone, PartialEq)]
pub struct PrismMagnet {
    /// Footprint outline in the xy plane, m. Implicitly closed; winding order
    /// does not matter, it is normalized to counter-clockwise.
    pub footprint: Vec<(f64, f64)>,
    /// Lower face z, m.
    pub z0_m: f64,
    /// Thickness along z, m.
    pub thickness_m: f64,
    /// Remanence `B_r`, tesla.
    pub remanence_t: f64,
    /// Which way north faces.
    pub polarity: Polarity,
}

impl PrismMagnet {
    /// Magnetization `M = B_r/μ₀`, A/m.
    pub fn magnetization(&self) -> f64 {
        self.remanence_t / MU_0 * self.polarity.sign()
    }

    /// Signed area of the footprint, m² — positive when counter-clockwise.
    fn signed_area(&self) -> f64 {
        let n = self.footprint.len();
        if n < 3 {
            return 0.0;
        }
        let mut s = 0.0;
        for i in 0..n {
            let (x1, y1) = self.footprint[i];
            let (x2, y2) = self.footprint[(i + 1) % n];
            s += x1 * y2 - x2 * y1;
        }
        s / 2.0
    }

    /// Pole-face area, m².
    pub fn face_area_m2(&self) -> f64 {
        self.signed_area().abs()
    }

    /// The equivalent bound-current loops, `n_axial` of them through the
    /// thickness.
    ///
    /// Each loop carries `M · thickness/n_axial` amperes and traces the
    /// footprint counter-clockwise, so a north-up magnet produces `+z` flux
    /// above it — the right-hand rule, same as a coil.
    pub fn to_filaments(&self, n_axial: usize) -> Vec<Filament> {
        let n_axial = n_axial.max(1);
        if self.footprint.len() < 3 || self.thickness_m <= 0.0 {
            return Vec::new();
        }
        // Normalize to counter-clockwise so the current sense follows polarity
        // rather than however the caller happened to order the points.
        let ccw = self.signed_area() >= 0.0;
        let dz = self.thickness_m / n_axial as f64;
        let current = self.magnetization() * dz;
        // The conductor radius regularizes the sheet: use half the axial slice
        // height, which is the physical thickness each loop stands in for.
        let wire_r = dz * 0.5;

        (0..n_axial)
            .map(|k| {
                let z = self.z0_m + (k as f64 + 0.5) * dz;
                let mut pts: Vec<Vec3> = self
                    .footprint
                    .iter()
                    .map(|&(x, y)| Vec3::new(x, y, z))
                    .collect();
                if !ccw {
                    pts.reverse();
                }
                Filament::closed_loop(pts, current, wire_r)
            })
            .collect()
    }
}

/// A ring of identical magnets on a pitch circle with alternating polarity —
/// the rotor of a surface-magnet machine.
#[derive(Debug, Clone, PartialEq)]
pub struct MagnetRing {
    /// The magnets, already positioned.
    pub magnets: Vec<PrismMagnet>,
}

impl MagnetRing {
    /// Discs of `diameter_m` centred on a `pitch_radius_m` circle, `poles` of
    /// them, alternating N/S.
    ///
    /// `poles` must be even for the ring to be magnetically balanced — an odd
    /// count leaves a net dipole moment, which breaks the two-plane image series
    /// (see [`crate::IronStack`]).
    #[allow(clippy::too_many_arguments)]
    pub fn discs(
        poles: usize,
        pitch_radius_m: f64,
        diameter_m: f64,
        z0_m: f64,
        thickness_m: f64,
        remanence_t: f64,
        facets: usize,
    ) -> Self {
        let facets = facets.max(12);
        let r = diameter_m * 0.5;
        let magnets = (0..poles)
            .map(|k| {
                let phi = 2.0 * PI * (k as f64) / (poles as f64);
                let (cx, cy) = (pitch_radius_m * phi.cos(), pitch_radius_m * phi.sin());
                let footprint = (0..facets)
                    .map(|j| {
                        let t = 2.0 * PI * (j as f64) / (facets as f64);
                        (cx + r * t.cos(), cy + r * t.sin())
                    })
                    .collect();
                PrismMagnet {
                    footprint,
                    z0_m,
                    thickness_m,
                    remanence_t,
                    polarity: Polarity::alternating(k),
                }
            })
            .collect();
        Self { magnets }
    }

    /// All bound-current loops for the ring.
    pub fn to_filaments(&self, n_axial: usize) -> Vec<Filament> {
        self.magnets.iter().flat_map(|m| m.to_filaments(n_axial)).collect()
    }

    /// The ring rotated about z by `angle` radians — the rotor position sweep.
    pub fn rotated_z(&self, angle: f64) -> MagnetRing {
        let (s, c) = angle.sin_cos();
        MagnetRing {
            magnets: self
                .magnets
                .iter()
                .map(|m| PrismMagnet {
                    footprint: m
                        .footprint
                        .iter()
                        .map(|&(x, y)| (c * x - s * y, s * x + c * y))
                        .collect(),
                    ..m.clone()
                })
                .collect(),
        }
    }

    /// Total pole-face area, m².
    pub fn face_area_m2(&self) -> f64 {
        self.magnets.iter().map(|m| m.face_area_m2()).sum()
    }
}
