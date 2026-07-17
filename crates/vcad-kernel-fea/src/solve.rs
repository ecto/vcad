//! Linear-elastic static solve on a tet mesh.
//!
//! Constant-strain tetrahedra, assembled matrix-free: each element stores
//! only its four shape-function gradients and volume, and the matvec
//! recomputes strain → stress → nodal forces on the fly. The system is
//! solved at **unit Young's modulus** with Jacobi-preconditioned
//! conjugate gradients; linearity rescales displacement by 1/E afterward
//! while stress is E-independent for force-driven problems (the same
//! trick as `vcad-kernel-topopt`).
//!
//! The stopping criterion is relative to the load-vector norm, so a
//! 1 N problem and a 10 kN problem converge to the same relative quality.

use crate::mesh::TetMesh;
use crate::spec::FeaSpec;

/// Per-element data for the matrix-free operator.
struct Elem {
    n: [u32; 4],
    grad: [[f64; 3]; 4],
    vol: f64,
}

/// Solve failures.
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The spec failed validation.
    Spec(crate::spec::SpecError),
    /// A load or support region selected no mesh node (fail-closed).
    EmptyRegion(String),
    /// The mesh contains a degenerate tetrahedron.
    DegenerateTet(usize),
    /// PCG did not reach the requested tolerance.
    NotConverged {
        /// Relative residual reached.
        residual_rel: f64,
        /// Iterations run.
        iterations: usize,
    },
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::Spec(e) => write!(f, "{e}"),
            SolveError::EmptyRegion(what) => write!(
                f,
                "{what} region selected no mesh node — check its coordinates against the \
                 part's bounding box (fail-closed: an unattached load/support is an error)"
            ),
            SolveError::DegenerateTet(i) => write!(f, "degenerate tetrahedron {i}"),
            SolveError::NotConverged {
                residual_rel,
                iterations,
            } => write!(
                f,
                "PCG stalled at relative residual {residual_rel:.3e} after {iterations} \
                 iterations"
            ),
        }
    }
}

impl std::error::Error for SolveError {}

impl From<crate::spec::SpecError> for SolveError {
    fn from(e: crate::spec::SpecError) -> Self {
        SolveError::Spec(e)
    }
}

/// Result of one static solve at one mesh resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Solution {
    /// Maximum nodal displacement magnitude, mm.
    pub max_displacement_mm: f64,
    /// Position of the most-displaced node, mm.
    pub max_displacement_at: [f64; 3],
    /// Maximum element von Mises stress, MPa (constant per tet; smears
    /// concentrations — grows toward the true value with refinement, and
    /// is unbounded with refinement at a genuinely singular re-entrant
    /// corner).
    pub max_von_mises_mpa: f64,
    /// Centroid of the most-stressed tet, mm.
    pub max_stress_at: [f64; 3],
    /// Compliance `fᵀu`, N·mm (work done by the loads; lower = stiffer).
    pub compliance_n_mm: f64,
    /// Meshed volume, mm³.
    pub volume_mm3: f64,
    /// Node count.
    pub nodes: usize,
    /// Tet count.
    pub tets: usize,
    /// Lattice pitch, mm.
    pub h_mm: f64,
    /// Lattice cell counts.
    pub grid: [usize; 3],
    /// PCG iterations used.
    pub iterations: usize,
    /// Final PCG relative residual.
    pub residual_rel: f64,
}

/// PCG controls.
#[derive(Debug, Clone, Copy)]
pub struct SolveOptions {
    /// Relative tolerance on the preconditioned residual.
    pub tol: f64,
    /// Iteration cap.
    pub max_iters: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            tol: 1e-8,
            max_iters: 20_000,
        }
    }
}

/// Solve one static case on a prepared tet mesh.
pub fn solve_static(
    mesh: &TetMesh,
    spec: &FeaSpec,
    opts: &SolveOptions,
) -> Result<Solution, SolveError> {
    spec.validate()?;

    // Element gradients: for a linear tet with nodes x0..x3, the
    // barycentric gradients are rows of the inverse edge matrix.
    let mut elems = Vec::with_capacity(mesh.tets.len());
    for (ti, t) in mesh.tets.iter().enumerate() {
        let p = |i: usize| mesh.nodes[t[i] as usize];
        let x0 = p(0);
        let e = [sub(p(1), x0), sub(p(2), x0), sub(p(3), x0)];
        let det = det3(e[0], e[1], e[2]);
        if det.abs() < 1e-30 {
            return Err(SolveError::DegenerateTet(ti));
        }
        let vol = det / 6.0;
        if vol <= 0.0 {
            return Err(SolveError::DegenerateTet(ti));
        }
        // Inverse of the edge matrix (rows e[0..3]) times identity gives
        // gradients of barycentric coords 1..3; grad0 = -sum.
        let inv = inv3([e[0], e[1], e[2]], det);
        let g1 = [inv[0][0], inv[1][0], inv[2][0]];
        let g2 = [inv[0][1], inv[1][1], inv[2][1]];
        let g3 = [inv[0][2], inv[1][2], inv[2][2]];
        let g0 = [
            -g1[0] - g2[0] - g3[0],
            -g1[1] - g2[1] - g3[1],
            -g1[2] - g2[2] - g3[2],
        ];
        elems.push(Elem {
            n: *t,
            grad: [g0, g1, g2, g3],
            vol,
        });
    }

    // Lamé constants at unit E.
    let nu = spec.poisson;
    let lam = nu / ((1.0 + nu) * (1.0 - 2.0 * nu));
    let mu = 1.0 / (2.0 * (1.0 + nu));

    let nn = mesh.nodes.len();
    let ndof = 3 * nn;
    let tol_geom = mesh.h * 0.25;

    // Supports → fixed-dof mask.
    let mut fixed = vec![false; ndof];
    for (si, s) in spec.supports.iter().enumerate() {
        let mut hit = false;
        for (i, p) in mesh.nodes.iter().enumerate() {
            if s.region.contains(*p, tol_geom) {
                hit = true;
                for a in 0..3 {
                    if s.fix[a] {
                        fixed[3 * i + a] = true;
                    }
                }
            }
        }
        if !hit {
            return Err(SolveError::EmptyRegion(format!("support {si}")));
        }
    }

    // Loads → force vector, split evenly over selected nodes.
    let mut f = vec![0.0f64; ndof];
    for (li, l) in spec.loads.iter().enumerate() {
        let sel: Vec<usize> = mesh
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, p)| l.region.contains(**p, tol_geom))
            .map(|(i, _)| i)
            .collect();
        if sel.is_empty() {
            return Err(SolveError::EmptyRegion(format!("load {li}")));
        }
        let per = 1.0 / sel.len() as f64;
        for i in sel {
            for a in 0..3 {
                f[3 * i + a] += l.force[a] * per;
            }
        }
    }
    for (d, fx) in fixed.iter().zip(f.iter_mut()) {
        if *d {
            *fx = 0.0;
        }
    }

    // Jacobi preconditioner: assembled diagonal of K (unit E).
    let mut diag = vec![0.0f64; ndof];
    for e in &elems {
        for (i, g) in e.grad.iter().enumerate() {
            let gg = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
            for a in 0..3 {
                diag[3 * e.n[i] as usize + a] +=
                    e.vol * (lam * g[a] * g[a] + mu * (g[a] * g[a] + gg));
            }
        }
    }
    for (d, v) in fixed.iter().zip(diag.iter_mut()) {
        if *d || *v == 0.0 {
            *v = 1.0;
        }
    }

    let matvec = |u: &[f64], y: &mut [f64]| {
        y.iter_mut().for_each(|v| *v = 0.0);
        for e in &elems {
            // Displacement gradient H_ab = sum_i u_i_a * g_i_b.
            let mut h = [[0.0f64; 3]; 3];
            for (i, g) in e.grad.iter().enumerate() {
                let base = 3 * e.n[i] as usize;
                for a in 0..3 {
                    let ua = u[base + a];
                    for b in 0..3 {
                        h[a][b] += ua * g[b];
                    }
                }
            }
            let tr = h[0][0] + h[1][1] + h[2][2];
            // Stress sigma = lam*tr*I + mu*(H + H^T).
            let mut s = [[0.0f64; 3]; 3];
            for a in 0..3 {
                for b in 0..3 {
                    s[a][b] = mu * (h[a][b] + h[b][a]);
                }
                s[a][a] += lam * tr;
            }
            for (i, g) in e.grad.iter().enumerate() {
                let base = 3 * e.n[i] as usize;
                for a in 0..3 {
                    y[base + a] += e.vol * (s[a][0] * g[0] + s[a][1] * g[1] + s[a][2] * g[2]);
                }
            }
        }
        for (d, v) in fixed.iter().zip(y.iter_mut()) {
            if *d {
                *v = 0.0;
            }
        }
    };

    // Jacobi-PCG at unit E.
    let mut u = vec![0.0f64; ndof];
    let mut r = f.clone();
    let mut z: Vec<f64> = r.iter().zip(&diag).map(|(r, d)| r / d).collect();
    let mut p = z.clone();
    let mut ap = vec![0.0f64; ndof];
    let fnorm = norm(&f);
    if fnorm == 0.0 {
        return Err(SolveError::Spec(crate::spec::SpecError::Invalid(
            "all load force lands on fixed nodes — nothing to solve".into(),
        )));
    }
    let mut rz = dot(&r, &z);
    let mut iterations = 0;
    let mut residual_rel = 1.0;
    for it in 0..opts.max_iters {
        matvec(&p, &mut ap);
        let pap = dot(&p, &ap);
        if pap <= 0.0 {
            break; // Numerical breakdown; report the residual we reached.
        }
        let alpha = rz / pap;
        for i in 0..ndof {
            u[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        residual_rel = norm(&r) / fnorm;
        iterations = it + 1;
        if residual_rel < opts.tol {
            break;
        }
        for i in 0..ndof {
            z[i] = r[i] / diag[i];
        }
        let rz_new = dot(&r, &z);
        let beta = rz_new / rz;
        rz = rz_new;
        for i in 0..ndof {
            p[i] = z[i] + beta * p[i];
        }
    }
    // Fail-closed: anything worse than 1e-6 relative is not a solution.
    if residual_rel >= opts.tol.max(1e-6) {
        return Err(SolveError::NotConverged {
            residual_rel,
            iterations,
        });
    }

    // QoIs. Displacement rescales by 1/E; stress from the unit-E solve is
    // already physical (E cancels for force-driven loads).
    let e_mod = spec.youngs_modulus_mpa;
    let compliance = dot(&f, &u) / e_mod;
    let mut max_d2 = 0.0f64;
    let mut max_d_node = 0usize;
    for i in 0..nn {
        let d2 = u[3 * i].powi(2) + u[3 * i + 1].powi(2) + u[3 * i + 2].powi(2);
        if d2 > max_d2 {
            max_d2 = d2;
            max_d_node = i;
        }
    }

    let mut max_vm = 0.0f64;
    let mut max_vm_elem = 0usize;
    for (ei, e) in elems.iter().enumerate() {
        let mut h = [[0.0f64; 3]; 3];
        for (i, g) in e.grad.iter().enumerate() {
            let base = 3 * e.n[i] as usize;
            for a in 0..3 {
                for b in 0..3 {
                    h[a][b] += u[base + a] * g[b];
                }
            }
        }
        let tr = h[0][0] + h[1][1] + h[2][2];
        let sxx = lam * tr + 2.0 * mu * h[0][0];
        let syy = lam * tr + 2.0 * mu * h[1][1];
        let szz = lam * tr + 2.0 * mu * h[2][2];
        let sxy = mu * (h[0][1] + h[1][0]);
        let syz = mu * (h[1][2] + h[2][1]);
        let sxz = mu * (h[0][2] + h[2][0]);
        let vm = (0.5 * ((sxx - syy).powi(2) + (syy - szz).powi(2) + (szz - sxx).powi(2))
            + 3.0 * (sxy * sxy + syz * syz + sxz * sxz))
            .sqrt();
        if vm > max_vm {
            max_vm = vm;
            max_vm_elem = ei;
        }
    }
    let centroid = {
        let t = &mesh.tets[max_vm_elem];
        let mut c = [0.0; 3];
        for &ni in t {
            for (ca, na) in c.iter_mut().zip(&mesh.nodes[ni as usize]) {
                *ca += na * 0.25;
            }
        }
        c
    };

    Ok(Solution {
        max_displacement_mm: max_d2.sqrt() / e_mod,
        max_displacement_at: mesh.nodes[max_d_node],
        max_von_mises_mpa: max_vm,
        max_stress_at: centroid,
        compliance_n_mm: compliance,
        volume_mm3: mesh.volume(),
        nodes: nn,
        tets: mesh.tets.len(),
        h_mm: mesh.h,
        grid: mesh.grid,
        iterations,
        residual_rel,
    })
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn det3(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

/// Inverse of the 3×3 matrix whose *rows* are `m[0..3]`, given its det.
fn inv3(m: [[f64; 3]; 3], det: f64) -> [[f64; 3]; 3] {
    let inv_det = 1.0 / det;
    let mut out = [[0.0; 3]; 3];
    for (j, row) in out.iter_mut().enumerate() {
        for (i, slot) in row.iter_mut().enumerate() {
            let (a, b) = ((i + 1) % 3, (i + 2) % 3);
            let (c, d) = ((j + 1) % 3, (j + 2) % 3);
            // Cofactor transpose: out[j][i] pattern gives the inverse.
            *slot = (m[a][c] * m[b][d] - m[a][d] * m[b][c]) * inv_det;
        }
    }
    out
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_mesh, tet_fill};
    use crate::spec::{Load, RegionBox, Support};

    fn bar_spec(force: [f64; 3], res: usize) -> FeaSpec {
        FeaSpec {
            resolution: res,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: None,
            loads: vec![Load {
                region: RegionBox {
                    min: [80.0, 0.0, 0.0],
                    max: [80.0, 10.0, 10.0],
                },
                force,
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
    fn axial_bar_matches_hookes_law() {
        // 80×10×10 mm bar, 1000 N axial: delta = FL/(EA)
        // = 1000*80/(69000*100) = 0.011594 mm. Constant-strain tets
        // represent uniform uniaxial stress exactly away from the clamped
        // end; the clamped-end Poisson constraint perturbs it slightly.
        let mesh = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let tm = tet_fill(&mesh, 32).unwrap();
        let spec = bar_spec([1000.0, 0.0, 0.0], 32);
        let s = solve_static(&tm, &spec, &SolveOptions::default()).unwrap();
        let exact = 1000.0 * 80.0 / (69_000.0 * 100.0);
        // Compliance f.u = F * (mean tip axial displacement) is the clean
        // closed-form comparison; the max *magnitude* also carries the
        // Poisson lateral motion of the tip corners, so it reads a few %
        // above the axial value.
        let compliance_exact = 1000.0 * exact;
        let rel_c = (s.compliance_n_mm - compliance_exact).abs() / compliance_exact;
        assert!(
            rel_c < 0.03,
            "compliance {} vs exact {compliance_exact}, rel {rel_c}",
            s.compliance_n_mm
        );
        let rel = (s.max_displacement_mm - exact).abs() / exact;
        assert!(
            rel < 0.10,
            "axial tip {} vs exact {exact}, rel {rel}",
            s.max_displacement_mm
        );
        // Nominal stress F/A = 10 MPa; the peak sits at the fully-clamped
        // end, where suppressing Poisson contraction is a genuine (edge-
        // singular) stress concentration — the max reads well above
        // nominal and grows slowly with refinement. Bound it loosely.
        assert!(
            s.max_von_mises_mpa > 9.0 && s.max_von_mises_mpa < 40.0,
            "bar stress {}",
            s.max_von_mises_mpa
        );
    }

    #[test]
    fn displacement_scales_inversely_with_e_and_stress_does_not() {
        let mesh = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let tm = tet_fill(&mesh, 16).unwrap();
        let alu = bar_spec([0.0, 0.0, -100.0], 16);
        let mut steel = alu.clone();
        steel.youngs_modulus_mpa = 200_000.0;
        let a = solve_static(&tm, &alu, &SolveOptions::default()).unwrap();
        let s = solve_static(&tm, &steel, &SolveOptions::default()).unwrap();
        let ratio = a.max_displacement_mm / s.max_displacement_mm;
        assert!((ratio - 200.0 / 69.0).abs() < 1e-6, "ratio {ratio}");
        assert!((a.max_von_mises_mpa - s.max_von_mises_mpa).abs() < 1e-9);
    }

    #[test]
    fn empty_regions_fail_closed() {
        let mesh = box_mesh([0.0; 3], [10.0; 3]);
        let tm = tet_fill(&mesh, 8).unwrap();
        let mut spec = bar_spec([0.0, 0.0, -1.0], 8);
        spec.loads[0].region = RegionBox {
            min: [500.0; 3],
            max: [501.0; 3],
        };
        assert!(matches!(
            solve_static(&tm, &spec, &SolveOptions::default()),
            Err(SolveError::EmptyRegion(_))
        ));
    }
}
