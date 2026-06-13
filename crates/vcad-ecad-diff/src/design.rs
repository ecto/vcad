//! Symbolic differentiable design system.
//!
//! A clone of `CompiledSystem`'s trace → sparsity → diff → simplify → compile
//! pipeline, but residuals are caller-supplied closures over the symbolic
//! parameter vars (built from the impedance leaves) rather than sketch
//! constraints, and it carries optional box bounds for the LM projection hook.

use tang_expr::{trace, ExprId};
use tang_la::DMat;
use vcad_kernel_constraints::{levenberg_marquardt, LeastSquares, SolveResult, SolverConfig};

/// Compiled multi-output closure: maps input params to output values in place.
type CompiledFn = Box<dyn Fn(&[f64], &mut [f64])>;

/// A single design residual as a function of the symbolic parameter vars.
///
/// `vars[i] == ExprId::var(i as u16)`. Return one `ExprId` whose value should
/// be driven to zero (e.g. `microstrip_z0(vars[0], ..) - target`).
///
/// IMPORTANT: lift every constant with `ExprId::from_f64` *inside* the closure.
/// The closure runs inside [`DesignSystem::build`]'s `trace`, so constants must
/// be created in that graph — an `ExprId` built elsewhere refers to a different
/// graph and would dangle.
pub type ResidualFn = Box<dyn Fn(&[ExprId]) -> ExprId>;

/// Sparse Jacobian entry (row = residual, col = param). Kept local so this
/// crate depends on `vcad-kernel-constraints` only for the `LeastSquares`
/// trait and the LM driver.
#[derive(Debug, Clone, Copy)]
struct SparseEntry {
    row: usize,
    col: usize,
}

/// A compiled differentiable design system. Build once, evaluate/solve many.
pub struct DesignSystem {
    /// Number of residual equations.
    pub num_residuals: usize,
    /// Number of free parameters.
    pub num_params: usize,
    /// Number of structurally non-zero Jacobian entries.
    pub num_nonzero: usize,
    /// Dense Jacobian size (`num_residuals * num_params`).
    pub dense_size: usize,
    residual_fn: CompiledFn,
    jacobian_fn: CompiledFn,
    sparse_entries: Vec<SparseEntry>,
    bounds: Option<(Vec<f64>, Vec<f64>)>,
}

impl DesignSystem {
    /// Trace `residuals` over `num_params` symbolic vars, sparsity-analyze,
    /// differentiate only the structurally non-zero entries, simplify, and
    /// compile both the residual and Jacobian closures.
    pub fn build(residuals: &[ResidualFn], num_params: usize) -> Self {
        let num_residuals = residuals.len();
        let dense_size = num_residuals * num_params;

        if num_residuals == 0 || num_params == 0 {
            return Self {
                num_residuals,
                num_params,
                num_nonzero: 0,
                dense_size,
                residual_fn: Box::new(|_, _| {}),
                jacobian_fn: Box::new(|_, _| {}),
                sparse_entries: Vec::new(),
                bounds: None,
            };
        }

        // Trace all residuals into one expression graph. The closures build
        // their constants here (inside the fresh graph), keeping node ids valid.
        let (mut graph, residual_exprs) = trace(|| {
            let vars: Vec<ExprId> = (0..num_params).map(|i| ExprId::var(i as u16)).collect();
            residuals.iter().map(|f| f(&vars)).collect::<Vec<ExprId>>()
        });

        // Structural sparsity: bit j of mask[i] is set iff residual i reads var j.
        // tang-expr's u64 bitmask caps at 64 vars; above that, treat as dense.
        let use_sparsity = num_params <= 64;
        let masks: Vec<u64> = if use_sparsity {
            graph.jacobian_sparsity(&residual_exprs, num_params)
        } else {
            vec![u64::MAX; num_residuals]
        };

        // Differentiate only the non-zero entries, row-major (i outer, j inner)
        // — the ordering eval_jtj_jtr relies on.
        let mut sparse_entries = Vec::new();
        let mut jac_exprs = Vec::new();
        for (i, r) in residual_exprs.iter().enumerate() {
            let mask = masks[i];
            for j in 0..num_params {
                if !use_sparsity || mask & (1u64 << j) != 0 {
                    sparse_entries.push(SparseEntry { row: i, col: j });
                    let d = graph.diff(*r, j as u16);
                    let d = graph.simplify(d);
                    jac_exprs.push(d);
                }
            }
        }
        let num_nonzero = sparse_entries.len();

        let simplified_residuals: Vec<ExprId> = residual_exprs
            .into_iter()
            .map(|r| graph.simplify(r))
            .collect();

        let residual_fn = graph.compile_many(&simplified_residuals);
        let jacobian_fn = graph.compile_many(&jac_exprs);

        Self {
            num_residuals,
            num_params,
            num_nonzero,
            dense_size,
            residual_fn,
            jacobian_fn,
            sparse_entries,
            bounds: None,
        }
    }

    /// Attach inclusive box bounds for the LM projection hook. Every trial step
    /// is clamped into `[lo, hi]` before its cost is measured. Panics if either
    /// slice length differs from `num_params`.
    pub fn with_bounds(mut self, lo: Vec<f64>, hi: Vec<f64>) -> Self {
        assert_eq!(lo.len(), self.num_params, "lo bounds length");
        assert_eq!(hi.len(), self.num_params, "hi bounds length");
        self.bounds = Some((lo, hi));
        self
    }

    /// Evaluate all residuals at `params`.
    pub fn eval_residuals(&self, params: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.num_residuals];
        (self.residual_fn)(params, &mut out);
        out
    }

    /// Evaluate the non-zero Jacobian entries, in `sparse_entries` order.
    pub fn eval_jacobian_sparse(&self, params: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.num_nonzero];
        (self.jacobian_fn)(params, &mut out);
        out
    }

    /// Compute `(J'J, J'r)` from the sparse Jacobian (row-major grouped accum),
    /// the same assembly `CompiledSystem` uses.
    pub fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>) {
        let np = self.num_params;
        let values = self.eval_jacobian_sparse(params);
        let residuals = self.eval_residuals(params);
        let mut jtj = DMat::zeros(np, np);
        let mut jtr = vec![0.0; np];
        let total = self.sparse_entries.len();
        let mut idx = 0;
        while idx < total {
            let row = self.sparse_entries[idx].row;
            let row_start = idx;
            while idx < total && self.sparse_entries[idx].row == row {
                idx += 1;
            }
            let row_entries = &self.sparse_entries[row_start..idx];
            let row_values = &values[row_start..idx];
            let r_i = residuals[row];
            for (e, &j_ip) in row_entries.iter().zip(row_values) {
                jtr[e.col] += j_ip * r_i;
            }
            for (e1, &j_ip) in row_entries.iter().zip(row_values) {
                for (e2, &j_iq) in row_entries.iter().zip(row_values) {
                    jtj[(e1.col, e2.col)] += j_ip * j_iq;
                }
            }
        }
        (jtj, jtr)
    }

    /// Sum of squared residuals at `params`.
    pub fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        self.eval_residuals(params).iter().map(|v| v * v).sum()
    }

    /// Solve via the generic Levenberg-Marquardt driver (mutates `params`).
    pub fn solve(&self, params: &mut [f64], config: &SolverConfig) -> SolveResult {
        levenberg_marquardt(self, params, config)
    }
}

impl LeastSquares for DesignSystem {
    fn num_params(&self) -> usize {
        self.num_params
    }
    fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>) {
        DesignSystem::eval_jtj_jtr(self, params)
    }
    fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        DesignSystem::residual_norm_squared(self, params)
    }
    fn project(&self, params: &mut [f64]) {
        if let Some((lo, hi)) = &self.bounds {
            for (i, p) in params.iter_mut().enumerate() {
                *p = p.clamp(lo[i], hi[i]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tang::Scalar;
    use vcad_ecad_sim::impedance::{diff_coupling_k, microstrip_z0};

    // Shared FR-4 microstrip stackup for the tests.
    const T: f64 = 0.035;
    const H: f64 = 0.2;
    const ER: f64 = 4.3;

    #[test]
    fn diff_pair_solves_to_targets() {
        let z0t = 50.0;
        let zdt = 90.0;

        // params = [w1, w2, s]. Constants are lifted inside each closure so they
        // live in build()'s trace graph. r2 reads all three params (genuinely
        // coupled) and reduces to 2*Z0*k at the symmetric solution.
        let residuals: Vec<ResidualFn> = vec![
            Box::new(move |v| {
                microstrip_z0(
                    v[0],
                    ExprId::from_f64(T),
                    ExprId::from_f64(H),
                    ExprId::from_f64(ER),
                ) - ExprId::from_f64(z0t)
            }),
            Box::new(move |v| {
                microstrip_z0(
                    v[1],
                    ExprId::from_f64(T),
                    ExprId::from_f64(H),
                    ExprId::from_f64(ER),
                ) - ExprId::from_f64(z0t)
            }),
            Box::new(move |v| {
                let (t, h, er) = (
                    ExprId::from_f64(T),
                    ExprId::from_f64(H),
                    ExprId::from_f64(ER),
                );
                let z1 = microstrip_z0(v[0], t, h, er);
                let z2 = microstrip_z0(v[1], t, h, er);
                let k = diff_coupling_k(v[2], h, ExprId::from_f64(0.48), ExprId::from_f64(0.96));
                (z1 + z2) * k - ExprId::from_f64(zdt)
            }),
        ];

        let sys = DesignSystem::build(&residuals, 3)
            .with_bounds(vec![0.05, 0.05, 0.01], vec![2.0, 2.0, 5.0]);

        // r0 -> {0}, r1 -> {1}, r2 -> {0,1,2}: 5 non-zero of 9 dense.
        assert_eq!(sys.num_residuals, 3);
        assert_eq!(sys.num_params, 3);
        assert_eq!(sys.num_nonzero, 5);
        assert_eq!(sys.dense_size, 9);

        let mut params = vec![0.45, 0.18, 0.8]; // deliberately asymmetric seed
        let res = sys.solve(&mut params, &SolverConfig::default());
        assert!(
            res.converged,
            "diff-pair solve should converge: {:?}",
            res.status
        );

        let (w1, w2, s) = (params[0], params[1], params[2]);

        // (a) identical per-line targets => symmetric widths.
        assert!(
            (w1 - w2).abs() < 1e-6,
            "expected symmetric widths, got {w1} vs {w2}"
        );

        // (b) re-verify each line's Z0 against the forward f64 model.
        let z0_1 = microstrip_z0(w1, T, H, ER);
        let z0_2 = microstrip_z0(w2, T, H, ER);
        assert!((z0_1 - z0t).abs() < 1e-4, "line1 Z0 = {z0_1}");
        assert!((z0_2 - z0t).abs() < 1e-4, "line2 Z0 = {z0_2}");

        // (c) re-verify Zdiff against the forward model.
        let zdiff = (z0_1 + z0_2) * diff_coupling_k(s, H, 0.48, 0.96);
        assert!((zdiff - zdt).abs() < 1e-4, "Zdiff = {zdiff}");

        // (d) loose geometry sanity (the solution lives strictly inside the box).
        assert!((0.1..0.6).contains(&w1), "w out of expected range: {w1}");
        assert!((0.1..1.0).contains(&s), "s out of expected range: {s}");
    }

    #[test]
    fn jacobian_matches_finite_difference() {
        let residuals: Vec<ResidualFn> = vec![Box::new(move |v| {
            microstrip_z0(
                v[0],
                ExprId::from_f64(T),
                ExprId::from_f64(H),
                ExprId::from_f64(ER),
            ) - ExprId::from_f64(50.0)
        })];
        let sys = DesignSystem::build(&residuals, 1);
        assert_eq!(sys.num_nonzero, 1);

        let sparse = sys.eval_jacobian_sparse(&[0.30]);
        let eps = 1e-6;
        let fd = (microstrip_z0(0.30 + eps, T, H, ER) - microstrip_z0(0.30 - eps, T, H, ER))
            / (2.0 * eps);
        assert!(
            (sparse[0] - fd).abs() < 1e-4,
            "symbolic {} vs FD {fd}",
            sparse[0]
        );
        assert!(sparse[0] < 0.0, "wider trace => lower Z0");
    }

    #[test]
    fn sparse_n_traces_diagonal_jacobian() {
        let n = 8usize;
        let residuals: Vec<ResidualFn> = (0..n)
            .map(|i| {
                let f: ResidualFn = Box::new(move |v| {
                    microstrip_z0(
                        v[i],
                        ExprId::from_f64(T),
                        ExprId::from_f64(H),
                        ExprId::from_f64(ER),
                    ) - ExprId::from_f64(50.0)
                });
                f
            })
            .collect();

        let sys = DesignSystem::build(&residuals, n).with_bounds(vec![0.05; n], vec![2.0; n]);

        // Each residual touches exactly one param => diagonal Jacobian.
        assert_eq!(sys.num_nonzero, n);
        assert_eq!(sys.dense_size, n * n);

        let mut params = vec![0.5; n];
        let res = sys.solve(&mut params, &SolverConfig::default());
        assert!(
            res.converged,
            "n-trace solve should converge: {:?}",
            res.status
        );
        for &w in &params {
            assert!((microstrip_z0(w, T, H, ER) - 50.0).abs() < 1e-4, "w={w}");
        }
    }

    /// Co-design foundation: drive a MOTOR performance target by gradient on
    /// geometry. The differentiable torque-constant leaf flows through the same
    /// DesignSystem engine — proving geometry→performance sizing works for the
    /// magnetics archetype, the bridge a future phyz co-design loop consumes.
    #[test]
    fn sizes_stator_radius_for_a_target_torque_constant() {
        use vcad_ecad_sim::magnetics::motor_torque_constant;
        let target_kt = 0.02; // N·m/A
                              // 12-pole (p=6), 60 series turns, kw=0.866, 0.4 T airgap, 5 mm bore.
                              // var(0) = outer stator radius (mm); everything else baked.
        let residuals: Vec<ResidualFn> = vec![Box::new(move |v| {
            motor_torque_constant(
                ExprId::from_f64(6.0),
                ExprId::from_f64(60.0),
                ExprId::from_f64(0.866),
                ExprId::from_f64(0.4),
                ExprId::from_f64(5.0),
                v[0],
            ) - ExprId::from_f64(target_kt)
        })];

        let sys = DesignSystem::build(&residuals, 1).with_bounds(vec![6.0], vec![60.0]);
        let mut params = vec![20.0]; // seed outer radius (mm)
        let res = sys.solve(&mut params, &SolverConfig::default());
        assert!(res.converged, "Kt sizing should converge: {:?}", res.status);

        // Re-verify the torque constant at the solved radius against the model.
        let kt = motor_torque_constant(6.0, 60.0, 0.866, 0.4, 5.0, params[0]);
        assert!(
            (kt - target_kt).abs() < 1e-6,
            "Kt {kt} vs target {target_kt}"
        );
        assert!(
            params[0] > 6.0 && params[0] < 60.0,
            "radius in box: {}",
            params[0]
        );
    }
}
