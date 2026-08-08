//! The coupled discrete adjoint of the flow ⇄ thermal conjugate loop.
//!
//! This is SU2's conjugate-heat-transfer case, in vcad's kernel. The
//! motivating number, from their validation slide: differentiating a CHT
//! heat flux with the cross terms dropped gives **39.4%** error, and in
//! their three-physics case dropping all coupling returns the **wrong
//! sign**. vcad had both single-physics adjoints (thermal's one-extra-
//! solve `smooth_max_gradient`, flow's Brinkman fixed-point adjoint) and
//! a conjugate loop joining them with no gradient at all — which is
//! precisely the configuration where somebody chains the two and ships
//! the 39%.
//!
//! # The structure that makes this cheap
//!
//! Read [`crate::conjugate::solve_conjugate`] and one fact stands out:
//! the thermal model's exposed boundary is the *only* place the flow
//! solution reaches the solid, and it takes exactly two numbers — a film
//! coefficient and a bulk temperature. Write `s = (h, T_bulk)`. Then the
//! conjugate loop factors:
//!
//! ```text
//! W : s ↦ T_wall        (conduct through the solid — one thermal solve)
//! Φ : T_wall ↦ s        (solve the flow, price the film)
//! Ψ = Φ ∘ W : s ↦ s     (one full outer iteration)
//! ```
//!
//! The wall-temperature field is high-dimensional, but the loop **factors
//! through R²**. So the coupled fixed point is two-dimensional, and the
//! coupled adjoint is a 2×2 solve rather than a field-sized one:
//!
//! ```text
//! J(θ) = Ĵ(θ, s*(θ))          with  s* = Ψ(θ, s*)
//! g    = ∂Ĵ/∂s                 (exact — the thermal adjoint)
//! A    = ∂Ψ/∂s                 (2×2 — four Ψ evaluations)
//! (I − A)ᵀ μ = g               (2×2 solve)
//! dJ/dθ = ∂Ĵ/∂θ + μᵀ ∂Ψ/∂θ
//! ```
//!
//! `A` is exactly the product of SU2's two cross terms: `∂Φ/∂T_wall`
//! (the flow's response to the solid, block `flow → thermal`) times
//! `∂W/∂s` (the solid's response to the film, block `thermal → flow`).
//! Delete either and `A = 0`; the ablation gates below do exactly that
//! and watch the error explode.
//!
//! # The demonstration parameter
//!
//! Inlet velocity is the one to watch. It reaches the hotspot temperature
//! **only** through the coupling — the thermal solver has never heard of
//! it. So the uncoupled adjoint reports `dJ/dv = 0` exactly: *"how fast
//! you blow air over the heatsink does not affect how hot it gets."* Not
//! 39% off. Infinitely off, with total confidence. That is what a missing
//! cross term buys you, and it is why [`Coupling::None`] produces an
//! `Incomplete` ledger whose sensitivity rows come back
//! [`vcad_receipt::ClaimVerdict::Unverifiable`] rather than merely
//! inaccurate.
//!
//! # Honesty
//!
//! - `g` and `∂Ĵ/∂θ` are **exact** (the thermal adjoint, one extra PCG
//!   solve, validated against finite differences in that crate).
//! - `A` and `∂Ψ/∂θ` are **central finite differences** on the composed
//!   half-step. Legitimate here because the block is 2×2 — the probe
//!   count is fixed at four regardless of grid size — but it is not
//!   machine-exact, so the ledger records `FiniteDifference` and the
//!   completeness rolls up as `Predicted`, never `Verified`. Making these
//!   analytic needs a thermal-lattice adjoint on the LBM side; that is a
//!   later milestone, not a hole, and the block is *implemented* either
//!   way.
//! - The coupling itself is film-averaged (one `h`, one `T_bulk` for the
//!   whole wetted surface), inherited from the primal loop. The gradient
//!   is the exact gradient *of that model*.
//! - Every parameter here is continuous and moves no voxel mask.
//!   Geometry parameters would, and their gradients are not available on
//!   this path — see [`vcad_kernel_adjoint::TrustRadius::from_grid`] for
//!   why a mask-moving finite difference reads a confident zero.

use vcad_kernel_adjoint::{
    ablation, AblationReport, BlockMethod, BlockStatus, CouplingLedger, Route, Sensitivity,
    SensitivityTable, TrustLimit, TrustRadius,
};
use vcad_kernel_thermal::adjoint::{smooth_max_gradient, ObjectiveOptions};
use vcad_kernel_thermal::model::{Boundary, ThermalModel};
use vcad_kernel_thermal::solve as thermal_solve;

use crate::conjugate::{
    check_pair, conduct, price_film, solve_conjugate, wetted_surface, ConjugateError,
    ConjugateOptions, FilmState, Wetted,
};
use crate::model::FlowModel;

/// Which coupling terms to include. The non-`Full` variants exist for
/// ablation: they are how a test proves a cross term is load-bearing,
/// and they are never what a caller wants for a real gradient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coupling {
    /// Every term. The gradient.
    Full,
    /// Keep the direct parameter→interface path, drop the interface's
    /// feedback on itself (`A := 0`, so `μ = g`). This is what "we
    /// linearized the coupling once" gets you.
    NoFeedback,
    /// Drop the coupling entirely (`μ := 0`). Each discipline's adjoint
    /// on its own. This is the configuration that returns zero for the
    /// inlet velocity.
    None,
}

impl Coupling {
    fn label(self) -> &'static str {
        match self {
            Coupling::Full => "full",
            Coupling::NoFeedback => "no-feedback",
            Coupling::None => "uncoupled",
        }
    }
}

/// A design parameter of the coupled problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConjugateParameter {
    /// Power of a thermal source, W.
    SourcePower(usize),
    /// One axis of one material region's conductivity, W/(m·K).
    Conductivity {
        /// Index into `ThermalModel::materials`.
        region: usize,
        /// Axis 0..3.
        axis: usize,
    },
    /// One axis of the flow inlet velocity, m/s. Reaches the objective
    /// **only** through the coupling.
    InletVelocity(usize),
}

impl ConjugateParameter {
    /// Stable name for reports.
    pub fn name(&self) -> String {
        match self {
            ConjugateParameter::SourcePower(i) => format!("source_power[{i}]"),
            ConjugateParameter::Conductivity { region, axis } => {
                format!("conductivity[{region}].{}", ["x", "y", "z"][*axis])
            }
            ConjugateParameter::InletVelocity(a) => {
                format!("inlet_velocity.{}", ["x", "y", "z"][*a])
            }
        }
    }

    /// Unit of `dJ/dθ` for a temperature objective.
    pub fn gradient_unit(&self) -> &'static str {
        match self {
            ConjugateParameter::SourcePower(_) => "K/W",
            ConjugateParameter::Conductivity { .. } => "K·m·K/W",
            ConjugateParameter::InletVelocity(_) => "K/(m/s)",
        }
    }

    /// Whether the thermal solver can see this parameter at all.
    fn is_thermal(&self) -> bool {
        !matches!(self, ConjugateParameter::InletVelocity(_))
    }

    /// Current value.
    fn value(&self, fm: &FlowModel, tm: &ThermalModel) -> Result<f64, ConjugateAdjointError> {
        match *self {
            ConjugateParameter::SourcePower(i) => tm
                .sources
                .get(i)
                .map(|s| s.power_w)
                .ok_or(ConjugateAdjointError::UnknownParameter(self.name())),
            ConjugateParameter::Conductivity { region, axis } => tm
                .materials
                .get(region)
                .and_then(|m| m.k_w_mk.get(axis).copied())
                .ok_or(ConjugateAdjointError::UnknownParameter(self.name())),
            ConjugateParameter::InletVelocity(a) => fm
                .inlet_velocity_m_s
                .get(a)
                .copied()
                .ok_or(ConjugateAdjointError::UnknownParameter(self.name())),
        }
    }

    /// Write a new value into a model pair.
    fn set(&self, fm: &mut FlowModel, tm: &mut ThermalModel, v: f64) {
        match *self {
            ConjugateParameter::SourcePower(i) => tm.sources[i].power_w = v,
            ConjugateParameter::Conductivity { region, axis } => {
                tm.materials[region].k_w_mk[axis] = v
            }
            ConjugateParameter::InletVelocity(a) => fm.inlet_velocity_m_s[a] = v,
        }
    }
}

/// A parameter plus what the author knows about its range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterSpec {
    /// The parameter.
    pub parameter: ConjugateParameter,
    /// Declared scrub range, if any. Combines with the measured curvature
    /// to give the reported trust radius.
    pub bounds: Option<(f64, f64)>,
}

impl ParameterSpec {
    /// A spec with no declared bounds.
    pub fn new(parameter: ConjugateParameter) -> Self {
        ParameterSpec {
            parameter,
            bounds: None,
        }
    }

    /// A spec with a declared scrub range.
    pub fn bounded(parameter: ConjugateParameter, min: f64, max: f64) -> Self {
        ParameterSpec {
            parameter,
            bounds: Some((min, max)),
        }
    }
}

/// Options for the coupled gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConjugateAdjointOptions {
    /// Primal loop options. The flow tolerance matters more here than in
    /// a plain solve: finite differences on the interface block are only
    /// meaningful when solver stopping noise sits well below the signal,
    /// so [`Self::tightened`] exists to make that explicit.
    pub conjugate: ConjugateOptions,
    /// Thermal objective (smoothed peak temperature) options.
    pub objective: ObjectiveOptions,
    /// Relative step for the 2×2 interface Jacobian.
    pub interface_step_rel: f64,
    /// Relative step for each parameter's `∂Ψ/∂θ`.
    pub parameter_step_rel: f64,
    /// Relative step for the curvature probe behind the linearity trust
    /// radius. Deliberately larger — a second difference at the
    /// first-derivative step size is all noise.
    pub curvature_step_rel: f64,
    /// Relative departure the linearity trust radius allows before it
    /// calls the linear model unusable.
    pub linearity_tol: f64,
    /// Which terms to include. Anything but [`Coupling::Full`] is an
    /// ablation.
    pub coupling: Coupling,
}

impl Default for ConjugateAdjointOptions {
    fn default() -> Self {
        ConjugateAdjointOptions {
            conjugate: ConjugateOptions::default(),
            objective: ObjectiveOptions::default(),
            interface_step_rel: 1e-3,
            parameter_step_rel: 1e-3,
            curvature_step_rel: 5e-2,
            linearity_tol: 0.1,
            coupling: Coupling::Full,
        }
    }
}

impl ConjugateAdjointOptions {
    /// Defaults with the primal tolerances tightened for differentiation.
    ///
    /// The lesson the thermal crate already learned and wrote down: a
    /// finite difference is only as good as the noise floor under it.
    /// The flow steadiness tolerance and the thermal CG tolerance both
    /// move well below the FD signal here.
    pub fn tightened() -> Self {
        let mut o = ConjugateAdjointOptions::default();
        o.conjugate.flow.steady_tol = 1e-9;
        o.conjugate.thermal_tol = 1e-12;
        o.conjugate.wall_tol_c = 1e-4;
        o.conjugate.max_outer = 60;
        o
    }
}

/// Why a coupled gradient could not be produced.
#[derive(Debug)]
pub enum ConjugateAdjointError {
    /// The primal loop or a probe solve failed.
    Conjugate(ConjugateError),
    /// The thermal adjoint failed.
    Thermal(thermal_solve::SolveError),
    /// A parameter does not exist in these models.
    UnknownParameter(String),
    /// The thermal model's exposed boundary is not the conjugate film —
    /// the objective's dependence on the interface cannot be read.
    ExposedNotConvective,
    /// `I − A` is singular: the coupled fixed point is neutrally stable
    /// and the sensitivity is unbounded. The primal loop's convergence is
    /// the same condition, so this means the *design* is marginal, not
    /// that the code is.
    SingularInterface {
        /// Determinant of `I − A`.
        det: f64,
        /// Spectral radius of `A`.
        spectral_radius: f64,
    },
}

impl std::fmt::Display for ConjugateAdjointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConjugateAdjointError::Conjugate(e) => write!(f, "conjugate solve: {e}"),
            ConjugateAdjointError::Thermal(e) => write!(f, "thermal adjoint: {e}"),
            ConjugateAdjointError::UnknownParameter(n) => write!(f, "unknown parameter {n}"),
            ConjugateAdjointError::ExposedNotConvective => write!(
                f,
                "the thermal model's exposed boundary must be the conjugate film"
            ),
            ConjugateAdjointError::SingularInterface {
                det,
                spectral_radius,
            } => write!(
                f,
                "interface fixed point is marginal: det(I - A) = {det:.3e}, \
                 spectral radius {spectral_radius:.6} — the coupled sensitivity is unbounded"
            ),
        }
    }
}

impl std::error::Error for ConjugateAdjointError {}

impl From<ConjugateError> for ConjugateAdjointError {
    fn from(e: ConjugateError) -> Self {
        ConjugateAdjointError::Conjugate(e)
    }
}

/// A coupled gradient, with everything needed to audit it.
#[derive(Debug, Clone)]
pub struct ConjugateGradient {
    /// Smoothed peak solid temperature, °C — the objective.
    pub objective_c: f64,
    /// Hard max, °C — the lower edge of the smoothing bracket.
    pub hard_max_c: f64,
    /// The converged interface state.
    pub film: FilmState,
    /// `∂Ĵ/∂s = (∂J/∂h, ∂J/∂T_bulk)` — exact, from the thermal adjoint.
    pub d_objective_d_film: [f64; 2],
    /// `A = ∂Ψ/∂s`, the composed interface Jacobian.
    pub interface_jacobian: [[f64; 2]; 2],
    /// `μ`, the interface adjoint.
    pub interface_adjoint: [f64; 2],
    /// Spectral radius of `A` — the coupling strength. Near zero means
    /// the disciplines barely see each other and the cross terms are a
    /// correction; near one means the coupling *is* the problem and an
    /// uncoupled gradient is worthless.
    pub coupling_strength: f64,
    /// One row per parameter.
    pub table: SensitivityTable,
    /// The ledger this was computed under.
    pub ledger: CouplingLedger,
    /// Ψ evaluations spent (each is one thermal solve plus one flow
    /// solve).
    pub psi_evaluations: usize,
    /// Outer iterations the primal loop took.
    pub outer_iters: usize,
}

impl ConjugateGradient {
    /// `dJ/dθ` for a parameter, by name.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.table
            .rows
            .iter()
            .find(|r| r.parameter == name)
            .map(|r| r.value)
    }
}

/// Build the ledger describing a given coupling mode.
fn build_ledger(coupling: Coupling) -> CouplingLedger {
    let mut l = CouplingLedger::new(["flow", "thermal"], BlockMethod::FiniteDifference)
        .expect("two distinct disciplines");
    // The thermal discipline differentiates itself exactly.
    l.set(1, 1, BlockStatus::implemented(BlockMethod::Analytic))
        .expect("diagonal");
    match coupling {
        Coupling::Full => {
            // dG(flow)/du(thermal): the film's response to wall
            // temperature. dG(thermal)/du(flow): the wall's response to
            // the film. Both live inside the composed 2x2 A, and both are
            // finite-differenced.
            l.set(
                0,
                1,
                BlockStatus::implemented(BlockMethod::FiniteDifference),
            )
            .expect("cross");
            l.set(
                1,
                0,
                BlockStatus::implemented(BlockMethod::FiniteDifference),
            )
            .expect("cross");
        }
        Coupling::NoFeedback => {
            l.set(
                0,
                1,
                BlockStatus::frozen(
                    "the film coefficient does not respond to the wall temperature it induces",
                ),
            )
            .expect("cross");
            l.set(
                1,
                0,
                BlockStatus::frozen("the wall temperature does not respond to the film it induces"),
            )
            .expect("cross");
        }
        Coupling::None => {
            l.set(
                0,
                1,
                BlockStatus::missing("no flow response to solid temperature"),
            )
            .expect("cross");
            l.set(
                1,
                0,
                BlockStatus::missing("no solid response to the film state"),
            )
            .expect("cross");
        }
    }
    l
}

/// Spectral radius of a 2×2.
fn spectral_radius(a: [[f64; 2]; 2]) -> f64 {
    let tr = a[0][0] + a[1][1];
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let disc = tr * tr - 4.0 * det;
    if disc >= 0.0 {
        let r = disc.sqrt();
        (0.5 * (tr + r)).abs().max((0.5 * (tr - r)).abs())
    } else {
        det.abs().sqrt()
    }
}

/// One Ψ evaluation: conduct with the film state, then re-price the film
/// from the resulting wall temperatures.
fn psi(
    fm: &FlowModel,
    tm: &ThermalModel,
    wetted: &Wetted,
    s: FilmState,
    opts: &ConjugateAdjointOptions,
) -> Result<FilmState, ConjugateAdjointError> {
    let t_opts = thermal_solve::SolveOptions {
        tol: opts.conjugate.thermal_tol,
        max_iters: opts.conjugate.thermal_max_iters,
    };
    let sol = conduct(tm, s, &t_opts)?;
    let (_, film) = price_film(fm, wetted, &sol.t_c, &opts.conjugate.flow, false)?;
    Ok(film)
}

/// The objective at a given film state, holding the models fixed.
fn objective_at(
    tm: &ThermalModel,
    s: FilmState,
    opts: &ConjugateAdjointOptions,
) -> Result<f64, ConjugateAdjointError> {
    let mut m = tm.clone();
    m.exposed = Boundary::Convection {
        h_w_m2k: s.h_w_m2k,
        ambient_c: s.t_bulk_c,
    };
    let t_opts = thermal_solve::SolveOptions {
        tol: opts.conjugate.thermal_tol,
        max_iters: opts.conjugate.thermal_max_iters,
    };
    let (_, g) = smooth_max_gradient(&m, &t_opts, &opts.objective)
        .map_err(ConjugateAdjointError::Thermal)?;
    Ok(g.value_c)
}

/// Compute the coupled gradient of the smoothed peak solid temperature
/// with respect to each supplied parameter.
pub fn conjugate_gradient(
    flow_model: &FlowModel,
    thermal_model: &ThermalModel,
    specs: &[ParameterSpec],
    opts: &ConjugateAdjointOptions,
) -> Result<ConjugateGradient, ConjugateAdjointError> {
    check_pair(flow_model, thermal_model)?;
    let wetted = wetted_surface(flow_model)?;

    // 1. Converge the primal loop. s* is the interface fixed point.
    let primal = solve_conjugate(flow_model, thermal_model, &opts.conjugate)?;
    let s_star = FilmState {
        h_w_m2k: primal.film_h_w_m2k,
        t_bulk_c: primal.t_bulk_c,
    };
    let mut psi_evaluations = 0usize;

    // 2. One thermal adjoint at the converged film: exact ∂Ĵ/∂θ for
    //    every thermal parameter at once, plus g = ∂Ĵ/∂s.
    let mut tm_star = thermal_model.clone();
    tm_star.exposed = Boundary::Convection {
        h_w_m2k: s_star.h_w_m2k,
        ambient_c: s_star.t_bulk_c,
    };
    let t_opts = thermal_solve::SolveOptions {
        tol: opts.conjugate.thermal_tol,
        max_iters: opts.conjugate.thermal_max_iters,
    };
    let (_, jgrad) = smooth_max_gradient(&tm_star, &t_opts, &opts.objective)
        .map_err(ConjugateAdjointError::Thermal)?;
    const EXPOSED: usize = 6;
    let g = [
        jgrad.d_film[EXPOSED].ok_or(ConjugateAdjointError::ExposedNotConvective)?,
        jgrad.d_ambient[EXPOSED].ok_or(ConjugateAdjointError::ExposedNotConvective)?,
    ];

    // 3. A = ∂Ψ/∂s by central differences — four Ψ evaluations, and four
    //    is the whole cost no matter how large the grid is.
    let mut a = [[0.0_f64; 2]; 2];
    if opts.coupling == Coupling::Full {
        let base = [s_star.h_w_m2k, s_star.t_bulk_c];
        for j in 0..2 {
            let step = (opts.interface_step_rel * base[j].abs()).max(1e-9);
            let mut up = base;
            let mut dn = base;
            up[j] += step;
            dn[j] -= step;
            let fu = psi(
                flow_model,
                thermal_model,
                &wetted,
                FilmState {
                    h_w_m2k: up[0],
                    t_bulk_c: up[1],
                },
                opts,
            )?;
            let fd = psi(
                flow_model,
                thermal_model,
                &wetted,
                FilmState {
                    h_w_m2k: dn[0],
                    t_bulk_c: dn[1],
                },
                opts,
            )?;
            psi_evaluations += 2;
            a[0][j] = (fu.h_w_m2k - fd.h_w_m2k) / (2.0 * step);
            a[1][j] = (fu.t_bulk_c - fd.t_bulk_c) / (2.0 * step);
        }
    }
    let rho = spectral_radius(a);

    // 4. (I − A)ᵀ μ = g.
    let mu = match opts.coupling {
        Coupling::None => [0.0, 0.0],
        Coupling::NoFeedback => g,
        Coupling::Full => {
            let (p, q) = (1.0 - a[0][0], 1.0 - a[1][1]);
            let det = p * q - a[0][1] * a[1][0];
            if !det.is_finite() || det.abs() < 1e-12 {
                return Err(ConjugateAdjointError::SingularInterface {
                    det,
                    spectral_radius: rho,
                });
            }
            [
                (q * g[0] + a[1][0] * g[1]) / det,
                (a[0][1] * g[0] + p * g[1]) / det,
            ]
        }
    };

    // 5. Per parameter: the direct term (exact, free) plus the coupling
    //    correction μᵀ ∂Ψ/∂θ (two Ψ evaluations).
    let ledger = build_ledger(opts.coupling);
    let completeness = ledger.completeness();
    let mut table = SensitivityTable::new();

    for spec in specs {
        let p = spec.parameter;
        let theta = p.value(flow_model, thermal_model)?;

        // Direct: ∂Ĵ/∂θ at fixed interface state. Zero for anything the
        // thermal solver cannot see — which is the whole story for inlet
        // velocity.
        let direct = match p {
            ConjugateParameter::SourcePower(i) => *jgrad
                .d_source_power
                .get(i)
                .ok_or_else(|| ConjugateAdjointError::UnknownParameter(p.name()))?,
            ConjugateParameter::Conductivity { region, axis } => *jgrad
                .d_conductivity
                .get(region)
                .and_then(|r| r.get(axis))
                .ok_or_else(|| ConjugateAdjointError::UnknownParameter(p.name()))?,
            ConjugateParameter::InletVelocity(_) => 0.0,
        };

        // Coupling correction.
        let mut coupled = 0.0;
        if opts.coupling != Coupling::None {
            let step = (opts.parameter_step_rel * theta.abs()).max(1e-9);
            let mut fu_f = flow_model.clone();
            let mut fu_t = thermal_model.clone();
            p.set(&mut fu_f, &mut fu_t, theta + step);
            let mut fd_f = flow_model.clone();
            let mut fd_t = thermal_model.clone();
            p.set(&mut fd_f, &mut fd_t, theta - step);
            let up = psi(&fu_f, &fu_t, &wetted, s_star, opts)?;
            let dn = psi(&fd_f, &fd_t, &wetted, s_star, opts)?;
            psi_evaluations += 2;
            let dpsi = [
                (up.h_w_m2k - dn.h_w_m2k) / (2.0 * step),
                (up.t_bulk_c - dn.t_bulk_c) / (2.0 * step),
            ];
            coupled = mu[0] * dpsi[0] + mu[1] * dpsi[1];
        }

        let value = direct + coupled;

        // Trust radius: the tighter of the declared bounds and a
        // curvature probe. The curvature is the *partial* second
        // derivative at fixed interface state — cheap (two thermal
        // solves, no flow), and a lower bound on the true curvature, so
        // the radius it yields is optimistic in the same direction the
        // reader would guess. Flow-only parameters have no partial
        // curvature at all and fall back to their bounds.
        let curvature_radius = if p.is_thermal() {
            let cstep = (opts.curvature_step_rel * theta.abs()).max(1e-6);
            let mut cu_f = flow_model.clone();
            let mut cu_t = thermal_model.clone();
            p.set(&mut cu_f, &mut cu_t, theta + cstep);
            let mut cd_f = flow_model.clone();
            let mut cd_t = thermal_model.clone();
            p.set(&mut cd_f, &mut cd_t, theta - cstep);
            let j0 = objective_at(&tm_star, s_star, opts)?;
            let mut up_t = cu_t.clone();
            up_t.exposed = tm_star.exposed;
            let mut dn_t = cd_t.clone();
            dn_t.exposed = tm_star.exposed;
            let jp = objective_at(&up_t, s_star, opts)?;
            let jm = objective_at(&dn_t, s_star, opts)?;
            let d2 = (jp - 2.0 * j0 + jm) / (cstep * cstep);
            TrustRadius::from_linearity(theta, value, d2, opts.linearity_tol)
        } else {
            None
        };
        let bounds_radius = spec
            .bounds
            .and_then(|(lo, hi)| TrustRadius::from_bounds(lo, hi));
        let trust = TrustRadius::tighter(bounds_radius, curvature_radius);

        let mut row = Sensitivity::new(
            p.name(),
            "hotspot_c",
            value,
            p.gradient_unit(),
            theta,
            Route::Coupled {
                completeness: completeness.clone(),
            },
        )
        .with_trust(trust);
        if opts.coupling != Coupling::Full {
            row = row.with_note(format!(
                "ABLATION ({}): direct {direct:.6e}, coupling {coupled:.6e}",
                opts.coupling.label()
            ));
        } else {
            row = row.with_note(format!(
                "direct {direct:.6e} + coupling {coupled:.6e}; interface spectral radius {rho:.4}"
            ));
        }
        table.push(row);
    }

    Ok(ConjugateGradient {
        objective_c: jgrad.value_c,
        hard_max_c: jgrad.hard_max_c,
        film: s_star,
        d_objective_d_film: g,
        interface_jacobian: a,
        interface_adjoint: mu,
        coupling_strength: rho,
        table,
        ledger,
        psi_evaluations,
        outer_iters: primal.outer_iters,
    })
}

/// Ablate one coupling mode against a reference and report what it costs.
///
/// The reference must be a finite-difference derivative of the **whole**
/// coupled objective — see the tests, which build it with
/// [`vcad_kernel_adjoint::fd_sweep`] so the reference has to demonstrate
/// convergence before it is allowed to judge anything.
pub fn ablate(
    flow_model: &FlowModel,
    thermal_model: &ThermalModel,
    spec: ParameterSpec,
    reference: f64,
    opts: &ConjugateAdjointOptions,
    without: Coupling,
) -> Result<AblationReport, ConjugateAdjointError> {
    let full = conjugate_gradient(
        flow_model,
        thermal_model,
        &[spec],
        &ConjugateAdjointOptions {
            coupling: Coupling::Full,
            ..*opts
        },
    )?;
    let ablated = conjugate_gradient(
        flow_model,
        thermal_model,
        &[spec],
        &ConjugateAdjointOptions {
            coupling: without,
            ..*opts
        },
    )?;
    let name = spec.parameter.name();
    Ok(ablation(
        format!("{} [{}]", name, without.label()),
        reference,
        full.table.rows[0].value,
        ablated.table.rows[0].value,
    ))
}

/// Evaluate the coupled objective end to end: converge the primal loop
/// and return the smoothed peak solid temperature.
///
/// This is the function a finite-difference reference differentiates. It
/// re-converges everything, which is why it is expensive and why the
/// adjoint exists.
pub fn coupled_objective(
    flow_model: &FlowModel,
    thermal_model: &ThermalModel,
    opts: &ConjugateAdjointOptions,
) -> Result<f64, ConjugateAdjointError> {
    let primal = solve_conjugate(flow_model, thermal_model, &opts.conjugate)?;
    objective_at(
        thermal_model,
        FilmState {
            h_w_m2k: primal.film_h_w_m2k,
            t_bulk_c: primal.t_bulk_c,
        },
        opts,
    )
}

/// Set one parameter on a cloned model pair — the helper a
/// finite-difference reference needs.
pub fn with_parameter(
    flow_model: &FlowModel,
    thermal_model: &ThermalModel,
    parameter: ConjugateParameter,
    value: f64,
) -> (FlowModel, ThermalModel) {
    let mut fm = flow_model.clone();
    let mut tm = thermal_model.clone();
    parameter.set(&mut fm, &mut tm, value);
    (fm, tm)
}

/// The trust limit a mask-moving parameter would carry on this path.
/// Exposed so callers building geometry parameters do not have to
/// rediscover that a voxel solver cannot differentiate below its cell.
pub const GEOMETRY_TRUST_LIMIT: TrustLimit = TrustLimit::GridResolution;
