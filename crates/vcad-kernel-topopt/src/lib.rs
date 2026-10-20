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

mod analyze;
mod domain;
mod extract;
mod fea;
mod simp;
mod spec;

pub use analyze::von_mises;
pub use analyze::{
    analyze, analyze_box, analyze_fields, analyze_fields_thermal, analyze_fields_thermal_warm,
    analyze_mesh, AnalysisSpec, AnalyzeError, StaticAnalysis, StaticFields, ThermalLoad,
};
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
        for v in result.mesh.vertices.as_chunks::<3>().0 {
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

    fn thermal_bar(supports: Vec<Support>) -> (Domain, StaticAnalysis, StaticFields) {
        // 40×4×4 mm bar, h = 1 mm, uniform ΔT = 10 K, α = 1e-5, E = 1e5 MPa.
        let d = Domain::from_bbox([0.0; 3], [40.0, 4.0, 4.0], 40);
        let spec = AnalysisSpec {
            resolution: 40,
            youngs_modulus_mpa: 1e5,
            poisson: 0.3,
            // A negligible mechanical load (a spec needs one).
            loads: vec![Load {
                region: RegionBox {
                    min: [20.0, 2.0, 4.0],
                    max: [20.0, 2.0, 4.0],
                },
                force: [0.0, 0.0, -1e-9],
            }],
            supports,
        };
        let t = ThermalLoad {
            delta_t_k: vec![10.0; d.num_elements()],
            alpha_per_k: 1e-5,
        };
        let (a, f) = analyze_fields_thermal(&d, &spec, Some(&t)).unwrap();
        (d, a, f)
    }

    fn face(x: f64, fix: [bool; 3]) -> Support {
        Support {
            region: RegionBox {
                min: [x, 0.0, 0.0],
                max: [x, 4.0, 4.0],
            },
            fix,
        }
    }

    #[test]
    fn thermal_free_expansion_is_stress_free() {
        // Statically determinate rollers: x on the x=0 face, plus one
        // y and one z pin — the bar may grow freely in every direction.
        let pins = |p: [f64; 3], fix| Support {
            region: RegionBox { min: p, max: p },
            fix,
        };
        let (_, a, f) = thermal_bar(vec![
            face(0.0, [true, false, false]),
            pins([0.0, 0.0, 0.0], [false, true, true]),
            pins([0.0, 4.0, 0.0], [false, false, true]),
        ]);
        let peak = f.von_mises_mpa.iter().cloned().fold(0.0, f64::max);
        // EαΔT = 10 MPa would be the fully restrained stress.
        assert!(
            peak < 1e-3,
            "free expansion should be stress-free, got {peak} MPa"
        );
        // Tip moves αΔT·L = 4e-3 mm (plus the lateral growth of the corner).
        assert!(
            (a.max_displacement_mm - 4e-3).abs() / 4e-3 < 0.02,
            "tip {}",
            a.max_displacement_mm
        );
    }

    #[test]
    fn thermal_clamped_bar_reads_e_alpha_dt() {
        // Roller ends (x fixed only) + lateral pins: exactly uniaxial, so
        // every element reads |σ_xx| = E·α·ΔT = 10 MPa.
        let pins = |p: [f64; 3], fix| Support {
            region: RegionBox { min: p, max: p },
            fix,
        };
        let (d, _, f) = thermal_bar(vec![
            face(0.0, [true, false, false]),
            face(40.0, [true, false, false]),
            pins([0.0, 0.0, 0.0], [false, true, true]),
            pins([0.0, 4.0, 0.0], [false, false, true]),
        ]);
        for e in [d.eidx(0, 0, 0), d.eidx(20, 2, 2), d.eidx(39, 3, 3)] {
            let vm = f.von_mises_mpa[e];
            assert!(
                (vm - 10.0).abs() < 1e-3,
                "element {e}: von Mises {vm} MPa, want 10"
            );
        }
        // Fully clamped ends add lateral restraint near the ends; mid-span
        // (10 widths away) stays within a few % of uniaxial.
        let (d, _, f) = thermal_bar(vec![face(0.0, [true; 3]), face(40.0, [true; 3])]);
        let mid = f.von_mises_mpa[d.eidx(20, 2, 2)];
        assert!(
            (mid - 10.0).abs() < 0.5,
            "clamped mid-span von Mises {mid} MPa"
        );
    }

    #[test]
    fn thermal_load_must_match_grid() {
        let d = Domain::from_bbox([0.0; 3], [4.0, 4.0, 4.0], 4);
        let spec = AnalysisSpec {
            resolution: 4,
            youngs_modulus_mpa: 1e5,
            poisson: 0.3,
            loads: vec![Load {
                region: RegionBox {
                    min: [4.0, 0.0, 0.0],
                    max: [4.0, 4.0, 4.0],
                },
                force: [1.0, 0.0, 0.0],
            }],
            supports: vec![face(0.0, [true; 3])],
        };
        let t = ThermalLoad {
            delta_t_k: vec![1.0; 3],
            alpha_per_k: 1e-5,
        };
        assert!(analyze_fields_thermal(&d, &spec, Some(&t)).is_err());
    }
}
