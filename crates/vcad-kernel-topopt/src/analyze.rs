//! Standalone static structural analysis on the voxel FE machinery.
//!
//! This is the fast inner loop of the two-tier physics pattern: the same
//! solver family serves both tiers, and **resolution is the fidelity dial**.
//! A coarse grid answers in milliseconds and is honest about being an
//! estimate (`basis: predicted` upstream); a fine grid is the trusted
//! verify pass. Because both tiers share one discretization and solver,
//! "verify" genuinely refines "predict" rather than being a different
//! oracle with different blind spots.

use crate::domain::Domain;
use crate::fea::FeSystem;
use crate::spec::{Load, Support};
use serde::{Deserialize, Serialize};
use vcad_kernel_tessellate::TriangleMesh;

/// Specification for a static analysis run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSpec {
    /// Voxel count along the longest axis. Clamped to `[2, 256]`.
    /// 32 is the fast predict tier; 64–96 is the verify tier. Trilinear
    /// hexes lock in bending below ~4 elements through the thinnest
    /// section — going coarser than 32 on slender parts is dishonest, not
    /// fast (a res-20 cantilever reads 2.2× too stiff).
    #[serde(default = "default_resolution")]
    pub resolution: usize,
    /// Young's modulus in MPa (N/mm²), e.g. 69_000 for 6061 aluminum.
    #[serde(default = "default_youngs_modulus")]
    pub youngs_modulus_mpa: f64,
    /// Poisson's ratio.
    #[serde(default = "default_poisson")]
    pub poisson: f64,
    /// Applied loads (at least one required). Forces in Newtons.
    pub loads: Vec<Load>,
    /// Supports (at least one required).
    pub supports: Vec<Support>,
}

fn default_resolution() -> usize {
    32
}
fn default_youngs_modulus() -> f64 {
    69_000.0 // 6061-T6 aluminum
}
fn default_poisson() -> f64 {
    0.33
}

/// Result of a static analysis solve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticAnalysis {
    /// Compliance `fᵀu` in N·mm — the work done by the loads. Lower is
    /// stiffer for the same loads.
    pub compliance_n_mm: f64,
    /// Maximum nodal displacement magnitude in mm.
    pub max_displacement_mm: f64,
    /// World position of the most-displaced node, mm.
    pub max_displacement_at: [f64; 3],
    /// Maximum element-centroid von Mises stress in MPa. Voxel FEA smears
    /// stress concentrations; treat as an estimate, tighter at higher
    /// resolution.
    pub max_von_mises_mpa: f64,
    /// World position of the centroid of the most-stressed element, mm.
    pub max_stress_at: [f64; 3],
    /// Voxel grid dimensions used, `[nx, ny, nz]`.
    pub grid: [usize; 3],
    /// Voxel edge length in mm.
    pub voxel_size_mm: f64,
    /// Relative residual the PCG solve reached.
    pub relative_residual: f64,
    /// Whether the solve converged below tolerance.
    pub converged: bool,
}

/// Errors from static analysis.
#[derive(Debug)]
pub enum AnalyzeError {
    /// The specification is invalid.
    InvalidSpec(String),
    /// Boundary conditions or the domain are unusable.
    Fe(crate::fea::FeError),
}

impl std::fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalyzeError::InvalidSpec(msg) => write!(f, "invalid analysis spec: {msg}"),
            AnalyzeError::Fe(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AnalyzeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AnalyzeError::Fe(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::fea::FeError> for AnalyzeError {
    fn from(e: crate::fea::FeError) -> Self {
        AnalyzeError::Fe(e)
    }
}

fn validate(spec: &AnalysisSpec) -> Result<(), AnalyzeError> {
    if spec.loads.is_empty() {
        return Err(AnalyzeError::InvalidSpec(
            "at least one load is required".into(),
        ));
    }
    if spec.supports.is_empty() {
        return Err(AnalyzeError::InvalidSpec(
            "at least one support is required".into(),
        ));
    }
    if spec.loads.iter().any(|l| l.force.iter().all(|c| *c == 0.0)) {
        return Err(AnalyzeError::InvalidSpec("a load has zero force".into()));
    }
    if !spec.youngs_modulus_mpa.is_finite() || spec.youngs_modulus_mpa <= 0.0 {
        return Err(AnalyzeError::InvalidSpec(format!(
            "youngs_modulus_mpa must be positive, got {}",
            spec.youngs_modulus_mpa
        )));
    }
    if !(0.0..0.5).contains(&spec.poisson) {
        return Err(AnalyzeError::InvalidSpec(format!(
            "poisson must be in [0, 0.5), got {}",
            spec.poisson
        )));
    }
    Ok(())
}

/// Natural-coordinate signs per local hex node (standard ordering).
const THERMAL_SIGNS: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

const SOLVE_TOL: f64 = 1e-8;
const SOLVE_MAX_ITER: usize = 6000;

/// Run a static solve on a prepared domain.
pub fn analyze(domain: &Domain, spec: &AnalysisSpec) -> Result<StaticAnalysis, AnalyzeError> {
    analyze_fields(domain, spec).map(|(a, _)| a)
}

/// Per-element result fields of a static solve, on the domain's full
/// element grid (index `(iz * ny + iy) * nx + ix`; inactive elements are 0).
#[derive(Debug, Clone)]
pub struct StaticFields {
    /// Element-centroid von Mises stress, MPa.
    pub von_mises_mpa: Vec<f64>,
    /// Displacement magnitude at the element centroid (mean of its 8
    /// nodal displacement vectors), mm.
    pub displacement_mm: Vec<f64>,
    /// Element-centroid stress tensor, MPa, Voigt order
    /// `[xx, yy, zz, xy, yz, xz]`. Linear in (loads, ΔT), so solutions
    /// superpose — e.g. a reduced-order sweep over many thermal states.
    pub stress_mpa: Vec<[f64; 6]>,
}

/// Von Mises equivalent of a Voigt stress `[xx, yy, zz, xy, yz, xz]`.
pub fn von_mises(s: &[f64; 6]) -> f64 {
    (0.5 * ((s[0] - s[1]).powi(2) + (s[1] - s[2]).powi(2) + (s[2] - s[0]).powi(2))
        + 3.0 * (s[3] * s[3] + s[4] * s[4] + s[5] * s[5]))
        .sqrt()
}

/// A thermal-expansion load: per-element temperature rise above the
/// stress-free reference, applied as the isotropic initial strain
/// `ε₀ = α·ΔT` (thermo-elastic coupling, one-way: the temperature field is
/// an input, e.g. from vcad-kernel-thermal on the same grid).
#[derive(Debug, Clone)]
pub struct ThermalLoad {
    /// ΔT per grid element (index `(iz * ny + iy) * nx + ix`), K. Entries
    /// for inactive elements are ignored.
    pub delta_t_k: Vec<f64>,
    /// Linear expansion coefficient, 1/K (6061-T6: 23e-6).
    pub alpha_per_k: f64,
}

/// [`analyze`], also returning the per-element stress and displacement
/// fields (for field export / visualization).
pub fn analyze_fields(
    domain: &Domain,
    spec: &AnalysisSpec,
) -> Result<(StaticAnalysis, StaticFields), AnalyzeError> {
    analyze_fields_thermal(domain, spec, None)
}

/// [`analyze_fields`] with an optional thermal-expansion load superposed on
/// the mechanical loads. Stresses are `σ = C (ε − ε₀)`.
///
/// The equivalent nodal force of a uniform `ε₀` over a trilinear hex is
/// exact with the centroid shape gradients: `∫ ∂N_k/∂x_a dV = s_k[a]·h²/4`,
/// so `f_k,a = s_k[a]·h²/4 · E·(c1 + 2·c2)·α·ΔT` (the isotropic `C·ε₀`).
pub fn analyze_fields_thermal(
    domain: &Domain,
    spec: &AnalysisSpec,
    thermal: Option<&ThermalLoad>,
) -> Result<(StaticAnalysis, StaticFields), AnalyzeError> {
    analyze_fields_thermal_warm(domain, spec, thermal, &mut Vec::new())
}

/// [`analyze_fields_thermal`], warm-starting the PCG solve from `warm`
/// (unit-E nodal displacements from a previous solve on the same domain
/// and supports; resized/zeroed when it doesn't match) and leaving this
/// solve's displacements there. For sequences of nearby load cases — the
/// frames of a transient — this cuts iterations sharply; the converged
/// answer is the same to solver tolerance.
pub fn analyze_fields_thermal_warm(
    domain: &Domain,
    spec: &AnalysisSpec,
    thermal: Option<&ThermalLoad>,
    warm: &mut Vec<f64>,
) -> Result<(StaticAnalysis, StaticFields), AnalyzeError> {
    validate(spec)?;
    if let Some(t) = thermal {
        if t.delta_t_k.len() != domain.num_elements() || !t.alpha_per_k.is_finite() {
            return Err(AnalyzeError::InvalidSpec(format!(
                "thermal load needs one ΔT per grid element ({}), got {}",
                domain.num_elements(),
                t.delta_t_k.len()
            )));
        }
    }
    let mut sys = FeSystem::build(domain, spec.poisson, &spec.loads, &spec.supports)?;
    // Isotropic C·ε₀ per unit (α·ΔT), unit E.
    let c_iso = {
        let nu = spec.poisson;
        ((1.0 - nu) + 2.0 * nu) / ((1.0 + nu) * (1.0 - 2.0 * nu))
    };
    // ε₀ per active element (by position in `sys.edofs`).
    let eps0: Vec<f64> = match thermal {
        Some(t) => sys
            .active_elems
            .iter()
            .map(|&e| t.alpha_per_k * t.delta_t_k[e as usize])
            .collect(),
        None => vec![0.0; sys.active_elems.len()],
    };
    if thermal.is_some() {
        // Real thermal forces scaled into the unit-E system: the solve below
        // divides every load by E, so add E·g here.
        let w = spec.youngs_modulus_mpa * c_iso * domain.h * domain.h / 4.0;
        for (ei, dofs) in sys.edofs.iter().enumerate() {
            for (k, sg) in THERMAL_SIGNS.iter().enumerate() {
                for a in 0..3 {
                    let d = dofs[3 * k + a] as usize;
                    if !sys.fixed[d] {
                        sys.f[d] += sg[a] * w * eps0[ei];
                    }
                }
            }
        }
    }

    // Solve at unit Young's modulus; linear elasticity lets us rescale.
    let scales = vec![1.0f64; sys.active_elems.len()];
    if warm.len() != sys.ndof {
        *warm = vec![0.0f64; sys.ndof];
    }
    let u: &mut [f64] = warm;
    let relres = sys.solve(&scales, u, SOLVE_TOL, SOLVE_MAX_ITER);
    let e = spec.youngs_modulus_mpa;
    // u_real = u_unit / E; compliance_real = fᵀu / E.
    let compliance = sys.f.iter().zip(u.iter()).map(|(a, b)| a * b).sum::<f64>() / e;

    // Max nodal displacement.
    let mut max_disp = 0.0f64;
    let mut max_disp_node = 0usize;
    for n in 0..domain.num_nodes() {
        let d2 = u[3 * n].powi(2) + u[3 * n + 1].powi(2) + u[3 * n + 2].powi(2);
        if d2 > max_disp {
            max_disp = d2;
            max_disp_node = n;
        }
    }
    let max_displacement_mm = max_disp.sqrt() / e;

    // Element-centroid von Mises stress. At the element center the shape
    // derivative of node k along axis a is s_k[a] / (4h) (trilinear hex).
    let (c1, c2, g) = {
        let nu = spec.poisson;
        (
            (1.0 - nu) / ((1.0 + nu) * (1.0 - 2.0 * nu)),
            nu / ((1.0 + nu) * (1.0 - 2.0 * nu)),
            1.0 / (2.0 * (1.0 + nu)),
        )
    };
    const SIGNS: [[f64; 3]; 8] = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let inv4h = 1.0 / (4.0 * domain.h);
    let mut fields = StaticFields {
        von_mises_mpa: vec![0.0; domain.num_elements()],
        stress_mpa: vec![[0.0; 6]; domain.num_elements()],
        displacement_mm: vec![0.0; domain.num_elements()],
    };
    let mut max_vm = 0.0f64;
    let mut max_vm_elem = 0u32;
    for (ei, dofs) in sys.edofs.iter().enumerate() {
        // Strain at centroid (unit-E displacements).
        let mut eps = [0.0f64; 6]; // xx, yy, zz, xy, yz, xz (engineering shear)
        for (k, s) in SIGNS.iter().enumerate() {
            let ux = u[dofs[3 * k] as usize];
            let uy = u[dofs[3 * k + 1] as usize];
            let uz = u[dofs[3 * k + 2] as usize];
            let (dx, dy, dz) = (s[0] * inv4h, s[1] * inv4h, s[2] * inv4h);
            eps[0] += dx * ux;
            eps[1] += dy * uy;
            eps[2] += dz * uz;
            eps[3] += dy * ux + dx * uy;
            eps[4] += dz * uy + dy * uz;
            eps[5] += dz * ux + dx * uz;
        }
        // Stress (the unit E cancels against 1/E on u, so this is real MPa),
        // minus the thermal initial stress E·C·ε₀ on the normal components.
        let s0 = e * c_iso * eps0[ei];
        let sx = c1 * eps[0] + c2 * (eps[1] + eps[2]) - s0;
        let sy = c1 * eps[1] + c2 * (eps[0] + eps[2]) - s0;
        let sz = c1 * eps[2] + c2 * (eps[0] + eps[1]) - s0;
        let (txy, tyz, txz) = (g * eps[3], g * eps[4], g * eps[5]);
        let vm = (0.5 * ((sx - sy).powi(2) + (sy - sz).powi(2) + (sz - sx).powi(2))
            + 3.0 * (txy * txy + tyz * tyz + txz * txz))
            .sqrt();
        let grid_e = sys.active_elems[ei] as usize;
        fields.von_mises_mpa[grid_e] = vm;
        fields.stress_mpa[grid_e] = [sx, sy, sz, txy, tyz, txz];
        let mut mean = [0.0f64; 3];
        for k in 0..8 {
            for (a, m) in mean.iter_mut().enumerate() {
                *m += u[dofs[3 * k + a] as usize] / 8.0;
            }
        }
        fields.displacement_mm[grid_e] =
            (mean[0].powi(2) + mean[1].powi(2) + mean[2].powi(2)).sqrt() / e;
        if vm > max_vm {
            max_vm = vm;
            max_vm_elem = sys.active_elems[ei];
        }
    }

    // World positions for the argmax node/element.
    let nxp = domain.nx + 1;
    let nyp = domain.ny + 1;
    let (nix, niy, niz) = (
        max_disp_node % nxp,
        (max_disp_node / nxp) % nyp,
        max_disp_node / (nxp * nyp),
    );
    let e_us = max_vm_elem as usize;
    let (eix, eiy, eiz) = (
        e_us % domain.nx,
        (e_us / domain.nx) % domain.ny,
        e_us / (domain.nx * domain.ny),
    );
    let ecenter = [
        domain.origin[0] + (eix as f64 + 0.5) * domain.h,
        domain.origin[1] + (eiy as f64 + 0.5) * domain.h,
        domain.origin[2] + (eiz as f64 + 0.5) * domain.h,
    ];

    Ok((
        StaticAnalysis {
            compliance_n_mm: compliance,
            max_displacement_mm,
            max_displacement_at: domain.node_pos(nix, niy, niz),
            max_von_mises_mpa: max_vm,
            max_stress_at: ecenter,
            grid: [domain.nx, domain.ny, domain.nz],
            voxel_size_mm: domain.h,
            relative_residual: relres,
            converged: relres < 1e-6,
        },
        fields,
    ))
}

/// Analyze an axis-aligned solid box.
pub fn analyze_box(
    min: [f64; 3],
    max: [f64; 3],
    spec: &AnalysisSpec,
) -> Result<StaticAnalysis, AnalyzeError> {
    if (0..3).any(|a| !(max[a] - min[a]).is_finite() || max[a] - min[a] <= 0.0) {
        return Err(AnalyzeError::InvalidSpec(
            "domain box must have positive size on every axis".into(),
        ));
    }
    let domain = Domain::from_bbox(min, max, spec.resolution);
    analyze(&domain, spec)
}

/// Analyze an existing solid via its tessellation (voxelized like
/// [`crate::optimize_mesh`]).
pub fn analyze_mesh(
    mesh: &TriangleMesh,
    spec: &AnalysisSpec,
) -> Result<StaticAnalysis, AnalyzeError> {
    let domain = Domain::from_mesh(mesh, spec.resolution);
    analyze(&domain, spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::RegionBox;

    fn cantilever_spec(resolution: usize) -> AnalysisSpec {
        AnalysisSpec {
            resolution,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            loads: vec![Load {
                region: RegionBox {
                    min: [80.0, 0.0, 0.0],
                    max: [80.0, 10.0, 10.0],
                },
                force: [0.0, 0.0, -100.0],
            }],
            supports: vec![Support {
                region: RegionBox {
                    min: [0.0, 0.0, 0.0],
                    max: [0.0, 10.0, 10.0],
                },
                fix: [true, true, true],
            }],
        }
    }

    #[test]
    fn cantilever_matches_beam_theory_order() {
        // 80×10×10 mm aluminum cantilever, 100 N tip load.
        // Euler–Bernoulli: δ = FL³/(3EI), I = bh³/12 = 10·10³/12 ≈ 833.3 mm⁴
        // δ ≈ 100·512000/(3·69000·833.3) ≈ 0.297 mm.
        let a = analyze_box([0.0; 3], [80.0, 10.0, 10.0], &cantilever_spec(32)).unwrap();
        assert!(a.converged, "relres {}", a.relative_residual);
        assert!(
            a.max_displacement_mm > 0.15 && a.max_displacement_mm < 0.6,
            "tip deflection {} outside beam-theory ballpark",
            a.max_displacement_mm
        );
        // Max deflection is at the loaded tip.
        assert!(a.max_displacement_at[0] > 70.0);
        // Peak stress near the fixed root: σ = Mc/I ≈ 100·80·5/833 ≈ 48 MPa.
        assert!(
            a.max_von_mises_mpa > 15.0 && a.max_von_mises_mpa < 150.0,
            "root stress {} outside ballpark",
            a.max_von_mises_mpa
        );
        assert!(a.max_stress_at[0] < 20.0, "peak stress not near root");
        assert!(a.compliance_n_mm > 0.0);
    }

    #[test]
    fn coarse_predicts_fine_within_tolerance() {
        // The two-tier contract: the predict tier must land in the same
        // ballpark as the verify tier for a smooth problem. Below ~4
        // elements through the thinnest section trilinear hexes lock in
        // bending (res 20 on this beam reads 2.2× too stiff) — which is
        // why the predict tier default is 32, not lower.
        let coarse = analyze_box([0.0; 3], [80.0, 10.0, 10.0], &cantilever_spec(32)).unwrap();
        let fine = analyze_box([0.0; 3], [80.0, 10.0, 10.0], &cantilever_spec(64)).unwrap();
        let rel = (coarse.max_displacement_mm - fine.max_displacement_mm).abs()
            / fine.max_displacement_mm;
        assert!(
            rel < 0.35,
            "coarse {} vs fine {} — rel err {}",
            coarse.max_displacement_mm,
            fine.max_displacement_mm,
            rel
        );
    }

    #[test]
    fn stiffer_material_deflects_less() {
        let mut alu = cantilever_spec(16);
        let mut steel = cantilever_spec(16);
        steel.youngs_modulus_mpa = 200_000.0;
        alu.youngs_modulus_mpa = 69_000.0;
        let a = analyze_box([0.0; 3], [80.0, 10.0, 10.0], &alu).unwrap();
        let s = analyze_box([0.0; 3], [80.0, 10.0, 10.0], &steel).unwrap();
        let ratio = a.max_displacement_mm / s.max_displacement_mm;
        assert!(
            (ratio - 200.0 / 69.0).abs() < 0.05,
            "displacement should scale inversely with E; ratio {ratio}"
        );
        // Stress is E-independent for a displacement-driven-by-force problem.
        assert!((a.max_von_mises_mpa - s.max_von_mises_mpa).abs() < 1e-6);
    }

    #[test]
    fn invalid_specs_rejected() {
        let mut s = cantilever_spec(16);
        s.loads.clear();
        assert!(matches!(
            analyze_box([0.0; 3], [10.0; 3], &s),
            Err(AnalyzeError::InvalidSpec(_))
        ));
        let mut s = cantilever_spec(16);
        s.youngs_modulus_mpa = -1.0;
        assert!(matches!(
            analyze_box([0.0; 3], [10.0; 3], &s),
            Err(AnalyzeError::InvalidSpec(_))
        ));
    }
}
