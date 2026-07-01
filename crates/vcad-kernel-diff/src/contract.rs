//! The physics hook: contract an external functional's mesh-gradient
//! against the seam's sensitivities.
//!
//! A physics engine (phyz) evaluating a functional `J` on the frozen mesh
//! supplies `∂J/∂x_i` per node — from an adjoint rollout, analytic
//! formulas, or its own AD. The shape derivative is then the contraction
//! `dJ/dθ = Σ_i (∂J/∂x_i) · (dx_i/dθ)` with the seam's
//! [`SeamMesh::velocities`]. Nothing else about the physics side needs to
//! know how the geometry was made.

use vcad_kernel_math::{Point3, Vec3};

use crate::SeamMesh;

/// `dJ/dθ = Σ_i (∂J/∂x_i) · (dx_i/dθ)`.
///
/// `dj_dx` must be indexed like the seam's nodes (one gradient per node).
pub fn contract_sensitivity(seam: &SeamMesh, dj_dx: &[Vec3]) -> f64 {
    assert_eq!(
        dj_dx.len(),
        seam.velocities.len(),
        "∂J/∂x must have one entry per seam node"
    );
    dj_dx
        .iter()
        .zip(&seam.velocities)
        .map(|(g, v)| g.dot(*v))
        .sum()
}

/// Analytic per-node gradient of the divergence-theorem volume:
/// `∂V/∂a = (b × c)/6` accumulated over each triangle `(a, b, c)`.
///
/// Doubles as the reference functional for testing the contraction path:
/// `contract_sensitivity(seam, volume_gradient(...))` must equal the dual
/// part of `volume_with_derivative(seam)` to machine precision.
pub fn volume_gradient(positions: &[Point3], triangles: &[[u32; 3]]) -> Vec<Vec3> {
    let mut grad = vec![Vec3::new(0.0, 0.0, 0.0); positions.len()];
    for t in triangles {
        let a = positions[t[0] as usize];
        let b = positions[t[1] as usize];
        let c = positions[t[2] as usize];
        let (va, vb, vc) = (
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, b.y, b.z),
            Vec3::new(c.x, c.y, c.z),
        );
        grad[t[0] as usize] += vb.cross(vc) / 6.0;
        grad[t[1] as usize] += vc.cross(va) / 6.0;
        grad[t[2] as usize] += va.cross(vb) / 6.0;
    }
    grad
}
