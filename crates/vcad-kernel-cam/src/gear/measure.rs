//! Measuring a cut gear: over pins, between pins, base tangent — and turning
//! the reading back into an offset for the next cut.
//!
//! All of it rests on one property: the locus of points a distance `ρ` from an
//! involute is another involute of the same base circle, rotated by `ρ/r_b`,
//! with the foot of the normal shifted by the same parameter. A pin in a tooth
//! space is a point on that locus, so the whole measurement is exact algebra
//! rather than a table lookup.

use super::{inverse_involute, involute, GearError, SpurGear};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// A measurement over (external) or between (internal) pins.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PinMeasurement {
    /// Pin or ball diameter, mm.
    pub pin_diameter: f64,
    /// Radius of the pin's centre, mm.
    pub pin_centre_radius: f64,
    /// Radius at which the pin touches the flank, mm.
    pub contact_radius: f64,
    /// Pressure angle at the pin's centre, radians.
    pub centre_pressure_angle: f64,
    /// The measurement itself, mm: over the pins for an external gear, between
    /// them for a ring.
    pub dimension: f64,
    /// True when the tooth count is even and the pins sit diametrically
    /// opposite; odd counts are measured across the nearest space instead.
    pub even_teeth: bool,
    /// How much `dimension` moves per mm of tooth thickness at the pitch
    /// circle — the sensitivity a compensation is divided by.
    pub sensitivity: f64,
}

/// A base tangent (span) measurement over `k` teeth.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpanMeasurement {
    /// Number of teeth spanned.
    pub teeth_spanned: u32,
    /// Base tangent length, mm.
    pub length: f64,
    /// Radius at which the anvils touch the flanks, mm.
    pub contact_radius: f64,
}

/// What to change after a test cut.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Compensation {
    /// Measured dimension, mm.
    pub measured: f64,
    /// Nominal dimension, mm.
    pub nominal: f64,
    /// Tooth thickness error at the pitch circle, mm (positive = too thick).
    pub thickness_error: f64,
    /// The same error read as a profile-shift error, in modules.
    pub profile_shift_error: f64,
    /// Offset to add to the cutter radius compensation, mm: negative cuts
    /// deeper. This is the knob on a contour op.
    pub tool_normal_offset: f64,
    /// Equivalent radial move of the whole tooth-space path, mm.
    pub tool_radial_offset: f64,
}

impl SpurGear {
    /// Half the angular width of the space at the base circle — the constant
    /// the pin equations hang off.
    fn space_half_angle_at_base_pub(&self) -> Result<f64, GearError> {
        self.space_half_angle_at(self.r_base())
    }

    /// Measurement over pins (external) or between pins (internal).
    ///
    /// `inv φ_M = ρ/r_b − σ_b` for an external gear and `σ_b − ρ/r_b` for a
    /// ring, where `σ_b` is the space half-angle at the base circle: the pin
    /// centre is the point of the offset involute that lands on the space
    /// centreline.
    pub fn over_pins(&self, pin_diameter: f64) -> Result<PinMeasurement, GearError> {
        self.validate()?;
        if !pin_diameter.is_finite() || pin_diameter <= 0.0 {
            return Err(GearError::InvalidPinDiameter(pin_diameter));
        }
        let rb = self.r_base();
        let rho = pin_diameter / 2.0;
        let delta = rho / rb;
        let sigma_b = self.space_half_angle_at_base_pub()?;
        let target = if self.internal {
            sigma_b - delta
        } else {
            delta - sigma_b
        };
        if target < 0.0 {
            return Err(GearError::InvalidPinDiameter(pin_diameter));
        }
        let phi_m = inverse_involute(target)?;
        let v = phi_m.tan();
        let centre_radius = rb * (1.0 + v * v).sqrt();
        let u = v + delta;
        let contact_radius = rb * (1.0 + u * u).sqrt();

        let even = self.teeth.is_multiple_of(2);
        let across = if even {
            2.0 * centre_radius
        } else {
            2.0 * centre_radius * (PI / (2.0 * self.teeth as f64)).cos()
        };
        let dimension = if self.internal {
            across - pin_diameter
        } else {
            across + pin_diameter
        };

        // dM/ds = cos α / sin φ_M (× the odd-tooth factor): from
        // d(inv φ_M)/ds = 1/(2r) and d(inv φ)/dφ = tan²φ.
        let factor = if even {
            1.0
        } else {
            (PI / (2.0 * self.teeth as f64)).cos()
        };
        let sensitivity = factor * self.pressure_angle.cos() / phi_m.sin();

        let low = self.r_root().min(self.r_tip()).max(rb);
        let high = self.r_root().max(self.r_tip());
        if contact_radius < low || contact_radius > high {
            return Err(GearError::PinContactOutsideFlank {
                pin_diameter,
                contact_radius,
                form_radius: low,
                tip_radius: self.r_tip(),
            });
        }
        Ok(PinMeasurement {
            pin_diameter,
            pin_centre_radius: centre_radius,
            contact_radius,
            centre_pressure_angle: phi_m,
            dimension,
            even_teeth: even,
            sensitivity: if self.internal {
                -sensitivity
            } else {
                sensitivity
            },
        })
    }

    /// The tooth thickness at the pitch circle that a measured dimension
    /// implies — the exact inverse of [`Self::over_pins`].
    pub fn tooth_thickness_from_over_pins(
        &self,
        pin_diameter: f64,
        measured: f64,
    ) -> Result<f64, GearError> {
        self.validate()?;
        if !pin_diameter.is_finite() || pin_diameter <= 0.0 {
            return Err(GearError::InvalidPinDiameter(pin_diameter));
        }
        let rb = self.r_base();
        let rho = pin_diameter / 2.0;
        let delta = rho / rb;
        let across = if self.internal {
            measured + pin_diameter
        } else {
            measured - pin_diameter
        };
        let factor = if self.teeth.is_multiple_of(2) {
            1.0
        } else {
            (PI / (2.0 * self.teeth as f64)).cos()
        };
        let centre_radius = across / (2.0 * factor);
        if centre_radius < rb {
            return Err(GearError::MeasurementOutOfRange(measured));
        }
        let phi_m = (rb / centre_radius).clamp(-1.0, 1.0).acos();
        let inv_phi = involute(phi_m);
        let rp = self.r_pitch();
        let inv_a = involute(self.pressure_angle);
        Ok(if self.internal {
            // σ_b = inv φ_M + δ, and σ_b = s_space/(2r) + inv α.
            let s_space = 2.0 * rp * (inv_phi + delta - inv_a);
            PI * self.module - s_space
        } else {
            // σ_b = δ − inv φ_M, and the tooth is the rest of the pitch.
            let sigma_b = delta - inv_phi;
            2.0 * rp * (self.angular_pitch() / 2.0 - sigma_b - inv_a)
        })
    }

    /// Pin diameter whose contact lands on the reference pitch circle.
    ///
    /// That is the classic "best size": contact at the pitch circle is where
    /// the reading is least sensitive to a pressure-angle or profile-shift
    /// error elsewhere on the flank. Bisected on the exact contact equation
    /// rather than taken from a table, so it follows the profile shift and the
    /// backlash thinning.
    pub fn recommended_pin_diameter(&self) -> Result<f64, GearError> {
        self.recommended_pin_diameter_for(self.r_pitch())
    }

    /// Pin diameter whose contact lands at `contact_radius`.
    pub fn recommended_pin_diameter_for(&self, contact_radius: f64) -> Result<f64, GearError> {
        self.validate()?;
        let rb = self.r_base();
        if contact_radius < rb {
            return Err(GearError::RadiusBelowBaseCircle {
                radius: contact_radius,
                base: rb,
            });
        }
        let u = ((contact_radius / rb).powi(2) - 1.0).sqrt();
        let sigma_b = self.space_half_angle_at_base_pub()?;
        // Contact at parameter u means the centre is at v = u − δ, and the
        // centre lands on the space centreline: inv(atan v) = ±(δ − σ_b).
        let residual = |delta: f64| -> f64 {
            let v = u - delta;
            if v < 0.0 {
                return f64::INFINITY;
            }
            let lhs = involute(v.atan());
            let rhs = if self.internal {
                sigma_b - delta
            } else {
                delta - sigma_b
            };
            lhs - rhs
        };
        let (mut lo, mut hi) = (0.0, u);
        // Residual runs from + to − (external) as δ grows; the ring is the
        // mirror, so orient the bisection by the endpoints.
        let increasing = residual(lo) < residual(hi.min(u * 0.999));
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            let r = residual(mid);
            if (r > 0.0) == increasing {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let delta = 0.5 * (lo + hi);
        Ok(2.0 * delta * rb)
    }

    /// Base tangent (span) length over `k` teeth, mm.
    ///
    /// Built from the base tooth thickness and the base pitch —
    /// `W = (k−1)·p_b + s_b` — so backlash thinning and profile shift flow
    /// through it without a second formula to keep in step.
    pub fn span_measurement(&self, teeth_spanned: u32) -> Result<SpanMeasurement, GearError> {
        self.validate()?;
        if self.internal {
            return Err(GearError::SpanOnInternalGear);
        }
        if teeth_spanned < 2 || teeth_spanned >= self.teeth {
            return Err(GearError::InvalidSpanTeeth(teeth_spanned));
        }
        let rb = self.r_base();
        let base_pitch = PI * self.module * self.pressure_angle.cos();
        let base_tooth = 2.0 * rb * (self.angular_pitch() / 2.0 - self.space_half_angle_at(rb)?);
        let length = (teeth_spanned - 1) as f64 * base_pitch + base_tooth;
        let contact_radius = rb.hypot(length / 2.0);
        Ok(SpanMeasurement {
            teeth_spanned,
            length,
            contact_radius,
        })
    }

    /// The span that puts the anvils closest to the pitch circle while still
    /// landing on the usable flank.
    ///
    /// `form_radius` is where the cutter's fillet starts — [`super::ToothSpace`]
    /// has it — so the answer is a span that can actually be measured on the
    /// part rather than one that lands on the fillet.
    pub fn recommended_span_teeth(&self, form_radius: f64) -> Result<u32, GearError> {
        let mut best: Option<(f64, u32)> = None;
        for k in 2..self.teeth {
            let Ok(span) = self.span_measurement(k) else {
                continue;
            };
            if span.contact_radius < form_radius || span.contact_radius > self.r_tip() {
                continue;
            }
            let err = (span.contact_radius - self.r_pitch()).abs();
            if best.is_none_or(|(b, _)| err < b) {
                best = Some((err, k));
            }
        }
        best.map(|(_, k)| k)
            .ok_or(GearError::InvalidSpanTeeth(self.teeth))
    }

    /// Turn a measurement into the offset the next cut needs.
    ///
    /// `measured` and `nominal` are over-pins dimensions taken with the same
    /// pin. A positive `thickness_error` means the teeth came out too thick, so
    /// the tool has to move into the material: `tool_normal_offset` is
    /// negative.
    pub fn compensation(
        &self,
        pin_diameter: f64,
        measured: f64,
        nominal: f64,
    ) -> Result<Compensation, GearError> {
        let want = self.tooth_thickness_from_over_pins(pin_diameter, nominal)?;
        let got = self.tooth_thickness_from_over_pins(pin_diameter, measured)?;
        let thickness_error = got - want;
        let profile_shift_error = thickness_error / (2.0 * self.module * self.pressure_angle.tan());
        Ok(Compensation {
            measured,
            nominal,
            thickness_error,
            profile_shift_error,
            // A flank moved `n` along its normal changes the circumferential
            // thickness by 2n/cos α, so invert that.
            tool_normal_offset: -thickness_error * self.pressure_angle.cos() / 2.0,
            // A profile-shift error of Δx is a radial move of Δx·m.
            tool_radial_offset: -profile_shift_error * self.module,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::*;

    /// Over pins and back, to the last bit the f64 has.
    #[test]
    fn over_pins_round_trips() {
        for g in [
            sun(),
            planet(),
            planet().with_backlash_thinning(0.03),
            ring(),
            SpurGear::external(1.0, 11).with_profile_shift(0.3),
        ] {
            let pin = g.recommended_pin_diameter().unwrap();
            let m = g.over_pins(pin).unwrap();
            let s = g.tooth_thickness_from_over_pins(pin, m.dimension).unwrap();
            let want = g.pitch_tooth_thickness();
            assert!(
                (s - want).abs() < 1e-9,
                "z{} internal={}: thickness {s} vs {want}",
                g.teeth,
                g.internal
            );
        }
    }

    /// An independent closed form for the one case that has one: on a standard
    /// external gear with an even tooth count, the pin whose *centre* sits on
    /// the pitch circle has ρ = π·m·cos α/4, and then M = m·z + d exactly.
    #[test]
    fn pin_on_the_pitch_circle_gives_m_equals_pitch_diameter_plus_pin() {
        for z in [20u32, 40, 64] {
            let g = SpurGear::external(1.0, z);
            let d = std::f64::consts::PI * g.pressure_angle.cos() / 2.0;
            let m = g.over_pins(d).unwrap();
            assert!(
                (m.pin_centre_radius - g.r_pitch()).abs() < 1e-12,
                "z{z}: centre {}",
                m.pin_centre_radius
            );
            assert!(
                (m.dimension - (g.module * z as f64 + d)).abs() < 1e-12,
                "z{z}: M {}",
                m.dimension
            );
        }
    }

    /// The recommended pin touches the flank exactly on the pitch circle.
    #[test]
    fn recommended_pin_contacts_at_the_pitch_circle() {
        for g in [
            sun(),
            planet(),
            planet().with_backlash_thinning(0.03),
            ring(),
        ] {
            let pin = g.recommended_pin_diameter().unwrap();
            let m = g.over_pins(pin).unwrap();
            assert!(
                (m.contact_radius - g.r_pitch()).abs() < 1e-6,
                "z{} internal={}: contact {} vs pitch {}",
                g.teeth,
                g.internal,
                m.contact_radius,
                g.r_pitch()
            );
        }
    }

    /// The published sensitivity dM/ds = cos α / sin φ_M, checked against a
    /// finite difference of the exact measurement.
    #[test]
    fn sensitivity_matches_a_finite_difference() {
        let g = planet().with_backlash_thinning(0.03);
        let pin = 1.5;
        let m0 = g.over_pins(pin).unwrap();
        let eps = 1e-6;
        // Thinning by eps reduces s by eps.
        let thinner = g.with_backlash_thinning(0.03 + eps);
        let m1 = thinner.over_pins(pin).unwrap();
        let fd = (m1.dimension - m0.dimension) / -eps;
        assert!(
            (fd - m0.sensitivity).abs() < 1e-5,
            "fd {fd} vs analytic {}",
            m0.sensitivity
        );
    }

    /// A gear cut 0.02 mm too thick: the compensation says move the tool in,
    /// and applying it lands on the nominal measurement.
    #[test]
    fn compensation_closes_the_loop() {
        let g = planet().with_backlash_thinning(0.03);
        let pin = 1.5;
        let nominal = g.over_pins(pin).unwrap().dimension;
        // Simulate a cut that left the teeth 0.02 thick by thinning 0.02 less.
        let cut = g.with_backlash_thinning(0.01);
        let measured = cut.over_pins(pin).unwrap().dimension;
        let comp = g.compensation(pin, measured, nominal).unwrap();
        assert!(
            (comp.thickness_error - 0.02).abs() < 1e-9,
            "thickness error {}",
            comp.thickness_error
        );
        assert!(comp.tool_normal_offset < 0.0, "should cut deeper");
        assert!((comp.tool_normal_offset + 0.02 * g.pressure_angle.cos() / 2.0).abs() < 1e-12);
        // Applying the radial offset as a profile-shift change reproduces the
        // nominal measurement.
        let fixed = g.with_profile_shift(g.profile_shift - comp.profile_shift_error);
        let after = fixed
            .with_backlash_thinning(0.01)
            .over_pins(pin)
            .unwrap()
            .dimension;
        assert!((after - nominal).abs() < 1e-9, "after {after} vs {nominal}");
    }

    /// Span from two independent derivations: the base-pitch sum this module
    /// uses, and the closed form `W = cos α[πm(k−½) + z·m·inv α] + 2xm·sin α`.
    #[test]
    fn span_matches_the_closed_form() {
        for g in [
            sun(),
            planet(),
            planet().with_backlash_thinning(0.03),
            SpurGear::external(2.0, 33).with_profile_shift(-0.2),
        ] {
            for k in 2..5 {
                let Ok(span) = g.span_measurement(k) else {
                    continue;
                };
                let a = g.pressure_angle;
                let closed = a.cos()
                    * (PI * g.module * (k as f64 - 0.5) + g.teeth as f64 * g.module * involute(a))
                    + 2.0 * g.profile_shift * g.module * a.sin()
                    - g.backlash_thinning * a.cos();
                assert!(
                    (span.length - closed).abs() < 1e-12,
                    "z{} k{k}: {} vs {closed}",
                    g.teeth,
                    span.length
                );
            }
        }
        assert!(ring().span_measurement(3).is_err());
    }

    /// The numbers the first milestone is measured against: one 20 T brass
    /// planet, module 1.0, x = 0, thinned 0.03 for backlash, cut with a Ø1 end
    /// mill. Pinned here so a change in the geometry shows up as a changed
    /// expected reading rather than as a part that measures wrong.
    #[test]
    fn milestone_planet_measurements() {
        let p = planet().with_backlash_thinning(0.03);
        let space = p.tooth_space(1.0).unwrap();

        // Best-size pin (contact on the pitch circle) and the stock sizes a
        // shop actually has. All three contact well inside the usable flank.
        for (pin, want_m, want_contact) in [
            (p.recommended_pin_diameter().unwrap(), 20.944636, 10.0),
            (1.4, 21.073815, 10.0662),
            (1.5, 21.487525, 10.2733),
        ] {
            let m = p.over_pins(pin).unwrap();
            assert!(
                (m.dimension - want_m).abs() < 1e-5,
                "pin {pin}: M {}",
                m.dimension
            );
            assert!((m.contact_radius - want_contact).abs() < 1e-3);
            assert!(m.contact_radius > space.form_radius && m.contact_radius < p.r_tip());
        }
        assert!(
            (p.recommended_pin_diameter().unwrap() - 1.371141).abs() < 1e-5,
            "best pin {}",
            p.recommended_pin_diameter().unwrap()
        );
        // A Ø2 pin rides above the tip: refused, not quietly extrapolated.
        assert!(p.over_pins(2.0).is_err());

        // The micrometer cross-check.
        let k = p.recommended_span_teeth(space.form_radius).unwrap();
        let s = p.span_measurement(k).unwrap();
        assert_eq!(k, 3);
        assert!((s.length - 7.632249).abs() < 1e-5, "span {}", s.length);

        // And what to do about a reading 0.02 mm over, on a Ø1.5 pin.
        let nominal = p.over_pins(1.5).unwrap().dimension;
        let c = p.compensation(1.5, nominal + 0.02, nominal).unwrap();
        assert!((c.thickness_error - 0.007273).abs() < 1e-6);
        assert!((c.tool_normal_offset + 0.003417).abs() < 1e-6);
        assert!((c.tool_radial_offset + 0.009991).abs() < 1e-6);
    }

    /// The recommended span lands on flank the cutter left intact.
    #[test]
    fn recommended_span_lands_on_live_involute() {
        let g = planet();
        let form = g.tooth_space(1.0).unwrap().form_radius;
        let k = g.recommended_span_teeth(form).unwrap();
        let span = g.span_measurement(k).unwrap();
        assert!(span.contact_radius >= form && span.contact_radius <= g.r_tip());
        assert_eq!(k, 3, "z20 20° spans 3 teeth");
    }

    /// A pin too big to touch the flank is refused with the radius it would
    /// have touched at.
    #[test]
    fn impossible_pins_fail_closed() {
        assert!(planet().over_pins(0.0).is_err());
        assert!(matches!(
            planet().over_pins(4.0),
            Err(GearError::PinContactOutsideFlank { .. })
        ));
    }
}
