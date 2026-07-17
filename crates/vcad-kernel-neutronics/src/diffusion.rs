//! The differentiable companion (M2): deterministic multigroup diffusion,
//! forward + adjoint, with dose gradients w.r.t. layer thicknesses.
//!
//! **The pairing, stated once and enforced everywhere:** Monte Carlo
//! ([`crate::transport`]) is the truth oracle — it carries the honest
//! error bars and the claims. The diffusion solver here is the **design
//! compass**: a smooth, deterministic model of the same multigroup
//! constants whose adjoint prices `d(dose)/d(thickness)` for every layer
//! in one extra solve. Design loops steer on the compass and verify on
//! the oracle; the compass's bias is *measured against the oracle in the
//! tests*, not assumed away.
//!
//! Known, quantified biases of the compass:
//! - **Void/air regions break diffusion.** Air's huge diffusion length
//!   flattens the flux across gaps, losing the local 1/4πr² falloff —
//!   absolute doses at an in-air detector are systematically high-biased
//!   by roughly (R_outer/r_detector)². The *log-gradient* w.r.t. shield
//!   thickness survives (it is dominated by shield attenuation, which
//!   diffusion tracks), and that is precisely the quantity the compass
//!   exists to provide. Verified against an MC finite difference in
//!   `tests/gradients.rs`.
//! - Diffusion theory itself degrades within a transport mean free path
//!   of strong absorbers and boundaries (textbook limitation).
//!
//! Discretization: cell-centered finite volume on a per-layer mesh,
//! harmonic-mean face diffusion coefficients, reflective inner boundary
//! (r = 0 / x = 0), Marshak vacuum outer boundary, downscatter-ordered
//! tridiagonal (Thomas) solves per group; the adjoint reuses the same
//! per-group operator (it is symmetric) with the group coupling
//! transposed, solved in reverse group order. Forward/adjoint duality
//! ⟨q†, φ⟩ = ⟨φ†, q⟩ is asserted to near machine precision in the tests
//! — the structural proof that the adjoint is *the* adjoint.
//!
//! Thickness-gradient semantics (chosen to match shield-design reality):
//! `d_dose_d_thickness_mm(i)` moves layer i's **outer** interface into
//! layer i+1 — the shield grows into the adjacent air gap; every other
//! boundary, the detector radius, and the outer wall stay fixed. The
//! derivative is the first-order perturbation-theory surface term
//! −A(r)·Σ_g [ΔD ∇φ·∇φ† + ΔΣ_r φφ† − ΣΔΣ_s φφ†′] evaluated with the
//! forward and adjoint fields at that interface, FD-validated.

use crate::dose::group_dose_factors_psv_cm2;
use crate::geometry::{Geometry, GeometryError};
use crate::groups::N_GROUPS;
use crate::materials::Material;

/// Mesh and solver options.
#[derive(Debug, Clone)]
pub struct DiffusionOptions {
    /// Target cell width, cm (default 0.25).
    pub target_dx_cm: f64,
    /// Minimum cells per layer (default 3).
    pub min_cells_per_layer: usize,
    /// Explicit per-layer cell counts (overrides the target width) —
    /// used by finite-difference validation so the mesh deforms smoothly
    /// with a perturbed thickness instead of re-gridding.
    pub cells_per_layer: Option<Vec<usize>>,
}

impl Default for DiffusionOptions {
    fn default() -> Self {
        DiffusionOptions {
            target_dx_cm: 0.25,
            min_cells_per_layer: 3,
            cells_per_layer: None,
        }
    }
}

/// Companion failures (fail-closed).
#[derive(Debug, Clone, PartialEq)]
pub enum DiffusionError {
    /// Geometry failed validation.
    Geometry(GeometryError),
    /// Detector radius outside the geometry.
    DetectorOutside {
        /// Requested detector position, mm.
        detector_mm: f64,
    },
    /// Gradient requested for the outermost layer — the growth semantics
    /// need a next layer to grow into.
    NoNextLayer(usize),
    /// `source_group` out of range.
    BadSourceGroup(usize),
}

impl std::fmt::Display for DiffusionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffusionError::Geometry(e) => write!(f, "invalid geometry: {e}"),
            DiffusionError::DetectorOutside { detector_mm } => {
                write!(f, "detector at {detector_mm} mm lies outside the geometry")
            }
            DiffusionError::NoNextLayer(i) => write!(
                f,
                "layer {i} is outermost: thickness gradients grow a layer into \
                 its neighbor, so the outermost layer has none"
            ),
            DiffusionError::BadSourceGroup(g) => write!(f, "source group {g} out of range"),
        }
    }
}

impl std::error::Error for DiffusionError {}

/// Assembled mesh + per-cell group constants.
pub struct DiffusionModel {
    kind: Kind,
    /// Cell faces, cm (len = cells + 1).
    faces: Vec<f64>,
    /// Cell centers, cm.
    centers: Vec<f64>,
    /// Cell volumes, cm³ (slab: per cm²).
    volumes: Vec<f64>,
    /// Layer index per cell.
    layer_of_cell: Vec<usize>,
    /// First cell index of each layer.
    first_cell_of_layer: Vec<usize>,
    /// Materials per layer (cloned from the geometry).
    materials: Vec<Material>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Slab,
    Sphere,
}

/// A per-cell, per-group scalar field (forward flux or adjoint
/// importance), plus the scalar response it prices.
pub struct Field {
    /// `values[cell][group]`.
    pub values: Vec<[f64; N_GROUPS]>,
}

/// Transport-corrected Σ_tr with a floor so voids stay well-posed
/// (documented bias — see module docs).
fn sigma_tr(m: &Material, g: usize) -> f64 {
    (m.sigma_t[g] - m.mu_bar[g] * m.sigma_s[g]).max(1.0e-6)
}

fn diff_coeff(m: &Material, g: usize) -> f64 {
    1.0 / (3.0 * sigma_tr(m, g))
}

/// Removal = total − in-group scattering.
fn sigma_removal(m: &Material, g: usize) -> f64 {
    m.sigma_t[g] - m.sigma_s[g] * m.transfer[g][g]
}

/// Scattering transfer Σ_s·P(g→g') for g′ ≠ g.
fn sigma_transfer(m: &Material, g_from: usize, g_to: usize) -> f64 {
    if g_from == g_to {
        0.0
    } else {
        m.sigma_s[g_from] * m.transfer[g_from][g_to]
    }
}

impl DiffusionModel {
    /// Build the mesh for a geometry.
    pub fn new(geometry: &Geometry, opts: &DiffusionOptions) -> Result<Self, DiffusionError> {
        geometry.validate().map_err(DiffusionError::Geometry)?;
        let kind = match geometry {
            Geometry::Slab(_) => Kind::Slab,
            Geometry::Sphere(_) => Kind::Sphere,
        };
        let layers = geometry.layers();
        let mut faces = vec![0.0f64];
        let mut layer_of_cell = Vec::new();
        let mut first_cell_of_layer = Vec::new();
        for (i, l) in layers.iter().enumerate() {
            let t_cm = l.thickness_mm * 0.1;
            let n = match &opts.cells_per_layer {
                Some(counts) => counts[i].max(1),
                None => ((t_cm / opts.target_dx_cm).ceil() as usize).max(opts.min_cells_per_layer),
            };
            first_cell_of_layer.push(layer_of_cell.len());
            let start = *faces.last().unwrap();
            for k in 1..=n {
                faces.push(start + t_cm * k as f64 / n as f64);
                layer_of_cell.push(i);
            }
        }
        let centers: Vec<f64> = faces.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect();
        let volumes: Vec<f64> = faces
            .windows(2)
            .map(|w| match kind {
                Kind::Slab => w[1] - w[0],
                Kind::Sphere => 4.0 / 3.0 * std::f64::consts::PI * (w[1].powi(3) - w[0].powi(3)),
            })
            .collect();
        Ok(DiffusionModel {
            kind,
            faces,
            centers,
            volumes,
            layer_of_cell,
            first_cell_of_layer,
            materials: layers.iter().map(|l| l.material.clone()).collect(),
        })
    }

    fn area(&self, r: f64) -> f64 {
        match self.kind {
            Kind::Slab => 1.0,
            Kind::Sphere => 4.0 * std::f64::consts::PI * r * r,
        }
    }

    fn mat(&self, cell: usize) -> &Material {
        &self.materials[self.layer_of_cell[cell]]
    }

    /// Center of a cell, cm — analytic comparisons must be evaluated
    /// here, not at the nominal probe radius (an 0.1 cm misalignment on
    /// an e^{−r/L}/r profile reads as a several-percent "error").
    pub fn cell_center_cm(&self, cell: usize) -> f64 {
        self.centers[cell]
    }

    /// Cell index containing a radius/depth given in mm.
    pub fn cell_at_mm(&self, position_mm: f64) -> Result<usize, DiffusionError> {
        let r_cm = position_mm * 0.1;
        if r_cm < 0.0 || r_cm > *self.faces.last().unwrap() {
            return Err(DiffusionError::DetectorOutside {
                detector_mm: position_mm,
            });
        }
        Ok(self
            .faces
            .windows(2)
            .position(|w| r_cm >= w[0] && r_cm <= w[1])
            .unwrap_or(self.centers.len() - 1))
    }

    /// Solve one group's tridiagonal system with an already-built
    /// volumetric source (per cm³).
    fn solve_group(&self, g: usize, source: &[f64]) -> Vec<f64> {
        let n = self.centers.len();
        let mut sub = vec![0.0f64; n];
        let mut diag = vec![0.0f64; n];
        let mut sup = vec![0.0f64; n];
        let mut rhs = vec![0.0f64; n];
        for j in 0..n {
            let m = self.mat(j);
            diag[j] += sigma_removal(m, g) * self.volumes[j];
            rhs[j] += source[j] * self.volumes[j];
            if j + 1 < n {
                // Conductance through the face between j and j+1:
                // harmonic mean of D over the two half-cells.
                let d_l = diff_coeff(m, g);
                let d_r = diff_coeff(self.mat(j + 1), g);
                let h_l = 0.5 * (self.faces[j + 1] - self.faces[j]);
                let h_r = 0.5 * (self.faces[j + 2] - self.faces[j + 1]);
                let cond = self.area(self.faces[j + 1]) / (h_l / d_l + h_r / d_r);
                diag[j] += cond;
                diag[j + 1] += cond;
                sup[j] -= cond;
                sub[j + 1] -= cond;
            } else {
                // Marshak vacuum boundary: outward current φ_cell /
                // (2 + h/D) — the series combination of the half-cell
                // diffusion resistance and the Marshak J = φ_face/2
                // condition.
                let d = diff_coeff(m, g);
                let h = 0.5 * (self.faces[j + 1] - self.faces[j]);
                diag[j] += self.area(self.faces[j + 1]) / (2.0 + h / d);
            }
        }
        // Thomas algorithm.
        let mut c_star = vec![0.0f64; n];
        let mut d_star = vec![0.0f64; n];
        c_star[0] = sup[0] / diag[0];
        d_star[0] = rhs[0] / diag[0];
        for j in 1..n {
            let m = diag[j] - sub[j] * c_star[j - 1];
            c_star[j] = sup[j] / m;
            d_star[j] = (rhs[j] - sub[j] * d_star[j - 1]) / m;
        }
        let mut x = vec![0.0f64; n];
        x[n - 1] = d_star[n - 1];
        for j in (0..n - 1).rev() {
            x[j] = d_star[j] - c_star[j] * x[j + 1];
        }
        x
    }

    /// Forward solve for a unit point source at the center in
    /// `source_group`. Returns φ per cell per group (per source neutron).
    pub fn forward(&self, source_group: usize) -> Result<Field, DiffusionError> {
        if source_group >= N_GROUPS {
            return Err(DiffusionError::BadSourceGroup(source_group));
        }
        let n = self.centers.len();
        let mut flux = vec![[0.0f64; N_GROUPS]; n];
        let mut source = vec![0.0f64; n];
        for g in 0..N_GROUPS {
            for s in source.iter_mut() {
                *s = 0.0;
            }
            if g == source_group {
                source[0] += 1.0 / self.volumes[0];
            }
            for (j, s) in source.iter_mut().enumerate() {
                let m = self.mat(j);
                for (gp, f) in flux[j].iter().enumerate().take(g) {
                    *s += sigma_transfer(m, gp, g) * f;
                }
            }
            let x = self.solve_group(g, &source);
            for (j, f) in flux.iter_mut().enumerate() {
                f[g] = x[j];
            }
        }
        Ok(Field { values: flux })
    }

    /// Adjoint solve for the dose response at `detector_cell`: the
    /// importance φ†(cell, g) = dose at the detector per neutron
    /// introduced in (cell, g). Solved in reverse group order with the
    /// scattering coupling transposed.
    pub fn adjoint_dose(&self, detector_cell: usize) -> Field {
        let h = group_dose_factors_psv_cm2();
        let n = self.centers.len();
        let mut adj = vec![[0.0f64; N_GROUPS]; n];
        let mut source = vec![0.0f64; n];
        for g in (0..N_GROUPS).rev() {
            for s in source.iter_mut() {
                *s = 0.0;
            }
            source[detector_cell] += h[g] / self.volumes[detector_cell];
            for (j, s) in source.iter_mut().enumerate() {
                let m = self.mat(j);
                for (gp, a) in adj[j].iter().enumerate().skip(g + 1) {
                    *s += sigma_transfer(m, g, gp) * a;
                }
            }
            let x = self.solve_group(g, &source);
            for (j, a) in adj.iter_mut().enumerate() {
                a[g] = x[j];
            }
        }
        Field { values: adj }
    }

    /// Dose response (pSv per source neutron) read from the forward
    /// field at a detector cell.
    pub fn dose_at(&self, forward: &Field, detector_cell: usize) -> f64 {
        let h = group_dose_factors_psv_cm2();
        (0..N_GROUPS)
            .map(|g| h[g] * forward.values[detector_cell][g])
            .sum()
    }

    /// Duality partner of [`Self::dose_at`]: the same response read from
    /// the adjoint field at the source. Equality of the two (to solver
    /// round-off) is the structural adjoint test.
    pub fn dose_via_adjoint(&self, adjoint: &Field, source_group: usize) -> f64 {
        adjoint.values[0][source_group]
    }

    /// d(dose)/d(thickness of layer `i`), pSv per source neutron per
    /// **mm**, under the growth semantics in the module docs (layer i
    /// grows into layer i+1; detector and outer wall fixed).
    pub fn d_dose_d_thickness_mm(
        &self,
        forward: &Field,
        adjoint: &Field,
        layer: usize,
    ) -> Result<f64, DiffusionError> {
        if layer + 1 >= self.materials.len() {
            return Err(DiffusionError::NoNextLayer(layer));
        }
        // Interface face between the last cell of `layer` and the first
        // cell of `layer + 1`.
        let jr = self.first_cell_of_layer[layer + 1];
        let jl = jr - 1;
        let r_f = self.faces[jr];
        let a = self.area(r_f);
        let m_l = &self.materials[layer];
        let m_r = &self.materials[layer + 1];
        let h_l = self.faces[jr] - self.centers[jl];
        let h_r = self.centers[jr] - self.faces[jr];
        // At a material interface ∇φ is discontinuous while φ and the
        // normal current J = −D∇φ are continuous. The diffusion part of
        // the surface term must therefore use the flux form
        // −[[1/D]]·J·J† (the 1D Hadamard interface shape-derivative:
        // δD·∇φ·∇φ† = −δ(1/D)·J·J† once the discontinuous gradient is
        // rewritten in the continuous current), and φ, φ† evaluated as
        // *interface* values (resistance-weighted, consistent with the
        // harmonic-mean face coupling). The naive δD·∇φ·∇φ† with a
        // straddling difference quotient is off by orders of magnitude
        // against a void-like neighbor (measured ×43 before this form;
        // the FD test pins both magnitude and sign now).
        let mut integrand = 0.0;
        for g in 0..N_GROUPS {
            let d_l = diff_coeff(m_l, g);
            let d_r = diff_coeff(m_r, g);
            let resist = h_l / d_l + h_r / d_r;
            let face = |vl: f64, vr: f64| -> (f64, f64) {
                // (interface value, current density toward +r)
                let j = -(vr - vl) / resist;
                let v = (vl * d_l / h_l + vr * d_r / h_r) / (d_l / h_l + d_r / h_r);
                (v, j)
            };
            let (phi, j_fwd) = face(forward.values[jl][g], forward.values[jr][g]);
            let (adj_g, j_adj) = face(adjoint.values[jl][g], adjoint.values[jr][g]);
            let dinv = 1.0 / d_l - 1.0 / d_r;
            let dsr = sigma_removal(m_l, g) - sigma_removal(m_r, g);
            integrand += -dinv * j_fwd * j_adj + dsr * phi * adj_g;
            for gp in 0..N_GROUPS {
                if gp == g {
                    continue;
                }
                let dst = sigma_transfer(m_l, g, gp) - sigma_transfer(m_r, g, gp);
                let d_l_p = diff_coeff(m_l, gp);
                let d_r_p = diff_coeff(m_r, gp);
                let adj_gp = (adjoint.values[jl][gp] * d_l_p / h_l
                    + adjoint.values[jr][gp] * d_r_p / h_r)
                    / (d_l_p / h_l + d_r_p / h_r);
                integrand -= dst * phi * adj_gp;
            }
        }
        // δR = −A·integrand per cm of growth; report per mm.
        Ok(-a * integrand * 0.1)
    }
}

/// One-call companion report for a spherical shield: dose at a detector
/// radius plus the thickness gradient of every layer that has a
/// neighbor to grow into (`None` for the outermost layer).
pub struct CompanionReport {
    /// Diffusion dose at the detector, pSv per source neutron. Carries
    /// the void bias documented in the module docs — the MC oracle owns
    /// absolute doses.
    pub dose_psv_per_source: f64,
    /// d(dose)/d(thickness), pSv per source neutron per mm, per layer.
    pub d_dose_d_thickness_mm: Vec<Option<f64>>,
    /// Forward/adjoint duality gap |R_fwd − R_adj| / R_fwd — should be
    /// at round-off; reported so a broken adjoint cannot hide.
    pub duality_gap: f64,
    /// Cells in the mesh.
    pub cells: usize,
}

/// Run the full companion: forward + adjoint + gradients.
pub fn companion_report(
    geometry: &Geometry,
    source_group: usize,
    detector_mm: f64,
    opts: &DiffusionOptions,
) -> Result<CompanionReport, DiffusionError> {
    let model = DiffusionModel::new(geometry, opts)?;
    let det = model.cell_at_mm(detector_mm)?;
    let fwd = model.forward(source_group)?;
    let adj = model.adjoint_dose(det);
    let r_fwd = model.dose_at(&fwd, det);
    let r_adj = model.dose_via_adjoint(&adj, source_group);
    let duality_gap = if r_fwd != 0.0 {
        ((r_fwd - r_adj) / r_fwd).abs()
    } else {
        f64::INFINITY
    };
    let grads = (0..geometry.region_count())
        .map(|i| model.d_dose_d_thickness_mm(&fwd, &adj, i).ok())
        .collect();
    Ok(CompanionReport {
        dose_psv_per_source: r_fwd,
        d_dose_d_thickness_mm: grads,
        duality_gap,
        cells: model.centers.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Layer;
    use crate::materials::Material;

    #[test]
    fn one_group_point_source_matches_diffusion_theory() {
        // Σs = 0.8, Σa = 0.05 (isotropic fiction): D = 1/(3·0.85) =
        // 0.392 cm, L = √(D/Σa) = 2.80 cm. Analytic infinite-medium
        // point-source diffusion: φ = e^{−r/L}/(4πDr). Compare at
        // r = 8–12 cm (≈3–4 L deep, ≥5 L from the vacuum boundary).
        let m = Material::one_group(0.8, 0.05);
        let g = Geometry::Sphere(vec![Layer::new(m, 250.0)]);
        let model = DiffusionModel::new(&g, &DiffusionOptions::default()).unwrap();
        let fwd = model.forward(0).unwrap();
        let d = 1.0 / (3.0 * 0.85);
        let l = (d / 0.05f64).sqrt();
        for r in [8.0f64, 10.0, 12.0] {
            let cell = model.cell_at_mm(r * 10.0).unwrap();
            // Compare at the actual cell center, not the nominal probe
            // radius — see `cell_center_cm`.
            let rc = model.cell_center_cm(cell);
            let analytic = (-rc / l).exp() / (4.0 * std::f64::consts::PI * d * rc);
            let ratio = fwd.values[cell][0] / analytic;
            assert!(
                (ratio - 1.0).abs() < 0.03,
                "diffusion vs analytic at r={rc}: ratio {ratio}"
            );
        }
    }

    #[test]
    fn forward_adjoint_duality_is_machine_tight() {
        let g = Geometry::Sphere(vec![
            Layer::new(crate::materials::air(), 300.0),
            Layer::new(crate::materials::hdpe(), 100.0),
            Layer::new(crate::materials::air(), 600.0),
        ]);
        let rep = companion_report(&g, 0, 800.0, &DiffusionOptions::default()).unwrap();
        assert!(
            rep.duality_gap < 1.0e-10,
            "duality gap {} — the adjoint is not the adjoint",
            rep.duality_gap
        );
    }

    #[test]
    fn thickness_gradient_matches_finite_differences() {
        // Grow the HDPE layer into the air gap; central FD on the
        // re-solved model with a mesh that deforms smoothly.
        let cells = vec![30, 40, 60];
        let opts = DiffusionOptions {
            cells_per_layer: Some(cells.clone()),
            ..DiffusionOptions::default()
        };
        let build = |t_shield: f64| {
            Geometry::Sphere(vec![
                Layer::new(crate::materials::air(), 300.0),
                Layer::new(crate::materials::hdpe(), t_shield),
                Layer::new(crate::materials::air(), 900.0 - t_shield),
            ])
        };
        let det_mm = 850.0;
        let base = companion_report(&build(100.0), 0, det_mm, &opts).unwrap();
        let grad = base.d_dose_d_thickness_mm[1].expect("shield has a neighbor");
        let h = 4.0;
        let up = companion_report(&build(100.0 + h), 0, det_mm, &opts).unwrap();
        let dn = companion_report(&build(100.0 - h), 0, det_mm, &opts).unwrap();
        let fd = (up.dose_psv_per_source - dn.dose_psv_per_source) / (2.0 * h);
        assert!(
            grad < 0.0,
            "thickening the shield must reduce dose (grad {grad})"
        );
        assert!(
            (grad / fd - 1.0).abs() < 0.05,
            "adjoint {grad} vs FD {fd}: ratio {}",
            grad / fd
        );
    }

    #[test]
    fn outermost_layer_gradient_fails_closed() {
        let g = Geometry::Sphere(vec![
            Layer::new(crate::materials::air(), 300.0),
            Layer::new(crate::materials::hdpe(), 100.0),
        ]);
        let model = DiffusionModel::new(&g, &DiffusionOptions::default()).unwrap();
        let fwd = model.forward(0).unwrap();
        let adj = model.adjoint_dose(0);
        assert_eq!(
            model.d_dose_d_thickness_mm(&fwd, &adj, 1).unwrap_err(),
            DiffusionError::NoNextLayer(1)
        );
    }
}
