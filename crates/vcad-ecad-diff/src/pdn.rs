//! Power-distribution-network (PDN) copper sizing by gradient.
//!
//! Models the PDN as a resistor mesh: nodes connected by copper segments, a VRM
//! at the reference node (0 V), and loads drawing current. Nodal analysis solves
//! `G·V = I` for the node voltages; the IR-drop at a load is `−V_node`. Sizing
//! adjusts segment widths to hit target drops.
//!
//! The gradient `d(drop)/d(width)` is the point of this module. We do NOT trace
//! the linear solve `V = G⁻¹·I` through `tang-expr` — for a mesh that graph
//! blows up quadratically (the phyz-diff `N≥8` wall). Instead the only
//! closed-form leaf is the per-segment conductance `g_e(w_e) = σ·t·w_e / L_e`
//! (differentiated by hand), and we chain through the solve with the
//! implicit-function theorem:
//!
//! ```text
//! G·V = I  (I fixed)  ⟹  (dG/dw_e)·V + G·(dV/dw_e) = 0
//!                     ⟹  dV/dw_e = −(dg_e/dw_e)·G⁻¹·(E_e·V)
//! ```
//!
//! where `E_e` is segment `e`'s elementary Laplacian. `G` is factored once (LU);
//! each segment's sensitivity is one cheap back-substitution. This is
//! "differentiate locally, chain globally" — the same principle as the
//! per-step-symbolic / through-time-adjoint split for dynamics.

use tang_la::{DMat, DVec};
use vcad_kernel_constraints::{levenberg_marquardt, LeastSquares, SolveResult, SolverConfig};

/// A copper segment between two PDN nodes; its width is a design parameter.
#[derive(Debug, Clone, Copy)]
pub struct PdnEdge {
    /// First node index (node 0 is the VRM / voltage reference).
    pub a: usize,
    /// Second node index.
    pub b: usize,
    /// Segment length in mm.
    pub length: f64,
}

impl PdnEdge {
    /// Construct a segment between nodes `a` and `b` of the given length (mm).
    pub fn new(a: usize, b: usize, length: f64) -> Self {
        Self { a, b, length }
    }
}

/// A resistor-mesh PDN to size for target IR-drops. Node 0 is the VRM (0 V).
pub struct PdnSystem {
    num_nodes: usize,
    edges: Vec<PdnEdge>,
    /// (node, current drawn in A).
    loads: Vec<(usize, f64)>,
    /// (node, target IR-drop in V) — one least-squares residual each.
    targets: Vec<(usize, f64)>,
    /// Copper conductivity σ in S/mm.
    sigma: f64,
    /// Copper thickness t in mm.
    thickness: f64,
    bounds: Option<(Vec<f64>, Vec<f64>)>,
}

impl PdnSystem {
    /// Build a PDN system. `num_nodes` includes node 0 (the VRM reference).
    /// `loads` and `targets` reference node indices `1..num_nodes`.
    pub fn new(
        num_nodes: usize,
        edges: Vec<PdnEdge>,
        loads: Vec<(usize, f64)>,
        targets: Vec<(usize, f64)>,
        sigma: f64,
        thickness: f64,
    ) -> Self {
        Self {
            num_nodes,
            edges,
            loads,
            targets,
            sigma,
            thickness,
            bounds: None,
        }
    }

    /// Attach inclusive width bounds (mm) for the LM projection hook.
    pub fn with_bounds(mut self, lo: Vec<f64>, hi: Vec<f64>) -> Self {
        assert_eq!(lo.len(), self.edges.len(), "lo bounds length");
        assert_eq!(hi.len(), self.edges.len(), "hi bounds length");
        self.bounds = Some((lo, hi));
        self
    }

    /// Reduced-system index of a node (node 0 = reference, eliminated).
    fn reduced(&self, node: usize) -> Option<usize> {
        if node == 0 {
            None
        } else {
            Some(node - 1)
        }
    }

    /// Per-segment conductance `g_e = σ·t·w / L`.
    fn conductance(&self, width: f64, length: f64) -> f64 {
        self.sigma * self.thickness * width / length
    }

    /// Assemble the reduced conductance (weighted Laplacian) matrix.
    fn build_g(&self, widths: &[f64]) -> DMat<f64> {
        let m = self.num_nodes - 1;
        let mut g = DMat::zeros(m, m);
        for (e, edge) in self.edges.iter().enumerate() {
            let cond = self.conductance(widths[e], edge.length);
            let ra = self.reduced(edge.a);
            let rb = self.reduced(edge.b);
            if let Some(a) = ra {
                g[(a, a)] += cond;
            }
            if let Some(b) = rb {
                g[(b, b)] += cond;
            }
            if let (Some(a), Some(b)) = (ra, rb) {
                g[(a, b)] -= cond;
                g[(b, a)] -= cond;
            }
        }
        g
    }

    /// Current injected at each non-reference node (loads draw → negative).
    fn injection(&self) -> Vec<f64> {
        let mut inj = vec![0.0; self.num_nodes - 1];
        for &(node, current) in &self.loads {
            if let Some(r) = self.reduced(node) {
                inj[r] = -current;
            }
        }
        inj
    }

    /// Solve `G·V = I` for the node voltages (forward pass).
    fn forward(&self, widths: &[f64]) -> DVec<f64> {
        self.build_g(widths)
            .lu()
            .solve(&DVec::from_vec(self.injection()))
            .expect("PDN conductance matrix is singular (disconnected mesh?)")
    }

    /// Full node voltage (reference node is 0 V).
    fn vfull(&self, v: &DVec<f64>, node: usize) -> f64 {
        match self.reduced(node) {
            Some(r) => v[r],
            None => 0.0,
        }
    }

    /// IR-drop (V) at each target node, from a fresh forward solve.
    pub fn drops(&self, widths: &[f64]) -> Vec<f64> {
        let v = self.forward(widths);
        self.targets
            .iter()
            .map(|&(node, _)| -self.vfull(&v, node))
            .collect()
    }

    /// Residuals `drop_k − target_k`.
    fn residuals(&self, widths: &[f64]) -> Vec<f64> {
        let v = self.forward(widths);
        self.targets
            .iter()
            .map(|&(node, target)| -self.vfull(&v, node) - target)
            .collect()
    }

    /// Jacobian `J[k][e] = d(drop_k)/d(w_e)` via the implicit-function theorem.
    /// Factor `G` once, then one back-substitution per segment.
    fn jacobian(&self, widths: &[f64]) -> Vec<Vec<f64>> {
        // Factor G once; reuse the factorization for every per-segment solve.
        let lu = self.build_g(widths).lu();
        let v = lu
            .solve(&DVec::from_vec(self.injection()))
            .expect("PDN conductance matrix is singular");
        let ne = self.edges.len();
        let nt = self.targets.len();
        let mut jac = vec![vec![0.0; ne]; nt];

        for (e, edge) in self.edges.iter().enumerate() {
            // RHS = E_e·V (elementary Laplacian of segment e applied to V).
            let d = self.vfull(&v, edge.a) - self.vfull(&v, edge.b);
            let mut rhs = vec![0.0; self.num_nodes - 1];
            if let Some(a) = self.reduced(edge.a) {
                rhs[a] += d;
            }
            if let Some(b) = self.reduced(edge.b) {
                rhs[b] -= d;
            }
            // x = G⁻¹·(E_e·V).  dV/dw_e = −(dg_e/dw_e)·x.
            let x = lu
                .solve(&DVec::from_vec(rhs))
                .expect("PDN sensitivity solve is singular");
            // dg_e/dw_e = σ·t/L_e (g is linear in width).
            let coef = self.sigma * self.thickness / edge.length;
            // d(drop_k)/dw_e = −dV_{node_k}/dw_e = coef·x_{node_k}.
            for (k, &(node, _)) in self.targets.iter().enumerate() {
                if let Some(r) = self.reduced(node) {
                    jac[k][e] = coef * x[r];
                }
            }
        }
        jac
    }

    /// Solve for segment widths via the generic LM driver (mutates `widths`).
    pub fn solve(&self, widths: &mut [f64], config: &SolverConfig) -> SolveResult {
        levenberg_marquardt(self, widths, config)
    }
}

impl LeastSquares for PdnSystem {
    fn num_params(&self) -> usize {
        self.edges.len()
    }

    fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>) {
        let ne = self.edges.len();
        let j = self.jacobian(params);
        let r = self.residuals(params);
        let mut jtj = DMat::zeros(ne, ne);
        let mut jtr = vec![0.0; ne];
        for (k, &r_k) in r.iter().enumerate() {
            let row = &j[k];
            for a in 0..ne {
                jtr[a] += row[a] * r_k;
                for b in 0..ne {
                    jtj[(a, b)] += row[a] * row[b];
                }
            }
        }
        (jtj, jtr)
    }

    fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        self.residuals(params).iter().map(|v| v * v).sum()
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

    // Copper: σ = 1/ρ with ρ = 1.68e-5 Ω·mm (matches the spiral-coil model).
    const SIGMA: f64 = 1.0 / 1.68e-5;
    const T: f64 = 0.035;

    /// A Wheatstone-bridge mesh: VRM(0) → {1,2} → 3, with a bridge edge 1–2.
    /// Non-series-parallel, so current genuinely redistributes with widths and
    /// the matrix-solve sensitivity (not a simple R-formula) is required.
    fn bridge(targets: Vec<(usize, f64)>) -> PdnSystem {
        let edges = vec![
            PdnEdge::new(0, 1, 10.0),
            PdnEdge::new(0, 2, 10.0),
            PdnEdge::new(1, 3, 10.0),
            PdnEdge::new(2, 3, 10.0),
            PdnEdge::new(1, 2, 8.0), // the bridge
        ];
        PdnSystem::new(4, edges, vec![(3, 1.0)], targets, SIGMA, T)
    }

    #[test]
    fn jacobian_matches_finite_difference() {
        // Targets at all three non-reference nodes → a rich 3×5 Jacobian.
        let sys = bridge(vec![(1, 0.0), (2, 0.0), (3, 0.0)]);
        // Deliberately asymmetric widths so the bridge carries current.
        let w = vec![0.20, 0.15, 0.25, 0.18, 0.12];

        let jac = sys.jacobian(&w);
        let eps = 1e-7;
        for (k, jac_row) in jac.iter().enumerate() {
            for (e, &analytic) in jac_row.iter().enumerate() {
                let mut wp = w.clone();
                let mut wm = w.clone();
                wp[e] += eps;
                wm[e] -= eps;
                let fd = (sys.drops(&wp)[k] - sys.drops(&wm)[k]) / (2.0 * eps);
                assert!(
                    (analytic - fd).abs() < 1e-6 * (1.0 + fd.abs()),
                    "J[{k}][{e}] adjoint {analytic} vs finite-difference {fd}"
                );
            }
        }
    }

    #[test]
    fn wider_copper_lowers_drop() {
        let sys = bridge(vec![(3, 0.0)]);
        let narrow = sys.drops(&[0.1, 0.1, 0.1, 0.1, 0.1])[0];
        let wide = sys.drops(&[0.4, 0.4, 0.4, 0.4, 0.4])[0];
        assert!(
            wide < narrow,
            "wider copper should drop less: {wide} vs {narrow}"
        );
    }

    #[test]
    fn sizes_copper_to_meet_target_drop() {
        // Start narrow (high drop); size widths to hit a 15 mV target at node 3.
        let target = 0.015;
        let sys = bridge(vec![(3, target)]).with_bounds(vec![0.05; 5], vec![3.0; 5]);

        let mut widths = vec![0.1; 5];
        let res = sys.solve(&mut widths, &SolverConfig::default());
        assert!(
            res.converged,
            "PDN sizing should converge: {:?}",
            res.status
        );

        // Re-verify the drop from a forward solve at the solved widths.
        let drop = sys.drops(&widths)[0];
        assert!(
            (drop - target).abs() < 1e-5,
            "node-3 drop {drop} vs target {target}"
        );
        // Widths stayed inside the box.
        for &w in &widths {
            assert!((0.05..=3.0).contains(&w), "width out of bounds: {w}");
        }
    }
}
