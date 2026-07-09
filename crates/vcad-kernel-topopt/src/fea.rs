//! Voxel finite-element analysis: 8-node hexahedral elements on a uniform
//! grid, solved matrix-free with Jacobi-preconditioned conjugate gradients.

use crate::domain::Domain;

/// Local node offsets within an element, standard hex ordering.
pub const NODE_OFFSETS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [1, 1, 0],
    [0, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [1, 1, 1],
    [0, 1, 1],
];

/// Natural-coordinate signs per local node (matches [`NODE_OFFSETS`]).
const NODE_SIGNS: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

/// Element stiffness matrix (24×24, row-major) for a cubic 8-node hex of
/// edge length `h`, unit Young's modulus, Poisson's ratio `nu`.
///
/// Computed by 2×2×2 Gauss integration of `Bᵀ C B`.
pub fn hex_stiffness(nu: f64, h: f64) -> Vec<f64> {
    // Isotropic elasticity matrix (Voigt: xx, yy, zz, xy, yz, xz), E = 1.
    let c2 = nu / ((1.0 + nu) * (1.0 - 2.0 * nu));
    let c1 = (1.0 - nu) / ((1.0 + nu) * (1.0 - 2.0 * nu));
    let g = 1.0 / (2.0 * (1.0 + nu));
    let mut c = [[0.0f64; 6]; 6];
    for i in 0..3 {
        for (j, entry) in c[i].iter_mut().enumerate().take(3) {
            *entry = if i == j { c1 } else { c2 };
        }
        c[3 + i][3 + i] = g;
    }

    let gp = 1.0 / 3.0f64.sqrt();
    let det_j = (h / 2.0).powi(3);
    let dnat_dx = 2.0 / h; // d(natural)/d(world)

    let mut ke = vec![0.0f64; 24 * 24];
    for &gx in &[-gp, gp] {
        for &gy in &[-gp, gp] {
            for &gz in &[-gp, gp] {
                // Shape function derivatives in world coordinates.
                let mut dn = [[0.0f64; 3]; 8];
                for (i, s) in NODE_SIGNS.iter().enumerate() {
                    dn[i][0] = s[0] * (1.0 + gy * s[1]) * (1.0 + gz * s[2]) / 8.0 * dnat_dx;
                    dn[i][1] = (1.0 + gx * s[0]) * s[1] * (1.0 + gz * s[2]) / 8.0 * dnat_dx;
                    dn[i][2] = (1.0 + gx * s[0]) * (1.0 + gy * s[1]) * s[2] / 8.0 * dnat_dx;
                }
                // Strain-displacement matrix B (6×24).
                let mut b = [[0.0f64; 24]; 6];
                for i in 0..8 {
                    let (dx, dy, dz) = (dn[i][0], dn[i][1], dn[i][2]);
                    b[0][3 * i] = dx;
                    b[1][3 * i + 1] = dy;
                    b[2][3 * i + 2] = dz;
                    b[3][3 * i] = dy;
                    b[3][3 * i + 1] = dx;
                    b[4][3 * i + 1] = dz;
                    b[4][3 * i + 2] = dy;
                    b[5][3 * i] = dz;
                    b[5][3 * i + 2] = dx;
                }
                // KE += Bᵀ C B · detJ (Gauss weights are 1).
                let mut cb = [[0.0f64; 24]; 6];
                for r in 0..6 {
                    for col in 0..24 {
                        let mut acc = 0.0;
                        for k in 0..6 {
                            acc += c[r][k] * b[k][col];
                        }
                        cb[r][col] = acc;
                    }
                }
                for r in 0..24 {
                    for col in 0..24 {
                        let mut acc = 0.0;
                        for k in 0..6 {
                            acc += b[k][r] * cb[k][col];
                        }
                        ke[r * 24 + col] += acc * det_j;
                    }
                }
            }
        }
    }
    ke
}

/// Assembled FE system context for one design domain.
#[derive(Debug)]
pub struct FeSystem {
    /// Element stiffness for unit E.
    pub ke: Vec<f64>,
    /// Per-element global DOF indices (24 per active element; inactive
    /// elements have an empty placeholder to keep indexing aligned).
    pub edofs: Vec<[u32; 24]>,
    /// Indices of active elements (into the domain's element array).
    pub active_elems: Vec<u32>,
    /// Fixed-DOF mask (includes all DOFs of nodes not touching any
    /// active element, so the system stays non-singular).
    pub fixed: Vec<bool>,
    /// Global load vector.
    pub f: Vec<f64>,
    /// Total DOF count.
    pub ndof: usize,
}

/// Errors from FE system construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeError {
    /// No grid nodes matched a load region.
    LoadOutsideDomain(usize),
    /// No grid nodes matched a support region.
    SupportOutsideDomain(usize),
    /// Every DOF is fixed — nothing to solve.
    FullyConstrained,
    /// The design domain has no active voxels.
    EmptyDomain,
}

impl std::fmt::Display for FeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeError::LoadOutsideDomain(i) => {
                write!(f, "load {i} does not touch any structure node")
            }
            FeError::SupportOutsideDomain(i) => {
                write!(f, "support {i} does not touch any structure node")
            }
            FeError::FullyConstrained => write!(f, "all degrees of freedom are fixed"),
            FeError::EmptyDomain => write!(f, "design domain contains no active voxels"),
        }
    }
}

impl std::error::Error for FeError {}

impl FeSystem {
    /// Build the FE context for `domain` with the given material and
    /// boundary conditions.
    pub fn build(
        domain: &Domain,
        poisson: f64,
        loads: &[crate::spec::Load],
        supports: &[crate::spec::Support],
    ) -> Result<Self, FeError> {
        if domain.num_active() == 0 {
            return Err(FeError::EmptyDomain);
        }
        let ndof = 3 * domain.num_nodes();
        let ke = hex_stiffness(poisson, domain.h);

        // Element → global DOF map for active elements.
        let mut active_elems = Vec::with_capacity(domain.num_active());
        let mut edofs = Vec::with_capacity(domain.num_active());
        let mut attached = vec![false; domain.num_nodes()];
        for iz in 0..domain.nz {
            for iy in 0..domain.ny {
                for ix in 0..domain.nx {
                    let e = domain.eidx(ix, iy, iz);
                    if !domain.active[e] {
                        continue;
                    }
                    let mut dofs = [0u32; 24];
                    for (k, off) in NODE_OFFSETS.iter().enumerate() {
                        let n = domain.nidx(ix + off[0], iy + off[1], iz + off[2]);
                        attached[n] = true;
                        for a in 0..3 {
                            dofs[3 * k + a] = (3 * n + a) as u32;
                        }
                    }
                    active_elems.push(e as u32);
                    edofs.push(dofs);
                }
            }
        }

        // Node-selection pad: half a voxel so thin regions still catch a
        // plane of nodes.
        let pad = domain.h * 0.5 + 1e-9;

        let mut fixed = vec![false; ndof];
        // Detached nodes are not part of the structure; fix them out.
        for (n, att) in attached.iter().enumerate() {
            if !att {
                for a in 0..3 {
                    fixed[3 * n + a] = true;
                }
            }
        }

        for (si, sup) in supports.iter().enumerate() {
            let mut hit = false;
            for iz in 0..=domain.nz {
                for iy in 0..=domain.ny {
                    for ix in 0..=domain.nx {
                        let n = domain.nidx(ix, iy, iz);
                        if !attached[n] {
                            continue;
                        }
                        if sup.region.contains(domain.node_pos(ix, iy, iz), pad) {
                            hit = true;
                            for a in 0..3 {
                                if sup.fix[a] {
                                    fixed[3 * n + a] = true;
                                }
                            }
                        }
                    }
                }
            }
            if !hit {
                return Err(FeError::SupportOutsideDomain(si));
            }
        }

        let mut f = vec![0.0f64; ndof];
        for (li, load) in loads.iter().enumerate() {
            let mut nodes = Vec::new();
            for iz in 0..=domain.nz {
                for iy in 0..=domain.ny {
                    for ix in 0..=domain.nx {
                        let n = domain.nidx(ix, iy, iz);
                        if !attached[n] {
                            continue;
                        }
                        if load.region.contains(domain.node_pos(ix, iy, iz), pad) {
                            nodes.push(n);
                        }
                    }
                }
            }
            if nodes.is_empty() {
                return Err(FeError::LoadOutsideDomain(li));
            }
            let scale = 1.0 / nodes.len() as f64;
            for n in nodes {
                for a in 0..3 {
                    if !fixed[3 * n + a] {
                        f[3 * n + a] += load.force[a] * scale;
                    }
                }
            }
        }

        if fixed.iter().all(|x| *x) {
            return Err(FeError::FullyConstrained);
        }

        Ok(FeSystem {
            ke,
            edofs,
            active_elems,
            fixed,
            f,
            ndof,
        })
    }

    /// `out = K(x) · u`, matrix-free. `scales` holds one stiffness scale
    /// per active element (SIMP-interpolated Young's modulus).
    pub fn apply(&self, scales: &[f64], u: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        let ke = &self.ke;
        for (ei, dofs) in self.edofs.iter().enumerate() {
            let s = scales[ei];
            if s == 0.0 {
                continue;
            }
            let mut ue = [0.0f64; 24];
            for (k, &d) in dofs.iter().enumerate() {
                let d = d as usize;
                ue[k] = if self.fixed[d] { 0.0 } else { u[d] };
            }
            let mut fe = [0.0f64; 24];
            for r in 0..24 {
                let row = &ke[r * 24..r * 24 + 24];
                let mut acc = 0.0;
                for k in 0..24 {
                    acc += row[k] * ue[k];
                }
                fe[r] = acc * s;
            }
            for (k, &d) in dofs.iter().enumerate() {
                let d = d as usize;
                if !self.fixed[d] {
                    out[d] += fe[k];
                }
            }
        }
    }

    /// Assembled diagonal of `K(x)` for Jacobi preconditioning. Fixed DOFs
    /// get 1.0.
    pub fn diagonal(&self, scales: &[f64]) -> Vec<f64> {
        let mut diag = vec![0.0f64; self.ndof];
        for (ei, dofs) in self.edofs.iter().enumerate() {
            let s = scales[ei];
            for (k, &d) in dofs.iter().enumerate() {
                diag[d as usize] += s * self.ke[k * 24 + k];
            }
        }
        for (d, v) in diag.iter_mut().enumerate() {
            if self.fixed[d] || *v <= 0.0 {
                *v = 1.0;
            }
        }
        diag
    }

    /// Solve `K(x) u = f` by Jacobi-PCG, warm-started from `u`.
    ///
    /// Returns the relative residual reached.
    pub fn solve(&self, scales: &[f64], u: &mut [f64], tol: f64, max_iter: usize) -> f64 {
        let n = self.ndof;
        let diag = self.diagonal(scales);
        for (d, ui) in u.iter_mut().enumerate() {
            if self.fixed[d] {
                *ui = 0.0;
            }
        }

        let fnorm = norm(&self.f);
        if fnorm == 0.0 {
            u.fill(0.0);
            return 0.0;
        }

        let mut ku = vec![0.0f64; n];
        self.apply(scales, u, &mut ku);
        let mut r: Vec<f64> = (0..n)
            .map(|d| {
                if self.fixed[d] {
                    0.0
                } else {
                    self.f[d] - ku[d]
                }
            })
            .collect();
        let mut z: Vec<f64> = (0..n).map(|d| r[d] / diag[d]).collect();
        let mut p = z.clone();
        let mut rz: f64 = dot(&r, &z);
        let mut relres = norm(&r) / fnorm;

        let mut q = vec![0.0f64; n];
        for _ in 0..max_iter {
            if relres < tol {
                break;
            }
            self.apply(scales, &p, &mut q);
            let pq = dot(&p, &q);
            if pq <= 0.0 || !pq.is_finite() {
                break; // Lost positive-definiteness; keep best iterate.
            }
            let alpha = rz / pq;
            for d in 0..n {
                u[d] += alpha * p[d];
                r[d] -= alpha * q[d];
            }
            relres = norm(&r) / fnorm;
            for d in 0..n {
                z[d] = r[d] / diag[d];
            }
            let rz_new = dot(&r, &z);
            let beta = rz_new / rz;
            rz = rz_new;
            for d in 0..n {
                p[d] = z[d] + beta * p[d];
            }
        }
        relres
    }
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
    use crate::spec::{Load, RegionBox, Support};

    #[test]
    fn stiffness_is_symmetric() {
        let ke = hex_stiffness(0.3, 2.0);
        for r in 0..24 {
            for c in 0..24 {
                let a = ke[r * 24 + c];
                let b = ke[c * 24 + r];
                assert!((a - b).abs() < 1e-10, "asymmetry at ({r},{c}): {a} vs {b}");
            }
        }
    }

    #[test]
    fn stiffness_annihilates_rigid_translation() {
        let ke = hex_stiffness(0.3, 1.0);
        // Unit translation along each axis produces zero force.
        for axis in 0..3 {
            let mut u = [0.0f64; 24];
            for node in 0..8 {
                u[3 * node + axis] = 1.0;
            }
            for r in 0..24 {
                let f: f64 = (0..24).map(|c| ke[r * 24 + c] * u[c]).sum();
                assert!(f.abs() < 1e-9, "axis {axis} row {r}: residual force {f}");
            }
        }
    }

    #[test]
    fn stiffness_scales_linearly_with_h() {
        let k1 = hex_stiffness(0.3, 1.0);
        let k2 = hex_stiffness(0.3, 2.0);
        for i in 0..(24 * 24) {
            assert!((k2[i] - 2.0 * k1[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn solve_cantilever_deflects_downward() {
        // Small solid cantilever, fixed at x=0, tip load in -z.
        let domain = crate::domain::Domain::from_bbox([0.0; 3], [8.0, 2.0, 2.0], 8);
        let loads = vec![Load {
            region: RegionBox {
                min: [8.0, 0.0, 0.0],
                max: [8.0, 2.0, 2.0],
            },
            force: [0.0, 0.0, -100.0],
        }];
        let supports = vec![Support {
            region: RegionBox {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 2.0, 2.0],
            },
            fix: [true, true, true],
        }];
        let sys = FeSystem::build(&domain, 0.3, &loads, &supports).unwrap();
        let scales = vec![1.0; sys.active_elems.len()];
        let mut u = vec![0.0; sys.ndof];
        let relres = sys.solve(&scales, &mut u, 1e-8, 4000);
        assert!(relres < 1e-6, "PCG did not converge: relres {relres}");

        // Tip nodes should move down; compliance must be positive.
        let tip = domain.nidx(domain.nx, 1, 1);
        assert!(u[3 * tip + 2] < 0.0, "tip did not deflect downward");
        let compliance: f64 = sys.f.iter().zip(&u).map(|(a, b)| a * b).sum();
        assert!(compliance > 0.0);
    }

    #[test]
    fn load_outside_domain_errors() {
        let domain = crate::domain::Domain::from_bbox([0.0; 3], [4.0, 4.0, 4.0], 4);
        let loads = vec![Load {
            region: RegionBox {
                min: [100.0; 3],
                max: [101.0; 3],
            },
            force: [0.0, 0.0, -1.0],
        }];
        let supports = vec![Support {
            region: RegionBox {
                min: [0.0; 3],
                max: [0.0, 4.0, 4.0],
            },
            fix: [true; 3],
        }];
        let err = FeSystem::build(&domain, 0.3, &loads, &supports).unwrap_err();
        assert_eq!(err, FeError::LoadOutsideDomain(0));
    }
}
