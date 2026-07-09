//! Problem specification types for topology optimization.

use serde::{Deserialize, Serialize};

/// Axis-aligned box region in world (mm) coordinates.
///
/// Used to select grid nodes for loads and supports. A node is inside the
/// region when it lies within the box expanded by half a voxel in every
/// direction, so a zero-thickness box (e.g. `min.x == max.x`) still catches
/// the plane of nodes nearest to it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RegionBox {
    /// Minimum corner `[x, y, z]` in mm.
    pub min: [f64; 3],
    /// Maximum corner `[x, y, z]` in mm.
    pub max: [f64; 3],
}

impl RegionBox {
    /// Whether `p` lies inside the box expanded by `pad` on all sides.
    pub fn contains(&self, p: [f64; 3], pad: f64) -> bool {
        (0..3).all(|a| p[a] >= self.min[a] - pad && p[a] <= self.max[a] + pad)
    }
}

/// A load applied to the structure.
///
/// `force` is the **total** force vector in Newtons (any consistent unit
/// works — the optimal layout is invariant to force scale), distributed
/// evenly over every grid node inside `region`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Load {
    /// Where the load is applied.
    pub region: RegionBox,
    /// Total force vector `[fx, fy, fz]`.
    pub force: [f64; 3],
}

/// A support (fixed boundary condition).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Support {
    /// Where the structure is anchored.
    pub region: RegionBox,
    /// Which translational directions are fixed (`[x, y, z]`).
    /// Defaults to fully fixed.
    #[serde(default = "default_fix")]
    pub fix: [bool; 3],
}

fn default_fix() -> [bool; 3] {
    [true, true, true]
}

/// Topology optimization parameters (SIMP method).
///
/// All fields have sensible defaults; a spec only needs `loads`,
/// `supports`, and (usually) `volume_fraction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopoOptSpec {
    /// Fraction of the design domain to keep as material, in `(0, 1)`.
    #[serde(default = "default_volume_fraction")]
    pub volume_fraction: f64,
    /// Voxel count along the longest axis of the domain bounding box.
    /// Clamped to `[2, 256]`; 32–64 is the practical sweet spot.
    #[serde(default = "default_resolution")]
    pub resolution: usize,
    /// SIMP penalization exponent (typically 3).
    #[serde(default = "default_penalty")]
    pub penalty: f64,
    /// Sensitivity filter radius in voxels (typically 1.2–2.5).
    #[serde(default = "default_filter_radius")]
    pub filter_radius: f64,
    /// Maximum optimization iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Convergence tolerance on the max density change per iteration.
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Poisson's ratio of the material.
    #[serde(default = "default_poisson")]
    pub poisson: f64,
    /// Taubin smoothing passes applied to the extracted surface.
    #[serde(default = "default_smooth_iterations")]
    pub smooth_iterations: usize,
    /// Applied loads (at least one required).
    pub loads: Vec<Load>,
    /// Supports (at least one required).
    pub supports: Vec<Support>,
}

fn default_volume_fraction() -> f64 {
    0.3
}
fn default_resolution() -> usize {
    48
}
fn default_penalty() -> f64 {
    3.0
}
fn default_filter_radius() -> f64 {
    1.5
}
fn default_max_iterations() -> usize {
    40
}
fn default_tolerance() -> f64 {
    0.01
}
fn default_poisson() -> f64 {
    0.3
}
fn default_smooth_iterations() -> usize {
    5
}

impl TopoOptSpec {
    /// A spec with default parameters and the given loads/supports.
    pub fn new(loads: Vec<Load>, supports: Vec<Support>) -> Self {
        Self {
            volume_fraction: default_volume_fraction(),
            resolution: default_resolution(),
            penalty: default_penalty(),
            filter_radius: default_filter_radius(),
            max_iterations: default_max_iterations(),
            tolerance: default_tolerance(),
            poisson: default_poisson(),
            smooth_iterations: default_smooth_iterations(),
            loads,
            supports,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_deserializes_with_defaults() {
        let json = r#"{
            "loads": [{"region": {"min": [0,0,0], "max": [0,0,0]}, "force": [0,0,-100]}],
            "supports": [{"region": {"min": [0,0,0], "max": [0,10,10]}}]
        }"#;
        let spec: TopoOptSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.resolution, 48);
        assert!((spec.volume_fraction - 0.3).abs() < 1e-12);
        assert_eq!(spec.supports[0].fix, [true, true, true]);
    }

    #[test]
    fn region_contains_with_pad() {
        let r = RegionBox {
            min: [0.0, 0.0, 0.0],
            max: [0.0, 10.0, 10.0],
        };
        assert!(r.contains([0.4, 5.0, 5.0], 0.5));
        assert!(!r.contains([1.0, 5.0, 5.0], 0.5));
    }
}
