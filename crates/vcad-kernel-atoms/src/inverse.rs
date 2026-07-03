//! Inverse design — M4.
//!
//! State a target property; descend a design parameter to it. A
//! [`DesignProblem`] couples three pieces: a `build` map from design parameters
//! θ to a concrete [`MoleculeSystem`] (this is where the parametric-CAD DAG
//! plugs in), a `property` measurement on the resulting structure, and a
//! `target` value. The objective is `0.5 (property(θ) − target)²`.
//!
//! Gradients here are computed by central differences over θ. This is the exact
//! seam where reverse-mode autodiff (`tang-ad`) drops in for cheap gradients
//! through the whole simulation — the optimizer and objective are written to be
//! agnostic to how `grad` is produced. The finite-difference path is also the
//! oracle the analytic path is checked against.

use vcad_ir::molecule::MoleculeSystem;

/// Maps design parameters θ to a concrete structure (where the parametric-CAD
/// DAG plugs in).
pub type BuildFn<'a> = Box<dyn Fn(&[f64]) -> MoleculeSystem + 'a>;

/// Measures a scalar property on a structure.
pub type PropertyFn<'a> = Box<dyn Fn(&MoleculeSystem) -> f64 + 'a>;

/// A property whose value we want to drive to a target by varying design
/// parameters.
pub struct DesignProblem<'a> {
    /// Map design parameters θ → a structure.
    pub build: BuildFn<'a>,
    /// Measure the property of interest on a structure.
    pub property: PropertyFn<'a>,
    /// Target property value.
    pub target: f64,
}

impl DesignProblem<'_> {
    /// Property value at θ.
    pub fn measure(&self, theta: &[f64]) -> f64 {
        (self.property)(&(self.build)(theta))
    }

    /// Objective `0.5 (property − target)²` at θ.
    pub fn objective(&self, theta: &[f64]) -> f64 {
        let p = self.measure(theta);
        0.5 * (p - self.target).powi(2)
    }

    /// Central-difference gradient of the objective wrt θ.
    pub fn grad_fd(&self, theta: &[f64], h: f64) -> Vec<f64> {
        let mut g = vec![0.0; theta.len()];
        let mut probe = theta.to_vec();
        for k in 0..theta.len() {
            let orig = theta[k];
            probe[k] = orig + h;
            let fp = self.objective(&probe);
            probe[k] = orig - h;
            let fm = self.objective(&probe);
            probe[k] = orig;
            g[k] = (fp - fm) / (2.0 * h);
        }
        g
    }
}

/// Options for the inverse-design optimizer.
#[derive(Debug, Clone)]
pub struct InverseOptions {
    /// Maximum iterations.
    pub max_iters: usize,
    /// Objective convergence tolerance.
    pub obj_tol: f64,
    /// Finite-difference step for gradients.
    pub fd_step: f64,
    /// Initial step size for the line search.
    pub step0: f64,
    /// Optional per-parameter bounds `[lo, hi]`.
    pub bounds: Option<Vec<[f64; 2]>>,
}

impl Default for InverseOptions {
    fn default() -> Self {
        Self {
            max_iters: 200,
            obj_tol: 1e-10,
            fd_step: 1e-4,
            step0: 1.0,
            bounds: None,
        }
    }
}

/// Result of an inverse-design run.
#[derive(Debug, Clone)]
pub struct InverseResult {
    /// Final design parameters.
    pub theta: Vec<f64>,
    /// Final objective value.
    pub objective: f64,
    /// Final measured property.
    pub property: f64,
    /// Iterations performed.
    pub iters: usize,
    /// Whether the objective tolerance was reached.
    pub converged: bool,
}

fn clamp_to_bounds(theta: &mut [f64], bounds: &Option<Vec<[f64; 2]>>) {
    if let Some(b) = bounds {
        for (t, bnd) in theta.iter_mut().zip(b.iter()) {
            *t = t.clamp(bnd[0], bnd[1]);
        }
    }
}

/// Gradient-descent inverse design with backtracking line search and bounds.
pub fn optimize(problem: &DesignProblem, theta0: &[f64], opts: &InverseOptions) -> InverseResult {
    let mut theta = theta0.to_vec();
    clamp_to_bounds(&mut theta, &opts.bounds);
    let mut obj = problem.objective(&theta);

    let mut iters = 0;
    for it in 1..=opts.max_iters {
        iters = it;
        if obj <= opts.obj_tol {
            break;
        }
        let grad = problem.grad_fd(&theta, opts.fd_step);
        let gnorm2: f64 = grad.iter().map(|g| g * g).sum();
        if gnorm2 < 1e-30 {
            break;
        }
        // Backtracking line search along -grad.
        let mut step = opts.step0;
        let mut improved = false;
        for _ in 0..40 {
            let mut trial = theta.clone();
            for k in 0..trial.len() {
                trial[k] -= step * grad[k];
            }
            clamp_to_bounds(&mut trial, &opts.bounds);
            let trial_obj = problem.objective(&trial);
            // Armijo-lite: accept any decrease.
            if trial_obj < obj {
                theta = trial;
                obj = trial_obj;
                improved = true;
                break;
            }
            step *= 0.5;
        }
        if !improved {
            break;
        }
    }

    InverseResult {
        property: problem.measure(&theta),
        objective: obj,
        converged: obj <= opts.obj_tol,
        iters,
        theta,
    }
}
