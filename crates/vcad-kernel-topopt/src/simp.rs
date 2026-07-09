//! SIMP density optimization loop (compliance minimization under a volume
//! constraint), in the style of the classic `top88`/`top3d` formulations.

use crate::domain::Domain;
use crate::fea::FeSystem;
use crate::spec::TopoOptSpec;

/// Minimum relative stiffness of a "void" element, keeps K non-singular.
const E_MIN: f64 = 1e-9;
/// Lower density bound (avoids dead sensitivities and division by zero in
/// the filter, exactly as in `top88`).
const X_MIN: f64 = 1e-3;
/// Density move limit per OC update.
const MOVE: f64 = 0.2;

/// Raw optimization output: the converged density field plus diagnostics.
pub struct SimpResult {
    /// One density per grid element (0 for inactive elements).
    pub densities: Vec<f64>,
    /// Compliance after each iteration.
    pub compliance_history: Vec<f64>,
    /// Iterations actually run.
    pub iterations: usize,
    /// Whether the density change dropped below the tolerance.
    pub converged: bool,
}

/// SIMP stiffness scale for a density.
#[inline]
fn stiffness_scale(x: f64, penalty: f64) -> f64 {
    E_MIN + x.powf(penalty) * (1.0 - E_MIN)
}

/// Run the SIMP loop on a prepared FE system.
pub fn optimize_densities(domain: &Domain, sys: &FeSystem, spec: &TopoOptSpec) -> SimpResult {
    let nact = sys.active_elems.len();
    let vf = spec.volume_fraction.clamp(0.01, 0.99);

    // Densities indexed by active-element position.
    let mut x = vec![vf; nact];
    let mut u = vec![0.0f64; sys.ndof];
    let mut history = Vec::with_capacity(spec.max_iterations);

    // Map grid element index -> active index for the filter.
    let mut act_of_elem = vec![u32::MAX; domain.num_elements()];
    for (ai, &e) in sys.active_elems.iter().enumerate() {
        act_of_elem[e as usize] = ai as u32;
    }

    // Solver budget: warm-started PCG needs a full-accuracy solve only on
    // the first iteration; after that the displacement field tracks the
    // slowly-changing densities, so a tight cap keeps large grids tractable
    // (approximate solves are standard practice for OC updates — the
    // sensitivity signs, not their last digits, drive the design).
    let cg_max = (sys.ndof / 2).clamp(500, 4_000);

    let mut converged = false;
    let mut iterations = 0;
    let mut scales = vec![0.0f64; nact];
    let mut dc = vec![0.0f64; nact];

    for it in 0..spec.max_iterations {
        iterations = it + 1;

        for (s, &xi) in scales.iter_mut().zip(&x) {
            *s = stiffness_scale(xi, spec.penalty);
        }
        sys.solve(&scales, &mut u, 1e-5, cg_max);

        // Compliance and element sensitivities.
        let mut compliance = 0.0;
        for (ai, dofs) in sys.edofs.iter().enumerate() {
            let mut ue = [0.0f64; 24];
            for (k, &d) in dofs.iter().enumerate() {
                ue[k] = u[d as usize];
            }
            // ce = ueᵀ · KE · ue (unit-E element strain energy measure).
            let mut ce = 0.0;
            for r in 0..24 {
                let row = &sys.ke[r * 24..r * 24 + 24];
                let mut acc = 0.0;
                for k in 0..24 {
                    acc += row[k] * ue[k];
                }
                ce += ue[r] * acc;
            }
            compliance += scales[ai] * ce;
            dc[ai] = -spec.penalty * x[ai].powf(spec.penalty - 1.0) * (1.0 - E_MIN) * ce;
        }
        history.push(compliance);

        // Mesh-independency (sensitivity) filter, computed on the fly over
        // the voxel neighborhood within `filter_radius`.
        let dcn = filter_sensitivities(domain, &act_of_elem, &x, &dc, spec.filter_radius);

        // Optimality-criteria update with bisection on the volume multiplier.
        let (mut l1, mut l2) = (0.0f64, 1e9f64);
        let mut xnew = vec![0.0f64; nact];
        while l2 - l1 > 1e-4 * (l1 + l2) + 1e-30 {
            let lmid = 0.5 * (l1 + l2);
            let mut vol = 0.0;
            for ai in 0..nact {
                let b = (-dcn[ai]).max(0.0) / lmid;
                let cand = x[ai] * b.sqrt();
                let lo = (x[ai] - MOVE).max(X_MIN);
                let hi = (x[ai] + MOVE).min(1.0);
                let xn = cand.clamp(lo, hi);
                xnew[ai] = xn;
                vol += xn;
            }
            if vol > vf * nact as f64 {
                l1 = lmid;
            } else {
                l2 = lmid;
            }
        }

        let change = x
            .iter()
            .zip(&xnew)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        x.copy_from_slice(&xnew);

        if change < spec.tolerance {
            converged = true;
            break;
        }
    }

    // Expand to full grid indexing.
    let mut densities = vec![0.0f64; domain.num_elements()];
    for (ai, &e) in sys.active_elems.iter().enumerate() {
        densities[e as usize] = x[ai];
    }

    SimpResult {
        densities,
        compliance_history: history,
        iterations,
        converged,
    }
}

/// Classic sensitivity filter: weighted average of `x·dc` over neighbors
/// within `rmin` voxels, normalized by `x_e · Σw`.
fn filter_sensitivities(
    domain: &Domain,
    act_of_elem: &[u32],
    x: &[f64],
    dc: &[f64],
    rmin: f64,
) -> Vec<f64> {
    let nact = x.len();
    if rmin <= 1.0 {
        return dc.to_vec();
    }
    let reach = rmin.ceil() as isize;
    let mut dcn = vec![0.0f64; nact];

    for iz in 0..domain.nz {
        for iy in 0..domain.ny {
            for ix in 0..domain.nx {
                let e = domain.eidx(ix, iy, iz);
                let ai = act_of_elem[e];
                if ai == u32::MAX {
                    continue;
                }
                let ai = ai as usize;
                let mut num = 0.0;
                let mut den = 0.0;
                for dz in -reach..=reach {
                    let jz = iz as isize + dz;
                    if jz < 0 || jz >= domain.nz as isize {
                        continue;
                    }
                    for dy in -reach..=reach {
                        let jy = iy as isize + dy;
                        if jy < 0 || jy >= domain.ny as isize {
                            continue;
                        }
                        for dx in -reach..=reach {
                            let jx = ix as isize + dx;
                            if jx < 0 || jx >= domain.nx as isize {
                                continue;
                            }
                            let dist = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
                            let w = rmin - dist;
                            if w <= 0.0 {
                                continue;
                            }
                            let je = domain.eidx(jx as usize, jy as usize, jz as usize);
                            let aj = act_of_elem[je];
                            if aj == u32::MAX {
                                continue;
                            }
                            let aj = aj as usize;
                            num += w * x[aj] * dc[aj];
                            den += w;
                        }
                    }
                }
                dcn[ai] = num / (den * x[ai].max(X_MIN));
            }
        }
    }
    dcn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Load, RegionBox, Support, TopoOptSpec};

    fn cantilever_spec() -> TopoOptSpec {
        let loads = vec![Load {
            region: RegionBox {
                min: [16.0, 0.0, 0.0],
                max: [16.0, 4.0, 1.0],
            },
            force: [0.0, 0.0, -100.0],
        }];
        let supports = vec![Support {
            region: RegionBox {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 4.0, 8.0],
            },
            fix: [true, true, true],
        }];
        let mut spec = TopoOptSpec::new(loads, supports);
        spec.volume_fraction = 0.4;
        spec.max_iterations = 12;
        spec
    }

    #[test]
    fn cantilever_optimization_improves_and_respects_volume() {
        let domain = Domain::from_bbox([0.0; 3], [16.0, 4.0, 8.0], 16);
        let spec = cantilever_spec();
        let sys = FeSystem::build(&domain, spec.poisson, &spec.loads, &spec.supports).unwrap();
        let result = optimize_densities(&domain, &sys, &spec);

        assert!(result.iterations >= 2);
        let first = result.compliance_history[0];
        let last = *result.compliance_history.last().unwrap();
        assert!(
            last < first,
            "compliance should decrease: {first} -> {last}"
        );

        let nact = domain.num_active() as f64;
        let vol: f64 = result.densities.iter().sum::<f64>() / nact;
        assert!(
            (vol - spec.volume_fraction).abs() < 0.02,
            "volume fraction {vol} vs target {}",
            spec.volume_fraction
        );

        // The design must differentiate: some elements nearly solid, some
        // nearly void.
        let solid = result.densities.iter().filter(|&&d| d > 0.8).count();
        let void = result
            .densities
            .iter()
            .zip(&domain.active)
            .filter(|(&d, &a)| a && d < 0.2)
            .count();
        assert!(solid > 0, "no solid elements formed");
        assert!(void > 0, "no void elements formed");
    }
}
