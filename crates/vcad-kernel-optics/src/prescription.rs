//! The lens data table: an ordered list of refracting surfaces.
//!
//! Follows the universal lens-design sign convention: light travels +z;
//! a surface's radius of curvature is **positive when the center of
//! curvature lies to the right of (after) the vertex**. Surface k's
//! `thickness_mm` is the axial gap from its vertex to the next surface's
//! vertex; its `glass` is the medium *following* the surface. Object
//! space is air.
//!
//! Validation is fail-closed at construction: a semi-diameter outside the
//! conic cap's domain of definition, a non-positive aperture, or a
//! negative gap is an error — never a surface that silently traces wrong.

use serde::{Deserialize, Serialize};

use crate::glass::Glass;

/// One refracting (or stop) surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// Radius of curvature, mm. `f64::INFINITY` for a plane.
    pub radius_mm: f64,
    /// Conic constant κ (0 = sphere, −1 = paraboloid).
    pub conic: f64,
    /// Aperture semi-diameter, mm. Rays landing outside are vignetted.
    pub semi_diameter_mm: f64,
    /// Axial gap to the next surface's vertex, mm.
    pub thickness_mm: f64,
    /// Medium following this surface.
    pub glass: Glass,
    /// Marks the aperture stop (at most one per prescription).
    pub is_stop: bool,
}

impl Surface {
    /// A refracting spherical surface.
    pub fn sphere(radius_mm: f64, semi_diameter_mm: f64, thickness_mm: f64, glass: Glass) -> Self {
        Surface {
            radius_mm,
            conic: 0.0,
            semi_diameter_mm,
            thickness_mm,
            glass,
            is_stop: false,
        }
    }

    /// A plane aperture stop in air.
    pub fn stop(semi_diameter_mm: f64, thickness_mm: f64) -> Self {
        Surface {
            radius_mm: f64::INFINITY,
            conic: 0.0,
            semi_diameter_mm,
            thickness_mm,
            glass: Glass::Air,
            is_stop: true,
        }
    }

    /// Curvature c = 1/R (0 for a plane).
    pub fn curvature(&self) -> f64 {
        if self.radius_mm.is_infinite() {
            0.0
        } else {
            1.0 / self.radius_mm
        }
    }

    /// Sag z(s²) of the conic cap at radial distance² `s2` from the axis,
    /// or `None` outside the cap's domain of definition.
    ///
    /// z = c·s² / (1 + √(1 − (1+κ)c²s²)).
    pub fn sag(&self, s2: f64) -> Option<f64> {
        let c = self.curvature();
        let arg = 1.0 - (1.0 + self.conic) * c * c * s2;
        if arg < 0.0 {
            return None;
        }
        Some(c * s2 / (1.0 + arg.sqrt()))
    }
}

/// Prescription construction errors (fail-closed).
#[derive(Debug, Clone, PartialEq)]
pub enum PrescriptionError {
    /// No surfaces.
    Empty,
    /// Surface with non-positive semi-diameter.
    BadAperture(usize),
    /// Surface with negative thickness.
    NegativeThickness(usize),
    /// Semi-diameter outside the conic cap's domain of definition.
    ApertureOutsideCap(usize),
    /// More than one surface flagged as the stop.
    MultipleStops,
}

impl std::fmt::Display for PrescriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrescriptionError::Empty => write!(f, "prescription has no surfaces"),
            PrescriptionError::BadAperture(i) => {
                write!(f, "surface {i}: semi-diameter must be positive")
            }
            PrescriptionError::NegativeThickness(i) => {
                write!(f, "surface {i}: thickness must be non-negative")
            }
            PrescriptionError::ApertureOutsideCap(i) => write!(
                f,
                "surface {i}: semi-diameter exceeds the conic cap's domain"
            ),
            PrescriptionError::MultipleStops => write!(f, "more than one stop surface"),
        }
    }
}

impl std::error::Error for PrescriptionError {}

/// An ordered sequential system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prescription {
    /// Surfaces in the order light meets them. Surface 0's vertex is z = 0.
    pub surfaces: Vec<Surface>,
}

impl Prescription {
    /// Build with fail-closed validation.
    pub fn new(surfaces: Vec<Surface>) -> Result<Self, PrescriptionError> {
        if surfaces.is_empty() {
            return Err(PrescriptionError::Empty);
        }
        let mut stops = 0;
        for (i, s) in surfaces.iter().enumerate() {
            if s.semi_diameter_mm.is_nan() || s.semi_diameter_mm <= 0.0 {
                return Err(PrescriptionError::BadAperture(i));
            }
            if s.thickness_mm < 0.0 {
                return Err(PrescriptionError::NegativeThickness(i));
            }
            if s.sag(s.semi_diameter_mm * s.semi_diameter_mm).is_none() {
                return Err(PrescriptionError::ApertureOutsideCap(i));
            }
            if s.is_stop {
                stops += 1;
            }
        }
        if stops > 1 {
            return Err(PrescriptionError::MultipleStops);
        }
        Ok(Prescription { surfaces })
    }

    /// Vertex z of surface `i` (surface 0 at z = 0).
    pub fn vertex_z(&self, i: usize) -> f64 {
        self.surfaces[..i].iter().map(|s| s.thickness_mm).sum()
    }

    /// z of the last surface's vertex.
    pub fn last_vertex_z(&self) -> f64 {
        self.vertex_z(self.surfaces.len() - 1)
    }

    /// Refractive index of the medium *before* surface `i` at `lambda_um`
    /// (object space is air).
    pub fn index_before(&self, i: usize, lambda_um: f64) -> f64 {
        if i == 0 {
            1.0
        } else {
            self.surfaces[i - 1].glass.index(lambda_um)
        }
    }

    /// Refractive index of the medium *after* surface `i` at `lambda_um`.
    pub fn index_after(&self, i: usize, lambda_um: f64) -> f64 {
        self.surfaces[i].glass.index(lambda_um)
    }

    /// Thin-lens-equivalent power (1/mm) of the element bounded by
    /// surfaces `i` and `i+1`: φ = (n − 1)(c_i − c_{i+1}) with n taken at
    /// `lambda_um`. Ignores the element's thickness — this is the quantity
    /// the classic achromat condition is written in.
    pub fn thin_element_power(&self, i: usize, lambda_um: f64) -> f64 {
        let n = self.surfaces[i].glass.index(lambda_um);
        (n - 1.0) * (self.surfaces[i].curvature() - self.surfaces[i + 1].curvature())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_is_fail_closed() {
        assert_eq!(Prescription::new(vec![]), Err(PrescriptionError::Empty));
        let bad_ap = Surface::sphere(50.0, 0.0, 1.0, Glass::Air);
        assert_eq!(
            Prescription::new(vec![bad_ap]),
            Err(PrescriptionError::BadAperture(0))
        );
        let neg_t = Surface::sphere(50.0, 10.0, -1.0, Glass::Air);
        assert_eq!(
            Prescription::new(vec![neg_t]),
            Err(PrescriptionError::NegativeThickness(0))
        );
        // Hemisphere limit: sphere R = 10 has a cap only out to s = 10.
        let too_wide = Surface::sphere(10.0, 10.5, 1.0, Glass::Air);
        assert_eq!(
            Prescription::new(vec![too_wide]),
            Err(PrescriptionError::ApertureOutsideCap(0))
        );
    }

    #[test]
    fn sag_matches_sphere_closed_form() {
        // For a sphere, sag = R − √(R² − s²).
        let s = Surface::sphere(50.0, 20.0, 1.0, Glass::Air);
        for r in [0.0, 5.0, 12.0, 20.0] {
            let exact = 50.0 - (50.0f64 * 50.0 - r * r).sqrt();
            let got = s.sag(r * r).unwrap();
            assert!((got - exact).abs() < 1e-12, "s={r}: {got} vs {exact}");
        }
        // Plane sag is exactly zero.
        let p = Surface::stop(10.0, 0.0);
        assert_eq!(p.sag(25.0), Some(0.0));
    }

    #[test]
    fn vertex_accumulates_thickness() {
        let p = Prescription::new(vec![
            Surface::sphere(60.0, 12.0, 4.0, Glass::n_bk7()),
            Surface::sphere(-45.0, 12.0, 2.5, Glass::f2()),
            Surface::sphere(-120.0, 12.0, 0.0, Glass::Air),
        ])
        .unwrap();
        assert_eq!(p.vertex_z(0), 0.0);
        assert_eq!(p.vertex_z(1), 4.0);
        assert_eq!(p.vertex_z(2), 6.5);
        assert_eq!(p.last_vertex_z(), 6.5);
    }

    #[test]
    fn serde_round_trip() {
        let p = Prescription::new(vec![
            Surface::sphere(60.0, 12.0, 4.0, Glass::n_bk7()),
            Surface::sphere(-45.0, 12.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: Prescription = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
