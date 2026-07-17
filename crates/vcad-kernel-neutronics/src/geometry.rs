//! 1D layered geometries: slab stacks and concentric spherical shells.
//!
//! Layer thicknesses are **millimeters** (vcad convention — shield
//! designs are dimensioned in mm); everything downstream of the public
//! API is centimeters because macroscopic cross sections are 1/cm.
//!
//! Regions are the layers themselves: every layer is a tally region.
//! Thin detector layers (e.g. a 2 cm air shell at the operator radius)
//! are how point-ish detectors are expressed at M0 — the track-length
//! estimator over a thin shell is the standard low-variance flux
//! estimator for isotropic sources; a next-event (point-detector)
//! estimator is a flagged later rung.

use crate::materials::Material;

/// One layer (slab) or shell (sphere).
#[derive(Debug, Clone)]
pub struct Layer {
    /// Material filling the layer.
    pub material: Material,
    /// Thickness (slab) or radial extent (sphere), millimeters.
    pub thickness_mm: f64,
}

impl Layer {
    /// Convenience constructor.
    pub fn new(material: Material, thickness_mm: f64) -> Self {
        Layer {
            material,
            thickness_mm,
        }
    }
}

/// A 1D layered geometry.
#[derive(Debug, Clone)]
pub enum Geometry {
    /// Layers stacked along +x from x = 0, vacuum on both outer faces.
    /// (Per unit area: fluxes are per cm² of slab face.)
    Slab(Vec<Layer>),
    /// Concentric shells from r = 0 outward, vacuum beyond the last.
    Sphere(Vec<Layer>),
}

/// Geometry construction/validation failures. Fail-closed: a geometry
/// that cannot be validated cannot be run.
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryError {
    /// No layers.
    Empty,
    /// A layer had a non-positive or non-finite thickness.
    BadThickness {
        /// Index of the offending layer.
        layer: usize,
        /// The rejected value, mm.
        thickness_mm: f64,
    },
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeometryError::Empty => write!(f, "geometry has no layers"),
            GeometryError::BadThickness {
                layer,
                thickness_mm,
            } => write!(f, "layer {layer} has invalid thickness {thickness_mm} mm"),
        }
    }
}

impl std::error::Error for GeometryError {}

impl Geometry {
    /// The layer list.
    pub fn layers(&self) -> &[Layer] {
        match self {
            Geometry::Slab(l) | Geometry::Sphere(l) => l,
        }
    }

    /// Number of regions (= layers).
    pub fn region_count(&self) -> usize {
        self.layers().len()
    }

    /// Validate: at least one layer, all thicknesses positive and finite.
    pub fn validate(&self) -> Result<(), GeometryError> {
        let layers = self.layers();
        if layers.is_empty() {
            return Err(GeometryError::Empty);
        }
        for (i, l) in layers.iter().enumerate() {
            if !(l.thickness_mm.is_finite() && l.thickness_mm > 0.0) {
                return Err(GeometryError::BadThickness {
                    layer: i,
                    thickness_mm: l.thickness_mm,
                });
            }
        }
        Ok(())
    }

    /// Cumulative boundaries in cm: `[0, b1, …, b_n]`.
    pub(crate) fn boundaries_cm(&self) -> Vec<f64> {
        let mut b = vec![0.0];
        for l in self.layers() {
            b.push(b.last().unwrap() + l.thickness_mm * 0.1);
        }
        b
    }

    /// Region volume in cm³ (slab: per cm² of face, i.e. thickness).
    pub(crate) fn region_volume_cc(&self, region: usize) -> f64 {
        let b = self.boundaries_cm();
        match self {
            Geometry::Slab(_) => b[region + 1] - b[region],
            Geometry::Sphere(_) => {
                4.0 / 3.0 * std::f64::consts::PI * (b[region + 1].powi(3) - b[region].powi(3))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::Material;

    #[test]
    fn boundaries_and_volumes() {
        let g = Geometry::Sphere(vec![
            Layer::new(Material::void(), 100.0),
            Layer::new(Material::void(), 50.0),
        ]);
        let b = g.boundaries_cm();
        assert_eq!(b, vec![0.0, 10.0, 15.0]);
        let v0 = g.region_volume_cc(0);
        assert!((v0 - 4.0 / 3.0 * std::f64::consts::PI * 1000.0).abs() < 1.0e-9);
        let s = Geometry::Slab(vec![Layer::new(Material::void(), 30.0)]);
        assert!((s.region_volume_cc(0) - 3.0).abs() < 1.0e-12);
    }

    #[test]
    fn validation_fails_closed() {
        assert_eq!(Geometry::Slab(vec![]).validate(), Err(GeometryError::Empty));
        let g = Geometry::Slab(vec![Layer::new(Material::void(), -1.0)]);
        assert!(matches!(
            g.validate(),
            Err(GeometryError::BadThickness { layer: 0, .. })
        ));
    }
}
