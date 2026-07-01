//! Frozen-tessellation mode: reproducible, differentiable meshing for the
//! differentiable seam.
//!
//! A finite-difference derivative `dx/dθ` of tessellated node positions is only
//! meaningful if node `i` at `θ` corresponds to node `i` at `θ ± h` — same
//! connectivity, same `(u, v)` sample pattern, same vertex ordering. A naive
//! re-tessellation reseeds samples and permutes vertices, turning both the
//! analytic derivative and its finite-difference oracle into noise.
//!
//! This module models a mesh as an explicit, **θ-independent** structure:
//!
//! - a list of [`SampleAddr`] node addresses `(surface_index, u, v)` — frozen
//!   parametric coordinates on the surfaces of a [`GeometryStore`];
//! - a fixed triangle connectivity indexing those nodes;
//! - a per-surface [`SurfaceSeed`] describing how each surface moves with `θ`.
//!
//! Only the *surface field values* change with `θ` (via the lift-bridge in
//! [`vcad_kernel_geom::diff`]); the sample pattern and connectivity never do.
//! That is the frozen-topology invariant, made native to the data model.
//!
//! A cheap [`TopoSignature`] (counts + connectivity hash + a θ-sensitive
//! orientation hash) is asserted invariant across the derivative step. If it
//! flips, the perturbation crossed a topology change (a subgradient) and the
//! seam refuses to return a derivative — [`audit`] returns
//! [`TopologyChanged`] rather than silently lying.

use tang::{Dual, Mat3, Scalar, Vec3 as TVec3};
use vcad_kernel_geom::{eval_surface_dual, GeometryStore, SeedMismatch, SurfaceSeed};
use vcad_kernel_math::Point3;

pub mod models;

/// A frozen parametric sample address: a point at fixed `(u, v)` on the surface
/// stored at `surface_index` in a [`GeometryStore`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleAddr {
    /// Index into `GeometryStore::surfaces`.
    pub surface_index: usize,
    /// Frozen `u` parameter.
    pub u: f64,
    /// Frozen `v` parameter.
    pub v: f64,
}

/// A frozen triangle mesh: node addresses + fixed connectivity + per-surface
/// parameter seeds. The structure is θ-independent; evaluating it against a
/// [`GeometryStore`] built at a particular `θ` yields node positions, and via
/// the lift-bridge, their `dx/dθ`.
#[derive(Debug, Clone)]
pub struct FrozenTessellation {
    /// Node addresses, in fixed order. Node `i` is `nodes[i]`.
    pub nodes: Vec<SampleAddr>,
    /// Triangles as triples of node indices, in fixed order.
    pub tris: Vec<[u32; 3]>,
    /// Seed per surface, indexed like `GeometryStore::surfaces`. Says how each
    /// surface's fields move with `θ`.
    pub seeds: Vec<SurfaceSeed>,
}

impl FrozenTessellation {
    /// Evaluate every node's primal `f64` position against the given store.
    /// Node ordering is identical to `self.nodes`.
    pub fn positions(&self, store: &GeometryStore) -> Vec<Point3> {
        self.nodes
            .iter()
            .map(|n| {
                store.surfaces[n.surface_index].evaluate(vcad_kernel_math::Point2::new(n.u, n.v))
            })
            .collect()
    }

    /// Evaluate every node's position **and** `dx/dθ` via the lift-bridge.
    /// The real part is the position; the dual part is the sensitivity.
    pub fn positions_dual(
        &self,
        store: &GeometryStore,
    ) -> Result<Vec<tang::Point3<Dual<f64>>>, SeedMismatch> {
        self.nodes
            .iter()
            .map(|n| {
                eval_surface_dual(
                    store.surfaces[n.surface_index].as_ref(),
                    &self.seeds[n.surface_index],
                    n.u,
                    n.v,
                )
            })
            .collect()
    }

    /// Enclosed volume of the mesh at this store, via signed tetrahedra.
    /// Requires a closed, consistently-wound (outward) mesh.
    pub fn volume(&self, store: &GeometryStore) -> f64 {
        let pts = self.positions(store);
        signed_volume(&pts, &self.tris)
    }

    /// Enclosed volume **and** `dV/dθ` as a dual number: `.real` is the volume,
    /// `.dual` is the exact analytic derivative propagated through the seam.
    pub fn volume_dual(&self, store: &GeometryStore) -> Result<Dual<f64>, SeedMismatch> {
        let pts = self.positions_dual(store)?;
        Ok(signed_volume(&pts, &self.tris))
    }

    /// Compute the topology signature of this mesh at a given store.
    pub fn signature(&self, store: &GeometryStore) -> TopoSignature {
        let pts = self.positions(store);
        TopoSignature::compute(&self.nodes, &self.tris, &pts)
    }
}

/// Enclosed volume via the divergence theorem: `V = (1/6) Σ vᵢ·(vⱼ×vₖ)` over
/// consistently-wound triangles. Generic over the scalar type so it runs on
/// both `f64` (primal / FD) and `Dual<f64>` (analytic derivative).
pub fn signed_volume<S: Scalar>(pts: &[tang::Point3<S>], tris: &[[u32; 3]]) -> S {
    let mut acc = S::ZERO;
    for t in tris {
        let a = pts[t[0] as usize];
        let b = pts[t[1] as usize];
        let c = pts[t[2] as usize];
        let va = TVec3::new(a.x, a.y, a.z);
        let vb = TVec3::new(b.x, b.y, b.z);
        let vc = TVec3::new(c.x, c.y, c.z);
        acc += va.dot(vb.cross(vc));
    }
    acc * S::from_f64(1.0 / 6.0)
}

/// Difference of enclosed volumes between two node-position sets sharing the
/// same connectivity, computed as `Σ (tet(plus) − tet(minus))`. Subtracting
/// matching tetrahedra before summing keeps frozen triangles at exact zero and
/// avoids the catastrophic cancellation of `volume(plus) − volume(minus)`.
pub fn signed_volume_diff(plus: &[Point3], minus: &[Point3], tris: &[[u32; 3]]) -> f64 {
    let tet = |p: &[Point3], t: &[u32; 3]| {
        let a = p[t[0] as usize];
        let b = p[t[1] as usize];
        let c = p[t[2] as usize];
        let va = TVec3::new(a.x, a.y, a.z);
        let vb = TVec3::new(b.x, b.y, b.z);
        let vc = TVec3::new(c.x, c.y, c.z);
        va.dot(vb.cross(vc))
    };
    let mut acc = 0.0_f64;
    for t in tris {
        acc += tet(plus, t) - tet(minus, t);
    }
    acc / 6.0
}

// =============================================================================
// Topology signature
// =============================================================================

/// A cheap signature of mesh topology used to detect topology changes across a
/// derivative step.
///
/// `connectivity_hash` is purely structural (counts + edges) and therefore
/// θ-independent. `orientation_hash` folds the *sign* of each triangle's signed
/// volume contribution and is θ-**sensitive**: a perturbation that inverts or
/// degenerates a triangle (a topology change / subgradient) flips it. Equality
/// of the whole signature across `θ`, `θ+h`, `θ−h` is the line between a correct
/// seam and a plausible wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopoSignature {
    /// Number of vertices (nodes).
    pub n_vertices: usize,
    /// Number of triangles.
    pub n_triangles: usize,
    /// Number of unique undirected edges.
    pub n_edges: usize,
    /// FNV-1a hash of the sorted unique undirected edge list (structural).
    pub connectivity_hash: u64,
    /// FNV-1a hash of per-triangle orientation signs (geometry-sensitive).
    pub orientation_hash: u64,
}

impl TopoSignature {
    fn compute(nodes: &[SampleAddr], tris: &[[u32; 3]], pts: &[Point3]) -> Self {
        // Unique undirected edges.
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(tris.len() * 3);
        for t in tris {
            for &(i, j) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                edges.push(if i < j { (i, j) } else { (j, i) });
            }
        }
        edges.sort_unstable();
        edges.dedup();

        let mut conn = FnvHasher::new();
        for (i, j) in &edges {
            conn.write_u32(*i);
            conn.write_u32(*j);
        }

        // Orientation signs: sign of each triangle's signed-tet contribution.
        let mut orient = FnvHasher::new();
        for t in tris {
            let a = pts[t[0] as usize];
            let b = pts[t[1] as usize];
            let c = pts[t[2] as usize];
            let va = TVec3::new(a.x, a.y, a.z);
            let vb = TVec3::new(b.x, b.y, b.z);
            let vc = TVec3::new(c.x, c.y, c.z);
            let s = va.dot(vb.cross(vc));
            // Robust sign with a tiny dead-band so genuine near-degeneracies
            // (which are the topology changes we want to catch) map to 0.
            let byte: u8 = if s > 1e-9 {
                2
            } else if s < -1e-9 {
                1
            } else {
                0
            };
            orient.write_u8(byte);
        }

        Self {
            n_vertices: nodes.len(),
            n_triangles: tris.len(),
            n_edges: edges.len(),
            connectivity_hash: conn.finish(),
            orientation_hash: orient.finish(),
        }
    }
}

/// Minimal FNV-1a hasher (no external deps, deterministic across runs).
struct FnvHasher(u64);
impl FnvHasher {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    #[inline]
    fn write_u8(&mut self, b: u8) {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(0x100000001b3);
    }
    #[inline]
    fn write_u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

// =============================================================================
// Parametric model + finite-difference audit harness
// =============================================================================

/// A parametric model: a shape whose geometry depends on one scalar `θ`.
///
/// `build(θ)` produces the concrete [`GeometryStore`] at that parameter (this is
/// where every geometric *predicate* is resolved on the primal `θ`, freezing
/// the branch). `tessellation()` returns the θ-independent frozen mesh
/// structure. `theta0()` is the nominal parameter value to differentiate at.
pub trait ParametricModel {
    /// Build the geometry store at parameter `theta`.
    fn build(&self, theta: f64) -> GeometryStore;
    /// The frozen mesh structure (node addresses, connectivity, seeds).
    fn tessellation(&self) -> FrozenTessellation;
    /// Nominal parameter value.
    fn theta0(&self) -> f64;
}

/// Result of a finite-difference audit of a [`ParametricModel`].
#[derive(Debug, Clone)]
pub struct FdReport {
    /// Max relative error, over all nodes and coordinates, between the analytic
    /// `dx/dθ` (dual) and the central-difference oracle.
    pub max_node_rel_err: f64,
    /// Analytic `dV/dθ` (dual).
    pub analytic_dvol: f64,
    /// Central-difference `dV/dθ`.
    pub fd_dvol: f64,
    /// Relative error between the two volume derivatives.
    pub vol_rel_err: f64,
    /// The (invariant) topology signature.
    pub signature: TopoSignature,
}

/// Raised when the topology signature is not invariant across the derivative
/// step — i.e. `θ ± h` crossed a topology change and the frozen seam cannot
/// return a meaningful derivative.
#[derive(Debug, Clone)]
pub struct TopologyChanged {
    /// Signature at `θ`.
    pub center: TopoSignature,
    /// Signature at `θ + h`.
    pub plus: TopoSignature,
    /// Signature at `θ − h`.
    pub minus: TopoSignature,
}

impl std::fmt::Display for TopologyChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "topology signature changed across the derivative step \
             (center={:?}, plus={:?}, minus={:?}); the perturbation crossed a \
             topology change and no frozen derivative is valid",
            self.center, self.plus, self.minus
        )
    }
}

impl std::error::Error for TopologyChanged {}

/// Errors from [`audit`].
#[derive(Debug)]
pub enum AuditError {
    /// The topology signature flipped across the step.
    TopologyChanged(TopologyChanged),
    /// A seed was applied to the wrong surface kind.
    Seed(SeedMismatch),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::TopologyChanged(e) => write!(f, "{e}"),
            AuditError::Seed(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for AuditError {}
impl From<SeedMismatch> for AuditError {
    fn from(e: SeedMismatch) -> Self {
        AuditError::Seed(e)
    }
}

/// The finite-difference oracle. Differentiates the model's node positions and
/// volume both analytically (forward-mode dual through the lift-bridge) and by
/// central differences under the **same frozen seeds**, and asserts the
/// topology signature is invariant across the step.
///
/// Returns an [`AuditError::TopologyChanged`] — never a silent wrong answer —
/// if the perturbation crosses a topology change.
pub fn audit(model: &dyn ParametricModel, h: f64) -> Result<FdReport, AuditError> {
    let theta = model.theta0();
    let tess = model.tessellation();

    let store0 = model.build(theta);
    let store_plus = model.build(theta + h);
    let store_minus = model.build(theta - h);

    // Frozen-topology guard: signature must be invariant across the step.
    let sig0 = tess.signature(&store0);
    let sig_plus = tess.signature(&store_plus);
    let sig_minus = tess.signature(&store_minus);
    if sig0 != sig_plus || sig0 != sig_minus {
        return Err(AuditError::TopologyChanged(TopologyChanged {
            center: sig0,
            plus: sig_plus,
            minus: sig_minus,
        }));
    }

    // Analytic node derivatives.
    let dual = tess.positions_dual(&store0)?;
    // Central-difference node derivatives under identical frozen node ordering.
    let x_plus = tess.positions(&store_plus);
    let x_minus = tess.positions(&store_minus);

    let mut max_rel = 0.0_f64;
    for (i, d) in dual.iter().enumerate() {
        let fd = [
            (x_plus[i].x - x_minus[i].x) / (2.0 * h),
            (x_plus[i].y - x_minus[i].y) / (2.0 * h),
            (x_plus[i].z - x_minus[i].z) / (2.0 * h),
        ];
        let an = [d.x.dual, d.y.dual, d.z.dual];
        for k in 0..3 {
            // Scale-relative error: relative where the derivative is large,
            // absolute-ish near zero.
            let denom = an[k].abs().max(fd[k].abs()).max(1e-9);
            let e = (an[k] - fd[k]).abs() / denom;
            max_rel = max_rel.max(e);
        }
    }

    // Analytic and FD volume derivatives. The FD subtracts matching tetrahedra
    // term-by-term so frozen (r-independent) triangles cancel at full
    // precision — otherwise the central difference of a large summed volume is
    // dominated by f64 cancellation, not by the derivative itself.
    let analytic_dvol = tess.volume_dual(&store0)?.dual;
    let fd_dvol = signed_volume_diff(&x_plus, &x_minus, &tess.tris) / (2.0 * h);
    let vol_rel_err = (analytic_dvol - fd_dvol).abs() / fd_dvol.abs().max(1e-12);

    Ok(FdReport {
        max_node_rel_err: max_rel,
        analytic_dvol,
        fd_dvol,
        vol_rel_err,
        signature: sig0,
    })
}

// =============================================================================
// Pillar 3: implicit differentiation of a defining system
// =============================================================================

/// A 3-equation defining system `F(x; θ) = 0` pinning a single point `x ∈ ℝ³`.
///
/// Implemented generically over the scalar type so the Jacobians `F_x` and
/// `F_θ` can be obtained by forward-mode dual seeding — this differentiates the
/// *equations that define* a trim/intersection point without touching the
/// kernel code that computed it (the Pillar-3 idea).
pub trait DefiningSystem {
    /// Evaluate `F(x; θ)` at a given scalar type.
    fn eval<S: Scalar>(&self, x: [S; 3], theta: S) -> [S; 3];
}

/// Solve `dx/dθ = −F_x⁻¹ F_θ` at the primal solution `x` (which must satisfy
/// `F(x; θ) = 0`). Returns `None` if `F_x` is singular.
///
/// Both Jacobians are formed by forward-mode AD (dual seeding), reusing the
/// existing `tang` machinery rather than any hand-derived derivative.
pub fn implicit_sensitivity<D: DefiningSystem>(
    sys: &D,
    x: [f64; 3],
    theta: f64,
) -> Option<[f64; 3]> {
    let cst = |a: f64| Dual::constant(a);

    // F_θ: seed θ, hold x constant.
    let ft = sys.eval([cst(x[0]), cst(x[1]), cst(x[2])], Dual::var(theta));
    let f_theta = TVec3::new(ft[0].dual, ft[1].dual, ft[2].dual);

    // F_x: seed each x_j in turn; column j is ∂F/∂x_j.
    let mut cols = [TVec3::new(0.0, 0.0, 0.0); 3];
    for (j, col) in cols.iter_mut().enumerate() {
        let mut xs = [cst(x[0]), cst(x[1]), cst(x[2])];
        xs[j] = Dual::var(x[j]);
        let f = sys.eval(xs, cst(theta));
        *col = TVec3::new(f[0].dual, f[1].dual, f[2].dual);
    }
    let fx = Mat3::from_cols(cols[0], cols[1], cols[2]);
    let inv = fx.try_inverse()?;
    let dxdt = inv * (f_theta * -1.0);
    Some([dxdt.x, dxdt.y, dxdt.z])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_rim_sensitivity_matches_analytic_and_fd() {
        // Rim point of a through-hole: on plane z=t, on cylinder radius r,
        // pinned to angular direction φ. dx/dr = (cosφ, sinφ, 0).
        struct Rim {
            t: f64,
            phi: f64,
        }
        impl DefiningSystem for Rim {
            fn eval<S: Scalar>(&self, x: [S; 3], r: S) -> [S; 3] {
                let t = S::from_f64(self.t);
                let (sphi, cphi) = (S::from_f64(self.phi.sin()), S::from_f64(self.phi.cos()));
                [
                    x[2] - t,                          // on plane z=t
                    x[0] * x[0] + x[1] * x[1] - r * r, // on cylinder r
                    x[0] * sphi - x[1] * cphi,         // angular pin φ
                ]
            }
        }

        let (r, t, phi) = (5.0, 3.0, 0.7);
        let sys = Rim { t, phi };
        let x = [r * phi.cos(), r * phi.sin(), t];
        let dxdt = implicit_sensitivity(&sys, x, r).unwrap();

        // Analytic.
        assert!((dxdt[0] - phi.cos()).abs() < 1e-9);
        assert!((dxdt[1] - phi.sin()).abs() < 1e-9);
        assert!(dxdt[2].abs() < 1e-12);

        // FD: re-solve the rim point at r±h (closed form here).
        let h = 1e-6;
        let solve = |r: f64| [r * phi.cos(), r * phi.sin(), t];
        let xp = solve(r + h);
        let xm = solve(r - h);
        for k in 0..3 {
            let fd = (xp[k] - xm[k]) / (2.0 * h);
            assert!((dxdt[k] - fd).abs() < 1e-6, "k={k}: {} vs {}", dxdt[k], fd);
        }
    }

    #[test]
    fn signed_volume_of_unit_cube_is_one() {
        // Build a 1×1×1 cube mesh directly and check volume.
        let m = models::ExtrudedBox::new([1.0, 1.0], 1.0);
        let store = m.build(1.0);
        let tess = m.tessellation();
        let v = tess.volume(&store);
        assert!((v - 1.0).abs() < 1e-12, "cube volume {v}");
    }
}
