//! What a round cutter actually leaves in a tooth space.
//!
//! The space an end mill opens is the ideal space eroded by the cutter radius
//! and dilated back. Its boundary is: the involute flanks down to the **form
//! radius**, the cutter's own circular fillet, and — when the space at the root
//! is wider than the cutter — an arc of the nominal root circle between two
//! fillets. Which of those two shapes you get is [`RootBinding`].
//!
//! Everything here is computed from the exact parallel-curve property of the
//! involute: offsetting an involute by `ρ` gives another involute of the *same*
//! base circle, rotated by `ρ/r_b`, with the foot of the normal at parameter
//! `u` landing on the offset at `u ∓ ρ/r_b`. So the cutter-centre curve is an
//! involute, the tangency is a parameter shift, and nothing is sampled.

use super::{GearError, SpurGear};
use serde::{Deserialize, Serialize};

/// Which constraint stops the cutter going deeper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootBinding {
    /// The space is narrower than the cutter down there: the cutter wedges
    /// between the two flanks and leaves a full-round root above (below, on a
    /// ring) the nominal root circle.
    Flanks,
    /// The space is wider than the cutter: the cutter reaches the nominal root
    /// circle and sweeps along it, leaving fillet–arc–fillet.
    RootCircle,
}

/// The tooth space as the cutter leaves it.
///
/// Angles are measured from the space centreline; the space is symmetric about
/// it, so one sign is enough.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToothSpace {
    /// Cutter radius, mm.
    pub cutter_radius: f64,
    /// Which constraint stopped the cutter.
    pub binding: RootBinding,
    /// Deepest radius the cutter reaches: smallest for an external gear,
    /// largest for a ring.
    pub effective_root_radius: f64,
    /// Radius at which the cutter's fillet becomes tangent to the flank. The
    /// involute survives outside it on an external gear, inside it on a ring.
    pub form_radius: f64,
    /// Radius of the fillet's centre, mm.
    pub fillet_centre_radius: f64,
    /// Angle of the fillet's centre from the space centreline, radians
    /// (positive; the other fillet is its mirror). Zero when the flanks bind.
    pub fillet_centre_angle: f64,
    /// Half the angular span of the root-circle arc between the two fillets,
    /// radians. Zero when the flanks bind.
    pub root_arc_half_angle: f64,
    /// Ideal space width at the form radius, mm.
    pub space_width_at_form: f64,
    /// Ideal space width at the mouth (the tip circle), mm.
    pub mouth_width: f64,
}

/// How badly the fillet intrudes on the active flank, graded.
///
/// The distinction matters because the two useful numbers disagree about how
/// alarming the same geometry is: a *radial* overlap of 0.034 mm on the
/// reference ring is only 1.4 µm of metal measured where it actually stands,
/// because the flank is steep there and the fillet leaves it quadratically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilletEncroachment {
    /// The fillet tangency is clear of the contact limit: the flank is exactly
    /// involute everywhere contact happens.
    Clear,
    /// The fillet has started before the contact limit, but the metal it leaves
    /// there is within the stated tolerance.
    WithinTolerance,
    /// The fillet leaves more metal on the active flank than the tolerance
    /// allows.
    Exceeds,
}

/// Whether the cutter leaves enough involute for the mesh, and by how much.
///
/// Read `flank_deviation_at_contact_limit` before `radial_overlap`: the first
/// is metal, the second is a radius difference that has to be projected onto
/// the flank before it means anything on a part.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Reachability {
    /// True when the geometry passes the criterion this verdict was taken
    /// under: exactly involute for [`SpurGear::reachability_exact`], within
    /// `flank_deviation_tolerance` for [`SpurGear::reachability_within`].
    pub ok: bool,
    /// How the fillet stands relative to the contact limit.
    pub encroachment: FilletEncroachment,
    /// True for an internal gear (the test flips direction).
    pub internal: bool,
    /// Cutter diameter this verdict is for, mm.
    pub cutter_diameter: f64,
    /// Fillet tangency radius, mm.
    pub form_radius: f64,
    /// The contact radius the fillet has to stay clear of, mm.
    pub contact_limit: f64,
    /// Positive margin means the involute survives past the contact limit, mm.
    pub margin: f64,
    /// How far past the contact limit the fillet has eaten, in radius, mm.
    /// Positive means it intrudes; this is `−margin`, and it is a radial
    /// difference, not a thickness of metal.
    pub radial_overlap: f64,
    /// Metal standing proud of the true involute at the contact limit,
    /// measured along the flank normal, mm. Zero when the fillet is clear.
    ///
    /// This is the number a machinist compares against the machine: it is what
    /// a probe or a mating tooth would feel.
    pub flank_deviation_at_contact_limit: f64,
    /// The deviation allowed by the criterion, mm. `None` for the exact
    /// verdict, which allows none.
    pub flank_deviation_tolerance: Option<f64>,
    /// Largest cutter diameter that keeps the flank *exactly* involute across
    /// contact, mm. `None` when no cutter does.
    pub largest_cutter_diameter: Option<f64>,
}

impl Reachability {
    /// One line a machinist can act on, with the deviation in µm.
    ///
    /// Kept out of the struct so the serialized claim stays numeric.
    pub fn note(&self) -> String {
        let um = self.flank_deviation_at_contact_limit * 1000.0;
        match self.encroachment {
            FilletEncroachment::Clear => format!(
                "Ø{:.3} cutter: involute intact through contact, {:.4} mm of radial margin",
                self.cutter_diameter, self.margin
            ),
            FilletEncroachment::WithinTolerance => format!(
                "Ø{:.3} cutter: fillet starts {:.4} mm (radially) inside the contact limit, but \
                 leaves only {um:.1} µm of metal on the active flank — within the {:.1} µm allowed",
                self.cutter_diameter,
                self.radial_overlap,
                self.flank_deviation_tolerance.unwrap_or(0.0) * 1000.0,
            ),
            FilletEncroachment::Exceeds => format!(
                "Ø{:.3} cutter: fillet leaves {um:.1} µm of metal on the active flank at r{:.4} \
                 ({:.4} mm of radial overlap){}",
                self.cutter_diameter,
                self.contact_limit,
                self.radial_overlap,
                match self.largest_cutter_diameter {
                    Some(d) => format!("; Ø{d:.3} or smaller keeps it exactly involute"),
                    None => String::new(),
                }
            ),
        }
    }
}

/// Cutter-centre position when the cutter is tangent to the flank at `foot`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CentrePoint {
    /// Radius of the cutter centre, mm.
    pub radius: f64,
    /// Angle of the cutter centre from the space centreline, radians.
    pub angle: f64,
}

impl SpurGear {
    /// Radius and angle of the cutter centre when the cutter of radius `rc`
    /// touches the `+` flank of a tooth space at radius `foot`.
    ///
    /// Above the base circle this is the exact parallel involute; below it (an
    /// external gear whose root dives inside the base circle) the flank is the
    /// radial continuation and the offset is a parallel line.
    pub(crate) fn cutter_centre_at_foot(
        &self,
        rc: f64,
        foot: f64,
    ) -> Result<CentrePoint, GearError> {
        let rb = self.r_base();
        let sigma_b = self.space_half_angle_at(rb)?;
        let delta = rc / rb;
        if self.internal {
            if foot < rb {
                return Err(GearError::RadiusBelowBaseCircle {
                    radius: foot,
                    base: rb,
                });
            }
            let u = ((foot / rb).powi(2) - 1.0).max(0.0).sqrt();
            let v = u - delta;
            if v < 0.0 {
                return Err(GearError::CutterTooLarge {
                    cutter_diameter: 2.0 * rc,
                    space_width: self.space_width_at(foot)?,
                    radius: foot,
                });
            }
            Ok(CentrePoint {
                radius: rb * (1.0 + v * v).sqrt(),
                angle: sigma_b - delta - (v - v.atan()),
            })
        } else if foot >= rb {
            let u = ((foot / rb).powi(2) - 1.0).max(0.0).sqrt();
            let v = u + delta;
            Ok(CentrePoint {
                radius: rb * (1.0 + v * v).sqrt(),
                angle: sigma_b - delta + (v - v.atan()),
            })
        } else {
            // Radial flank: the offset is a line parallel to it at distance rc.
            let radius = foot.hypot(rc);
            Ok(CentrePoint {
                radius,
                angle: sigma_b - (rc / radius).asin(),
            })
        }
    }

    /// The tooth space a cutter of `cutter_diameter` leaves.
    pub fn tooth_space(&self, cutter_diameter: f64) -> Result<ToothSpace, GearError> {
        self.validate()?;
        if !cutter_diameter.is_finite() || cutter_diameter <= 0.0 {
            return Err(GearError::InvalidCutterDiameter(cutter_diameter));
        }
        let rc = cutter_diameter / 2.0;
        let (mouth, bottom) = (self.r_tip(), self.r_root());
        let mouth_width = self.space_width_at(mouth)?;

        // Can the cutter even enter? The centre has to sit on the space side of
        // the centreline at the mouth.
        let at_mouth = self.cutter_centre_at_foot(rc, mouth)?;
        if at_mouth.angle <= 0.0 {
            return Err(GearError::CutterTooLarge {
                cutter_diameter,
                space_width: mouth_width,
                radius: mouth,
            });
        }

        // The centre angle falls monotonically as the cutter is pushed toward
        // the root; where it reaches the centreline the two offset flanks have
        // met and the cutter is wedged.
        let at_bottom = self.cutter_centre_at_foot(rc, bottom);
        let wedged = match &at_bottom {
            Ok(c) => c.angle <= 0.0,
            Err(_) => true,
        };
        let (mut lo, mut hi) = (mouth, bottom); // lo: angle > 0, hi: angle <= 0
        let meet = if wedged {
            for _ in 0..200 {
                let mid = 0.5 * (lo + hi);
                match self.cutter_centre_at_foot(rc, mid) {
                    Ok(c) if c.angle > 0.0 => lo = mid,
                    _ => hi = mid,
                }
            }
            Some(self.cutter_centre_at_foot(rc, lo)?)
        } else {
            None
        };

        // Does the nominal root circle stop the cutter first?
        let root_locus = if self.internal {
            bottom - rc
        } else {
            bottom + rc
        };
        let flanks_bind = match &meet {
            Some(c) => {
                if self.internal {
                    c.radius + rc <= bottom
                } else {
                    c.radius - rc >= bottom
                }
            }
            None => false,
        };

        let (binding, effective_root_radius, form_radius, centre) = if flanks_bind {
            let c = meet.expect("flanks bind implies a meeting point");
            let root = if self.internal {
                c.radius + rc
            } else {
                c.radius - rc
            };
            (RootBinding::Flanks, root, lo, c)
        } else {
            // Run the centre down the offset flank until it reaches the root
            // locus; that last flank-touching centre is the fillet's centre.
            let (mut a, mut b) = (mouth, bottom);
            for _ in 0..200 {
                let mid = 0.5 * (a + b);
                let r = match self.cutter_centre_at_foot(rc, mid) {
                    Ok(c) => c.radius,
                    Err(_) => break,
                };
                let past = if self.internal {
                    r >= root_locus
                } else {
                    r <= root_locus
                };
                if past {
                    b = mid;
                } else {
                    a = mid;
                }
            }
            let foot = 0.5 * (a + b);
            let c = self.cutter_centre_at_foot(rc, foot)?;
            (RootBinding::RootCircle, bottom, foot, c)
        };

        let root_arc_half_angle = match binding {
            RootBinding::Flanks => 0.0,
            RootBinding::RootCircle => centre.angle.max(0.0),
        };
        Ok(ToothSpace {
            cutter_radius: rc,
            binding,
            effective_root_radius,
            form_radius,
            fillet_centre_radius: centre.radius,
            fillet_centre_angle: centre.angle.max(0.0),
            root_arc_half_angle,
            space_width_at_form: self.space_width_at(form_radius)?,
            mouth_width,
        })
    }

    /// Does the cutter leave the involute intact across the contact range?
    ///
    /// `contact_limit` is the lowest contact radius on an external gear and the
    /// highest on a ring — [`super::ContactRadii`] has both.
    pub fn reachability_exact(
        &self,
        cutter_diameter: f64,
        contact_limit: f64,
    ) -> Result<Reachability, GearError> {
        self.reachability_graded(cutter_diameter, contact_limit, None)
    }

    /// The same verdict taken at a stated tolerance: the fillet may start
    /// before the contact limit as long as the metal it leaves on the active
    /// flank is no more than `max_flank_deviation` mm.
    ///
    /// This is the verdict to hand a machinist. A hobby router holding ±50 µm
    /// does not care that a radius crossed a line; it cares how much metal is
    /// standing where the mating tooth runs. Use
    /// [`SpurGear::reachability_exact`] when the answer has to be "the flank is
    /// an involute", with no tolerance argued about.
    pub fn reachability_within(
        &self,
        cutter_diameter: f64,
        contact_limit: f64,
        max_flank_deviation: f64,
    ) -> Result<Reachability, GearError> {
        if !max_flank_deviation.is_finite() || max_flank_deviation < 0.0 {
            return Err(GearError::InvalidFlankTolerance(max_flank_deviation));
        }
        self.reachability_graded(cutter_diameter, contact_limit, Some(max_flank_deviation))
    }

    fn reachability_graded(
        &self,
        cutter_diameter: f64,
        contact_limit: f64,
        tolerance: Option<f64>,
    ) -> Result<Reachability, GearError> {
        let form = self.tooth_space(cutter_diameter)?.form_radius;
        let margin = if self.internal {
            form - contact_limit
        } else {
            contact_limit - form
        };
        let deviation = self.flank_deviation_at(cutter_diameter, contact_limit)?;
        let encroachment = if margin >= 0.0 {
            FilletEncroachment::Clear
        } else {
            match tolerance {
                // The exact verdict allows nothing: any encroachment exceeds it.
                Some(t) if deviation <= t => FilletEncroachment::WithinTolerance,
                _ => FilletEncroachment::Exceeds,
            }
        };
        Ok(Reachability {
            ok: encroachment != FilletEncroachment::Exceeds,
            encroachment,
            internal: self.internal,
            cutter_diameter,
            form_radius: form,
            contact_limit,
            margin,
            radial_overlap: -margin,
            flank_deviation_at_contact_limit: deviation,
            flank_deviation_tolerance: tolerance,
            largest_cutter_diameter: self.largest_cutter_exact(contact_limit, 0.0).ok(),
        })
    }

    /// Metal standing proud of the true involute at `radius`, measured along
    /// the flank normal, mm.
    ///
    /// Zero on the reachable side of the form radius, where the cutter follows
    /// the involute exactly. Past it the surface is the cutter's fillet, so the
    /// deviation is found by walking from the ideal flank point along its own
    /// normal until it meets the fillet circle — solved exactly, not by the
    /// usual `r − √(r² − s²)` estimate, which treats the flank as a straight
    /// line and the arc length as a radius difference over `cos φ`.
    pub fn flank_deviation_at(&self, cutter_diameter: f64, radius: f64) -> Result<f64, GearError> {
        let space = self.tooth_space(cutter_diameter)?;
        let reached = if self.internal {
            radius <= space.form_radius
        } else {
            radius >= space.form_radius
        };
        if reached {
            return Ok(0.0);
        }
        let half = self.space_half_angle_at(radius)?;
        let p = (radius * half.cos(), radius * half.sin());
        let c = (
            space.fillet_centre_radius * space.fillet_centre_angle.cos(),
            space.fillet_centre_radius * space.fillet_centre_angle.sin(),
        );
        let n = self.flank_normal_into_space(radius)?;
        let rc = space.cutter_radius;
        let (dx, dy) = (p.0 - c.0, p.1 - c.1);
        // |P + t·n − C|² = rc²
        let b = n.0 * dx + n.1 * dy;
        let cc = dx * dx + dy * dy - rc * rc;
        let disc = b * b - cc;
        if disc < 0.0 {
            // The normal misses the fillet entirely — only possible far past
            // the tangency, where the nearest cut surface is what is left.
            return Ok((dx.hypot(dy) - rc).max(0.0));
        }
        let t = -b - disc.sqrt();
        Ok(if t >= 0.0 {
            t
        } else {
            (-b + disc.sqrt()).max(0.0)
        })
    }

    /// Unit normal to the tooth-space flank at `radius`, pointing into the
    /// space (toward the space centreline).
    ///
    /// The involute's tangent at roll parameter `u` points along the direction
    /// `σ_b ± u`, so the normal is that turned a quarter turn toward the
    /// centreline. Below the base circle the flank is the radial continuation
    /// and the normal is simply perpendicular to it.
    pub fn flank_normal_into_space(&self, radius: f64) -> Result<(f64, f64), GearError> {
        let rb = self.r_base();
        let sigma_b = self.space_half_angle_at(rb)?;
        let u = if radius >= rb {
            ((radius / rb).powi(2) - 1.0).max(0.0).sqrt()
        } else {
            0.0
        };
        let tangent = if self.internal {
            sigma_b - u
        } else {
            sigma_b + u
        };
        // The tangent always leads outward in radius (its radial component is
        // cos φ > 0), so the quarter turn clockwise from it always points at
        // smaller angle — which is the space side of the `+` flank, external or
        // internal alike.
        let n = tangent - std::f64::consts::FRAC_PI_2;
        Ok((n.cos(), n.sin()))
    }

    /// Largest cutter diameter whose fillet still clears `contact_limit` by
    /// `required_margin` mm of radius — the exact, zero-deviation criterion.
    ///
    /// A bigger cutter always pushes the form radius the wrong way, so this is
    /// a plain bisection on a monotone function.
    pub fn largest_cutter_exact(
        &self,
        contact_limit: f64,
        required_margin: f64,
    ) -> Result<f64, GearError> {
        self.validate()?;
        let margin_of = |d: f64| -> Option<f64> {
            let form = self.tooth_space(d).ok()?.form_radius;
            Some(if self.internal {
                form - contact_limit
            } else {
                contact_limit - form
            })
        };
        // A cutter this small cannot be run anyway, but it brackets the search.
        let smallest = 1e-4 * self.module;
        match margin_of(smallest) {
            Some(m) if m >= required_margin => {}
            _ => {
                return Err(GearError::NoUsableCutter {
                    limit: contact_limit,
                })
            }
        }
        let mut lo = smallest;
        let mut hi = 4.0 * self.module;
        // Grow the upper bound until it fails, so the bracket is real.
        for _ in 0..20 {
            match margin_of(hi) {
                Some(m) if m >= required_margin => hi *= 2.0,
                _ => break,
            }
        }
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            match margin_of(mid) {
                Some(m) if m >= required_margin => lo = mid,
                _ => hi = mid,
            }
        }
        Ok(lo)
    }

    /// Largest cutter diameter that leaves no more than `max_flank_deviation`
    /// mm of metal on the active flank at `contact_limit`.
    ///
    /// The tolerance-aware twin of [`SpurGear::largest_cutter_exact`], and the
    /// one that decides what is in the collet: the deviation grows with cutter
    /// diameter (the fillet starts further up the flank faster than the bigger
    /// radius flattens it), so this is again a bisection on a monotone
    /// function — `deviation_grows_with_cutter_diameter` holds it to that.
    pub fn largest_cutter_within(
        &self,
        contact_limit: f64,
        max_flank_deviation: f64,
    ) -> Result<f64, GearError> {
        self.validate()?;
        if !max_flank_deviation.is_finite() || max_flank_deviation < 0.0 {
            return Err(GearError::InvalidFlankTolerance(max_flank_deviation));
        }
        let within = |d: f64| -> bool {
            self.flank_deviation_at(d, contact_limit)
                .map(|dev| dev <= max_flank_deviation)
                .unwrap_or(false)
        };
        let smallest = 1e-4 * self.module;
        if !within(smallest) {
            return Err(GearError::NoUsableCutter {
                limit: contact_limit,
            });
        }
        let mut lo = smallest;
        let mut hi = 4.0 * self.module;
        for _ in 0..20 {
            if within(hi) {
                hi *= 2.0;
            } else {
                break;
            }
        }
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if within(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// The radius at which the fillet has left the involute by exactly
    /// `deviation` mm — the furthest point still worth calling involute at
    /// that tolerance.
    ///
    /// With `deviation = 0` this is the form radius itself. It is how a
    /// measured "form radius" gets compared with this model: a probe that
    /// declares the flank intact until it is `d` off the ideal curve is
    /// reporting this radius, not the tangency.
    pub fn limiting_radius_within(
        &self,
        cutter_diameter: f64,
        deviation: f64,
    ) -> Result<f64, GearError> {
        if !deviation.is_finite() || deviation < 0.0 {
            return Err(GearError::InvalidFlankTolerance(deviation));
        }
        let space = self.tooth_space(cutter_diameter)?;
        let (mut clear, mut past) = (space.form_radius, space.effective_root_radius);
        for _ in 0..200 {
            let mid = 0.5 * (clear + past);
            if self.flank_deviation_at(cutter_diameter, mid)? <= deviation {
                clear = mid;
            } else {
                past = mid;
            }
        }
        Ok(clear)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::super::GearPair;
    use super::*;

    /// The 15 µm probe the fixture's generator uses to find the form radius:
    /// it bisects for the radius at which a point 15 µm inside the ideal flank
    /// is still cut, so it reports the radius where the fillet has pulled 15 µm
    /// away from the flank — not the tangency itself. Reproducing the fixture
    /// means reproducing that probe, which is what this does: the probe point
    /// at radius `r`, and the fillet circle it is tested against.
    fn probed_form_radius(g: &SpurGear, space: &ToothSpace, probe: f64) -> f64 {
        let rc = space.cutter_radius;
        let (cx, cy) = (
            space.fillet_centre_radius * space.fillet_centre_angle.cos(),
            space.fillet_centre_radius * space.fillet_centre_angle.sin(),
        );
        // Distance from the probe point at radius r to the fillet centre. The
        // probe sits `probe` mm from the ideal flank, into the space.
        let dist = |r: f64| {
            let th = g.space_half_angle_at(r).unwrap();
            let (px, py) = (r * th.cos(), r * th.sin());
            // Into the space is toward the centreline: -theta_hat at +th.
            let (nx, ny) = (th.sin(), -th.cos());
            let (px, py) = (px + probe * nx, py + probe * ny);
            (px - cx).hypot(py - cy)
        };
        // Only the far side of the tangency is unreachable, so bisect there:
        // between the nominal root (not cut) and the tangency (cut).
        let (mut not_cut, mut cut) = (g.r_root(), space.form_radius);
        for _ in 0..200 {
            let mid = 0.5 * (not_cut + cut);
            if dist(mid) <= rc {
                cut = mid;
            } else {
                not_cut = mid;
            }
        }
        0.5 * (not_cut + cut)
    }

    /// Both external gears in the train have a space narrower than the Ø1
    /// cutter at the root, yet the cutter still bottoms on the root circle
    /// (the space only has to be 1 mm wide 0.5 mm *above* the root). That is
    /// the fixture's `rf_*_eff`: nominal, to within the shapely artifact.
    #[test]
    fn fixture_effective_roots() {
        let s = sun().tooth_space(1.0).unwrap();
        assert_eq!(s.binding, RootBinding::RootCircle);
        // The fixture measures 4.219683278, 3.2e-4 inside the nominal root.
        // That is the shapely artifact, not geometry: its buffer draws the root
        // disc as a 256-gon *inscribed* in the root circle, so the measurement
        // reads a facet. r·cos(π/256) accounts for it to a part in 1e6.
        let facet = 4.22 * (std::f64::consts::PI / 256.0).cos();
        assert!((facet - 4.219683278).abs() < 2e-6, "facet model {facet}");
        assert!((s.effective_root_radius - 4.219683277807481).abs() < 4e-4);
        assert!((s.effective_root_radius - 4.22).abs() < 1e-12);

        let p = planet().tooth_space(1.0).unwrap();
        assert_eq!(p.binding, RootBinding::RootCircle);
        // Same artifact on the planet; here the facet accounts for 96 % of the
        // 6.4e-4 offset and the buffer's own RES=64 rounding for the rest.
        let facet = 8.75 * (std::f64::consts::PI / 256.0).cos();
        assert!((facet - 8.74936455).abs() < 3e-5, "facet model {facet}");
        assert!((p.effective_root_radius - 8.74936455024915).abs() < 7e-4);
        assert!((p.effective_root_radius - 8.75).abs() < 1e-12);

        // The ring's space narrows outward, so the cutter wedges well short of
        // the nominal 26.72 root. No polygon artifact here: 6e-6.
        let r = ring().tooth_space(1.0).unwrap();
        assert_eq!(r.binding, RootBinding::Flanks);
        assert!(
            (r.effective_root_radius - 26.52577211671196).abs() < 1e-5,
            "ring root {}",
            r.effective_root_radius
        );
    }

    /// The fixture has two ring roots. `rf_r_eff` is the zero-backlash space;
    /// `ring_root_eff` is the same space widened by the 0.03 backlash, which is
    /// what the part is actually cut to. Reproducing both, from the same code,
    /// is what proves the reading.
    #[test]
    fn fixture_ring_has_two_roots_and_backlash_explains_the_difference() {
        let sharp = ring().tooth_space(1.0).unwrap();
        let cut = ring()
            .with_backlash_thinning(0.03)
            .tooth_space(1.0)
            .unwrap();
        assert!((sharp.effective_root_radius - 26.52577211671196).abs() < 1e-5);
        assert!((cut.effective_root_radius - 26.558435634383095).abs() < 1e-5);
        assert!(cut.effective_root_radius > sharp.effective_root_radius);
    }

    /// The fixture's `form_*` are probe-biased by 15 µm. Reproduce the probe
    /// and the numbers come back; the residual is the shapely buffer's own
    /// faceting (RES=64, so ~3e-4 on a 0.5 mm fillet).
    #[test]
    fn fixture_form_radii_via_the_generator_probe() {
        for (g, want, tol) in [
            (sun(), 4.571617443472178, 5e-4),
            (planet(), 9.114907439495147, 5e-4),
            (ring(), 26.34356294823303, 5e-4),
        ] {
            let space = g.tooth_space(1.0).unwrap();
            let probed = probed_form_radius(&g, &space, 0.015);
            assert!(
                (probed - want).abs() < tol,
                "z{} probed form {probed} vs fixture {want} (true tangency {})",
                g.teeth,
                space.form_radius
            );
        }
    }

    /// The true tangency radii, which are what the mesh actually sees.
    /// Documented here so the numbers in the report are pinned by a test.
    #[test]
    fn true_form_radii() {
        assert!((sun().tooth_space(1.0).unwrap().form_radius - 4.693442).abs() < 1e-5);
        assert!((planet().tooth_space(1.0).unwrap().form_radius - 9.236477).abs() < 1e-5);
        assert!((ring().tooth_space(1.0).unwrap().form_radius - 26.244841).abs() < 1e-5);
    }

    /// The three contact limits the train imposes, so every test below reads
    /// the same numbers as the mesh.
    fn contact_limits() -> [(&'static str, SpurGear, f64); 3] {
        let sp = GearPair::external(sun(), planet());
        let pr = GearPair::internal(planet(), ring());
        let c_sp = sp.contact_radii().unwrap();
        let c_pr = pr.contact_radii().unwrap();
        [
            ("sun", sun(), c_sp.pinion_lowest),
            // The planet meshes with both; the binding limit is the lower of
            // its two lowest contact radii.
            (
                "planet",
                planet(),
                c_sp.wheel_lowest.min(c_pr.pinion_lowest),
            ),
            ("ring", ring(), c_pr.wheel_highest),
        ]
    }

    /// The verdict the milestone hangs on. The sun and planet keep their
    /// involute past the contact limit; the **ring does not** — the Ø1 cutter's
    /// fillet starts 0.034 mm inside the ring's highest contact radius. The
    /// design file claims the opposite (26.34 > 26.28) because it compares the
    /// probe-biased form radius, which reads 0.099 mm optimistic.
    #[test]
    fn reachability_verdicts() {
        let sp = GearPair::external(sun(), planet());
        let pr = GearPair::internal(planet(), ring());
        let c_sp = sp.contact_radii().unwrap();
        let c_pr = pr.contact_radii().unwrap();

        let v = sun().reachability_exact(1.0, c_sp.pinion_lowest).unwrap();
        assert!(v.ok, "sun {v:?}");
        assert!(
            (v.margin - 0.067466).abs() < 1e-4,
            "sun margin {}",
            v.margin
        );

        // The planet meshes with both; the binding limit is the lower of the
        // two lowest contact radii.
        let limit = c_sp.wheel_lowest.min(c_pr.pinion_lowest);
        assert!((limit - 9.414537845344137).abs() < 1e-9);
        let v = planet().reachability_exact(1.0, limit).unwrap();
        assert!(v.ok, "planet {v:?}");
        assert!(
            (v.margin - 0.178061).abs() < 1e-4,
            "planet margin {}",
            v.margin
        );

        let v = ring().reachability_exact(1.0, c_pr.wheel_highest).unwrap();
        assert!(!v.ok, "ring should fail: {v:?}");
        assert!(
            (v.margin + 0.033641).abs() < 1e-4,
            "ring margin {}",
            v.margin
        );
        let largest = v.largest_cutter_diameter.expect("a smaller cutter works");
        // Ø0.965 is the biggest cutter that leaves the ring's involute intact
        // across contact — a Ø0.9 end mill would do it, a Ø1 will not.
        assert!(
            (0.965..0.966).contains(&largest),
            "largest ring cutter {largest}"
        );
        // And that cutter really does clear it.
        let ok = ring()
            .reachability_exact(largest - 1e-6, c_pr.wheel_highest)
            .unwrap();
        assert!(ok.ok, "largest cutter should pass: {ok:?}");
        let fails = ring()
            .reachability_exact(largest + 1e-3, c_pr.wheel_highest)
            .unwrap();
        assert!(!fails.ok, "a hair larger should fail: {fails:?}");
    }

    /// What that 0.034 mm is *made of*. The radial overlap sounds like a
    /// hundredth of a millimetre of missing flank; the metal actually standing
    /// where the planet tooth runs is **1.36 µm**, because the ring's flank
    /// climbs at φ ≈ 26° there and the fillet leaves it quadratically. On a
    /// machine holding ±50 µm that is not a defect — which is the whole point
    /// of reporting the deviation next to the overlap.
    #[test]
    fn ring_encroachment_is_micrometres_not_hundredths() {
        let (_, ring, limit) = contact_limits()[2];

        let strict = ring.reachability_exact(1.0, limit).unwrap();
        assert!(!strict.ok, "strict verdict should fail: {strict:?}");
        assert_eq!(strict.encroachment, FilletEncroachment::Exceeds);
        assert!(
            (strict.radial_overlap - 0.033641).abs() < 1e-4,
            "radial overlap {}",
            strict.radial_overlap
        );
        assert!(
            (strict.flank_deviation_at_contact_limit - 0.0013557).abs() < 1e-6,
            "flank deviation {} mm",
            strict.flank_deviation_at_contact_limit
        );
        assert_eq!(strict.flank_deviation_tolerance, None);

        // The verdict a machinist gets, at two tolerances that bracket it.
        let loose = ring.reachability_within(1.0, limit, 0.005).unwrap();
        assert!(loose.ok, "5 µm should pass: {}", loose.note());
        assert_eq!(loose.encroachment, FilletEncroachment::WithinTolerance);
        let tight = ring.reachability_within(1.0, limit, 0.001).unwrap();
        assert!(!tight.ok, "1 µm should fail: {}", tight.note());
        assert_eq!(tight.encroachment, FilletEncroachment::Exceeds);
        // Both verdicts are about the same metal; only the bar moved.
        assert_eq!(
            loose.flank_deviation_at_contact_limit,
            tight.flank_deviation_at_contact_limit
        );

        // A Ø0.9 cutter clears it outright: no tolerance argument needed.
        let smaller = ring.reachability_exact(0.9, limit).unwrap();
        assert!(smaller.ok, "Ø0.9 {smaller:?}");
        assert_eq!(smaller.encroachment, FilletEncroachment::Clear);
        assert_eq!(smaller.flank_deviation_at_contact_limit, 0.0);
        assert!(
            (smaller.radial_overlap + 0.062844).abs() < 1e-4,
            "Ø0.9 overlap {}",
            smaller.radial_overlap
        );
    }

    /// The externals have no encroachment at either diameter, so their
    /// deviation is exactly zero — not a small number that happens to pass.
    #[test]
    fn sun_and_planet_have_no_encroachment_at_either_cutter() {
        for (name, g, limit) in contact_limits().iter().take(2) {
            for d in [1.0, 0.9] {
                let v = g.reachability_within(d, *limit, 0.0).unwrap();
                assert!(v.ok, "{name} Ø{d}: {v:?}");
                assert_eq!(v.encroachment, FilletEncroachment::Clear);
                assert_eq!(v.flank_deviation_at_contact_limit, 0.0);
                assert!(v.radial_overlap < 0.0, "{name} Ø{d} overlaps");
            }
        }
    }

    /// rana's generator declares the flank intact until a point 15 µm inside it
    /// is no longer cut. Read through this model that is a **flank-deviation
    /// tolerance**, not a bias: the deviation at each fixture `form_*` comes out
    /// at 15.07 / 15.00 / 11.82 µm.
    ///
    /// The two externals land on 15 µm almost exactly because their flank is
    /// the radial continuation there, so the probe's circumferential offset
    /// *is* the flank normal. The ring's flank is a real involute at φ ≈ 26°,
    /// so the same 15 µm circumferential step is only ~12 µm of normal
    /// deviation — which is why quoting 15 µm as if it were a normal deviation
    /// misses the ring's form radius by 12 µm while hitting the externals to
    /// better than 3e-4 mm.
    #[test]
    fn ranas_probe_is_a_fifteen_micron_flank_tolerance() {
        for (name, g, fixture_form, want_um) in [
            ("sun", sun(), 4.571617443472178, 15.0683),
            ("planet", planet(), 9.114907439495147, 15.0042),
            ("ring", ring(), 26.34356294823303, 11.8190),
        ] {
            let dev = g.flank_deviation_at(1.0, fixture_form).unwrap();
            assert!(
                (dev * 1000.0 - want_um).abs() < 1e-3,
                "{name}: deviation at the fixture form radius is {:.4} µm, expected {want_um}",
                dev * 1000.0
            );
            // And the inverse lands back on the fixture's own number.
            let back = g.limiting_radius_within(1.0, dev).unwrap();
            assert!(
                (back - fixture_form).abs() < 1e-9,
                "{name}: round trip {back} vs {fixture_form}"
            );
            // Quoting 15 µm as a normal deviation: fine on the externals,
            // 12 µm out on the ring.
            let at_15 = g.limiting_radius_within(1.0, 0.015).unwrap();
            let miss = (at_15 - fixture_form).abs();
            if g.internal {
                assert!(miss > 1e-2, "ring: expected a visible miss, got {miss}");
            } else {
                assert!(miss < 5e-4, "{name}: 15 µm misses by {miss}");
            }
        }

        // The mechanism, asserted rather than asserted-about: at the sun's form
        // radius the flank normal is purely circumferential (the flank is
        // radial), while the ring's has a radial component of sin φ.
        let sun_form = sun().tooth_space(1.0).unwrap().form_radius;
        let n = sun().flank_normal_into_space(sun_form).unwrap();
        let half = sun().space_half_angle_at(sun_form).unwrap();
        let radial = n.0 * half.cos() + n.1 * half.sin();
        assert!(radial.abs() < 1e-9, "sun normal has radial part {radial}");

        let ring_form = ring().tooth_space(1.0).unwrap().form_radius;
        let n = ring().flank_normal_into_space(ring_form).unwrap();
        let half = ring().space_half_angle_at(ring_form).unwrap();
        let radial = (n.0 * half.cos() + n.1 * half.sin()).abs();
        let phi = ring().pressure_angle_at(ring_form).unwrap();
        assert!(
            (radial - phi.sin()).abs() < 1e-9,
            "ring normal radial part {radial} vs sin φ {}",
            phi.sin()
        );
    }

    /// A bigger cutter always leaves more metal on the active flank — the
    /// property `largest_cutter_within` bisects on.
    #[test]
    fn deviation_grows_with_cutter_diameter() {
        for (name, g, limit) in contact_limits() {
            let mut previous = -1.0;
            let mut d = 0.3;
            while d < 1.45 {
                let dev = g.flank_deviation_at(d, limit).unwrap();
                assert!(
                    dev >= previous - 1e-12,
                    "{name}: deviation fell from {previous} to {dev} at Ø{d}"
                );
                previous = dev;
                d += 0.05;
            }
            assert!(previous > 0.0, "{name}: nothing encroached even at Ø1.45");
        }
    }

    /// The tolerance-aware cutter is bigger than the exact one, and it sits on
    /// the boundary: at the answer the deviation is the tolerance.
    #[test]
    fn largest_cutter_within_sits_on_its_tolerance() {
        for (name, g, limit) in contact_limits() {
            let exact = g.largest_cutter_exact(limit, 0.0).unwrap();
            let within = g.largest_cutter_within(limit, 0.005).unwrap();
            assert!(
                within > exact,
                "{name}: within {within} should exceed exact {exact}"
            );
            let dev = g.flank_deviation_at(within, limit).unwrap();
            assert!(
                (dev - 0.005).abs() < 1e-6,
                "{name}: deviation at the answer is {dev}, not the 0.005 asked for"
            );
            assert!(g.reachability_within(within, limit, 0.005).unwrap().ok);
            assert!(
                !g.reachability_within(within * 1.02, limit, 0.005)
                    .unwrap()
                    .ok
            );
        }
        // The ring's numbers, for the record: Ø0.965 exact, Ø1.032 at 5 µm.
        let (_, ring, limit) = contact_limits()[2];
        assert!((ring.largest_cutter_exact(limit, 0.0).unwrap() - 0.965499).abs() < 1e-5);
        assert!((ring.largest_cutter_within(limit, 0.005).unwrap() - 1.032451).abs() < 1e-5);
        assert!((ring.largest_cutter_within(limit, 0.001).unwrap() - 0.995080).abs() < 1e-5);
    }

    /// Deviation is zero on the cut side of the form radius and grows past it,
    /// and a negative tolerance is refused rather than treated as zero.
    #[test]
    fn deviation_is_zero_where_the_cutter_followed_the_flank() {
        let (_, ring, limit) = contact_limits()[2];
        let form = ring.tooth_space(1.0).unwrap().form_radius;
        assert_eq!(ring.flank_deviation_at(1.0, form - 0.5).unwrap(), 0.0);
        assert_eq!(ring.flank_deviation_at(1.0, form).unwrap(), 0.0);
        assert!(ring.flank_deviation_at(1.0, form + 0.01).unwrap() > 0.0);

        let (_, sun, _) = contact_limits()[0];
        let form = sun.tooth_space(1.0).unwrap().form_radius;
        assert_eq!(sun.flank_deviation_at(1.0, form + 0.5).unwrap(), 0.0);
        assert!(sun.flank_deviation_at(1.0, form - 0.01).unwrap() > 0.0);

        assert!(matches!(
            ring.reachability_within(1.0, limit, -1e-9),
            Err(GearError::InvalidFlankTolerance(_))
        ));
        assert!(ring.largest_cutter_within(limit, f64::NAN).is_err());
        assert!(ring.limiting_radius_within(1.0, -1.0).is_err());
    }

    /// The deviation is measured along the flank normal, and the normal is the
    /// real one: perpendicular to the flank's own tangent, both regimes.
    #[test]
    fn flank_normal_is_perpendicular_to_the_flank() {
        for g in [sun(), planet(), ring()] {
            let space = g.tooth_space(1.0).unwrap();
            for t in [0.1, 0.5, 0.9] {
                let (lo, hi) = if g.internal {
                    (g.r_tip(), space.form_radius)
                } else {
                    (space.form_radius, g.r_tip())
                };
                let r = lo + (hi - lo) * t;
                let h = 1e-6;
                let p = |r: f64| {
                    let a = g.space_half_angle_at(r).unwrap();
                    (r * a.cos(), r * a.sin())
                };
                let (a, b) = (p(r - h), p(r + h));
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let l = dx.hypot(dy);
                let n = g.flank_normal_into_space(r).unwrap();
                let dot = (n.0 * dx + n.1 * dy) / l;
                assert!(
                    dot.abs() < 1e-7,
                    "z{} at r{r}: normal·tangent = {dot}",
                    g.teeth
                );
                // And it points into the space: toward smaller angle.
                let ang = g.space_half_angle_at(r).unwrap();
                let theta_hat = (-ang.sin(), ang.cos());
                assert!(
                    n.0 * theta_hat.0 + n.1 * theta_hat.1 < 0.0,
                    "z{} at r{r}: normal points out of the space",
                    g.teeth
                );
            }
        }
    }

    /// A cutter wider than the mouth of the space is refused with the width
    /// that refused it, not silently clamped.
    #[test]
    fn oversize_cutter_fails_closed() {
        let err = planet().tooth_space(6.0).unwrap_err();
        match err {
            GearError::CutterTooLarge {
                cutter_diameter,
                space_width,
                ..
            } => {
                assert_eq!(cutter_diameter, 6.0);
                assert!(space_width < 6.0, "space {space_width}");
            }
            other => panic!("expected CutterTooLarge, got {other}"),
        }
        assert!(planet().tooth_space(0.0).is_err());
        assert!(planet().tooth_space(f64::NAN).is_err());
    }

    /// A wide space bottoms on the root circle and leaves a real flat-ish arc;
    /// a narrow one wedges and leaves none. Both branches, one gear, by cutter
    /// size alone.
    #[test]
    fn both_root_bindings_occur_on_one_gear() {
        let g = SpurGear::external(2.0, 30);
        let small = g.tooth_space(1.0).unwrap();
        assert_eq!(small.binding, RootBinding::RootCircle);
        assert!(small.root_arc_half_angle > 0.0);
        assert!((small.effective_root_radius - g.r_root()).abs() < 1e-12);

        let big = g.tooth_space(4.4).unwrap();
        assert_eq!(big.binding, RootBinding::Flanks);
        assert_eq!(big.root_arc_half_angle, 0.0);
        assert!(
            big.effective_root_radius > g.r_root(),
            "wedged cutter cannot reach the root: {} vs {}",
            big.effective_root_radius,
            g.r_root()
        );
    }

    /// The fillet is exactly the cutter: its centre is `rc` from the flank at
    /// the form radius, and no closer anywhere else on the flank.
    #[test]
    fn fillet_centre_is_exactly_one_cutter_radius_off_the_flank() {
        for g in [sun(), planet(), ring()] {
            let s = g.tooth_space(1.0).unwrap();
            let (cx, cy) = (
                s.fillet_centre_radius * s.fillet_centre_angle.cos(),
                s.fillet_centre_radius * s.fillet_centre_angle.sin(),
            );
            let (lo, hi) = if g.internal {
                (g.r_tip(), s.form_radius)
            } else {
                (s.form_radius, g.r_tip())
            };
            let mut closest = f64::INFINITY;
            for i in 0..=2000 {
                let r = lo + (hi - lo) * i as f64 / 2000.0;
                let th = g.space_half_angle_at(r).unwrap();
                let d = (r * th.cos() - cx).hypot(r * th.sin() - cy);
                closest = closest.min(d);
            }
            assert!(
                (closest - s.cutter_radius).abs() < 1e-6,
                "z{}: closest flank approach {closest}, cutter radius {}",
                g.teeth,
                s.cutter_radius
            );
        }
    }
}
