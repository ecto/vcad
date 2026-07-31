//! Combined field sampling: interpolated E from the Poisson grid, analytic
//! B from every current-carrying ring (exact off-axis loop field via
//! complete elliptic integrals).

use crate::constants::MU_0;
use crate::device::Device;
use crate::elliptic::ellip_ke;
use crate::poisson::Solution;

/// A circular current loop, SI units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingCoil {
    /// Loop radius, m.
    pub radius_m: f64,
    /// Axial position, m.
    pub z_m: f64,
    /// Circulating current, ampere-turns.
    pub ampere_turns: f64,
    /// Conductor (minor) radius, m — used to regularize the field inside
    /// the conductor, where the vacuum formula diverges.
    pub wire_radius_m: f64,
}

/// Magnetic field `(B_r, B_z)` of a single loop at `(r, z)`, tesla.
///
/// Uses the standard elliptic-integral solution. Inside the conductor
/// (distance to the wire centerline < wire radius) the field is evaluated
/// at the conductor surface and scaled linearly toward zero at the
/// centerline — the field of a uniform current density, which keeps the
/// integrator finite for trajectories that terminate on the wire.
pub fn b_ring(coil: &RingCoil, r_m: f64, z_m: f64) -> (f64, f64) {
    let big_r = coil.radius_m;
    let zz = z_m - coil.z_m;
    let rho = r_m.abs();

    // Regularize inside the conductor.
    let dw = ((rho - big_r).powi(2) + zz * zz).sqrt();
    if dw < coil.wire_radius_m {
        if dw < 1e-15 {
            return (0.0, 0.0);
        }
        let scale = dw / coil.wire_radius_m;
        let rr = big_r + (rho - big_r) * (coil.wire_radius_m / dw);
        let zzs = coil.z_m + zz * (coil.wire_radius_m / dw);
        let (br, bz) = b_ring(
            &RingCoil {
                wire_radius_m: 0.0,
                ..*coil
            },
            rr,
            zzs,
        );
        return (br * scale, bz * scale);
    }

    // On-axis limit (also covers rho → 0 numerically).
    if rho < 1e-9 * big_r.max(1.0) {
        let bz =
            MU_0 * coil.ampere_turns * big_r * big_r / (2.0 * (big_r * big_r + zz * zz).powf(1.5));
        return (0.0, bz);
    }

    let sum2 = (big_r + rho).powi(2) + zz * zz;
    let d2 = (big_r - rho).powi(2) + zz * zz;
    let m = 4.0 * big_r * rho / sum2;
    let (k, e) = ellip_ke(m);
    let pref = MU_0 * coil.ampere_turns / (2.0 * std::f64::consts::PI);
    let denom = sum2.sqrt();
    let br = pref * zz / (rho * denom) * (-k + e * (big_r * big_r + rho * rho + zz * zz) / d2);
    let bz = pref / denom * (k + e * (big_r * big_r - rho * rho - zz * zz) / d2);
    (br, bz)
}

/// Multiplier on the wire radius inside which coil B is evaluated
/// analytically instead of from the cached grid (matches the tracer's
/// near-wire step refinement zone, where the 1/ρ field must be exact).
const B_NEAR_WIRE_FACTOR: f64 = 8.0;

#[derive(Debug, Clone)]
struct BGrid {
    br: Vec<f64>,
    bz: Vec<f64>,
}

/// Field sampler for a solved device: E from the grid, B from the coils.
///
/// Coil B is precomputed on the Poisson grid once (analytic elliptic
/// integrals at every node) and bilinearly sampled during tracing — except
/// within `B_NEAR_WIRE_FACTOR` wire radii of a conductor, where the
/// exact analytic sum is used so the shielding sheath keeps its 1/ρ
/// structure. This removes the per-substep elliptic-integral cost that
/// dominated high-current traces.
#[derive(Debug, Clone)]
pub struct FieldMap<'a> {
    solution: &'a Solution,
    coils: Vec<RingCoil>,
    bgrid: Option<BGrid>,
}

impl<'a> FieldMap<'a> {
    /// Build the sampler from a device and its Poisson solution. Rings with
    /// zero ampere-turns contribute no coil.
    pub fn new(device: &Device, solution: &'a Solution) -> Self {
        let coils: Vec<RingCoil> = device
            .rings
            .iter()
            .filter(|r| r.ampere_turns != 0.0)
            .map(|r| RingCoil {
                radius_m: r.ring_radius_mm * 1e-3,
                z_m: r.z_mm * 1e-3,
                ampere_turns: r.ampere_turns,
                wire_radius_m: (r.wire_radius_mm * 1e-3).max(1e-6),
            })
            .collect();
        let bgrid = if coils.is_empty() {
            None
        } else {
            let (nr, nz) = (solution.nr, solution.nz);
            let mut br = vec![0.0_f64; nr * nz];
            let mut bz = vec![0.0_f64; nr * nz];
            for i in 0..nr {
                let r = i as f64 * solution.dr;
                for j in 0..nz {
                    let z = -solution.z_half + j as f64 * solution.dz;
                    let mut sr = 0.0;
                    let mut sz = 0.0;
                    for c in &coils {
                        let (cr, cz) = b_ring(c, r, z);
                        sr += cr;
                        sz += cz;
                    }
                    br[i * nz + j] = sr;
                    bz[i * nz + j] = sz;
                }
            }
            Some(BGrid { br, bz })
        };
        Self {
            solution,
            coils,
            bgrid,
        }
    }

    /// Whether any coil carries current (skips B math when false).
    pub fn has_magnetics(&self) -> bool {
        !self.coils.is_empty()
    }

    /// Electrostatic potential at a Cartesian point, volts.
    pub fn potential(&self, p: [f64; 3]) -> f64 {
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        self.solution.potential_at(r, p[2])
    }

    /// Electric field at a Cartesian point, V/m.
    pub fn e_cart(&self, p: [f64; 3]) -> [f64; 3] {
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        let (er, ez) = self.solution.e_at(r, p[2]);
        if r < 1e-12 {
            // E_r → 0 on the axis by symmetry.
            return [0.0, 0.0, ez];
        }
        [er * p[0] / r, er * p[1] / r, ez]
    }

    /// Magnetic field at a Cartesian point, tesla. Grid-cached far from
    /// conductors, exact analytic within the near-wire zone.
    pub fn b_cart(&self, p: [f64; 3]) -> [f64; 3] {
        if self.coils.is_empty() {
            return [0.0, 0.0, 0.0];
        }
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        let z = p[2];

        let near_wire = self.coils.iter().any(|c| {
            let dr = r - c.radius_m;
            let dz = z - c.z_m;
            let lim = B_NEAR_WIRE_FACTOR * c.wire_radius_m;
            dr * dr + dz * dz < lim * lim
        });

        let (br, bz) = if let (Some(g), false) = (&self.bgrid, near_wire) {
            let s = self.solution;
            let (i0, fu) = crate::poisson::cell_index(r / s.dr, s.nr);
            let (j0, fw) = crate::poisson::cell_index((z + s.z_half) / s.dz, s.nz);
            let idx = |i: usize, j: usize| i * s.nz + j;
            let samp = |d: &[f64]| {
                d[idx(i0, j0)] * (1.0 - fu) * (1.0 - fw)
                    + d[idx(i0 + 1, j0)] * fu * (1.0 - fw)
                    + d[idx(i0, j0 + 1)] * (1.0 - fu) * fw
                    + d[idx(i0 + 1, j0 + 1)] * fu * fw
            };
            (samp(&g.br), samp(&g.bz))
        } else {
            let mut sr = 0.0;
            let mut sz = 0.0;
            for c in &self.coils {
                let (cr, cz) = b_ring(c, r, z);
                sr += cr;
                sz += cz;
            }
            (sr, sz)
        };

        if r < 1e-12 {
            return [0.0, 0.0, bz];
        }
        [br * p[0] / r, br * p[1] / r, bz]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_1ka() -> RingCoil {
        RingCoil {
            radius_m: 0.05,
            z_m: 0.0,
            ampere_turns: 1_000.0,
            wire_radius_m: 0.002,
        }
    }

    #[test]
    fn on_axis_matches_the_textbook_formula() {
        let c = loop_1ka();
        for z in [0.0, 0.01, 0.05, 0.12] {
            let (br, bz) = b_ring(&c, 0.0, z);
            let expect = MU_0 * c.ampere_turns * c.radius_m * c.radius_m
                / (2.0 * (c.radius_m * c.radius_m + z * z).powf(1.5));
            assert!(br.abs() < 1e-15);
            assert!(
                (bz - expect).abs() < 1e-9 * expect.abs().max(1e-9),
                "z={z}: bz={bz}, expect={expect}"
            );
        }
        // Sanity: B(0,0) = μ₀I/2R = 4π×10⁻⁷·1000/(0.1) ≈ 12.57 mT.
        let (_, b0) = b_ring(&c, 0.0, 0.0);
        assert!((b0 - 0.012_566).abs() < 1e-4, "b0={b0}");
    }

    #[test]
    fn off_axis_is_consistent_with_the_axis_limit() {
        let c = loop_1ka();
        let (_, bz_axis) = b_ring(&c, 0.0, 0.03);
        let (_, bz_near) = b_ring(&c, 1e-6, 0.03);
        assert!((bz_axis - bz_near).abs() / bz_axis.abs() < 1e-4);
    }

    #[test]
    fn midplane_symmetry() {
        let c = loop_1ka();
        let (br_up, bz_up) = b_ring(&c, 0.03, 0.02);
        let (br_dn, bz_dn) = b_ring(&c, 0.03, -0.02);
        assert!((br_up + br_dn).abs() < 1e-12, "B_r must be odd in z");
        assert!((bz_up - bz_dn).abs() < 1e-12, "B_z must be even in z");
    }

    #[test]
    fn cached_grid_matches_analytic_far_from_wires() {
        use crate::device::Device;
        use crate::poisson::{solve, SolveOptions};
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -500.0, 20_000.0);
        let sol = solve(&device, 81, 161, &SolveOptions::default()).unwrap();
        let fields = FieldMap::new(&device, &sol);
        let coils: Vec<RingCoil> = device
            .rings
            .iter()
            .map(|r| RingCoil {
                radius_m: r.ring_radius_mm * 1e-3,
                z_m: r.z_mm * 1e-3,
                ampere_turns: r.ampere_turns,
                wire_radius_m: r.wire_radius_mm * 1e-3,
            })
            .collect();
        // Far-field points (well outside the near-wire analytic zone).
        for (x, z) in [(0.005, 0.0), (0.02, 0.06), (0.08, -0.04), (0.06, 0.08)] {
            let got = fields.b_cart([x, 0.0, z]);
            let mut want_r = 0.0;
            let mut want_z = 0.0;
            for c in &coils {
                let (cr, cz) = b_ring(c, x, z);
                want_r += cr;
                want_z += cz;
            }
            let scale = (want_r * want_r + want_z * want_z).sqrt().max(1e-9);
            assert!(
                ((got[0] - want_r).powi(2) + (got[2] - want_z).powi(2)).sqrt() < 0.05 * scale,
                "grid B off at ({x},{z}): got {got:?}, want ({want_r},{want_z})"
            );
        }
    }

    #[test]
    fn interior_regularization_is_finite_and_continuous() {
        let c = loop_1ka();
        // On the midplane the near-wire field is all B_z (B_r is odd in z);
        // sample just outside, at, and inside the conductor surface.
        let (_, b_out) = b_ring(&c, c.radius_m + c.wire_radius_m * 1.01, 0.0);
        let (_, b_at) = b_ring(&c, c.radius_m + c.wire_radius_m * 0.999, 0.0);
        let (_, b_in) = b_ring(&c, c.radius_m + c.wire_radius_m * 0.5, 0.0);
        let (_, b_center) = b_ring(&c, c.radius_m, 0.0);
        for b in [b_out, b_at, b_in, b_center] {
            assert!(b.is_finite());
        }
        assert!(
            (b_at - b_out).abs() < 0.05 * b_out.abs(),
            "discontinuous at surface: {b_at} vs {b_out}"
        );
        assert!(b_in.abs() < b_at.abs(), "field must fall toward centerline");
        assert!(b_center.abs() < 1e-12);
    }
}
