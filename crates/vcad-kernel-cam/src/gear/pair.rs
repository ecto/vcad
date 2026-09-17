//! A meshing pair: working pressure angle, centre distance, contact.

use super::{inverse_involute, involute, GearError, SpurGear};
use serde::{Deserialize, Serialize};

/// Two gears in mesh: `pinion` is always external, `wheel` may be internal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GearPair {
    /// The external member (sun, or planet in the ring mesh).
    pub pinion: SpurGear,
    /// The mating gear: external for an ordinary mesh, internal for a ring.
    pub wheel: SpurGear,
}

/// Where contact starts and stops on each member of a pair.
///
/// "Lowest" and "highest" are radii, not roles: on an internal ring the active
/// flank runs *outward* from its tip, so its highest contact radius is the one
/// the fillet must stay clear of.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContactRadii {
    /// Lowest radius at which the pinion flank carries contact, mm.
    pub pinion_lowest: f64,
    /// Highest radius at which the pinion flank carries contact, mm.
    pub pinion_highest: f64,
    /// Lowest radius at which the wheel flank carries contact, mm.
    pub wheel_lowest: f64,
    /// Highest radius at which the wheel flank carries contact, mm.
    pub wheel_highest: f64,
}

/// Everything a pair has to answer for before it is cut.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeshReport {
    /// True when the wheel is internal.
    pub internal: bool,
    /// Working pressure angle, radians.
    pub working_pressure_angle: f64,
    /// Operating centre distance, mm.
    pub centre_distance: f64,
    /// Transverse contact ratio.
    pub contact_ratio: f64,
    /// Length of the path of contact, mm.
    pub path_of_contact: f64,
    /// Contact limits on both members.
    pub contact: ContactRadii,
    /// Normal backlash from the two thinnings, mm.
    pub normal_backlash: f64,
    /// Circumferential backlash at the working pitch circles, mm.
    pub circumferential_backlash: f64,
    /// Radial clearance at the pinion tip against the wheel's nominal root, mm.
    pub pinion_tip_clearance: f64,
    /// Radial clearance at the wheel tip against the pinion's nominal root, mm.
    pub wheel_tip_clearance: f64,
}

/// Working pressure angle of a pair from its profile shifts.
///
/// External: `inv αw = inv α + 2(x₁+x₂)tanα/(z₁+z₂)`.
/// Internal: `inv αw = inv α + 2(x₂−x₁)tanα/(z₂−z₁)` with subscript 2 the ring.
pub fn working_pressure_angle(
    z_pinion: u32,
    z_wheel: u32,
    x_pinion: f64,
    x_wheel: f64,
    pressure_angle: f64,
    internal: bool,
) -> Result<f64, GearError> {
    let (zp, zw) = (z_pinion as f64, z_wheel as f64);
    let (shift_sum, tooth_sum) = if internal {
        if z_wheel <= z_pinion {
            return Err(GearError::InternalToothCounts {
                pinion: z_pinion,
                ring: z_wheel,
            });
        }
        (x_wheel - x_pinion, zw - zp)
    } else {
        (x_pinion + x_wheel, zp + zw)
    };
    let target = involute(pressure_angle) + 2.0 * shift_sum * pressure_angle.tan() / tooth_sum;
    inverse_involute(target).map_err(|_| GearError::NoWorkingPressureAngle { shift_sum })
}

/// The profile-shift sum a pair needs to run at centre distance `a` — the
/// inverse of [`working_pressure_angle`].
pub fn profile_shift_sum_for_centre_distance(
    module: f64,
    z_pinion: u32,
    z_wheel: u32,
    pressure_angle: f64,
    internal: bool,
    centre_distance: f64,
) -> Result<f64, GearError> {
    let (zp, zw) = (z_pinion as f64, z_wheel as f64);
    let tooth_sum = if internal { zw - zp } else { zp + zw };
    if tooth_sum <= 0.0 {
        return Err(GearError::InternalToothCounts {
            pinion: z_pinion,
            ring: z_wheel,
        });
    }
    let a_ref = module * tooth_sum / 2.0;
    let cos_aw = a_ref * pressure_angle.cos() / centre_distance;
    if !(-1.0..=1.0).contains(&cos_aw) || centre_distance <= 0.0 {
        return Err(GearError::InvalidCentreDistance(centre_distance));
    }
    let aw = cos_aw.acos();
    Ok((involute(aw) - involute(pressure_angle)) * tooth_sum / (2.0 * pressure_angle.tan()))
}

impl GearPair {
    /// A pair of external gears.
    pub fn external(pinion: SpurGear, wheel: SpurGear) -> Self {
        Self { pinion, wheel }
    }

    /// A pinion running in an internal ring.
    pub fn internal(pinion: SpurGear, ring: SpurGear) -> Self {
        Self {
            pinion,
            wheel: ring,
        }
    }

    /// True when the wheel is internal.
    pub fn is_internal(&self) -> bool {
        self.wheel.internal
    }

    fn check(&self) -> Result<(), GearError> {
        self.pinion.validate()?;
        self.wheel.validate()?;
        if self.pinion.internal {
            return Err(GearError::InvalidPairing);
        }
        if (self.pinion.module - self.wheel.module).abs() > 1e-12 {
            return Err(GearError::ModuleMismatch {
                a: self.pinion.module,
                b: self.wheel.module,
            });
        }
        if (self.pinion.pressure_angle - self.wheel.pressure_angle).abs() > 1e-12 {
            return Err(GearError::PressureAngleMismatch {
                a: self.pinion.pressure_angle,
                b: self.wheel.pressure_angle,
            });
        }
        Ok(())
    }

    /// Working pressure angle, radians.
    pub fn working_pressure_angle(&self) -> Result<f64, GearError> {
        self.check()?;
        working_pressure_angle(
            self.pinion.teeth,
            self.wheel.teeth,
            self.pinion.profile_shift,
            self.wheel.profile_shift,
            self.pinion.pressure_angle,
            self.is_internal(),
        )
    }

    /// Operating centre distance `a = a_ref·cos α / cos αw`, mm.
    pub fn centre_distance(&self) -> Result<f64, GearError> {
        let aw = self.working_pressure_angle()?;
        let (zp, zw) = (self.pinion.teeth as f64, self.wheel.teeth as f64);
        let tooth_sum = if self.is_internal() { zw - zp } else { zp + zw };
        Ok(self.pinion.module * tooth_sum / 2.0 * self.pinion.pressure_angle.cos() / aw.cos())
    }

    /// The working pressure angle a given centre distance implies — the
    /// inverse of [`Self::centre_distance`], for reading a measured centre
    /// distance back into the mesh.
    pub fn working_pressure_angle_at(&self, centre_distance: f64) -> Result<f64, GearError> {
        self.check()?;
        let (rbp, rbw) = (self.pinion.r_base(), self.wheel.r_base());
        let sum = if self.is_internal() {
            rbw - rbp
        } else {
            rbp + rbw
        };
        if centre_distance <= 0.0 || sum / centre_distance > 1.0 {
            return Err(GearError::InvalidCentreDistance(centre_distance));
        }
        Ok((sum / centre_distance).acos())
    }

    /// Length of the path of contact, mm, at the operating centre distance.
    pub fn path_of_contact(&self) -> Result<f64, GearError> {
        let a = self.centre_distance()?;
        let aw = self.working_pressure_angle()?;
        let (rbp, rbw) = (self.pinion.r_base(), self.wheel.r_base());
        let tp = tangent_length(self.pinion.r_tip(), rbp)?;
        let tw = tangent_length(self.wheel.r_tip(), rbw)?;
        Ok(if self.is_internal() {
            tp - tw + a * aw.sin()
        } else {
            tp + tw - a * aw.sin()
        })
    }

    /// Transverse contact ratio: path of contact over base pitch.
    pub fn contact_ratio(&self) -> Result<f64, GearError> {
        let base_pitch =
            std::f64::consts::PI * self.pinion.module * self.pinion.pressure_angle.cos();
        Ok(self.path_of_contact()? / base_pitch)
    }

    /// Where contact starts and stops on each flank.
    pub fn contact_radii(&self) -> Result<ContactRadii, GearError> {
        let a = self.centre_distance()?;
        let aw = self.working_pressure_angle()?;
        let (rbp, rbw) = (self.pinion.r_base(), self.wheel.r_base());
        let tp = tangent_length(self.pinion.r_tip(), rbp)?;
        let tw = tangent_length(self.wheel.r_tip(), rbw)?;
        let sin_aw = a * aw.sin();
        Ok(if self.is_internal() {
            // The ring's active flank runs outward from its tip; the pinion's
            // runs outward from the point the ring tip reaches down to.
            ContactRadii {
                pinion_lowest: rbp.hypot(tw - sin_aw),
                pinion_highest: self.pinion.r_tip(),
                wheel_lowest: self.wheel.r_tip(),
                wheel_highest: rbw.hypot(tp + sin_aw),
            }
        } else {
            ContactRadii {
                pinion_lowest: rbp.hypot(sin_aw - tw),
                pinion_highest: self.pinion.r_tip(),
                wheel_lowest: rbw.hypot(sin_aw - tp),
                wheel_highest: self.wheel.r_tip(),
            }
        })
    }

    /// Normal backlash: the play measured along the line of action, mm.
    ///
    /// Thinning a tooth by `b` at the reference pitch circle rotates each of
    /// its flanks by `b/(2r)`, which moves the flank `b/2·cos α` along the line
    /// of action — so the two members' thinnings simply add.
    pub fn normal_backlash(&self) -> f64 {
        (self.pinion.backlash_thinning + self.wheel.backlash_thinning)
            * self.pinion.pressure_angle.cos()
    }

    /// Backlash measured circumferentially at the working pitch circles, mm.
    pub fn circumferential_backlash(&self) -> Result<f64, GearError> {
        Ok(self.normal_backlash() / self.working_pressure_angle()?.cos())
    }

    /// Radial clearance between the pinion tip and the wheel's nominal root, mm.
    pub fn pinion_tip_clearance(&self) -> Result<f64, GearError> {
        let a = self.centre_distance()?;
        Ok(if self.is_internal() {
            self.wheel.r_root() - a - self.pinion.r_tip()
        } else {
            a - self.pinion.r_tip() - self.wheel.r_root()
        })
    }

    /// Radial clearance between the wheel tip and the pinion's nominal root, mm.
    pub fn wheel_tip_clearance(&self) -> Result<f64, GearError> {
        let a = self.centre_distance()?;
        Ok(if self.is_internal() {
            // The ring tip points inward; its closest approach to the pinion
            // centre is |r_tip − a|, and the ring tip circle is the larger one.
            self.wheel.r_tip() - a - self.pinion.r_root()
        } else {
            a - self.wheel.r_tip() - self.pinion.r_root()
        })
    }

    /// Tip clearance measured against what the cutter actually leaves rather
    /// than the nominal root: the number that decides whether the tip bottoms.
    pub fn tip_clearance_against_cut_root(
        &self,
        cutter_diameter: f64,
    ) -> Result<(f64, f64), GearError> {
        let a = self.centre_distance()?;
        let pinion_root = self
            .pinion
            .tooth_space(cutter_diameter)?
            .effective_root_radius;
        let wheel_root = self
            .wheel
            .tooth_space(cutter_diameter)?
            .effective_root_radius;
        Ok(if self.is_internal() {
            (
                wheel_root - a - self.pinion.r_tip(),
                self.wheel.r_tip() - a - pinion_root,
            )
        } else {
            (
                a - self.pinion.r_tip() - wheel_root,
                a - self.wheel.r_tip() - pinion_root,
            )
        })
    }

    /// Minimum clearance between the two tip lands over a full mesh cycle, mm.
    ///
    /// This is the trimming (tip-interference) check that matters for an
    /// internal pair, where the pinion tip and ring tip pass each other on
    /// every tooth. Negative means they collide. It is measured, not asserted:
    /// the two tip-land arcs are swept through one angular pitch at `samples`
    /// steps and the closest approach is reported.
    pub fn tip_land_clearance(&self, samples: usize) -> Result<f64, GearError> {
        let a = self.centre_distance()?;
        let samples = samples.max(8);
        let (rap, raw) = (self.pinion.r_tip(), self.wheel.r_tip());
        if !self.is_internal() {
            // External tips never approach: the closest the two tip circles get
            // is along the line of centres.
            return Ok(a - rap - raw);
        }
        let half_p = self.pinion.tooth_half_angle_at(rap)?;
        let half_w = self.wheel.tooth_half_angle_at(raw)?;
        let ratio = self.pinion.teeth as f64 / self.wheel.teeth as f64;
        // Phase so that a pinion tooth points into a ring space at delta = 0:
        // the pinion's tooth 0 points along the line of centres, so the ring is
        // rolled back half a pitch to put a space there.
        let pinion_phase = 0.0;
        let wheel_phase = -self.wheel.angular_pitch() / 2.0;
        let mut worst = f64::INFINITY;
        for i in 0..=samples {
            let delta = (i as f64 / samples as f64 - 0.5) * self.pinion.angular_pitch();
            // Pinion tip land, in world coordinates (pinion centre at (a, 0)).
            for j in 0..=samples {
                let tp = (j as f64 / samples as f64 - 0.5) * 2.0 * half_p;
                let ang = pinion_phase + delta + tp;
                let p = (a + rap * ang.cos(), rap * ang.sin());
                for k in 0..=samples {
                    let tw = (k as f64 / samples as f64 - 0.5) * 2.0 * half_w;
                    let ang = wheel_phase + delta * ratio + tw;
                    let q = (raw * ang.cos(), raw * ang.sin());
                    let d = (p.0 - q.0).hypot(p.1 - q.1);
                    if d < worst {
                        worst = d;
                    }
                }
            }
        }
        Ok(worst)
    }

    /// Everything above, in one serializable answer.
    pub fn mesh_report(&self) -> Result<MeshReport, GearError> {
        Ok(MeshReport {
            internal: self.is_internal(),
            working_pressure_angle: self.working_pressure_angle()?,
            centre_distance: self.centre_distance()?,
            contact_ratio: self.contact_ratio()?,
            path_of_contact: self.path_of_contact()?,
            contact: self.contact_radii()?,
            normal_backlash: self.normal_backlash(),
            circumferential_backlash: self.circumferential_backlash()?,
            pinion_tip_clearance: self.pinion_tip_clearance()?,
            wheel_tip_clearance: self.wheel_tip_clearance()?,
        })
    }
}

/// Tangent length from the tip circle to the base circle, `√(ra² − rb²)`.
fn tangent_length(r_tip: f64, r_base: f64) -> Result<f64, GearError> {
    if r_tip < r_base {
        return Err(GearError::RadiusBelowBaseCircle {
            radius: r_tip,
            base: r_base,
        });
    }
    Ok((r_tip * r_tip - r_base * r_base).sqrt())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::*;

    fn sun_planet() -> GearPair {
        GearPair::external(sun(), planet())
    }

    fn planet_ring() -> GearPair {
        GearPair::internal(planet(), ring())
    }

    /// The fixture's headline numbers. These are closed-form apart from the
    /// inverse involute, which is solved to 1e-16, so 1e-9 is loose.
    #[test]
    fn fixture_working_angles_and_centre_distance() {
        let sp = sun_planet();
        let pr = planet_ring();
        let aw = sp.working_pressure_angle().unwrap().to_degrees();
        assert!((aw - 23.98817744086808).abs() < 1e-9, "aw_sp {aw}");
        let aw = pr.working_pressure_angle().unwrap().to_degrees();
        assert!((aw - 23.98817744086808).abs() < 1e-9, "aw_pr {aw}");
        let a = sp.centre_distance().unwrap();
        assert!((a - 15.427907472541792).abs() < 1e-9, "a {a}");
        let a = pr.centre_distance().unwrap();
        assert!((a - 15.427907472541792).abs() < 1e-9, "a_ring {a}");
    }

    /// The inverse: the shift sum that puts the pair at that centre distance,
    /// and the working angle read back from it.
    #[test]
    fn centre_distance_inverts() {
        let a = 15.427907472541792;
        let sum =
            profile_shift_sum_for_centre_distance(1.0, 10, 20, sun().pressure_angle, false, a)
                .unwrap();
        assert!((sum - 0.47).abs() < 1e-9, "external shift sum {sum}");
        let diff =
            profile_shift_sum_for_centre_distance(1.0, 20, 50, sun().pressure_angle, true, a)
                .unwrap();
        assert!(
            (diff - 0.47).abs() < 1e-9,
            "internal shift difference {diff}"
        );

        let aw = sun_planet().working_pressure_angle_at(a).unwrap();
        assert!((aw - sun_planet().working_pressure_angle().unwrap()).abs() < 1e-12);
        let aw = planet_ring().working_pressure_angle_at(a).unwrap();
        assert!((aw - planet_ring().working_pressure_angle().unwrap()).abs() < 1e-12);
    }

    #[test]
    fn fixture_contact_ratios() {
        let cr = sun_planet().contact_ratio().unwrap();
        assert!((cr - 1.2365192525876858).abs() < 1e-9, "cr_sp {cr}");
        let cr = planet_ring().contact_ratio().unwrap();
        assert!((cr - 1.6693148397641442).abs() < 1e-9, "cr_pr {cr}");
    }

    #[test]
    fn fixture_contact_radii() {
        let c = sun_planet().contact_radii().unwrap();
        assert!((c.pinion_lowest - 4.760907943575896).abs() < 1e-9, "{c:?}");
        assert!((c.wheel_lowest - 9.577932062278455).abs() < 1e-9, "{c:?}");
        let c = planet_ring().contact_radii().unwrap();
        assert!((c.pinion_lowest - 9.414537845344137).abs() < 1e-9, "{c:?}");
        assert!((c.wheel_highest - 26.278481948457706).abs() < 1e-9, "{c:?}");
    }

    /// Clearances against the *nominal* roots, as the fixture reports them
    /// (it quotes them against the milled roots; the difference is the cutter
    /// reach, checked in `cutter.rs`).
    #[test]
    fn fixture_clearances() {
        // fixture clr_sun: planet tip vs the sun's milled root (4.2197)
        let c = sun_planet().tip_clearance_against_cut_root(1.0).unwrap();
        assert!((c.1 - 0.3182241947343103).abs() < 1e-3, "clr_sun {:?}", c);
        // fixture clr_planet: sun tip vs the planet's milled root (8.7494)
        assert!(
            (c.0 - 0.2285429222926414).abs() < 1e-3,
            "clr_planet {:?}",
            c
        );
        // fixture clr_ring: planet tip vs the ring's milled root (26.5258)
        let c = planet_ring().tip_clearance_against_cut_root(1.0).unwrap();
        assert!((c.0 - 0.2078646441701668).abs() < 1e-3, "clr_ring {:?}", c);
    }

    /// Backlash is thinning turned into play along the line of action.
    #[test]
    fn backlash_adds_across_the_pair() {
        let pair = GearPair::external(
            sun().with_backlash_thinning(0.03),
            planet().with_backlash_thinning(0.03),
        );
        let jn = pair.normal_backlash();
        assert!((jn - 0.06 * 20f64.to_radians().cos()).abs() < 1e-15, "{jn}");
        assert!((jn - 0.056381557).abs() < 1e-8, "{jn}");
    }

    /// The planet tip and the ring tip pass each other every tooth; they must
    /// not touch. 0.18 mm is what this train leaves.
    #[test]
    fn planet_and_ring_tips_clear_each_other() {
        let clearance = planet_ring().tip_land_clearance(48).unwrap();
        assert!(clearance > 0.05, "tip land clearance {clearance}");
    }

    /// Dense world-frame sample of the working flanks and tip lands of one
    /// gear, rotated by `phase` about its own centre and placed at `centre`.
    ///
    /// Only the parts that can meet the mate are sampled: the flanks from the
    /// form radius out to the tip, and the tip land. The root and the fillet
    /// clear by an order of magnitude more (0.21–0.32 mm here against 0.028 of
    /// backlash), so leaving them out cannot hide an interference — and
    /// `tip_clearance_against_cut_root` checks them directly.
    fn mesh_samples(g: &SpurGear, phase: f64, centre: (f64, f64)) -> Vec<(f64, f64)> {
        let space = g.tooth_space(1.0).expect("Ø1 cutter fits");
        let (lo, hi) = if g.internal {
            (g.r_tip(), space.form_radius)
        } else {
            (space.form_radius, g.r_tip())
        };
        let steps = 240;
        let mut pts = Vec::new();
        for i in 0..g.teeth {
            for side in [-1.0_f64, 1.0] {
                for k in 0..=steps {
                    let r = lo + (hi - lo) * k as f64 / steps as f64;
                    let a =
                        g.space_centre_angle(i) + side * g.space_half_angle_at(r).unwrap() + phase;
                    pts.push((centre.0 + r * a.cos(), centre.1 + r * a.sin()));
                }
            }
            // Tip land.
            let ra = g.r_tip();
            let half = g.tooth_half_angle_at(ra).unwrap();
            for k in 0..=24 {
                let a = g.tooth_centre_angle(i) - half + 2.0 * half * k as f64 / 24.0 + phase;
                pts.push((centre.0 + ra * a.cos(), centre.1 + ra * a.sin()));
            }
        }
        pts
    }

    /// Closest approach between two sampled boundaries, restricted to a window
    /// around the mesh so the far side of each gear cannot win.
    fn closest_approach(a: &[(f64, f64)], b: &[(f64, f64)], window: (f64, f64), r: f64) -> f64 {
        let near = |p: &&(f64, f64)| (p.0 - window.0).hypot(p.1 - window.1) < r;
        let a: Vec<_> = a.iter().filter(near).copied().collect();
        let b: Vec<_> = b.iter().filter(near).copied().collect();
        assert!(!a.is_empty() && !b.is_empty(), "nothing in the mesh window");
        let mut best = f64::INFINITY;
        for p in &a {
            for q in &b {
                let d = (p.0 - q.0).hypot(p.1 - q.1);
                if d < best {
                    best = d;
                }
            }
        }
        best
    }

    /// Conjugacy, measured rather than assumed: turn the generated sun and
    /// planet against each other at the working centre distance and watch the
    /// gap between the flanks.
    ///
    /// Two things have to be true at once. The gap must never close (no
    /// interference), and it must not *change* — a pair of profiles that are
    /// not conjugate would breathe as the teeth roll through, which is exactly
    /// what the transmission error of a wrong profile is. With both members
    /// centred in each other's spaces the gap is half the designed backlash,
    /// `(b₁+b₂)·cos α / 2 = 0.0282 mm`, on each flank.
    #[test]
    fn sun_and_planet_are_conjugate_with_the_designed_backlash() {
        let s = sun().with_backlash_thinning(0.03);
        let p = planet().with_backlash_thinning(0.03);
        let pair = GearPair::external(s, p);
        let a = pair.centre_distance().unwrap();
        let expected = pair.normal_backlash() / 2.0;
        let pitch_point = (a * s.teeth as f64 / (s.teeth + p.teeth) as f64, 0.0);
        // A sun tooth centred in a planet space at delta = 0.
        let planet_phase = std::f64::consts::PI - p.angular_pitch() / 2.0;
        let ratio = -(s.teeth as f64) / p.teeth as f64;

        let (mut lo, mut hi) = (f64::INFINITY, 0.0_f64);
        for k in 0..=20 {
            let delta = s.angular_pitch() * k as f64 / 20.0;
            let sp = mesh_samples(&s, delta, (0.0, 0.0));
            let pp = mesh_samples(&p, planet_phase + delta * ratio, (a, 0.0));
            let gap = closest_approach(&sp, &pp, pitch_point, 2.5);
            lo = lo.min(gap);
            hi = hi.max(gap);
        }
        assert!(lo > 0.0, "flanks interfere: min gap {lo}");
        assert!(
            (lo - expected).abs() < 5e-4,
            "min gap {lo} vs designed half-backlash {expected}"
        );
        assert!(
            hi - lo < 5e-4,
            "gap breathes by {} over the sweep: not conjugate",
            hi - lo
        );
    }

    /// The same for the internal mesh, where the ring's space is widened by its
    /// backlash instead of its tooth being thinned.
    #[test]
    fn planet_and_ring_are_conjugate_with_the_designed_backlash() {
        let p = planet().with_backlash_thinning(0.03);
        let r = ring().with_backlash_thinning(0.03);
        let pair = GearPair::internal(p, r);
        let a = pair.centre_distance().unwrap();
        let expected = pair.normal_backlash() / 2.0;
        let pitch_point = (a * r.teeth as f64 / (r.teeth - p.teeth) as f64, 0.0);
        // A planet tooth centred in a ring space at delta = 0.
        let ring_phase = -r.angular_pitch() / 2.0;
        let ratio = p.teeth as f64 / r.teeth as f64;

        let (mut lo, mut hi) = (f64::INFINITY, 0.0_f64);
        for k in 0..=20 {
            let delta = p.angular_pitch() * k as f64 / 20.0;
            let pp = mesh_samples(&p, delta, (a, 0.0));
            let rp = mesh_samples(&r, ring_phase + delta * ratio, (0.0, 0.0));
            let gap = closest_approach(&pp, &rp, pitch_point, 2.5);
            lo = lo.min(gap);
            hi = hi.max(gap);
        }
        assert!(lo > 0.0, "flanks interfere: min gap {lo}");
        assert!(
            (lo - expected).abs() < 5e-4,
            "min gap {lo} vs designed half-backlash {expected}"
        );
        assert!(
            hi - lo < 5e-4,
            "gap breathes by {} over the sweep: not conjugate",
            hi - lo
        );
    }

    /// The conjugacy sweep has teeth: spread the centre distance by 0.05 mm and
    /// the gap opens and starts to breathe, which is what the test is watching
    /// for. (A mutation check kept as a test, so it cannot rot.)
    #[test]
    fn conjugacy_sweep_detects_a_wrong_centre_distance() {
        let s = sun().with_backlash_thinning(0.03);
        let p = planet().with_backlash_thinning(0.03);
        let pair = GearPair::external(s, p);
        let a = pair.centre_distance().unwrap() + 0.05;
        let pitch_point = (a * s.teeth as f64 / (s.teeth + p.teeth) as f64, 0.0);
        let planet_phase = std::f64::consts::PI - p.angular_pitch() / 2.0;
        let ratio = -(s.teeth as f64) / p.teeth as f64;
        let (mut lo, mut hi) = (f64::INFINITY, 0.0_f64);
        for k in 0..=20 {
            let delta = s.angular_pitch() * k as f64 / 20.0;
            let sp = mesh_samples(&s, delta, (0.0, 0.0));
            let pp = mesh_samples(&p, planet_phase + delta * ratio, (a, 0.0));
            let gap = closest_approach(&sp, &pp, pitch_point, 2.5);
            lo = lo.min(gap);
            hi = hi.max(gap);
        }
        let expected = pair.normal_backlash() / 2.0;
        assert!(
            (lo - expected).abs() > 5e-4,
            "a 0.05 mm centre-distance error should move the gap, got {lo}"
        );
    }

    /// A pair whose modules differ is refused, not averaged.
    #[test]
    fn mismatched_pairs_are_refused() {
        let pair = GearPair::external(SpurGear::external(1.0, 10), SpurGear::external(1.5, 20));
        assert!(matches!(
            pair.centre_distance(),
            Err(GearError::ModuleMismatch { .. })
        ));
        let pair = GearPair::internal(ring(), planet());
        assert!(pair.centre_distance().is_err());
    }
}
