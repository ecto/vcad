//! First-order air-gap magnetic field via a magnetic-equivalent-circuit (MEC)
//! reluctance network.
//!
//! The air-gap flux density `B_gap` is the single most important magnetic input
//! to motor performance (it sets the torque constant), yet
//! [`crate::magnetics::motor_torque_constant`] takes it as a *given*. This module
//! closes that loop: it computes `B_gap` from magnet and geometry parameters so
//! the rest of the magnetics stack no longer hardcodes a flux guess.
//!
//! # Model and its limits (read this)
//!
//! This is an explicitly **first-order** lumped reluctance network for the common
//! PM-rotor + soft-iron-stator topology (radial or axial flux). It is deliberately
//! coarse:
//!
//! - **No slotting** — the air gap is treated as smooth; there is no Carter
//!   coefficient correcting for stator teeth/slots.
//! - **No fringing** — flux is assumed to cross the gap straight, with no leakage
//!   spreading at the pole edges.
//! - **No saturation *by default*** — by default the soft-iron path is treated
//!   as *infinite* permeability (zero iron reluctance), so the magnet works only
//!   against the air gap. A finite-permeability iron path is available as an
//!   optional refinement; that path is linear unless
//!   [`AirGapSpec::iron_js_t`] is supplied, which switches the iron to the
//!   arctangent B–H law of [`vcad_kernel_em::material`], solved by bisecting
//!   the MMF balance (see [`airgap_solve`]).
//!
//! Use it for sizing intuition and as a differentiable leaf for co-design, not as
//! a substitute for FEA.
//!
//! # The reluctance network
//!
//! A permanent magnet of remanence `Br`, recoil relative permeability `mu_rec`,
//! thickness `l_m` and pole face area `A_m` is modeled as a flux source `phi_r =
//! Br * A_m` behind an internal reluctance `R_m = l_m / (mu0 * mu_rec * A_m)`.
//! That drives flux through the series air-gap reluctance `R_g = g / (mu0 * A_g)`
//! (and, optionally, an iron-path reluctance `R_fe`). With iron taken as infinite
//! permeability (`R_fe = 0`), the gap flux is
//!
//! ```text
//!   phi_g = phi_r * R_m / (R_m + R_g)
//!   B_gap = phi_g / A_g
//!         = Br * (A_m / A_g) / (1 + mu_rec * (g / l_m) * (A_m / A_g))
//! ```
//!
//! For the equal-area case (`A_m == A_g`) this collapses to the familiar
//! `B_gap = Br / (1 + mu_rec * g / l_m)`.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use vcad_kernel_em::material::{nu_from_b, Saturation};

/// Permeability of free space, T·m/A.
const MU0: f64 = 4.0 * PI * 1e-7;

/// Inputs for the cored (back-iron) air-gap MEC model.
///
/// All lengths in millimetres, areas in mm² (the area *ratio* is what matters,
/// so consistent units cancel). `Br` in tesla. The model returns the operating
/// air-gap flux density via [`airgap_flux_density`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirGapSpec {
    /// Magnet remanent flux density `Br`, tesla (NdFeB ≈ 1.2, ferrite ≈ 0.4).
    pub remanence_tesla: f64,
    /// Magnet thickness in the magnetization direction, mm.
    pub magnet_thickness_mm: f64,
    /// Recoil relative permeability of the magnet (NdFeB ≈ 1.05, ferrite ≈ 1.1).
    pub recoil_mu_rel: f64,
    /// Air-gap length, mm.
    pub airgap_mm: f64,
    /// Magnet pole face area, mm² (cross-section the flux leaves the magnet).
    pub magnet_area_mm2: f64,
    /// Air-gap face area, mm² (cross-section the flux crosses the gap).
    pub gap_area_mm2: f64,
    /// Soft-iron path relative permeability. `None` (or non-finite) treats the
    /// iron as ideal (infinite permeability, zero reluctance) — the first cut.
    /// `Some(mu_r)` adds a finite, *linear* iron reluctance to the loop.
    pub iron_mu_rel: Option<f64>,
    /// Mean soft-iron flux path length (stator + rotor back-iron), mm. Only used
    /// when `iron_mu_rel` is `Some`. Ignored otherwise.
    pub iron_path_mm: f64,
    /// Iron cross-section area carrying the flux, mm². Only used when
    /// `iron_mu_rel` is `Some`. Ignored otherwise.
    pub iron_area_mm2: f64,
    /// Saturation polarization `J_s` of the soft iron, tesla (silicon steel
    /// ≈ 1.6–2.0, ferrite ≈ 0.35–0.5). `None` keeps the iron **linear** — the
    /// historical behaviour, and still the default. `Some(js)` switches the
    /// iron reluctances to the arctangent B–H law and solves the network with
    /// a nonlinear solve, so the returned `B_gap` falls once the iron saturates.
    ///
    /// Only meaningful together with `iron_mu_rel` (the initial slope) and/or
    /// [`AirGapSpec::teeth`]; with ideal iron there is nothing to saturate.
    #[serde(default)]
    pub iron_js_t: Option<f64>,
    /// Stator tooth geometry. `None` (the default) means the model has no
    /// tooth concept at all and cannot see tooth saturation — the failure mode
    /// this field exists to close. `Some(_)` reports the tooth flux density
    /// and, when `tooth_path_mm > 0`, adds the teeth as their own reluctance
    /// segment (the narrowest iron in the loop, and so the first to saturate).
    #[serde(default)]
    pub teeth: Option<TeethSpec>,
}

/// Stator tooth geometry for the MEC network.
///
/// Gap flux crossing a whole tooth *pitch* funnels into one tooth *body*, so
/// the iron there runs at `B_gap · pitch / width` — routinely 2× the gap field
/// on a 50%-tooth-width machine, which is exactly what pushes M19-class steel
/// past its ~1.5–1.7 T knee while `B_gap` still looks comfortable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeethSpec {
    /// Stator slot (tooth) count.
    pub slots: f64,
    /// Circumferential tooth width at `mean_radius_mm`, mm.
    pub tooth_width_mm: f64,
    /// Mean radius of the active annulus, mm — where the tooth pitch is taken.
    pub mean_radius_mm: f64,
    /// Iron path length through the tooth body, mm. `0.0` makes the teeth
    /// *diagnostic only*: their flux density is reported (and gated) but they
    /// add no reluctance to the loop.
    #[serde(default)]
    pub tooth_path_mm: f64,
}

impl TeethSpec {
    /// Tooth pitch at the mean radius, mm (`2π·r / slots`).
    pub fn tooth_pitch_mm(&self) -> f64 {
        if self.slots <= 0.0 || self.mean_radius_mm <= 0.0 {
            return 0.0;
        }
        2.0 * PI * self.mean_radius_mm / self.slots
    }

    /// Flux-concentration factor `pitch / width`, clamped to `>= 1` (a tooth
    /// wider than its own pitch is not physical, and must never *dilute*).
    pub fn concentration(&self) -> f64 {
        let pitch = self.tooth_pitch_mm();
        if pitch <= 0.0 || self.tooth_width_mm <= 0.0 {
            return 1.0;
        }
        (pitch / self.tooth_width_mm).max(1.0)
    }
}

/// Flux density above which soft iron is treated as saturating when no
/// `iron_js_t` is supplied, tesla — the low end of the M19-class silicon-steel
/// knee. Used only to *warn*; it changes no number.
pub const SILICON_STEEL_KNEE_T: f64 = 1.5;

/// Everything the reluctance network knows after a solve.
///
/// [`airgap_flux_density`] returns only `b_gap_tesla` and is unchanged;
/// [`airgap_solve`] returns this so a caller can see the tooth field and
/// whether the iron is off its linear slope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirGapSolution {
    /// Operating air-gap flux density, tesla.
    pub b_gap_tesla: f64,
    /// Flux density in the tooth body, tesla. `None` when no [`TeethSpec`].
    pub b_tooth_tesla: Option<f64>,
    /// Flux density in the yoke / back-iron path, tesla. `None` when the iron
    /// is ideal (`iron_mu_rel` unset).
    pub b_iron_tesla: Option<f64>,
    /// Tooth flux-concentration factor actually used (`pitch / width`).
    pub tooth_concentration: Option<f64>,
    /// True when the iron was solved nonlinearly (`iron_js_t` supplied).
    pub nonlinear: bool,
    /// Nonlinear-solve iterations used (0 for a linear solve).
    pub iterations: usize,
    /// False if the nonlinear solve failed to converge. The bisection is
    /// bracketed on a monotone residual, so this is always true today; it is
    /// kept in the result so a future solver can report failure honestly.
    pub converged: bool,
    /// Human-readable saturation warnings; empty when nothing is past the knee.
    /// Populated even for a *linear* solve — that is the whole point: the
    /// linear model's optimism is now announced instead of silent.
    pub warnings: Vec<String>,
}

impl AirGapSpec {
    /// A sensible NdFeB starting point: Br = 1.2 T, 3 mm magnet, 1 mm gap,
    /// recoil permeability 1.05, equal magnet/gap areas, ideal iron.
    ///
    /// Areas are unit (the ratio is 1), so only the length terms matter.
    pub fn ndfeb_default() -> Self {
        Self {
            remanence_tesla: 1.2,
            magnet_thickness_mm: 3.0,
            recoil_mu_rel: 1.05,
            airgap_mm: 1.0,
            magnet_area_mm2: 1.0,
            gap_area_mm2: 1.0,
            iron_mu_rel: None,
            iron_path_mm: 0.0,
            iron_area_mm2: 1.0,
            iron_js_t: None,
            teeth: None,
        }
    }
}

/// Solve the reluctance network for the operating air-gap flux density (tesla).
///
/// Returns 0.0 for any non-physical input that would zero or break the loop
/// (`Br <= 0`, `magnet_thickness <= 0`, or any area `<= 0`). A non-positive air
/// gap is treated as zero gap (`B_gap == Br * A_m / A_g`, no gap reluctance).
///
/// First-order only: no slotting, no fringing. Iron saturation is modeled only
/// when [`AirGapSpec::iron_js_t`] is set — see [`airgap_solve`], which also
/// returns the tooth field and any saturation warnings this function drops.
/// See the module docs for the full reluctance derivation and assumptions.
pub fn airgap_flux_density(spec: &AirGapSpec) -> f64 {
    airgap_solve(spec).b_gap_tesla
}

/// Solve the reluctance network and report *everything* it knows: gap field,
/// tooth and yoke fields, whether the iron was solved nonlinearly, and
/// saturation warnings.
///
/// # Nonlinear iron
///
/// With [`AirGapSpec::iron_js_t`] set, each iron segment's reluctance uses the
/// secant reluctivity `ν(B)` of the arctangent B–H law
/// ([`vcad_kernel_em::material::nu_from_b`] — the *same* law the axisymmetric
/// FV solver uses, not a second one) and the loop flux is found by bisecting
/// the (strictly monotone) MMF balance. Without it the iron stays linear and
/// the result is bit-identical to the historical model.
///
/// # Teeth
///
/// [`TeethSpec`] makes the tooth flux density visible: `B_tooth = B_gap ·
/// pitch / width`. With `tooth_path_mm > 0` the teeth also enter the network
/// as their own reluctance segment, so a saturating tooth actually pulls
/// `B_gap` down instead of merely being reported.
pub fn airgap_solve(spec: &AirGapSpec) -> AirGapSolution {
    let br = spec.remanence_tesla;
    let l_m = spec.magnet_thickness_mm;
    let mu_rec = spec.recoil_mu_rel;
    let g = spec.airgap_mm.max(0.0);
    let a_m = spec.magnet_area_mm2;
    let a_g = spec.gap_area_mm2;

    // Non-physical guards: collapse to zero rather than NaN/inf.
    if br <= 0.0 || l_m <= 0.0 || mu_rec <= 0.0 || a_m <= 0.0 || a_g <= 0.0 {
        return AirGapSolution {
            b_gap_tesla: 0.0,
            b_tooth_tesla: None,
            b_iron_tesla: None,
            tooth_concentration: None,
            nonlinear: false,
            iterations: 0,
            converged: true,
            warnings: Vec::new(),
        };
    }

    // Reluctances. Lengths in mm, areas in mm² — the 1e-3 / 1e-6 factors are a
    // common scale across every reluctance, so they cancel in the flux divider.
    // We keep them explicit for readability (and so the iron term, which has a
    // different geometry, scales correctly relative to the gap and magnet).
    let scale = 1e-3 / 1e-6; // mm / mm² -> 1/m, applied uniformly below
    let r_m = l_m / (MU0 * mu_rec * a_m) * scale; // magnet internal reluctance
    let r_g = g / (MU0 * a_g) * scale; // air-gap reluctance

    // Magnet as a flux source phi_r = Br * A_m behind R_m, driving the series
    // gap + iron reluctances.
    let phi_r = br * (a_m * 1e-6); // Wb (A_m converted to m²)

    // Iron segments in the loop. Each is (path length m, cross-section m²);
    // an empty list is the ideal-iron case (zero iron reluctance).
    let teeth = spec.teeth.filter(|t| t.concentration().is_finite());
    let k_tooth = teeth.map(|t| t.concentration());
    let yoke = match spec.iron_mu_rel {
        Some(mu_fe) if mu_fe.is_finite() && mu_fe > 0.0 && spec.iron_area_mm2 > 0.0 => {
            Some((spec.iron_path_mm, spec.iron_area_mm2))
        }
        _ => None,
    };
    // Flux crossing the gap funnels into tooth iron of area A_g / k.
    let tooth_seg = match (teeth, k_tooth) {
        (Some(t), Some(k)) if t.tooth_path_mm > 0.0 => Some((t.tooth_path_mm, a_g / k)),
        _ => None,
    };

    // Initial slope of the iron. Both segments share the same material.
    let mu_ri = spec.iron_mu_rel.filter(|m| m.is_finite() && *m > 1.0);
    let sat = match (spec.iron_js_t, mu_ri) {
        (Some(js), Some(mu)) if js.is_finite() && js > 0.0 => Some((Saturation { js_t: js }, mu)),
        _ => None,
    };
    // Linear reluctivity of the iron. Taken from `iron_mu_rel` as given (not
    // the >1 filtered `mu_ri`) so the historical linear path is untouched.
    let nu_lin = match spec.iron_mu_rel {
        Some(mu) if mu.is_finite() && mu > 0.0 => 1.0 / (MU0 * mu),
        _ => 0.0,
    };

    // R = ν·l/A (ν = 1/(μ0·μ_r) recovers the linear l/(μ0·μ_r·A)).
    let r_seg = |seg: Option<(f64, f64)>, nu: f64| -> f64 {
        seg.map_or(0.0, |(l_mm, a_mm2)| nu * (l_mm / a_mm2) * scale)
    };

    let mut iterations = 0usize;
    let phi_g = match sat {
        // Linear iron: one closed-form flux divider, as before.
        None => {
            let r_fe = r_seg(yoke, nu_lin) + r_seg(tooth_seg, nu_lin);
            phi_r * r_m / (r_m + r_g + r_fe)
        }
        // Saturating iron. Solve the MMF balance
        //
        //   F = Φ·(R_m + R_g) + Σ_segments ν(Φ/A)·(l/A)·Φ,   F = φ_r·R_m
        //
        // whose right-hand side is *strictly increasing* in Φ (ν rises with B
        // under the arctangent law), so the root is unique and bisection finds
        // it without a relaxation constant to tune. Successive substitution on
        // ν was tried first and oscillates: dν/dΦ is steep past the knee, so
        // convergence depended on a hand-picked damping factor — exactly the
        // kind of hidden knob that makes a saturating result untrustworthy.
        Some((s, mu)) => {
            let mmf = phi_r * r_m;
            let residual = |phi: f64| {
                let nu_of = |seg: Option<(f64, f64)>| {
                    seg.map_or(nu_lin, |(_, a)| nu_from_b(mu, s, phi / (a * 1e-6)))
                };
                phi * (r_m + r_g)
                    + r_seg(yoke, nu_of(yoke)) * phi
                    + r_seg(tooth_seg, nu_of(tooth_seg)) * phi
                    - mmf
            };
            // The linear-iron flux is an upper bound (saturation only ever adds
            // reluctance), and zero flux is a lower bound.
            let (mut lo, mut hi) = (0.0, phi_r * r_m / (r_m + r_g));
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if residual(mid) > 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
                iterations += 1;
            }
            0.5 * (lo + hi)
        }
    };
    // Bisection over a bracketed, monotone residual: 80 halvings exhaust f64.
    let converged = true;

    // B_gap = phi_g / A_g (A_g in m²).
    let b_gap = phi_g / (a_g * 1e-6);
    let b_tooth = k_tooth.map(|k| b_gap * k);
    let b_iron = yoke.map(|(_, a)| phi_g / (a * 1e-6));

    // Warn on anything past the knee — the point of the exercise. With an
    // explicit J_s the knee is a property of the material; without one we fall
    // back to the M19-class figure, because a *linear* model going quietly past
    // 1.5 T is exactly the silent optimism worth flagging.
    let knee = spec
        .iron_js_t
        .filter(|j| j.is_finite() && *j > 0.0)
        .map_or(SILICON_STEEL_KNEE_T, |js| 0.85 * js);
    let mut warnings = Vec::new();
    let mut warn_if_past = |label: &str, b: Option<f64>| {
        if let Some(b) = b {
            if b > knee {
                warnings.push(format!(
                    "{label} flux density {b:.2} T exceeds the {knee:.2} T knee — {}",
                    if sat.is_some() {
                        "the saturating solve accounts for this, but the iron is working hard"
                    } else {
                        "the LINEAR iron model over-predicts B_gap and Kt here; pass iron_js_t \
                         to solve the saturating network"
                    }
                ));
            }
        }
    };
    warn_if_past("tooth", b_tooth);
    warn_if_past("yoke/back-iron", b_iron);

    AirGapSolution {
        b_gap_tesla: b_gap,
        b_tooth_tesla: b_tooth,
        b_iron_tesla: b_iron,
        tooth_concentration: k_tooth,
        nonlinear: sat.is_some(),
        iterations,
        converged,
        warnings,
    }
}

/// First-order Carter-like fringing derate for the MEC gap field.
///
/// [`airgap_flux_density`] assumes flux crosses the gap straight — no
/// spreading at the pole edges. Real flux fringes outward by roughly one gap
/// length per pole edge (the classical straight-line-plus-quarter-circle
/// fringe-tube estimate), so the same total flux crosses an effectively wider
/// pole and the density *under* the pole face drops. Modeling the widening in
/// the one dimension that matters (across the pole width `w`, gap `g`):
///
/// ```text
///   B_derated = B_raw · w / (w + 2g)  =  B_raw · ρ / (ρ + 2),   ρ = w/g
/// ```
///
/// This is the fringing analogue of Carter's slotting coefficient — a pure
/// geometry ratio, first-order in `g/w`. It is honest only while the pole is
/// wide compared to the gap (`ρ ≳ 2`); below that the fringe tubes overlap
/// and the closed form under-predicts the field, so treat small-`ρ` results
/// as a lower bound. Returns 1.0 (no derate) for a non-positive gap and 0.0
/// for a non-positive pole width.
pub fn fringing_derate(pole_width_mm: f64, airgap_mm: f64) -> f64 {
    if airgap_mm <= 0.0 {
        return 1.0;
    }
    if pole_width_mm <= 0.0 {
        return 0.0;
    }
    pole_width_mm / (pole_width_mm + 2.0 * airgap_mm)
}

/// Coarse coreless / air-cored air-gap flux density (tesla) — **no back-iron**.
///
/// With no soft-iron return path the field is set directly by the coil MMF
/// (`N * I` ampere-turns) driving flux across the gap, with permeability `mu0`
/// everywhere:
///
/// ```text
///   B_gap ≈ mu0 * N * I / g
/// ```
///
/// Be honest: this is a *very* coarse estimate. A real air-cored machine has the
/// MMF distributed around the winding, large fringing, and a path length that is
/// not simply the mechanical gap — so this systematically over-predicts the field
/// in the gap centre. Treat it as an order-of-magnitude figure for coreless
/// (e.g. PCB-stator / Halbach-less) layouts, not a design value.
///
/// `turns` and `current_amps` are the per-pole ampere-turns; `airgap_mm` is the
/// effective magnetic gap (coil-to-coil or coil-to-rotor spacing). Returns 0.0
/// for a non-positive gap.
pub fn aircored_airgap_flux_density(turns: f64, current_amps: f64, airgap_mm: f64) -> f64 {
    if airgap_mm <= 0.0 {
        return 0.0;
    }
    let g = airgap_mm * 1e-3; // m
    MU0 * turns * current_amps / g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndfeb_default_is_physically_plausible() {
        // NdFeB Br=1.2, 3mm magnet across 1mm gap -> ~0.4..0.9 T.
        let b = airgap_flux_density(&AirGapSpec::ndfeb_default());
        assert!(
            (0.4..=0.9).contains(&b),
            "B_gap {b} out of plausible NdFeB range"
        );
        // Closed form for equal areas: Br / (1 + mu_rec * g / l_m).
        let expected = 1.2 / (1.0 + 1.05 * 1.0 / 3.0);
        assert!(
            (b - expected).abs() < 1e-9,
            "B_gap {b} vs closed form {expected}"
        );
    }

    #[test]
    fn larger_gap_means_smaller_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        let b_small = airgap_flux_density(&spec);
        spec.airgap_mm = 3.0; // triple the gap
        let b_large = airgap_flux_density(&spec);
        assert!(
            b_large < b_small,
            "bigger gap should drop B_gap: {b_large} !< {b_small}"
        );
    }

    #[test]
    fn zero_remanence_gives_zero_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        spec.remanence_tesla = 0.0;
        assert_eq!(airgap_flux_density(&spec), 0.0);
    }

    #[test]
    fn thicker_magnet_means_stronger_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        let b_thin = airgap_flux_density(&spec);
        spec.magnet_thickness_mm = 6.0; // thicker magnet, same gap
        let b_thick = airgap_flux_density(&spec);
        assert!(
            b_thick > b_thin,
            "thicker magnet should raise B_gap: {b_thick} !> {b_thin}"
        );
        // Asymptote: B_gap -> Br * (A_m/A_g) as l_m -> infinity. Stay below it.
        assert!(b_thick < spec.remanence_tesla);
    }

    #[test]
    fn unequal_areas_concentrate_flux() {
        // Magnet bigger than the gap face concentrates flux -> higher B_gap.
        let mut spec = AirGapSpec::ndfeb_default();
        spec.magnet_area_mm2 = 2.0;
        spec.gap_area_mm2 = 1.0;
        let b_focus = airgap_flux_density(&spec);
        let b_equal = airgap_flux_density(&AirGapSpec::ndfeb_default());
        assert!(
            b_focus > b_equal,
            "flux focusing should raise B_gap: {b_focus} !> {b_equal}"
        );
    }

    #[test]
    fn finite_iron_reluctance_lowers_field_vs_ideal() {
        let ideal = AirGapSpec::ndfeb_default();
        let b_ideal = airgap_flux_density(&ideal);

        let mut with_iron = ideal;
        with_iron.iron_mu_rel = Some(1000.0); // good but finite silicon steel
        with_iron.iron_path_mm = 50.0;
        with_iron.iron_area_mm2 = 1.0;
        let b_iron = airgap_flux_density(&with_iron);

        assert!(
            b_iron < b_ideal,
            "finite iron reluctance should drop B_gap below ideal: {b_iron} !< {b_ideal}"
        );
        // But only slightly, since mu_fe is large.
        assert!(
            b_iron > 0.95 * b_ideal,
            "iron drop should be small: {b_iron}"
        );
    }

    /// The 24-slot / 20-pole axial-flux QDD actuator that exposed the silent
    /// optimism: ⌀76→⌀120 annulus, 4 mm N42, 1.0 mm gap, iron μ_r 4000.
    fn qdd_actuator() -> AirGapSpec {
        AirGapSpec {
            remanence_tesla: 1.30,
            magnet_thickness_mm: 4.0,
            recoil_mu_rel: 1.05,
            airgap_mm: 1.0,
            magnet_area_mm2: 1.0,
            gap_area_mm2: 1.0,
            iron_mu_rel: Some(4000.0),
            iron_path_mm: 30.0,
            iron_area_mm2: 1.0,
            iron_js_t: None,
            teeth: Some(TeethSpec {
                slots: 24.0,
                tooth_width_mm: 6.4, // ~50% of the 12.83 mm pitch at r = 49 mm
                mean_radius_mm: 49.0,
                tooth_path_mm: 0.0,
            }),
        }
    }

    #[test]
    fn tooth_pitch_and_concentration_match_the_geometry() {
        let t = qdd_actuator().teeth.unwrap();
        // 2π·49/24 = 12.827 mm
        assert!(
            (t.tooth_pitch_mm() - 12.8282).abs() < 1e-3,
            "{}",
            t.tooth_pitch_mm()
        );
        // 50% tooth width concentrates ~2x.
        assert!(
            (t.concentration() - 2.0044).abs() < 1e-3,
            "{}",
            t.concentration()
        );
        // A tooth as wide as its pitch never dilutes.
        let full = TeethSpec {
            tooth_width_mm: t.tooth_pitch_mm() * 2.0,
            ..t
        };
        assert_eq!(full.concentration(), 1.0);
    }

    #[test]
    fn linear_iron_hides_a_saturating_tooth_and_says_so() {
        // The motivating case: B_gap looks comfortable (~1.0 T) while the teeth
        // sit near 2 T, well past the M19 knee. The linear model must WARN.
        let sol = airgap_solve(&qdd_actuator());
        assert!(!sol.nonlinear);
        assert!(
            (0.9..1.1).contains(&sol.b_gap_tesla),
            "B_gap {} off the expected ~1.03 T",
            sol.b_gap_tesla
        );
        let b_tooth = sol.b_tooth_tesla.expect("teeth were supplied");
        assert!(
            b_tooth > SILICON_STEEL_KNEE_T,
            "tooth {b_tooth} should be past the knee"
        );
        assert!(
            sol.warnings.iter().any(|w| w.starts_with("tooth")),
            "a past-knee tooth must warn: {:?}",
            sol.warnings
        );
        // The old entry point is unchanged and still returns just the number.
        assert_eq!(airgap_flux_density(&qdd_actuator()), sol.b_gap_tesla);
    }

    #[test]
    fn saturating_iron_is_materially_pessimistic_vs_linear() {
        // Same machine, teeth now in the loop and narrowed to 4 mm (k ≈ 3.2) —
        // a real design mistake, and one the linear model quietly rewards.
        let mut lin = qdd_actuator();
        lin.teeth = lin.teeth.map(|t| TeethSpec {
            tooth_width_mm: 4.0,
            tooth_path_mm: 20.0,
            ..t
        });
        let b_lin = airgap_solve(&lin);

        let mut sat = lin;
        sat.iron_js_t = Some(2.0); // M19-class silicon steel
        let b_sat = airgap_solve(&sat);

        assert!(b_sat.nonlinear && b_sat.converged, "{b_sat:?}");
        assert!(b_sat.iterations > 0);
        assert!(
            b_sat.b_gap_tesla < 0.95 * b_lin.b_gap_tesla,
            "saturating solve must be materially lower: {} vs linear {}",
            b_sat.b_gap_tesla,
            b_lin.b_gap_tesla
        );
        // And the tooth it predicts is likewise below the linear model's.
        assert!(b_sat.b_tooth_tesla.unwrap() < b_lin.b_tooth_tesla.unwrap());
    }

    #[test]
    fn below_the_knee_linear_and_saturating_agree() {
        // Wide teeth (little concentration) and a thin magnet keep every iron
        // segment well under the knee — the two models must then agree closely.
        let mut lin = qdd_actuator();
        lin.remanence_tesla = 0.40; // ferrite-class drive
        lin.teeth = lin.teeth.map(|t| TeethSpec {
            tooth_width_mm: 11.5, // ~90% fill: almost no concentration
            tooth_path_mm: 20.0,
            ..t
        });
        let b_lin = airgap_solve(&lin);
        let mut sat = lin;
        sat.iron_js_t = Some(2.0);
        let b_sat = airgap_solve(&sat);

        assert!(
            b_lin.b_tooth_tesla.unwrap() < SILICON_STEEL_KNEE_T,
            "fixture should sit below the knee: {:?}",
            b_lin.b_tooth_tesla
        );
        assert!(b_lin.warnings.is_empty(), "{:?}", b_lin.warnings);
        let rel = (b_sat.b_gap_tesla - b_lin.b_gap_tesla).abs() / b_lin.b_gap_tesla;
        assert!(
            rel < 0.03,
            "below the knee the models should agree: rel {rel:.4}"
        );
    }

    #[test]
    fn saturation_is_opt_in_and_changes_nothing_by_default() {
        // Every pre-existing spec (no iron_js_t, no teeth) is bit-identical.
        for mut spec in [AirGapSpec::ndfeb_default(), qdd_actuator()] {
            spec.teeth = None;
            spec.iron_js_t = None;
            let sol = airgap_solve(&spec);
            assert!(!sol.nonlinear);
            assert_eq!(sol.iterations, 0);
            assert!(sol.b_tooth_tesla.is_none());
            assert!(sol.warnings.is_empty());
        }
    }

    #[test]
    fn yoke_saturation_also_warns() {
        // No teeth at all: a thin back-iron cross-section saturates on its own.
        let mut spec = AirGapSpec::ndfeb_default();
        spec.iron_mu_rel = Some(4000.0);
        spec.iron_path_mm = 30.0;
        spec.iron_area_mm2 = 0.4; // yoke narrower than the gap face
        let sol = airgap_solve(&spec);
        assert!(sol.b_iron_tesla.unwrap() > SILICON_STEEL_KNEE_T);
        assert!(
            sol.warnings.iter().any(|w| w.starts_with("yoke")),
            "{:?}",
            sol.warnings
        );
    }

    #[test]
    fn fringing_derate_behaves_like_a_carter_factor() {
        // Wide pole, small gap: barely any derate.
        assert!(fringing_derate(20.0, 0.5) > 0.95);
        // ρ = w/g = 2 → w/(w+2g) = 0.5.
        assert!((fringing_derate(2.0, 1.0) - 0.5).abs() < 1e-12);
        // Monotonic: bigger gap, more fringing, lower B.
        assert!(fringing_derate(10.0, 2.0) < fringing_derate(10.0, 1.0));
        // Degenerate guards.
        assert_eq!(fringing_derate(10.0, 0.0), 1.0);
        assert_eq!(fringing_derate(0.0, 1.0), 0.0);
        // Always a derate, never a boost.
        assert!(fringing_derate(5.0, 1.0) < 1.0);
    }

    #[test]
    fn aircored_is_small_and_scales_as_expected() {
        // 100 ampere-turns across a 1 mm gap: tiny field (no iron to amplify).
        let b = aircored_airgap_flux_density(100.0, 1.0, 1.0);
        let expected = MU0 * 100.0 / 1e-3;
        assert!((b - expected).abs() < 1e-12);
        assert!(b < 0.2, "air-cored field should be small: {b}");

        // More ampere-turns -> more field; bigger gap -> less.
        assert!(aircored_airgap_flux_density(200.0, 1.0, 1.0) > b);
        assert!(aircored_airgap_flux_density(100.0, 1.0, 2.0) < b);
        // Non-positive gap -> 0.
        assert_eq!(aircored_airgap_flux_density(100.0, 1.0, 0.0), 0.0);
    }
}
