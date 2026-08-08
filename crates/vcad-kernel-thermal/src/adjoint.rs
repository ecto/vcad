//! Adjoint gradients of a smoothed peak temperature.
//!
//! The steady conduction operator A is **symmetric — literally
//! self-adjoint** — so the gradient of any scalar objective J(T) with
//! A·T = b costs exactly one extra linear solve with the *same* operator
//! (the same trick the particle crate uses for its Poisson adjoint):
//!
//! ```text
//! A·λ = ∂J/∂T,     dJ/dθ = λᵀ·(∂b/∂θ − ∂A/∂θ·T)
//! ```
//!
//! **The objective.** A hard max is non-differentiable — at a tie the
//! gradient jumps between voxels, and any optimizer chasing it chatters.
//! We smooth it as a p-norm of the temperature *excess* over the
//! reference:
//!
//! ```text
//! J = T_ref + ( Σ_v (T_v − T_ref)₊ᵖ )^(1/p)
//! ```
//!
//! over **free** voxels (pinned reservoirs are boundary data, not design
//! outcomes). The exponent p (default 16) is a documented trade: the
//! smoothed value brackets the hard max as
//!
//! ```text
//! max ≤ J − T_ref ≤ max · N_active^(1/p)
//! ```
//!
//! (N_active = voxels with positive excess; at p = 16 and N = 10⁵ the
//! upper factor is ~2.05), and larger p sharpens the bracket while
//! stiffening the gradient onto fewer voxels. Both the smoothed value and
//! the hard max are reported so the bracket is checkable. The positive
//! part means sub-reference voxels contribute nothing — with a single
//! ambient and non-negative sources the maximum principle makes that
//! vacuous; with cold reservoirs it is the stated behavior. Powers are
//! computed on excess normalized by its maximum, so no overflow at any p.
//!
//! **Parameters.** Gradients flow to per-region conductivity (per axis,
//! through the harmonic-face chain rule), per-slot film coefficients, and
//! per-source powers — everything that changes A or b *without moving the
//! material mask*. Geometry parameters move the discrete mask and are
//! deliberately not smoothed over: they stay finite-difference until a
//! shape-adjoint milestone, as in the particle crate.
//!
//! **Validation** (tests below) follows the frozen-discretization lesson:
//! k, h, and P perturbations never re-voxelize (the mask depends only on
//! shapes), so central differences on the same grid are a clean probe;
//! the CG tolerance is tightened to 1e-12 in the tests so solver stopping
//! noise sits far below the FD resolution.

use crate::model::{Boundary, ThermalModel};
use crate::solve::{
    assemble_solution, build, pcg, resolve_reference, LinkKind, Solution, SolveError, SolveOptions,
    EXPOSED_SLOT,
};
use serde::Serialize;

/// Options for [`smooth_max_gradient`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectiveOptions {
    /// Smooth-max exponent p (> 1). Larger p tightens the bracket around
    /// the hard max and concentrates the gradient on the hottest voxels.
    pub p: f64,
    /// Reference temperature for the excess. `None` uses the model's θ
    /// reference (explicit `reference_c` or the unique convection
    /// ambient); with neither resolvable this fails closed.
    pub reference_c: Option<f64>,
}

impl Default for ObjectiveOptions {
    fn default() -> Self {
        Self {
            p: 16.0,
            reference_c: None,
        }
    }
}

/// The objective value and its gradients.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SmoothMaxGradient {
    /// Smoothed peak temperature J, °C.
    pub value_c: f64,
    /// Hard max over free voxels, °C — the lower edge of the bracket.
    pub hard_max_c: f64,
    /// Reference the excess is measured from, °C.
    pub reference_c: f64,
    /// Voxels with positive excess (the upper bracket factor is
    /// `n_active^(1/p)`).
    pub n_active: usize,
    /// Exponent used.
    pub p: f64,
    /// dJ/d(power) per source, K/W, in `ThermalModel::sources` order.
    pub d_source_power: Vec<f64>,
    /// dJ/d(k_axis) per material region, (K·m·K)/W per axis, in
    /// `ThermalModel::materials` order.
    pub d_conductivity: Vec<[f64; 3]>,
    /// dJ/dh per BC slot: indices 0..=5 are the domain faces
    /// (`face_index` order), 6 is the exposed rule. `None` where the slot
    /// is not a convection BC.
    pub d_film: [Option<f64>; 7],
    /// dJ/d(ambient temperature) per BC slot, K/K, same slot indexing as
    /// [`Self::d_film`]. `None` where the slot is not a convection BC.
    ///
    /// This is the *other* half of a convection boundary. On its own it
    /// looks redundant — of course a hotter ambient makes a hotter part —
    /// but it is the term a conjugate coupling needs: the fluid hands the
    /// solid both a film coefficient and a bulk temperature, and a
    /// coupled gradient that differentiates only the film is missing half
    /// the channel.
    pub d_ambient: [Option<f64>; 7],
    /// CG iterations of the forward solve.
    pub forward_iterations: usize,
    /// CG iterations of the adjoint solve.
    pub adjoint_iterations: usize,
}

/// Solve the steady problem and the adjoint, returning the solution and
/// the gradient of the smoothed peak temperature.
pub fn smooth_max_gradient(
    model: &ThermalModel,
    opts: &SolveOptions,
    oopts: &ObjectiveOptions,
) -> Result<(Solution, SmoothMaxGradient), SolveError> {
    if !oopts.p.is_finite() || oopts.p <= 1.0 {
        return Err(SolveError::InvalidSmoothingExponent);
    }
    let sys = build(model)?;
    let model_ref = resolve_reference(&sys, model)?;
    let reference_c = match oopts.reference_c.or(model_ref) {
        Some(r) => r,
        None => return Err(SolveError::AmbiguousReference),
    };

    let nvox = sys.b.len();
    let zeros = vec![0.0_f64; nvox];
    let (t, forward_iterations, res_fwd) = pcg(&sys, &sys.diag, &sys.b, &zeros, opts)?;

    // Objective: p-norm of the positive excess, max-normalized so
    // arbitrary p never overflows.
    let p = oopts.p;
    let mut hard_max = f64::NEG_INFINITY;
    for (q, &tv) in t.iter().enumerate() {
        if sys.free[q] && tv > hard_max {
            hard_max = tv;
        }
    }
    let m_excess = (hard_max - reference_c).max(0.0);
    let mut n_active = 0usize;
    let mut s = 0.0_f64;
    if m_excess > 0.0 {
        for (q, &tv) in t.iter().enumerate() {
            if sys.free[q] {
                let x = (tv - reference_c).max(0.0);
                if x > 0.0 {
                    n_active += 1;
                    s += (x / m_excess).powf(p);
                }
            }
        }
    }

    if n_active == 0 {
        // Nothing above reference: J = T_ref exactly, gradient zero.
        let solution = assemble_solution(&sys, model, &t, forward_iterations, res_fwd, model_ref);
        let grad = SmoothMaxGradient {
            value_c: reference_c,
            hard_max_c: hard_max,
            reference_c,
            n_active: 0,
            p,
            d_source_power: vec![0.0; model.sources.len()],
            d_conductivity: vec![[0.0; 3]; model.materials.len()],
            d_film: film_slots(model, |_| 0.0),
            d_ambient: film_slots(model, |_| 0.0),
            forward_iterations,
            adjoint_iterations: 0,
        };
        return Ok((solution, grad));
    }

    let j_excess = m_excess * s.powf(1.0 / p);
    let value_c = reference_c + j_excess;

    // ∂J/∂T_v = (x_v / J_excess)^(p−1), zero on non-positive excess.
    let mut g = vec![0.0_f64; nvox];
    for (q, gq) in g.iter_mut().enumerate() {
        if sys.free[q] {
            let x = (t[q] - reference_c).max(0.0);
            if x > 0.0 {
                *gq = (x / j_excess).powf(p - 1.0);
            }
        }
    }

    // One adjoint solve with the SAME operator (A = Aᵀ).
    let (lam, adjoint_iterations, _res_adj) = pcg(&sys, &sys.diag, &g, &zeros, opts)?;

    // Temperature of any solid voxel (free → solved, pinned → reservoir).
    let t_of = |q: usize| if sys.free[q] { t[q] } else { sys.tfix[q] };
    let lam_of = |q: usize| if sys.free[q] { lam[q] } else { 0.0 };

    // dJ/dP per source: b carries share 1/N per covered voxel.
    let d_source_power = sys
        .sources
        .iter()
        .map(|(_, _, ids)| ids.iter().map(|&q| lam[q]).sum::<f64>() / ids.len() as f64)
        .collect::<Vec<_>>();

    // Conductivity gradients: internal pair faces (both orientations of
    // the harmonic mean) plus the half-cells of every boundary link.
    // For a link of conductance G between row values (T_a − T_b), the
    // residual derivative gives dJ/dG = −(λ_a − λ_b)·(T_a − T_b).
    let mut d_conductivity = vec![[0.0_f64; 3]; model.materials.len()];
    let [nx, ny, nz] = sys.n;
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let a = idx(i, j, k);
                if !sys.solid[a] {
                    continue;
                }
                for (axis, kf) in sys.kfield.iter().enumerate() {
                    let (ii, jj, kk) = match axis {
                        0 => (i + 1, j, k),
                        1 => (i, j + 1, k),
                        _ => (i, j, k + 1),
                    };
                    if ii >= nx || jj >= ny || kk >= nz {
                        continue;
                    }
                    let bvox = idx(ii, jj, kk);
                    if !sys.solid[bvox] || (!sys.free[a] && !sys.free[bvox]) {
                        continue;
                    }
                    let ka = kf[a];
                    let kb = kf[bvox];
                    let d = sys.d_m[axis];
                    let denom = 0.5 * d / ka + 0.5 * d / kb;
                    let dj_dg = -(lam_of(a) - lam_of(bvox)) * (t_of(a) - t_of(bvox));
                    let common = sys.area[axis] / (denom * denom) * 0.5 * d;
                    d_conductivity[sys.mat_id[a]][axis] += dj_dg * common / (ka * ka);
                    d_conductivity[sys.mat_id[bvox]][axis] += dj_dg * common / (kb * kb);
                }
            }
        }
    }
    // Boundary links: half-cell conductivity, film coefficients, and the
    // reference temperatures those films are measured against.
    let mut d_film_acc = [0.0_f64; 7];
    let mut d_ambient_acc = [0.0_f64; 7];
    for l in &sys.links {
        let dj_dg = -lam[l.voxel] * (t[l.voxel] - l.t_ref);
        let d = sys.d_m[l.axis];
        let area = sys.area[l.axis];
        let kv = sys.kfield[l.axis][l.voxel];
        match l.kind {
            LinkKind::FixedRegion(_) => {
                // Voxel↔voxel harmonic face (already covered by the pair
                // sweep above — FixedRegion links duplicate a solid pair,
                // so skip to avoid double counting).
                let _ = kv;
            }
            LinkKind::FixedFace => {
                let denom = 0.5 * d / kv;
                d_conductivity[sys.mat_id[l.voxel]][l.axis] +=
                    dj_dg * area / (denom * denom) * 0.5 * d / (kv * kv);
            }
            LinkKind::Convection(slot) => {
                let h = slot_h(model, slot);
                let denom = 0.5 * d / kv + 1.0 / h;
                let dd = area / (denom * denom);
                d_conductivity[sys.mat_id[l.voxel]][l.axis] += dj_dg * dd * 0.5 * d / (kv * kv);
                d_film_acc[slot] += dj_dg * dd / (h * h);
                // b carries G·T_ambient on this row, so ∂r_v/∂T_amb = −G
                // and dJ/dT_amb = −λᵀ ∂r/∂T_amb = +λ_v·G.
                d_ambient_acc[slot] += lam[l.voxel] * l.g;
            }
        }
    }
    let d_film = film_slots(model, |slot| d_film_acc[slot]);
    let d_ambient = film_slots(model, |slot| d_ambient_acc[slot]);

    let solution = assemble_solution(&sys, model, &t, forward_iterations, res_fwd, model_ref);
    Ok((
        solution,
        SmoothMaxGradient {
            value_c,
            hard_max_c: hard_max,
            reference_c,
            n_active,
            p,
            d_source_power,
            d_conductivity,
            d_film,
            d_ambient,
            forward_iterations,
            adjoint_iterations,
        },
    ))
}

fn slot_bc(model: &ThermalModel, slot: usize) -> Boundary {
    if slot == EXPOSED_SLOT {
        model.exposed
    } else {
        model.domain_faces[slot]
    }
}

fn slot_h(model: &ThermalModel, slot: usize) -> f64 {
    match slot_bc(model, slot) {
        Boundary::Convection { h_w_m2k, .. } => h_w_m2k,
        _ => unreachable!("convection link from a non-convection slot"),
    }
}

fn film_slots(model: &ThermalModel, value: impl Fn(usize) -> f64) -> [Option<f64>; 7] {
    let mut out = [None; 7];
    for (slot, o) in out.iter_mut().enumerate() {
        if matches!(slot_bc(model, slot), Boundary::Convection { .. }) {
            *o = Some(value(slot));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Boundary, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
    };
    use crate::solve::solve_steady;

    /// Two-material board: an aluminum-ish spreader inset in an FR4-ish
    /// plate, one 3 W source, convection top and bottom, plus a cold
    /// reservoir strip — every gradient path (k per region, h per slot,
    /// source power) is live.
    fn test_model() -> ThermalModel {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [40.0, 40.0, 4.0], [20, 20, 2]);
        m.materials.push(
            MaterialRegion::anisotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [40.0, 40.0, 4.0],
                },
                [2.0, 2.0, 0.8],
            )
            .with_heat_capacity(1.8e6),
        );
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [12.0, 12.0, 0.0],
                    size_mm: [16.0, 16.0, 4.0],
                },
                160.0,
            )
            .with_heat_capacity(2.4e6),
        );
        m.sources.push(PowerSource {
            name: "die".into(),
            shape: Shape::Box {
                min_mm: [16.0, 16.0, 0.0],
                size_mm: [8.0, 8.0, 4.0],
            },
            power_w: 3.0,
        });
        m.fixed.push(FixedTemperature {
            shape: Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [4.0, 40.0, 4.0],
            },
            temperature_c: 20.0,
        });
        let conv = Boundary::Convection {
            h_w_m2k: 12.0,
            ambient_c: 25.0,
        };
        m.domain_faces[4] = conv;
        m.domain_faces[5] = Boundary::Convection {
            h_w_m2k: 8.0,
            ambient_c: 25.0,
        };
        m.reference_c = Some(25.0);
        m
    }

    /// Tight solver tolerance for FD probes: stopping noise must sit far
    /// below the FD resolution (the particle-crate lesson).
    fn tight() -> SolveOptions {
        SolveOptions {
            tol: 1e-12,
            max_iters: 200_000,
        }
    }

    fn objective(model: &ThermalModel) -> f64 {
        smooth_max_gradient(model, &tight(), &ObjectiveOptions::default())
            .unwrap()
            .1
            .value_c
    }

    #[test]
    fn value_brackets_the_hard_max() {
        let m = test_model();
        let (sol, grad) = smooth_max_gradient(&m, &tight(), &ObjectiveOptions::default()).unwrap();
        assert!(grad.hard_max_c > 25.0);
        let excess = grad.value_c - grad.reference_c;
        let hard = grad.hard_max_c - grad.reference_c;
        assert!(
            excess >= hard - 1e-9,
            "smooth value below hard max: {excess} < {hard}"
        );
        let upper = hard * (grad.n_active as f64).powf(1.0 / grad.p);
        assert!(
            excess <= upper + 1e-9,
            "smooth value above bracket: {excess} > {upper}"
        );
        // The forward solution is the plain steady solution.
        let steady = solve_steady(&m, &tight()).unwrap();
        assert!((sol.t_max_c - steady.t_max_c).abs() < 1e-9);
    }

    #[test]
    fn adjoint_matches_finite_differences() {
        let m = test_model();
        let (_, grad) = smooth_max_gradient(&m, &tight(), &ObjectiveOptions::default()).unwrap();

        // Source power (J is nonlinear in T but T is linear in P; central
        // FD at 1% converges fast).
        let hp = 0.03;
        let fd_p = {
            let mut up = m.clone();
            up.sources[0].power_w += hp;
            let mut dn = m.clone();
            dn.sources[0].power_w -= hp;
            (objective(&up) - objective(&dn)) / (2.0 * hp)
        };
        let rel = (grad.d_source_power[0] - fd_p).abs() / fd_p.abs();
        assert!(
            rel < 1e-6,
            "dJ/dP: adjoint {:.9e}, fd {fd_p:.9e} (rel {rel:.3e})",
            grad.d_source_power[0]
        );

        // Conductivity, both regions, mixed axes. The mask never moves —
        // k perturbations don't re-voxelize — so the discretization is
        // frozen across probes by construction.
        for (region, axis, h) in [(0usize, 0usize, 0.002), (0, 2, 0.001), (1, 1, 0.2)] {
            let fd_k = {
                let mut up = m.clone();
                up.materials[region].k_w_mk[axis] += h;
                let mut dn = m.clone();
                dn.materials[region].k_w_mk[axis] -= h;
                (objective(&up) - objective(&dn)) / (2.0 * h)
            };
            let adj = grad.d_conductivity[region][axis];
            let rel = (adj - fd_k).abs() / fd_k.abs().max(1e-12);
            assert!(
                rel < 2e-4,
                "dJ/dk region {region} axis {axis}: adjoint {adj:.9e}, fd {fd_k:.9e} (rel {rel:.3e})"
            );
        }

        // Film coefficients, both convection slots.
        for (slot, h) in [(4usize, 0.01), (5, 0.01)] {
            let fd_h = {
                let mut up = m.clone();
                let mut dn = m.clone();
                if let Boundary::Convection { h_w_m2k, .. } = &mut up.domain_faces[slot] {
                    *h_w_m2k += h;
                }
                if let Boundary::Convection { h_w_m2k, .. } = &mut dn.domain_faces[slot] {
                    *h_w_m2k -= h;
                }
                (objective(&up) - objective(&dn)) / (2.0 * h)
            };
            let adj = grad.d_film[slot].expect("convection slot");
            let rel = (adj - fd_h).abs() / fd_h.abs().max(1e-12);
            assert!(
                rel < 1e-5,
                "dJ/dh slot {slot}: adjoint {adj:.9e}, fd {fd_h:.9e} (rel {rel:.3e})"
            );
        }

        // Ambient temperatures, both convection slots — the other half of
        // a convection boundary, and the half a conjugate coupling needs.
        for (slot, h) in [(4usize, 0.05), (5, 0.05)] {
            let fd_a = {
                let mut up = m.clone();
                let mut dn = m.clone();
                if let Boundary::Convection { ambient_c, .. } = &mut up.domain_faces[slot] {
                    *ambient_c += h;
                }
                if let Boundary::Convection { ambient_c, .. } = &mut dn.domain_faces[slot] {
                    *ambient_c -= h;
                }
                (objective(&up) - objective(&dn)) / (2.0 * h)
            };
            let adj = grad.d_ambient[slot].expect("convection slot");
            let rel = (adj - fd_a).abs() / fd_a.abs().max(1e-12);
            assert!(
                rel < 1e-5,
                "dJ/dT_amb slot {slot}: adjoint {adj:.9e}, fd {fd_a:.9e} (rel {rel:.3e})"
            );
        }
    }

    /// Raising *every* ambient together must agree with the sum of the
    /// per-slot ambient gradients — the composite check the per-slot
    /// finite differences cannot give, since it exercises all six slots
    /// at once.
    ///
    /// Note the sum is deliberately **not** 1. A uniform ambient shift
    /// translates the whole field by one degree, but the objective
    /// measures excess over a *fixed* reference, and the p-norm's weights
    /// `(x_v/J_ex)^(p−1)` sum to more than 1 whenever several voxels are
    /// active. The sum is that weight total, which is a property of the
    /// smoothing, not of the boundary condition.
    #[test]
    fn ambient_gradients_compose_across_slots() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [20.0, 20.0, 4.0], [10, 10, 2]);
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [20.0, 20.0, 4.0],
            },
            5.0,
        ));
        m.sources.push(PowerSource {
            name: "die".into(),
            shape: Shape::Box {
                min_mm: [8.0, 8.0, 0.0],
                size_mm: [4.0, 4.0, 4.0],
            },
            power_w: 1.0,
        });
        // Every domain face convects to the same ambient; no fixed
        // reservoirs, so the ambient is the only thing anchoring the field.
        for f in 0..6 {
            m.domain_faces[f] = Boundary::Convection {
                h_w_m2k: 15.0,
                ambient_c: 25.0,
            };
        }
        m.reference_c = Some(25.0);
        let (_, grad) = smooth_max_gradient(&m, &tight(), &ObjectiveOptions::default()).unwrap();
        let total: f64 = grad.d_ambient.iter().flatten().sum();

        // Central FD on a simultaneous shift of all six ambients.
        let shift = |d: f64| {
            let mut mm = m.clone();
            for f in 0..6 {
                if let Boundary::Convection { ambient_c, .. } = &mut mm.domain_faces[f] {
                    *ambient_c += d;
                }
            }
            objective(&mm)
        };
        let hstep = 0.05;
        let fd = (shift(hstep) - shift(-hstep)) / (2.0 * hstep);
        let rel = (total - fd).abs() / fd.abs();
        assert!(
            rel < 1e-5,
            "summed ambient gradient {total:.9e} vs uniform-shift fd {fd:.9e} (rel {rel:.3e})"
        );
        // Several voxels are active here, so the weight total exceeds 1.
        assert!(
            total > 1.0,
            "expected a p-norm weight total above 1, got {total}"
        );
    }

    #[test]
    fn gradient_signs_follow_the_physics() {
        let m = test_model();
        let (_, grad) = smooth_max_gradient(&m, &tight(), &ObjectiveOptions::default()).unwrap();
        // More power → hotter.
        assert!(grad.d_source_power[0] > 0.0);
        // Better spreader → cooler.
        for axis in 0..3 {
            assert!(
                grad.d_conductivity[1][axis] < 0.0,
                "spreader axis {axis}: {:?}",
                grad.d_conductivity[1]
            );
        }
        // More film → cooler, on both faces.
        assert!(grad.d_film[4].unwrap() < 0.0);
        assert!(grad.d_film[5].unwrap() < 0.0);
        // Non-convection slots carry no gradient.
        assert!(grad.d_film[0].is_none());
        assert!(grad.d_film[EXPOSED_SLOT].is_none());
    }

    #[test]
    fn nothing_above_reference_means_zero_gradient() {
        // A sourceless block convecting at 25 °C, measured against a
        // 30 °C reference: everything sits strictly below reference, the
        // positive part clips every excess to zero, and J = T_ref with a
        // zero gradient — stated behavior, not an error. (Measuring
        // against the ambient itself is the degenerate T ≡ T_ref case,
        // where CG rounding legitimately leaves ~1e-13 K excesses.)
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [10.0, 10.0, 10.0], [4, 4, 4]);
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [10.0, 10.0, 10.0],
            },
            10.0,
        ));
        m.domain_faces[0] = Boundary::Convection {
            h_w_m2k: 10.0,
            ambient_c: 25.0,
        };
        let (_, grad) = smooth_max_gradient(
            &m,
            &tight(),
            &ObjectiveOptions {
                p: 16.0,
                reference_c: Some(30.0),
            },
        )
        .unwrap();
        assert_eq!(grad.n_active, 0);
        assert_eq!(grad.value_c, 30.0);
        assert!(grad.d_conductivity[0].iter().all(|&d| d == 0.0));
    }

    #[test]
    fn bad_exponent_fails_closed() {
        let m = test_model();
        for p in [1.0, 0.5, f64::NAN, f64::INFINITY] {
            let err = smooth_max_gradient(
                &m,
                &tight(),
                &ObjectiveOptions {
                    p,
                    reference_c: None,
                },
            )
            .unwrap_err();
            assert!(matches!(err, SolveError::InvalidSmoothingExponent));
        }
    }
}
