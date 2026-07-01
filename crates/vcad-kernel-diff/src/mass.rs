//! Mass properties of a seam mesh, with exact θ-derivatives.
//!
//! Volume, mass, centroid, and the inertia tensor are polynomial surface
//! integrals over the closed mesh (divergence theorem via signed
//! origin-tetrahedra), so evaluating them generically over the scalar type
//! and feeding `Dual<f64>` points (real = position, dual = dx/dθ) yields
//! the quantity *and* its θ-derivative in one pass. This is the first
//! physics-facing QoI family of the differentiable seam: spin-up time,
//! stored rotational energy, and balance objectives are all functions of
//! these tensors.
//!
//! Exact tetrahedron integrals (vertex at the origin, opposite face
//! `(a, b, c)`, signed volume `V = det(a,b,c)/6`):
//!
//! ```text
//! ∫ x_i dV     = V · (a + b + c)_i / 4
//! ∫ x_i x_j dV = (V/20) · (a_i a_j + b_i b_j + c_i c_j + s_i s_j),  s = a+b+c
//! ```

use tang::{Dual, Scalar};

use crate::SeamMesh;

/// Mass properties of a closed mesh (density in mass per mm³; positions in
/// mm). All tensors are row-major symmetric 3×3.
#[derive(Debug, Clone, Copy)]
pub struct MassProperties<S> {
    /// Signed volume (positive for outward winding).
    pub volume: S,
    /// Mass = density · volume.
    pub mass: S,
    /// Centroid.
    pub centroid: tang::Point3<S>,
    /// Inertia tensor about the origin.
    pub inertia_origin: [[S; 3]; 3],
    /// Inertia tensor about the centroid (parallel-axis shift of
    /// `inertia_origin`).
    pub inertia_centroid: [[S; 3]; 3],
}

/// Compute mass properties of a closed, consistently outward-wound mesh,
/// generic over the scalar type. Evaluate with `Dual<f64>` positions for
/// exact θ-derivatives of every field (see
/// [`mass_properties_with_derivative`]), or `Dual<Dual<f64>>` for second
/// derivatives.
///
/// Precondition: the mesh must enclose a nonzero volume — the centroid and
/// the parallel-axis shift divide by it, so a degenerate or inside-out
/// mesh yields NaN/∞ fields rather than an error.
pub fn mass_properties<S: Scalar>(
    positions: &[tang::Point3<S>],
    triangles: &[[u32; 3]],
    density: f64,
) -> MassProperties<S> {
    let mut volume = S::ZERO;
    let mut first = [S::ZERO; 3];
    let mut second = [[S::ZERO; 3]; 3];

    for t in triangles {
        let a = &positions[t[0] as usize];
        let b = &positions[t[1] as usize];
        let c = &positions[t[2] as usize];
        let a = [a.x, a.y, a.z];
        let b = [b.x, b.y, b.z];
        let c = [c.x, c.y, c.z];

        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let v = det / S::from_f64(6.0);
        volume += v;

        let s = [a[0] + b[0] + c[0], a[1] + b[1] + c[1], a[2] + b[2] + c[2]];
        for i in 0..3 {
            first[i] += v * s[i] / S::from_f64(4.0);
        }
        for i in 0..3 {
            for j in 0..3 {
                second[i][j] +=
                    v / S::from_f64(20.0) * (a[i] * a[j] + b[i] * b[j] + c[i] * c[j] + s[i] * s[j]);
            }
        }
    }

    let rho = S::from_f64(density);
    let mass = rho * volume;
    let centroid = tang::Point3::new(first[0] / volume, first[1] / volume, first[2] / volume);

    // I_origin = ρ (tr(P) δ − P), with P the second-moment matrix.
    let trace = second[0][0] + second[1][1] + second[2][2];
    let mut inertia_origin = [[S::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let kronecker = if i == j { trace } else { S::ZERO };
            inertia_origin[i][j] = rho * (kronecker - second[i][j]);
        }
    }

    // Parallel axis: I_centroid = I_origin − m (|d|² δ − d dᵀ).
    let d = [centroid.x, centroid.y, centroid.z];
    let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let mut inertia_centroid = [[S::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let kronecker = if i == j { d2 } else { S::ZERO };
            inertia_centroid[i][j] = inertia_origin[i][j] - mass * (kronecker - d[i] * d[j]);
        }
    }

    MassProperties {
        volume,
        mass,
        centroid,
        inertia_origin,
        inertia_centroid,
    }
}

fn split_dual(props: &MassProperties<Dual<f64>>) -> (MassProperties<f64>, MassProperties<f64>) {
    let split3 = |m: &[[Dual<f64>; 3]; 3]| -> ([[f64; 3]; 3], [[f64; 3]; 3]) {
        let mut real = [[0.0; 3]; 3];
        let mut dual = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                real[i][j] = m[i][j].real;
                dual[i][j] = m[i][j].dual;
            }
        }
        (real, dual)
    };
    let (io_r, io_d) = split3(&props.inertia_origin);
    let (ic_r, ic_d) = split3(&props.inertia_centroid);
    (
        MassProperties {
            volume: props.volume.real,
            mass: props.mass.real,
            centroid: tang::Point3::new(
                props.centroid.x.real,
                props.centroid.y.real,
                props.centroid.z.real,
            ),
            inertia_origin: io_r,
            inertia_centroid: ic_r,
        },
        MassProperties {
            volume: props.volume.dual,
            mass: props.mass.dual,
            centroid: tang::Point3::new(
                props.centroid.x.dual,
                props.centroid.y.dual,
                props.centroid.z.dual,
            ),
            inertia_origin: io_d,
            inertia_centroid: ic_d,
        },
    )
}

/// Mass properties of a seam mesh and their θ-derivatives: node positions
/// and velocities are packed into `Dual<f64>` points and pushed through the
/// generic integrals, so every returned derivative is the exact contraction
/// `Σ_i (∂(·)/∂x_i) · (dx_i/dθ)`.
pub fn mass_properties_with_derivative(
    seam: &SeamMesh,
    density: f64,
) -> (MassProperties<f64>, MassProperties<f64>) {
    let pts: Vec<tang::Point3<Dual<f64>>> = seam
        .positions
        .iter()
        .zip(&seam.velocities)
        .map(|(p, v)| {
            tang::Point3::new(
                Dual::new(p.x, v.x),
                Dual::new(p.y, v.y),
                Dual::new(p.z, v.z),
            )
        })
        .collect();
    let props = mass_properties(&pts, &seam.triangles, density);
    split_dual(&props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Point3;

    /// Closed cuboid mesh [0,w]×[0,l]×[0,h] with outward winding.
    fn cuboid(w: f64, l: f64, h: f64) -> (Vec<Point3>, Vec<[u32; 3]>) {
        let p = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(w, 0.0, 0.0),
            Point3::new(w, l, 0.0),
            Point3::new(0.0, l, 0.0),
            Point3::new(0.0, 0.0, h),
            Point3::new(w, 0.0, h),
            Point3::new(w, l, h),
            Point3::new(0.0, l, h),
        ];
        let t = vec![
            [0, 2, 1],
            [0, 3, 2], // bottom (−z)
            [4, 5, 6],
            [4, 6, 7], // top (+z)
            [0, 1, 5],
            [0, 5, 4], // −y
            [2, 3, 7],
            [2, 7, 6], // +y
            [1, 2, 6],
            [1, 6, 5], // +x
            [3, 0, 4],
            [3, 4, 7], // −x
        ];
        (p, t)
    }

    #[test]
    fn cuboid_closed_forms() {
        let (w, l, h) = (4.0, 3.0, 2.0);
        let rho = 2.5;
        let (p, t) = cuboid(w, l, h);
        let props = mass_properties(&p, &t, rho);

        let m = rho * w * l * h;
        assert!((props.volume - w * l * h).abs() < 1e-12);
        assert!((props.mass - m).abs() < 1e-12);
        assert!((props.centroid.x - w / 2.0).abs() < 1e-12);
        assert!((props.centroid.y - l / 2.0).abs() < 1e-12);
        assert!((props.centroid.z - h / 2.0).abs() < 1e-12);

        // Cuboid inertia about its centroid: I_xx = m(l²+h²)/12, etc.
        let expect = [
            m * (l * l + h * h) / 12.0,
            m * (w * w + h * h) / 12.0,
            m * (w * w + l * l) / 12.0,
        ];
        for (i, e) in expect.iter().enumerate() {
            assert!(
                (props.inertia_centroid[i][i] - e).abs() / e < 1e-12,
                "I[{i}][{i}] = {} vs {e}",
                props.inertia_centroid[i][i]
            );
            for j in 0..3 {
                if i != j {
                    assert!(props.inertia_centroid[i][j].abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn dual_derivative_matches_finite_difference() {
        // Grow the cuboid height: compare dual-carried derivatives of every
        // field against central differences of the f64 evaluation.
        let (w, l) = (4.0, 3.0);
        let rho = 1.0;
        let h0 = 2.0;
        let eps = 1e-6;

        let (p_plus, t) = cuboid(w, l, h0 + eps);
        let (p_minus, _) = cuboid(w, l, h0 - eps);
        let plus = mass_properties(&p_plus, &t, rho);
        let minus = mass_properties(&p_minus, &t, rho);

        let (p0, _) = cuboid(w, l, h0);
        let pts: Vec<tang::Point3<Dual<f64>>> = p0
            .iter()
            .map(|p| {
                // Top-face nodes (z = h0) move at ż = 1; bottom pinned.
                let vz = if p.z > h0 - 1e-9 { 1.0 } else { 0.0 };
                tang::Point3::new(Dual::constant(p.x), Dual::constant(p.y), Dual::new(p.z, vz))
            })
            .collect();
        let props = mass_properties(&pts, &t, rho);

        let check = |dual: f64, fd: f64, what: &str| {
            let scale = fd.abs().max(1.0);
            assert!(
                (dual - fd).abs() / scale < 1e-6,
                "{what}: dual {dual} vs fd {fd}"
            );
        };
        check(
            props.volume.dual,
            (plus.volume - minus.volume) / (2.0 * eps),
            "dV/dh",
        );
        check(
            props.centroid.z.dual,
            (plus.centroid.z - minus.centroid.z) / (2.0 * eps),
            "dcz/dh",
        );
        for i in 0..3 {
            check(
                props.inertia_centroid[i][i].dual,
                (plus.inertia_centroid[i][i] - minus.inertia_centroid[i][i]) / (2.0 * eps),
                "dI/dh",
            );
        }
    }
}
