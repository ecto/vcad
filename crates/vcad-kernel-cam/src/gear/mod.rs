//! Involute spur gears, described the way a 3-axis router has to cut them.
//!
//! The gears this module exists for are cut from above with an end mill: every
//! tooth space is the ideal involute space *opened* by the cutter, so its root
//! is the round the cutter leaves, not the trochoid a hob would leave. That
//! makes three questions load-bearing, and all three are answered here rather
//! than eyeballed off a polyline:
//!
//! 1. **Where does the involute actually end?** The cutter's fillet is tangent
//!    to the flank at the *form radius*. Below it (above it, on a ring) there
//!    is no involute left to mesh on.
//! 2. **Is that low enough?** The fillet tangency must sit below the lowest
//!    point of contact on an external gear and above the highest on an
//!    internal one, or the mating tip runs on the fillet.
//!    [`SpurGear::reachability_exact`] reports that margin and the largest
//!    cutter that keeps it positive. But a radius crossing a line is not yet a
//!    defect: what matters on a part is the *metal* the fillet leaves on the
//!    active flank, which is quadratically smaller —
//!    [`SpurGear::flank_deviation_at`] measures it along the flank normal and
//!    [`SpurGear::reachability_within`] grades the same geometry against a
//!    tolerance a machine can be held to. The reference ring overlaps by
//!    0.034 mm of radius and 1.4 µm of metal; those are the same fact.
//! 3. **What does the machine follow?** The tool-centre path is the exact
//!    parallel curve of the flank, which for an involute is another involute
//!    of the same base circle (see [`contour::tool_centre_path`]) — never a
//!    polygon offset of a sampled profile.
//!
//! # Conventions
//!
//! Units are mm and radians. Profiles are centred on the origin in the XY
//! plane, tooth `i` centred on angle `i * 2π/z`, so space `i` is centred on
//! `(i + 1/2) * 2π/z`. Profile shift `x` is in modules.
//!
//! For an **internal** gear the shift displaces both tip and root radially
//! outward by `x·m` and thins the ring tooth by `2·x·m·tanα`; equivalently,
//! the ring's *space* has the same shape as the tooth of an external gear with
//! the same `m`, `z` and `x`. Every formula here is written once against that
//! duality: the cutter always works in the space.
//!
//! **Backlash** is circumferential thinning at the reference pitch circle,
//! applied per gear: an external tooth loses `backlash_thinning`, an internal
//! gear's space gains it. A pair each thinned `b` runs `(b₁+b₂)·cos α` of
//! normal backlash ([`GearPair::normal_backlash`]).
//!
//! # Below the base circle
//!
//! There is no involute inside the base circle, and both external gears in the
//! reference train have their root there (sun `rf` 4.22 < `rb` 4.698). The
//! flank is therefore continued as a **radial line** at the base-circle angle —
//! the involute's own tangent there — which is the geometry the part files were
//! drawn from. [`SpurGear::flank_is_involute_at`] says which regime a radius is
//! in; nothing else in this module silently pretends the involute continues.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

mod cutter;
mod error;
mod measure;
mod pair;
mod planetary;
mod report;

pub mod contour;

pub use cutter::{FilletEncroachment, Reachability, RootBinding, ToothSpace};
pub use error::GearError;
pub use measure::{Compensation, PinMeasurement, SpanMeasurement};
pub use pair::{
    profile_shift_sum_for_centre_distance, working_pressure_angle, ContactRadii, GearPair,
    MeshReport,
};
pub use planetary::{PlanetaryMesh, PlanetaryTrain};
pub use report::GearReport;

/// ISO full-depth addendum coefficient (`h_a* = 1.0` modules).
pub const ADDENDUM_COEFFICIENT: f64 = 1.0;
/// ISO full-depth bottom clearance coefficient (`c* = 0.25` modules).
pub const CLEARANCE_COEFFICIENT: f64 = 0.25;
/// 20°, the pressure angle of every gear in the reference train.
pub const DEFAULT_PRESSURE_ANGLE: f64 = 20.0 * PI / 180.0;

/// Involute function `inv(a) = tan(a) − a`.
#[inline]
pub fn involute(a: f64) -> f64 {
    a.tan() - a
}

/// Largest pressure angle [`inverse_involute`] will return (≈89°).
///
/// `inv` blows up at 90°; nothing in gear geometry gets near this, so a value
/// past it is a sign the caller's inputs are wrong rather than a case to solve.
const MAX_ROLL_ANGLE: f64 = 1.5533430342749532;

/// Inverse involute: the pressure angle whose `inv` is `v`.
///
/// Newton on `tan φ − φ − v`, safeguarded by the bracket `[0, MAX_ROLL_ANGLE]`
/// — `inv` is monotone there, so a Newton step that leaves the bracket is
/// replaced by bisection and the iteration cannot run away near the pole.
pub fn inverse_involute(v: f64) -> Result<f64, GearError> {
    if !v.is_finite() || v < 0.0 {
        return Err(GearError::InverseInvolute(v));
    }
    if v == 0.0 {
        return Ok(0.0);
    }
    if v > involute(MAX_ROLL_ANGLE) {
        return Err(GearError::InverseInvolute(v));
    }
    let (mut lo, mut hi) = (0.0_f64, MAX_ROLL_ANGLE);
    // The classic cube-root seed: inv(φ) ≈ φ³/3 for small φ.
    let mut phi = (3.0 * v).cbrt().clamp(lo, hi);
    for _ in 0..100 {
        let f = involute(phi) - v;
        if f > 0.0 {
            hi = phi;
        } else {
            lo = phi;
        }
        let d = phi.tan().powi(2); // d/dφ inv(φ)
        let step = if d > 0.0 { phi - f / d } else { f64::NAN };
        let next = if step.is_finite() && step > lo && step < hi {
            step
        } else {
            0.5 * (lo + hi)
        };
        // Converge on the step, not on the residual: `inv` is `tan φ − φ`, so
        // near the base circle its value is ~φ³/3 and a residual small in
        // absolute terms is still a large error in φ.
        let moved = (next - phi).abs();
        phi = next;
        if moved <= f64::EPSILON * phi.max(1e-9) {
            break;
        }
    }
    Ok(phi)
}

/// A spur gear: external or internal, profile-shifted, with the tip optionally
/// overridden (shortened addendum) and a circumferential backlash thinning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpurGear {
    /// Module (mm of reference pitch diameter per tooth).
    pub module: f64,
    /// Tooth count.
    pub teeth: u32,
    /// Profile shift `x`, in modules.
    pub profile_shift: f64,
    /// Pressure angle at the reference pitch circle, radians.
    pub pressure_angle: f64,
    /// True for an internal (ring) gear.
    pub internal: bool,
    /// Tip radius override, mm. `None` uses the nominal addendum.
    ///
    /// A shortened addendum is how a planetary train buys tip clearance
    /// against a milled root, so this is a first-class field rather than a
    /// derived one.
    pub tip_radius: Option<f64>,
    /// Circumferential tooth thinning at the reference pitch circle, mm.
    pub backlash_thinning: f64,
    /// Face width (the depth of cut), mm.
    pub face_width: f64,
}

impl SpurGear {
    /// An external gear with standard proportions and a 20° pressure angle.
    pub fn external(module: f64, teeth: u32) -> Self {
        Self {
            module,
            teeth,
            profile_shift: 0.0,
            pressure_angle: DEFAULT_PRESSURE_ANGLE,
            internal: false,
            tip_radius: None,
            backlash_thinning: 0.0,
            face_width: 1.0,
        }
    }

    /// An internal (ring) gear with standard proportions and a 20° pressure angle.
    pub fn internal(module: f64, teeth: u32) -> Self {
        Self {
            internal: true,
            ..Self::external(module, teeth)
        }
    }

    /// Set the profile shift, in modules.
    pub fn with_profile_shift(mut self, x: f64) -> Self {
        self.profile_shift = x;
        self
    }

    /// Override the tip radius (mm).
    pub fn with_tip_radius(mut self, r: f64) -> Self {
        self.tip_radius = Some(r);
        self
    }

    /// Set the circumferential backlash thinning (mm at the pitch circle).
    pub fn with_backlash_thinning(mut self, b: f64) -> Self {
        self.backlash_thinning = b;
        self
    }

    /// Set the face width (mm).
    pub fn with_face_width(mut self, w: f64) -> Self {
        self.face_width = w;
        self
    }

    /// Reject a spec that cannot make a gear. Every public method that depends
    /// on the proportions calls this first, so a bad spec fails at the call
    /// that used it rather than producing a plausible wrong number.
    pub fn validate(&self) -> Result<(), GearError> {
        if !self.module.is_finite() || self.module <= 0.0 {
            return Err(GearError::InvalidModule(self.module));
        }
        if self.teeth < 4 {
            return Err(GearError::TooFewTeeth(self.teeth));
        }
        if !self.pressure_angle.is_finite()
            || self.pressure_angle <= 0.0
            || self.pressure_angle >= PI / 2.0
        {
            return Err(GearError::InvalidPressureAngle(self.pressure_angle));
        }
        if !self.face_width.is_finite() || self.face_width <= 0.0 {
            return Err(GearError::InvalidFaceWidth(self.face_width));
        }
        if !self.backlash_thinning.is_finite() || self.backlash_thinning < 0.0 {
            return Err(GearError::InvalidBacklash(self.backlash_thinning));
        }
        if let Some(ra) = self.tip_radius {
            if !ra.is_finite() || ra <= 0.0 {
                return Err(GearError::InvalidTipRadius(ra));
            }
        }
        let (ra, rf) = (self.r_tip(), self.r_root());
        if self.internal {
            if ra >= rf {
                return Err(GearError::DegenerateProportions {
                    tip: ra,
                    root: rf,
                    internal: true,
                });
            }
            if ra <= self.r_base() {
                return Err(GearError::TipInsideBaseCircle {
                    tip: ra,
                    base: self.r_base(),
                });
            }
        } else if rf >= ra || rf <= 0.0 {
            return Err(GearError::DegenerateProportions {
                tip: ra,
                root: rf,
                internal: false,
            });
        }
        if self.tooth_half_angle_at(ra)? <= 0.0 {
            return Err(GearError::PointedTooth {
                teeth: self.teeth,
                tip_radius: ra,
            });
        }
        Ok(())
    }

    /// Reference pitch radius `r = m·z/2`.
    pub fn r_pitch(&self) -> f64 {
        self.module * self.teeth as f64 / 2.0
    }

    /// Base radius `r_b = r·cos α` — the circle the involute unwinds from.
    pub fn r_base(&self) -> f64 {
        self.r_pitch() * self.pressure_angle.cos()
    }

    /// Angular pitch `2π/z`, radians.
    pub fn angular_pitch(&self) -> f64 {
        2.0 * PI / self.teeth as f64
    }

    /// Nominal tip radius from the standard addendum and the profile shift.
    pub fn nominal_r_tip(&self) -> f64 {
        let ha = ADDENDUM_COEFFICIENT * self.module;
        if self.internal {
            self.r_pitch() - ha + self.profile_shift * self.module
        } else {
            self.r_pitch() + ha + self.profile_shift * self.module
        }
    }

    /// Tip radius: the override if there is one, else [`Self::nominal_r_tip`].
    pub fn r_tip(&self) -> f64 {
        self.tip_radius.unwrap_or_else(|| self.nominal_r_tip())
    }

    /// Nominal root radius (the ideal, sharp-cornered root the cutter aims at).
    ///
    /// The cutter generally cannot reach it — see [`ToothSpace`].
    pub fn r_root(&self) -> f64 {
        let hf = (ADDENDUM_COEFFICIENT + CLEARANCE_COEFFICIENT) * self.module;
        if self.internal {
            self.r_pitch() + hf + self.profile_shift * self.module
        } else {
            self.r_pitch() - hf + self.profile_shift * self.module
        }
    }

    /// Circular tooth thickness at the reference pitch circle, after backlash
    /// thinning.
    pub fn pitch_tooth_thickness(&self) -> f64 {
        let nominal = PI * self.module / 2.0
            + 2.0 * self.profile_shift * self.module * self.pressure_angle.tan();
        if self.internal {
            // +x thins the ring tooth; the backlash widens the space, which is
            // the same thing said from the other side.
            PI * self.module - nominal - self.backlash_thinning
        } else {
            nominal - self.backlash_thinning
        }
    }

    /// Half the angular width of the tooth **space** at the base circle.
    ///
    /// This is the one constant the whole flank hangs off: the space half-angle
    /// at any radius is this plus (external) or minus (internal) `inv φ(r)`.
    fn space_half_angle_at_base(&self) -> f64 {
        let inv_a = involute(self.pressure_angle);
        let rp = self.r_pitch();
        let s_space = PI * self.module - self.pitch_tooth_thickness();
        if self.internal {
            s_space / (2.0 * rp) + inv_a
        } else {
            self.angular_pitch() / 2.0 - (self.pitch_tooth_thickness() / (2.0 * rp) + inv_a)
        }
    }

    /// Pressure angle of the involute at radius `r`, `acos(r_b/r)`.
    ///
    /// Errors below the base circle, where there is no involute. Callers that
    /// want the radial continuation ask [`Self::space_half_angle_at`] instead.
    pub fn pressure_angle_at(&self, r: f64) -> Result<f64, GearError> {
        let rb = self.r_base();
        if r < rb {
            return Err(GearError::RadiusBelowBaseCircle {
                radius: r,
                base: rb,
            });
        }
        Ok((rb / r).clamp(-1.0, 1.0).acos())
    }

    /// True where the flank at `r` is a real involute; false where it is the
    /// radial continuation below the base circle.
    pub fn flank_is_involute_at(&self, r: f64) -> bool {
        r >= self.r_base()
    }

    /// Half the angular width of the tooth space at radius `r`.
    ///
    /// Below the base circle the flank is continued radially, so this is
    /// constant there (see the module docs).
    pub fn space_half_angle_at(&self, r: f64) -> Result<f64, GearError> {
        if !r.is_finite() || r <= 0.0 {
            return Err(GearError::InvalidRadius(r));
        }
        let inv_phi = involute((self.r_base() / r).clamp(-1.0, 1.0).acos());
        Ok(if self.internal {
            self.space_half_angle_at_base() - inv_phi
        } else {
            self.space_half_angle_at_base() + inv_phi
        })
    }

    /// Half the angular width of the tooth at radius `r`.
    pub fn tooth_half_angle_at(&self, r: f64) -> Result<f64, GearError> {
        Ok(self.angular_pitch() / 2.0 - self.space_half_angle_at(r)?)
    }

    /// Circular (arc) tooth thickness at radius `r`.
    pub fn tooth_thickness_at(&self, r: f64) -> Result<f64, GearError> {
        Ok(2.0 * r * self.tooth_half_angle_at(r)?)
    }

    /// Circular (arc) width of the tooth space at radius `r`.
    pub fn space_width_at(&self, r: f64) -> Result<f64, GearError> {
        Ok(2.0 * r * self.space_half_angle_at(r)?)
    }

    /// Tooth thickness at the tip circle — the land left on top of the tooth.
    ///
    /// A negative or vanishing land means the flanks meet before the tip: the
    /// tooth is pointed and the addendum has to come down.
    pub fn tip_land(&self) -> Result<f64, GearError> {
        self.tooth_thickness_at(self.r_tip())
    }

    /// Smallest profile shift that a rack cutter can generate without
    /// undercutting: `x_min = h_a* − z·sin²α/2`.
    pub fn undercut_limit_shift(&self) -> f64 {
        ADDENDUM_COEFFICIENT - self.teeth as f64 * self.pressure_angle.sin().powi(2) / 2.0
    }

    /// True when a rack cutter would undercut this external gear.
    ///
    /// Informational for a milled gear — nothing generates the flank here — but
    /// it is the standard reading of "this tooth count needs a shift", and the
    /// consequence it warns about (no involute low on the flank) is real: see
    /// [`Self::flank_is_involute_at`].
    pub fn is_rack_undercut(&self) -> bool {
        !self.internal && self.profile_shift < self.undercut_limit_shift()
    }

    /// True when the nominal root dives inside the base circle, so part of the
    /// flank is the radial continuation rather than an involute.
    pub fn root_below_base_circle(&self) -> bool {
        !self.internal && self.r_root() < self.r_base()
    }

    /// Centre angle of tooth `i`, radians.
    pub fn tooth_centre_angle(&self, i: u32) -> f64 {
        self.angular_pitch() * i as f64
    }

    /// Centre angle of tooth space `i` — the space that follows tooth `i`.
    pub fn space_centre_angle(&self, i: u32) -> f64 {
        self.angular_pitch() * (i as f64 + 0.5)
    }

    /// A point on the tooth-space flank at radius `r`, in the frame of space
    /// `i`: `side = +1` is the flank at increasing angle.
    ///
    /// The returned angle is absolute (not relative to the space centre).
    pub fn flank_angle(&self, i: u32, r: f64, side: f64) -> Result<f64, GearError> {
        Ok(self.space_centre_angle(i) + side.signum() * self.space_half_angle_at(r)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference train: module 1.0, 10/20/50 teeth, x = +0.47/0/+0.47,
    /// 20°, cut with a Ø1 end mill. Every number the tests check comes from
    /// `docs/cam-fixtures/gears-60-cnc.json`.
    pub(crate) fn sun() -> SpurGear {
        SpurGear::external(1.0, 10)
            .with_profile_shift(0.47)
            .with_tip_radius(6.45)
            .with_face_width(5.6)
    }

    pub(crate) fn planet() -> SpurGear {
        SpurGear::external(1.0, 20)
            .with_tip_radius(10.89)
            .with_face_width(5.0)
    }

    pub(crate) fn ring() -> SpurGear {
        SpurGear::internal(1.0, 50)
            .with_profile_shift(0.47)
            .with_face_width(6.0)
    }

    /// Radii that are pure definitions reproduce the fixture exactly: there is
    /// no iteration in them, so anything but bit-level agreement is a bug.
    #[test]
    fn fixture_radii_are_exact() {
        for (g, rp, rb, rf) in [
            (sun(), 5.0, 4.698463103929543, 4.22),
            (planet(), 10.0, 9.396926207859085, 8.75),
            (ring(), 25.0, 23.49231551964771, 26.72),
        ] {
            g.validate().unwrap();
            assert!((g.r_pitch() - rp).abs() < 1e-12, "pitch {}", g.r_pitch());
            assert!((g.r_base() - rb).abs() < 1e-12, "base {}", g.r_base());
            assert!((g.r_root() - rf).abs() < 1e-12, "root {}", g.r_root());
        }
        // The ring's tip is nominal; the two external tips are overrides.
        assert!((ring().r_tip() - 24.47).abs() < 1e-12);
        assert!((ring().nominal_r_tip() - 24.47).abs() < 1e-12);
        assert!((planet().nominal_r_tip() - 11.0).abs() < 1e-12);
        assert!((planet().r_tip() - 10.89).abs() < 1e-12);
    }

    #[test]
    fn fixture_tip_lands() {
        assert!((sun().tip_land().unwrap() - 0.26367882397676573).abs() < 1e-12);
        assert!((planet().tip_land().unwrap() - 0.8186418694711965).abs() < 1e-12);
    }

    /// Space width at the root. The fixture measures this off a shapely
    /// polygon whose root disc is a 256-gon *inscribed* in the root circle, and
    /// measures it at exactly that radius, so a slice of the test arc counts as
    /// inside the disc and the measurement reads short: sun −4.6e-3 (0.5 %),
    /// planet −1.4e-3 (0.12 %). The ring is measured 0.02 mm off its root,
    /// clear of the facets, and agrees to 5e-6 — which is what pins the
    /// explanation on the faceting rather than on the formula.
    #[test]
    fn fixture_space_widths() {
        let s = sun().space_width_at(4.22).unwrap();
        assert!((s - 0.9065812049673181).abs() < 5e-3, "sun space {s}");
        let p = planet().space_width_at(8.75).unwrap();
        assert!((p - 1.1122446879251624).abs() < 2e-3, "planet space {p}");
        let r = ring().space_width_at(26.70).unwrap();
        assert!((r - 0.44159073842207286).abs() < 1e-5, "ring space {r}");
    }

    /// Both externals in this train have their root inside the base circle, so
    /// the radial continuation is not an edge case here — it is most of the
    /// working root.
    #[test]
    fn reference_externals_run_below_the_base_circle() {
        assert!(sun().root_below_base_circle());
        assert!(planet().root_below_base_circle());
        assert!(!ring().root_below_base_circle());
        assert!(sun().flank_is_involute_at(4.70));
        assert!(!sun().flank_is_involute_at(4.69));
    }

    /// The inverse solve, over the whole range gear geometry uses. Below about
    /// 1e-3 rad `inv` is `tan a − a` losing every significant bit to
    /// cancellation, so the sweep starts where the function itself is still
    /// worth inverting.
    #[test]
    fn involute_inverse_round_trips() {
        let mut a = 1e-3;
        while a < 1.4 {
            let v = involute(a);
            let back = inverse_involute(v).unwrap();
            assert!((back - a).abs() < 1e-12, "inv⁻¹(inv({a})) = {back}");
            a *= 1.2;
        }
        assert!(inverse_involute(-1e-9).is_err());
        assert!(inverse_involute(1e6).is_err());
    }

    /// Backlash is a thinning, and it is the only thing that separates the
    /// tooth from the space: thinning by `b` widens the space by exactly `b`.
    #[test]
    fn backlash_thins_the_tooth_and_widens_the_space() {
        let sharp = planet();
        let thin = planet().with_backlash_thinning(0.03);
        let r = 10.0;
        let dt = sharp.tooth_thickness_at(r).unwrap() - thin.tooth_thickness_at(r).unwrap();
        let ds = thin.space_width_at(r).unwrap() - sharp.space_width_at(r).unwrap();
        assert!((dt - 0.03).abs() < 1e-12, "tooth {dt}");
        assert!((ds - 0.03).abs() < 1e-12, "space {ds}");

        let sharp = ring();
        let thin = ring().with_backlash_thinning(0.03);
        let r = 25.0;
        let dt = sharp.tooth_thickness_at(r).unwrap() - thin.tooth_thickness_at(r).unwrap();
        let ds = thin.space_width_at(r).unwrap() - sharp.space_width_at(r).unwrap();
        assert!((dt - 0.03).abs() < 1e-12, "ring tooth {dt}");
        assert!((ds - 0.03).abs() < 1e-12, "ring space {ds}");
    }

    /// A tooth and its space fill the pitch, at every radius, both parities.
    #[test]
    fn tooth_plus_space_is_the_angular_pitch() {
        for g in [sun(), planet(), ring()] {
            for r in [g.r_tip(), g.r_pitch(), g.r_root()] {
                let sum = g.tooth_half_angle_at(r).unwrap() + g.space_half_angle_at(r).unwrap();
                assert!((sum - g.angular_pitch() / 2.0).abs() < 1e-15);
            }
        }
    }

    #[test]
    fn degenerate_specs_are_refused() {
        assert!(SpurGear::external(0.0, 20).validate().is_err());
        assert!(SpurGear::external(1.0, 3).validate().is_err());
        assert!(SpurGear::external(1.0, 20)
            .with_face_width(0.0)
            .validate()
            .is_err());
        // A tip so far out that the flanks have already crossed.
        assert!(SpurGear::external(1.0, 20)
            .with_tip_radius(30.0)
            .validate()
            .is_err());
    }
}
