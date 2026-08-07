//! M3 spike: discrete adjoint `d(tip deflection)/d(thickness)` for the
//! static FEA solver, on a cantilever where the answer is known.
//!
//! The parameter is the beam thickness `t`, entering through the **node
//! coordinates of a frozen discretization** (every node's z scales by
//! `t/t0`) — the same frozen-tessellation contract as `vcad-kernel-diff`.
//! Topology never changes; only coordinates carry the derivative.
//!
//! Two checks, both of which can fail:
//!
//! (a) solved tip deflection and extreme-fiber von Mises stress vs the
//!     Timoshenko / bending-theory closed forms, errors printed;
//! (b) `dJ/dt` from the one-extra-solve adjoint (K is symmetric, so the
//!     adjoint solve reuses the same PCG) vs central finite differences
//!     on the same frozen mesh, relative error printed.
//!
//! Exit code is nonzero if any gate fails.
//!
//! The element assembly here mirrors `solve.rs` (constant-strain tets,
//! matrix-free, unit-E solve); the forward solve is cross-checked against
//! `solve_static` compliance to tie the spike to the production path.

use vcad_kernel_fea::mesh::{box_mesh, tet_fill, TetMesh};
use vcad_kernel_fea::solve::{solve_static, SolveOptions};
use vcad_kernel_fea::spec::{FeaSpec, Load, RegionBox, Support};

const L: f64 = 80.0; // mm, along x
const B: f64 = 10.0; // mm, along y
const T0: f64 = 8.0; // mm, along z — the design parameter's base value
const P: f64 = 100.0; // N, applied in -z over the tip face
const E_MOD: f64 = 69_000.0; // MPa (aluminium)
const NU: f64 = 0.33;
// Lattice cells along the longest axis (h = L/RES); override with argv[1].
const RES: usize = 80;
const PCG_TOL: f64 = 1e-12;
const PCG_MAX: usize = 200_000;

struct Elem {
    n: [usize; 4],
    grad: [[f64; 3]; 4],
    vol: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn det3(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

/// Inverse of the 3×3 with rows `m`, given det (same as solve.rs).
fn inv3(m: [[f64; 3]; 3], det: f64) -> [[f64; 3]; 3] {
    let inv_det = 1.0 / det;
    let mut out = [[0.0; 3]; 3];
    for (j, row) in out.iter_mut().enumerate() {
        for (i, slot) in row.iter_mut().enumerate() {
            let (a, b) = ((i + 1) % 3, (i + 2) % 3);
            let (c, d) = ((j + 1) % 3, (j + 2) % 3);
            *slot = (m[a][c] * m[b][d] - m[a][d] * m[b][c]) * inv_det;
        }
    }
    out
}

fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut c = [[0.0; 3]; 3];
    for (i, ci) in c.iter_mut().enumerate() {
        for (j, cij) in ci.iter_mut().enumerate() {
            for (k, bk) in b.iter().enumerate() {
                *cij += a[i][k] * bk[j];
            }
        }
    }
    c
}

fn build_elems(nodes: &[[f64; 3]], tets: &[[u32; 4]]) -> Vec<Elem> {
    tets.iter()
        .map(|t| {
            let p = |i: usize| nodes[t[i] as usize];
            let x0 = p(0);
            let e = [sub(p(1), x0), sub(p(2), x0), sub(p(3), x0)];
            let det = det3(e[0], e[1], e[2]);
            assert!(det > 1e-30, "degenerate tet");
            let inv = inv3(e, det);
            let g1 = [inv[0][0], inv[1][0], inv[2][0]];
            let g2 = [inv[0][1], inv[1][1], inv[2][1]];
            let g3 = [inv[0][2], inv[1][2], inv[2][2]];
            let g0 = [
                -g1[0] - g2[0] - g3[0],
                -g1[1] - g2[1] - g3[1],
                -g1[2] - g2[2] - g3[2],
            ];
            Elem {
                n: [t[0] as usize, t[1] as usize, t[2] as usize, t[3] as usize],
                grad: [g0, g1, g2, g3],
                vol: det / 6.0,
            }
        })
        .collect()
}

/// Directional derivatives (d grad, d vol) of every element under nodal
/// velocity field `vel`: dA⁻¹ = −A⁻¹·Ȧ·A⁻¹, d det = det·tr(A⁻¹·Ȧ).
fn elem_derivs(
    nodes: &[[f64; 3]],
    tets: &[[u32; 4]],
    vel: &[[f64; 3]],
) -> Vec<([[f64; 3]; 4], f64)> {
    tets.iter()
        .map(|t| {
            let p = |i: usize| nodes[t[i] as usize];
            let v = |i: usize| vel[t[i] as usize];
            let x0 = p(0);
            let v0 = v(0);
            let e = [sub(p(1), x0), sub(p(2), x0), sub(p(3), x0)];
            let de = [sub(v(1), v0), sub(v(2), v0), sub(v(3), v0)];
            let det = det3(e[0], e[1], e[2]);
            let inv = inv3(e, det);
            // tr(A⁻¹·Ȧ): (A⁻¹)ᵢₖ (Ȧ)ₖᵢ where Ȧ rows are de.
            let mut tr = 0.0;
            for i in 0..3 {
                for k in 0..3 {
                    tr += inv[i][k] * de[k][i];
                }
            }
            let dvol = det * tr / 6.0;
            // dA⁻¹ = −A⁻¹·Ȧ·A⁻¹ (Ȧ as a row matrix).
            let da = [de[0], de[1], de[2]];
            let m = mat_mul(&inv, &da);
            let dinv = mat_mul(&m, &inv);
            let dinv = [
                [-dinv[0][0], -dinv[0][1], -dinv[0][2]],
                [-dinv[1][0], -dinv[1][1], -dinv[1][2]],
                [-dinv[2][0], -dinv[2][1], -dinv[2][2]],
            ];
            let dg1 = [dinv[0][0], dinv[1][0], dinv[2][0]];
            let dg2 = [dinv[0][1], dinv[1][1], dinv[2][1]];
            let dg3 = [dinv[0][2], dinv[1][2], dinv[2][2]];
            let dg0 = [
                -dg1[0] - dg2[0] - dg3[0],
                -dg1[1] - dg2[1] - dg3[1],
                -dg1[2] - dg2[2] - dg3[2],
            ];
            ([dg0, dg1, dg2, dg3], dvol)
        })
        .collect()
}

fn lame(nu: f64) -> (f64, f64) {
    (
        nu / ((1.0 + nu) * (1.0 - 2.0 * nu)),
        1.0 / (2.0 * (1.0 + nu)),
    )
}

fn matvec(elems: &[Elem], lam: f64, mu: f64, fixed: &[bool], u: &[f64], y: &mut [f64]) {
    y.iter_mut().for_each(|v| *v = 0.0);
    for e in elems {
        let mut h = [[0.0f64; 3]; 3];
        for (i, g) in e.grad.iter().enumerate() {
            let base = 3 * e.n[i];
            for a in 0..3 {
                let ua = u[base + a];
                for b in 0..3 {
                    h[a][b] += ua * g[b];
                }
            }
        }
        let tr = h[0][0] + h[1][1] + h[2][2];
        let mut s = [[0.0f64; 3]; 3];
        for a in 0..3 {
            for b in 0..3 {
                s[a][b] = mu * (h[a][b] + h[b][a]);
            }
            s[a][a] += lam * tr;
        }
        for (i, g) in e.grad.iter().enumerate() {
            let base = 3 * e.n[i];
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
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Jacobi-PCG for K·u = rhs at unit E. Returns (u, iterations, rel residual).
fn pcg(elems: &[Elem], lam: f64, mu: f64, fixed: &[bool], rhs: &[f64]) -> (Vec<f64>, usize, f64) {
    let ndof = rhs.len();
    let mut diag = vec![0.0f64; ndof];
    for e in elems {
        for (i, g) in e.grad.iter().enumerate() {
            let gg = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
            for a in 0..3 {
                diag[3 * e.n[i] + a] += e.vol * (lam * g[a] * g[a] + mu * (g[a] * g[a] + gg));
            }
        }
    }
    for (d, v) in fixed.iter().zip(diag.iter_mut()) {
        if *d || *v == 0.0 {
            *v = 1.0;
        }
    }
    let mut u = vec![0.0f64; ndof];
    let mut r = rhs.to_vec();
    for (d, v) in fixed.iter().zip(r.iter_mut()) {
        if *d {
            *v = 0.0;
        }
    }
    let fnorm = dot(&r, &r).sqrt();
    assert!(fnorm > 0.0, "zero rhs");
    let mut z: Vec<f64> = r.iter().zip(&diag).map(|(r, d)| r / d).collect();
    let mut p = z.clone();
    let mut ap = vec![0.0f64; ndof];
    let mut rz = dot(&r, &z);
    let (mut iters, mut res) = (0usize, 1.0f64);
    for it in 0..PCG_MAX {
        matvec(elems, lam, mu, fixed, &p, &mut ap);
        let pap = dot(&p, &ap);
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..ndof {
            u[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        res = dot(&r, &r).sqrt() / fnorm;
        iters = it + 1;
        if res < PCG_TOL {
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
    (u, iters, res)
}

/// Scale every node's z by `t/T0` (frozen topology; only coordinates move).
fn scaled_nodes(base: &[[f64; 3]], t: f64) -> Vec<[f64; 3]> {
    base.iter().map(|p| [p[0], p[1], p[2] * t / T0]).collect()
}

struct Model<'a> {
    tets: &'a [[u32; 4]],
    fixed: Vec<bool>,
    f: Vec<f64>,
    /// Tip-face selector: J = (g·u)/E = mean tip z-displacement (mm, signed).
    g: Vec<f64>,
}

impl Model<'_> {
    /// Forward solve at thickness `t`; returns (J_phys_mm, u_unit_e).
    fn solve(&self, base_nodes: &[[f64; 3]], t: f64, lam: f64, mu: f64) -> (f64, Vec<f64>) {
        let nodes = scaled_nodes(base_nodes, t);
        let elems = build_elems(&nodes, self.tets);
        let (u, iters, res) = pcg(&elems, lam, mu, &self.fixed, &self.f);
        assert!(
            res < 1e-10,
            "forward PCG stalled: res {res:.3e} after {iters} iters"
        );
        (dot(&self.g, &u) / E_MOD, u)
    }
}

fn main() {
    let res: usize = std::env::args()
        .nth(1)
        .map(|a| a.parse().expect("resolution must be an integer"))
        .unwrap_or(RES);

    // --- Mesh through the production path -------------------------------
    let tri = box_mesh([0.0, 0.0, 0.0], [L, B, T0]);
    let tm: TetMesh = tet_fill(&tri, res).expect("tet fill");
    let nn = tm.nodes.len();
    let ndof = 3 * nn;
    println!(
        "mesh: {} nodes / {} tets, h = {} mm ({} cells through thickness)",
        nn,
        tm.tets.len(),
        tm.h,
        (T0 / tm.h).round()
    );

    let tol = tm.h * 0.25;
    let clamp: Vec<usize> = (0..nn).filter(|&i| tm.nodes[i][0] < tol).collect();
    let tip: Vec<usize> = (0..nn).filter(|&i| tm.nodes[i][0] > L - tol).collect();
    assert!(!clamp.is_empty() && !tip.is_empty());

    let mut fixed = vec![false; ndof];
    for &i in &clamp {
        for a in 0..3 {
            fixed[3 * i + a] = true;
        }
    }
    let mut f = vec![0.0f64; ndof];
    for &i in &tip {
        f[3 * i + 2] = -P / tip.len() as f64;
    }
    let mut g = vec![0.0f64; ndof];
    for &i in &tip {
        g[3 * i + 2] = 1.0 / tip.len() as f64;
    }
    let model = Model {
        tets: &tm.tets,
        fixed,
        f,
        g,
    };
    let (lam, mu) = lame(NU);

    // --- Forward solve + cross-check vs solve_static --------------------
    let (j0, u0) = model.solve(&tm.nodes, T0, lam, mu);
    let deflection = -j0; // load is -z, so J is negative

    let spec = FeaSpec {
        resolution: res,
        youngs_modulus_mpa: E_MOD,
        poisson: NU,
        yield_strength_mpa: None,
        loads: vec![Load {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T0 + 1.0],
            },
            force: [0.0, 0.0, -P],
        }],
        supports: vec![Support {
            region: RegionBox {
                min: [0.0, -1.0, -1.0],
                max: [0.0, B + 1.0, T0 + 1.0],
            },
            fix: [true, true, true],
        }],
    };
    let prod = solve_static(&tm, &spec, &SolveOptions::default()).expect("solve_static");
    let my_compliance = dot(&model.f, &u0) / E_MOD;
    let cross = (my_compliance - prod.compliance_n_mm).abs() / prod.compliance_n_mm;
    println!(
        "cross-check vs solve_static: compliance {:.6e} vs {:.6e} (rel diff {:.2e})",
        my_compliance, prod.compliance_n_mm, cross
    );

    // --- Check (a): deflection + stress vs closed form ------------------
    let i_zz = B * T0.powi(3) / 12.0;
    let g_mod = E_MOD / (2.0 * (1.0 + NU));
    let area = B * T0;
    let d_bend = P * L.powi(3) / (3.0 * E_MOD * i_zz);
    let d_shear = P * L / (5.0 / 6.0 * g_mod * area);
    let d_exact = d_bend + d_shear;
    let err_defl = (deflection - d_exact).abs() / d_exact;
    println!(
        "(a) tip deflection: solved {:.6} mm vs Timoshenko {:.6} mm  -> rel error {:.2}%",
        deflection,
        d_exact,
        err_defl * 100.0
    );

    // Extreme-fiber stress probe at x ≈ L/4, away from the clamped-end
    // singularity: max element von Mises in the slab, compared to bending
    // + transverse-shear theory at that element's centroid.
    let elems0 = build_elems(&tm.nodes, &tm.tets);
    let x_probe = L / 4.0;
    let mut best: Option<(f64, f64)> = None; // (vm_num, vm_exact)
    for e in &elems0 {
        let mut c = [0.0f64; 3];
        for &ni in &e.n {
            for (ca, na) in c.iter_mut().zip(&tm.nodes[ni]) {
                *ca += na * 0.25;
            }
        }
        if (c[0] - x_probe).abs() > tm.h {
            continue;
        }
        // Element von Mises from the unit-E solve (stress is E-independent).
        let mut h = [[0.0f64; 3]; 3];
        for (i, gr) in e.grad.iter().enumerate() {
            let base = 3 * e.n[i];
            for a in 0..3 {
                for b in 0..3 {
                    h[a][b] += u0[base + a] * gr[b];
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
        // Bending theory at the centroid: sigma = M(x)(z-c)/I, parabolic
        // transverse shear tau_xz.
        let zc = c[2] - T0 / 2.0;
        let sigma = P * (L - c[0]) * zc / i_zz;
        let tau = 1.5 * P / area * (1.0 - (zc / (T0 / 2.0)).powi(2));
        let vm_exact = (sigma * sigma + 3.0 * tau * tau).sqrt();
        if best.map(|(v, _)| vm > v).unwrap_or(true) {
            best = Some((vm, vm_exact));
        }
    }
    let (vm_num, vm_exact) = best.expect("probe slab empty");
    let err_vm = (vm_num - vm_exact).abs() / vm_exact;
    println!(
        "(a) extreme-fiber von Mises at x = L/4: solved {:.3} MPa vs theory {:.3} MPa  \
         -> rel error {:.2}%  (element max_vm overall: {:.1} MPa; nominal clamp-line \
         bending stress {:.1} MPa — the excess is the clamped-corner concentration)",
        vm_num,
        vm_exact,
        err_vm * 100.0,
        prod.max_von_mises_mpa,
        P * L * (T0 / 2.0) / i_zz
    );

    // --- Check (b): adjoint vs central FD -------------------------------
    // Adjoint: K·lambda = g (same operator, K symmetric), then
    // dJ/dt = -(lambda · K'(t)·u)/E with K' from the nodal velocity field
    // v_i = d x_i/dt = (0, 0, z_i/T0).
    let vel: Vec<[f64; 3]> = tm.nodes.iter().map(|p| [0.0, 0.0, p[2] / T0]).collect();
    let derivs = elem_derivs(&tm.nodes, &tm.tets, &vel);

    let (lambda, it_a, res_a) = pcg(&elems0, lam, mu, &model.fixed, &model.g);
    assert!(res_a < 1e-10, "adjoint PCG stalled: {res_a:.3e}");
    println!("adjoint solve: {it_a} PCG iterations, rel residual {res_a:.1e}");

    // Contraction lambda · K'u, element by element:
    //   (K'u)_i = vol'·s(H)·g_i + vol·s(H')·g_i + vol·s(H)·g'_i
    let mut dj_unit = 0.0f64; // d(g·u)/dt at unit E
    for (e, (dgrad, dvol)) in elems0.iter().zip(&derivs) {
        let mut h = [[0.0f64; 3]; 3];
        let mut dh = [[0.0f64; 3]; 3];
        for (i, dgi) in dgrad.iter().enumerate() {
            let base = 3 * e.n[i];
            for a in 0..3 {
                let ua = u0[base + a];
                for b in 0..3 {
                    h[a][b] += ua * e.grad[i][b];
                    dh[a][b] += ua * dgi[b];
                }
            }
        }
        let stress = |m: &[[f64; 3]; 3]| {
            let tr = m[0][0] + m[1][1] + m[2][2];
            let mut s = [[0.0f64; 3]; 3];
            for a in 0..3 {
                for b in 0..3 {
                    s[a][b] = mu * (m[a][b] + m[b][a]);
                }
                s[a][a] += lam * tr;
            }
            s
        };
        let s = stress(&h);
        let ds = stress(&dh);
        for (i, dgi) in dgrad.iter().enumerate() {
            let base = 3 * e.n[i];
            if model.fixed[base] {
                continue; // lambda is zero on fully-fixed nodes
            }
            for a in 0..3 {
                let la = lambda[base + a];
                if la == 0.0 {
                    continue;
                }
                let gi = e.grad[i];
                let row = dvol * (s[a][0] * gi[0] + s[a][1] * gi[1] + s[a][2] * gi[2])
                    + e.vol * (ds[a][0] * gi[0] + ds[a][1] * gi[1] + ds[a][2] * gi[2])
                    + e.vol * (s[a][0] * dgi[0] + s[a][1] * dgi[1] + s[a][2] * dgi[2]);
                dj_unit -= la * row;
            }
        }
    }
    let dj_adjoint = dj_unit / E_MOD;

    // Central finite differences on the SAME frozen mesh.
    let dt = 1e-3 * T0;
    let (jp, _) = model.solve(&tm.nodes, T0 + dt, lam, mu);
    let (jm, _) = model.solve(&tm.nodes, T0 - dt, lam, mu);
    let dj_fd = (jp - jm) / (2.0 * dt);
    let err_grad = (dj_adjoint - dj_fd).abs() / dj_fd.abs();
    println!(
        "(b) dJ/dt (J = mean tip z-displacement): adjoint {:.9e} vs central FD {:.9e}  \
         -> rel error {:.2e}",
        dj_adjoint, dj_fd, err_grad
    );
    println!(
        "    context: Euler-Bernoulli predicts d(deflection)/dt = -3*delta_bend/t = {:.4e} \
         (discrete value {:.4e}; gap tracks the deflection discretization error)",
        -3.0 * d_bend / T0,
        -dj_adjoint
    );

    // --- Gates (all can fail) -------------------------------------------
    let mut failed = false;
    let mut gate = |name: &str, ok: bool| {
        println!("gate {name}: {}", if ok { "PASS" } else { "FAIL" });
        failed |= !ok;
    };
    gate("cross-check vs solve_static (< 1e-6)", cross < 1e-6);
    gate("deflection vs closed form (< 10%)", err_defl < 0.10);
    gate("stress vs closed form at L/4 (< 15%)", err_vm < 0.15);
    gate("adjoint vs central FD (< 1e-4)", err_grad < 1e-4);
    if failed {
        std::process::exit(1);
    }
}
