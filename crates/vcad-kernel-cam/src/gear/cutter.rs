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

/// Whether the cutter leaves enough involute for the mesh, and by how much.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Reachability {
    /// True when the involute survives across the whole contact range.
    pub ok: bool,
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
    /// Largest cutter diameter with a non-negative margin, mm. `None` when no
    /// cutter works.
    pub largest_cutter_diameter: Option<f64>,
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
    pub fn reachability(
        &self,
        cutter_diameter: f64,
        contact_limit: f64,
    ) -> Result<Reachability, GearError> {
        let form = self.tooth_space(cutter_diameter)?.form_radius;
        let margin = if self.internal {
            form - contact_limit
        } else {
            contact_limit - form
        };
        Ok(Reachability {
            ok: margin >= 0.0,
            internal: self.internal,
            cutter_diameter,
            form_radius: form,
            contact_limit,
            margin,
            largest_cutter_diameter: self.largest_cutter(contact_limit, 0.0).ok(),
        })
    }

    /// Largest cutter diameter whose fillet still clears `contact_limit` by
    /// `required_margin` mm.
    ///
    /// A bigger cutter always pushes the form radius the wrong way, so this is
    /// a plain bisection on a monotone function.
    pub fn largest_cutter(
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

        let v = sun().reachability(1.0, c_sp.pinion_lowest).unwrap();
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
        let v = planet().reachability(1.0, limit).unwrap();
        assert!(v.ok, "planet {v:?}");
        assert!(
            (v.margin - 0.178061).abs() < 1e-4,
            "planet margin {}",
            v.margin
        );

        let v = ring().reachability(1.0, c_pr.wheel_highest).unwrap();
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
            .reachability(largest - 1e-6, c_pr.wheel_highest)
            .unwrap();
        assert!(ok.ok, "largest cutter should pass: {ok:?}");
        let fails = ring()
            .reachability(largest + 1e-3, c_pr.wheel_highest)
            .unwrap();
        assert!(!fails.ok, "a hair larger should fail: {fails:?}");
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
