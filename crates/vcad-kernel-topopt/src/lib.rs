#![warn(missing_docs)]

//! Topology optimization for the vcad kernel.
//!
//! Given a design domain (a bounding box or an existing solid's mesh),
//! loads, and supports, this crate finds the stiffest material layout that
//! uses only a target fraction of the domain volume — the classic
//! compliance-minimization problem solved with the SIMP method
//! (Solid Isotropic Material with Penalization):
//!
//! 1. The domain is voxelized into a uniform grid of 8-node hexahedral
//!    finite elements.
//! 2. Each SIMP iteration solves the elasticity system matrix-free with
//!    Jacobi-preconditioned conjugate gradients, computes compliance
//!    sensitivities, filters them for mesh independence, and updates the
//!    per-voxel densities with an optimality-criteria step under the
//!    volume constraint.
//! 3. The converged density field is turned back into a watertight
//!    triangle mesh with surface nets + Taubin smoothing — the organic,
//!    bone-like geometry topology optimization is known for.
//!
//! Coordinates are Z-up, units are millimeters, matching the rest of the
//! kernel. Results are deterministic for a given spec.
//!
//! # Example
//!
//! ```
//! use vcad_kernel_topopt::{optimize_box, Load, RegionBox, Support, TopoOptSpec};
//!
//! // A 40×10×20 mm cantilever: anchored on the x=0 face, tip load at the
//! // lower far edge pointing down.
//! let mut spec = TopoOptSpec::new(
//!     vec![Load {
//!         region: RegionBox { min: [40.0, 0.0, 0.0], max: [40.0, 10.0, 2.0] },
//!         force: [0.0, 0.0, -100.0],
//!     }],
//!     vec![Support {
//!         region: RegionBox { min: [0.0, 0.0, 0.0], max: [0.0, 10.0, 20.0] },
//!         fix: [true, true, true],
//!     }],
//! );
//! spec.resolution = 16;      // coarse for the doc test
//! spec.max_iterations = 6;
//! spec.volume_fraction = 0.4;
//!
//! let result = optimize_box([0.0, 0.0, 0.0], [40.0, 10.0, 20.0], &spec).unwrap();
//! assert!(result.mesh.num_triangles() > 0);
//! assert!(result.compliance_history.len() >= 2);
//! ```

mod domain;
mod extract;
mod fea;
mod simp;
mod spec;

pub use domain::Domain;
pub use fea::FeError;
pub use spec::{Load, RegionBox, Support, TopoOptSpec};

use vcad_kernel_tessellate::TriangleMesh;

/// Result of a topology optimization run.
#[derive(Debug)]
pub struct TopoOptResult {
    /// The optimized structure as a watertight triangle mesh (mm, Z-up).
    pub mesh: TriangleMesh,
    /// Compliance (strain energy measure) after each iteration; decreasing
    /// values mean a stiffer structure.
    pub compliance_history: Vec<f64>,
    /// SIMP iterations actually run.
    pub iterations: usize,
    /// Whether the density change converged below the spec tolerance.
    pub converged: bool,
    /// Material fraction of the design domain actually used.
    pub volume_fraction_achieved: f64,
    /// Voxel grid dimensions used, `[nx, ny, nz]`.
    pub grid: [usize; 3],
    /// Voxel edge length in mm.
    pub voxel_size: f64,
}

/// Errors from [`optimize`].
#[derive(Debug)]
pub enum TopoOptError {
    /// The problem specification is invalid.
    InvalidSpec(String),
    /// Boundary conditions or the domain are unusable.
    Fe(FeError),
}

impl std::fmt::Display for TopoOptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopoOptError::InvalidSpec(msg) => {
                write!(f, "invalid topology optimization spec: {msg}")
            }
            TopoOptError::Fe(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TopoOptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TopoOptError::Fe(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FeError> for TopoOptError {
    fn from(e: FeError) -> Self {
        TopoOptError::Fe(e)
    }
}

fn validate(spec: &TopoOptSpec) -> Result<(), TopoOptError> {
    if spec.loads.is_empty() {
        return Err(TopoOptError::InvalidSpec(
            "at least one load is required".into(),
        ));
    }
    if spec.supports.is_empty() {
        return Err(TopoOptError::InvalidSpec(
            "at least one support is required".into(),
        ));
    }
    if !(0.01..=0.99).contains(&spec.volume_fraction) {
        return Err(TopoOptError::InvalidSpec(format!(
            "volume_fraction must be in (0.01, 0.99), got {}",
            spec.volume_fraction
        )));
    }
    if spec.loads.iter().any(|l| l.force.iter().all(|c| *c == 0.0)) {
        return Err(TopoOptError::InvalidSpec("a load has zero force".into()));
    }
    if !(1.0..=6.0).contains(&spec.penalty) {
        return Err(TopoOptError::InvalidSpec(format!(
            "penalty must be in [1, 6], got {}",
            spec.penalty
        )));
    }
    if !(0.0..0.5).contains(&spec.poisson) {
        return Err(TopoOptError::InvalidSpec(format!(
            "poisson must be in [0, 0.5), got {}",
            spec.poisson
        )));
    }
    if spec.max_iterations == 0 {
        return Err(TopoOptError::InvalidSpec(
            "max_iterations must be > 0".into(),
        ));
    }
    Ok(())
}

/// Run topology optimization on a prepared design domain.
pub fn optimize(domain: &Domain, spec: &TopoOptSpec) -> Result<TopoOptResult, TopoOptError> {
    validate(spec)?;
    let sys = fea::FeSystem::build(domain, spec.poisson, &spec.loads, &spec.supports)?;
    let simp = simp::optimize_densities(domain, &sys, spec);

    let nact = domain.num_active().max(1) as f64;
    let volume_fraction_achieved = simp.densities.iter().sum::<f64>() / nact;

    let mesh = extract::extract_mesh(domain, &simp.densities, spec.smooth_iterations);

    Ok(TopoOptResult {
        mesh,
        compliance_history: simp.compliance_history,
        iterations: simp.iterations,
        converged: simp.converged,
        volume_fraction_achieved,
        grid: [domain.nx, domain.ny, domain.nz],
        voxel_size: domain.h,
    })
}

/// Optimize within an axis-aligned box design domain.
pub fn optimize_box(
    min: [f64; 3],
    max: [f64; 3],
    spec: &TopoOptSpec,
) -> Result<TopoOptResult, TopoOptError> {
    if (0..3).any(|a| !(max[a] - min[a]).is_finite() || max[a] - min[a] <= 0.0) {
        return Err(TopoOptError::InvalidSpec(
            "domain box must have positive size on every axis".into(),
        ));
    }
    let domain = Domain::from_bbox(min, max, spec.resolution);
    optimize(&domain, spec)
}

/// Optimize using an existing solid's tessellation as the design domain.
///
/// The mesh is voxelized (voxel centers inside the closed mesh become the
/// design domain), so material only ever appears inside the original part —
/// this is the "lightweight an existing bracket" workflow.
pub fn optimize_mesh(
    mesh: &TriangleMesh,
    spec: &TopoOptSpec,
) -> Result<TopoOptResult, TopoOptError> {
    let domain = Domain::from_mesh(mesh, spec.resolution);
    optimize(&domain, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bracket_spec() -> TopoOptSpec {
        let mut spec = TopoOptSpec::new(
            vec![Load {
                region: RegionBox {
                    min: [24.0, 0.0, 0.0],
                    max: [24.0, 6.0, 2.0],
                },
                force: [0.0, 0.0, -100.0],
            }],
            vec![Support {
                region: RegionBox {
                    min: [0.0, 0.0, 0.0],
                    max: [0.0, 6.0, 12.0],
                },
                fix: [true, true, true],
            }],
        );
        spec.resolution = 24;
        spec.max_iterations = 15;
        spec.volume_fraction = 0.35;
        spec
    }

    #[test]
    fn end_to_end_cantilever() {
        let spec = bracket_spec();
        let result = optimize_box([0.0; 3], [24.0, 6.0, 12.0], &spec).unwrap();

        assert!(result.mesh.num_triangles() > 100);
        assert_eq!(result.mesh.vertices.len(), result.mesh.normals.len());
        assert!(
            (result.volume_fraction_achieved - 0.35).abs() < 0.03,
            "volume fraction {}",
            result.volume_fraction_achieved
        );
        let first = result.compliance_history[0];
        let last = *result.compliance_history.last().unwrap();
        assert!(last < first * 0.9, "compliance {first} -> {last}");

        // Mesh must stay inside the design domain (small tolerance for
        // smoothing overshoot).
        for v in result.mesh.vertices.chunks_exact(3) {
            assert!(v[0] >= -0.5 && v[0] <= 24.5);
            assert!(v[1] >= -0.5 && v[1] <= 6.5);
            assert!(v[2] >= -0.5 && v[2] <= 12.5);
        }
    }

    #[test]
    fn deterministic_output() {
        let mut spec = bracket_spec();
        spec.resolution = 12;
        spec.max_iterations = 6;
        let a = optimize_box([0.0; 3], [24.0, 6.0, 12.0], &spec).unwrap();
        let b = optimize_box([0.0; 3], [24.0, 6.0, 12.0], &spec).unwrap();
        assert_eq!(a.mesh.vertices, b.mesh.vertices);
        assert_eq!(a.mesh.indices, b.mesh.indices);
    }

    #[test]
    fn missing_loads_rejected() {
        let spec = TopoOptSpec::new(
            vec![],
            vec![Support {
                region: RegionBox {
                    min: [0.0; 3],
                    max: [0.0, 10.0, 10.0],
                },
                fix: [true; 3],
            }],
        );
        assert!(matches!(
            optimize_box([0.0; 3], [10.0; 3], &spec),
            Err(TopoOptError::InvalidSpec(_))
        ));
    }

    #[test]
    fn spec_roundtrips_through_json() {
        let spec = bracket_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: TopoOptSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.resolution, spec.resolution);
        assert_eq!(back.loads.len(), 1);
    }
}
