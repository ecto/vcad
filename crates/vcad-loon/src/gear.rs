//! Involute spur-gear profiles.
//!
//! # Why a true involute and not the box approximation
//!
//! Every rana gear to date is "tangential cubes on the root circle": a tooth
//! count of boxes unioned onto a pitch cylinder. That recipe exists because
//! the shapes were built by a mesh-append helper with no 2D profile step, so a
//! curved flank had nowhere to live. vcad has one: [`vcad_ir::CsgOp::Sketch2D`]
//! takes a closed profile and [`vcad_ir::CsgOp::Extrude`] turns it into a solid
//! wall-by-wall, with no boolean pass at all. A gear is therefore *cheaper* as
//! an exact involute polygon than as N unioned boxes — N booleans is what the
//! box recipe actually costs, and booleans are where the cracks came from.
//!
//! So: exact involute flanks, sampled at [`FLANK_SAMPLES`] points per flank.
//! The two documented approximations, both the same ones `rana/GEARS.md`
//! already accepts for its wire-EDM profiles:
//!
//! 1. **The root fillet is a circular arc, not the true trochoid** left by a
//!    hob. Below the base circle the flank continues as a radial line down to
//!    the root circle. For printed and EDM'd prototypes at these loads this is
//!    the normal simplification; a hobbed production gear gets the real
//!    fillet from the tool regardless of what the CAD said.
//! 2. **Tip and root arcs are polyline-sampled**, like every other curved
//!    surface in the tessellated pipeline.
//!
//! No profile shift, no tip relief, no crowning: `x = 0` full-depth ISO teeth
//! with a 20° pressure angle. The solid is extruded from `z = 0` up the face
//! width, matching what `cylinder` and `prism` actually build.
//!
//! # Internal gears
//!
//! `internal = true` returns the *bore* profile — the material a ring gear
//! removes, not the ring itself. Subtract it from a tower or cover and what
//! is left has internal teeth. This is deliberate: the field failure being
//! fixed here is a brief that specified ring teeth tangent to the planet root
//! circle (0.0 clearance), which happens when the ring is modelled as its own
//! positive solid and then eyeballed against the planet. As a subtraction the
//! clearances fall out of the same standard proportions as the external case.

use std::f64::consts::PI;

use vcad_ir::{SketchSegment2D, Vec2};

/// Pressure angle, radians (20°, ISO full-depth standard).
pub const PRESSURE_ANGLE: f64 = 20.0 * PI / 180.0;
/// Addendum, in modules. Tip sits one module off the pitch circle.
pub const ADDENDUM: f64 = 1.0;
/// Dedendum, in modules. The extra 0.25 is the standard bottom clearance
/// that keeps a mating tip off this root — see [`GearDims::root_diameter`].
pub const DEDENDUM: f64 = 1.25;
/// Points sampled along each involute flank.
pub const FLANK_SAMPLES: usize = 14;
/// Points sampled along each tip arc.
pub const TIP_SAMPLES: usize = 5;
/// Points sampled along each root arc (the gap between two teeth).
pub const ROOT_SAMPLES: usize = 7;

/// Involute function: `inv(a) = tan(a) - a`.
fn inv(a: f64) -> f64 {
    a.tan() - a
}

/// The analytic diameters of a gear. Every one of these is exact — no
/// tessellation enters here, which is what makes them assertable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearDims {
    /// Module (mm of pitch diameter per tooth).
    pub module: f64,
    /// Tooth count.
    pub teeth: u32,
    /// True for a ring gear (teeth point inward).
    pub internal: bool,
}

impl GearDims {
    /// Pitch diameter `d = m z`.
    pub fn pitch_diameter(&self) -> f64 {
        self.module * self.teeth as f64
    }

    /// Base diameter `db = d cos(alpha)` — the circle the involute unwinds from.
    pub fn base_diameter(&self) -> f64 {
        self.pitch_diameter() * PRESSURE_ANGLE.cos()
    }

    /// Tip diameter. External teeth grow outward (`d + 2m`); internal teeth
    /// point inward, so the ring's tip circle is the *smaller* one (`d - 2m`).
    pub fn tip_diameter(&self) -> f64 {
        let a = 2.0 * ADDENDUM * self.module;
        if self.internal {
            self.pitch_diameter() - a
        } else {
            self.pitch_diameter() + a
        }
    }

    /// Root diameter. Mirrors [`Self::tip_diameter`]: `d -/+ 2.5m`.
    pub fn root_diameter(&self) -> f64 {
        let b = 2.0 * DEDENDUM * self.module;
        if self.internal {
            self.pitch_diameter() + b
        } else {
            self.pitch_diameter() - b
        }
    }
}

/// A gear to generate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearSpec {
    /// Module, mm.
    pub module: f64,
    /// Tooth count (>= 4).
    pub teeth: u32,
    /// Face width — the extrusion depth, mm.
    pub face_width: f64,
    /// Circumferential tooth thinning at the pitch circle, mm. Each member of
    /// a mesh is thinned by its own `backlash`, so a pair thinned 0.02 each
    /// runs ~0.04 of circumferential backlash per mesh.
    pub backlash: f64,
    /// True for a ring gear — see the module docs, this yields the bore.
    pub internal: bool,
}

impl GearSpec {
    /// The analytic diameters for this spec.
    pub fn dims(&self) -> GearDims {
        GearDims {
            module: self.module,
            teeth: self.teeth,
            internal: self.internal,
        }
    }

    /// Validate. Returns a human-readable reason when the spec cannot make a
    /// gear — these are hard errors rather than silent clamps because a
    /// silently-degenerate gear is exactly the failure this primitive exists
    /// to stop (a variant once shipped with blank pitch cylinders).
    pub fn validate(&self) -> Result<(), String> {
        if self.module.is_nan() || self.module <= 0.0 {
            return Err(format!("gear module must be > 0, got {}", self.module));
        }
        if self.teeth < 4 {
            return Err(format!("gear needs at least 4 teeth, got {}", self.teeth));
        }
        if self.face_width.is_nan() || self.face_width <= 0.0 {
            return Err(format!(
                "gear face width must be > 0, got {}",
                self.face_width
            ));
        }
        if self.backlash < 0.0 {
            return Err(format!("gear backlash must be >= 0, got {}", self.backlash));
        }
        // Half the circular tooth thickness at the pitch circle. If backlash
        // eats the whole tooth there is nothing to cut.
        if self.pitch_half_angle() <= 0.0 {
            return Err(format!(
                "gear backlash {} removes the entire tooth (circular thickness {:.4})",
                self.backlash,
                PI * self.module / 2.0
            ));
        }
        let d = self.dims();
        if d.tip_diameter() <= 0.0 || d.root_diameter() <= 0.0 {
            return Err("gear proportions produce a non-positive diameter".to_string());
        }
        Ok(())
    }

    /// Angular half-thickness of the tooth at the pitch circle, radians.
    fn pitch_half_angle(&self) -> f64 {
        let thickness = PI * self.module / 2.0 - self.backlash;
        thickness / self.dims().pitch_diameter()
    }

    /// Angular half-thickness of the tooth at radius `r`, radians.
    ///
    /// The involute identity: rolling out from the pitch circle to `r` the
    /// tooth narrows by `inv(phi(r)) - inv(alpha)`. Same relation for external
    /// and internal teeth — internal flanks are involutes of the same base
    /// circle, they simply run inward from the root.
    fn half_angle_at(&self, r: f64) -> f64 {
        let rb = self.dims().base_diameter() / 2.0;
        let cos_phi = (rb / r).clamp(-1.0, 1.0);
        let phi = cos_phi.acos();
        self.pitch_half_angle() + inv(PRESSURE_ANGLE) - inv(phi)
    }

    /// The closed tooth-profile polygon, counter-clockwise, centred on the
    /// origin. First point is not repeated at the end.
    pub fn profile(&self) -> Result<Vec<[f64; 2]>, String> {
        self.validate()?;
        let d = self.dims();
        let rb = d.base_diameter() / 2.0;
        let r_tip = d.tip_diameter() / 2.0;
        let r_root = d.root_diameter() / 2.0;

        // The flank runs between the root and tip radii, but the involute
        // itself only exists at or outside the base circle. Where the root
        // dives under the base circle (any external gear below ~17 teeth,
        // which is every sun in this train) the profile continues as a radial
        // line at the base-circle angle — the documented arc-root
        // approximation standing in for the hob's trochoid.
        let (r_near, r_far) = if self.internal {
            (r_tip, r_root) // internal: tip is the inner radius
        } else {
            (r_root, r_tip)
        };
        let r_lo = r_near.max(rb).min(r_far);

        // Guard a pointed tooth: if the flanks cross before the tip circle
        // the profile self-intersects, which is unbuildable and silently ugly.
        let far_half = self.half_angle_at(r_far.max(rb));
        if far_half <= 0.0 {
            return Err(format!(
                "gear m{} z{} is pointed: the flanks meet below the tip circle \
                 (half-angle {far_half:.5} rad). Reduce backlash or raise the tooth count.",
                self.module, self.teeth
            ));
        }

        let tau = 2.0 * PI / self.teeth as f64;
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let polar = |r: f64, a: f64| [r * a.cos(), r * a.sin()];

        for k in 0..self.teeth {
            let theta = tau * k as f64;

            // The angle the flank holds while it is under the base circle.
            let a_lo = self.half_angle_at(r_lo);

            // Radial run-out below the base circle, trailing side.
            if r_near < r_lo {
                pts.push(polar(r_near, theta - a_lo));
            }
            // Trailing flank, near -> far.
            for i in 0..FLANK_SAMPLES {
                let t = i as f64 / (FLANK_SAMPLES - 1) as f64;
                let r = r_lo + (r_far - r_lo) * t;
                pts.push(polar(r, theta - self.half_angle_at(r)));
            }
            // Tip arc.
            let a_far = self.half_angle_at(r_far);
            for i in 1..TIP_SAMPLES {
                let t = i as f64 / TIP_SAMPLES as f64;
                pts.push(polar(r_far, theta - a_far + 2.0 * a_far * t));
            }
            // Leading flank, far -> near.
            for i in 0..FLANK_SAMPLES {
                let t = i as f64 / (FLANK_SAMPLES - 1) as f64;
                let r = r_far + (r_lo - r_far) * t;
                pts.push(polar(r, theta + self.half_angle_at(r)));
            }
            if r_near < r_lo {
                pts.push(polar(r_near, theta + a_lo));
            }
            // Root arc across to the next tooth.
            let start = theta + a_lo;
            let end = theta + tau - a_lo;
            for i in 1..ROOT_SAMPLES {
                let t = i as f64 / ROOT_SAMPLES as f64;
                pts.push(polar(r_near, start + (end - start) * t));
            }
        }

        // Normalise winding to counter-clockwise (positive signed area). An
        // internal profile traversed this way comes out clockwise.
        let area: f64 = pts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = pts[(i + 1) % pts.len()];
                p[0] * q[1] - q[0] * p[1]
            })
            .sum();
        if area < 0.0 {
            pts.reverse();
        }
        Ok(pts)
    }

    /// The profile as closed sketch segments, ready for [`vcad_ir::CsgOp::Sketch2D`].
    pub fn sketch_segments(&self) -> Result<Vec<SketchSegment2D>, String> {
        let pts = self.profile()?;
        Ok((0..pts.len())
            .map(|i| {
                let a = pts[i];
                let b = pts[(i + 1) % pts.len()];
                SketchSegment2D::Line {
                    start: Vec2 { x: a[0], y: a[1] },
                    end: Vec2 { x: b[0], y: b[1] },
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(module: f64, teeth: u32, internal: bool) -> GearSpec {
        GearSpec {
            module,
            teeth,
            face_width: 6.0,
            backlash: 0.0,
            internal,
        }
    }

    #[test]
    fn analytic_diameters_match_iso_full_depth() {
        // The rana-60c train: sun 10T, planet 20T, ring 50T, all m0.5.
        let sun = spec(0.5, 10, false).dims();
        assert_eq!(sun.pitch_diameter(), 5.0);
        assert_eq!(sun.tip_diameter(), 6.0);
        assert_eq!(sun.root_diameter(), 3.75);

        let planet = spec(0.5, 20, false).dims();
        assert_eq!(planet.pitch_diameter(), 10.0);
        assert_eq!(planet.tip_diameter(), 11.0);
        assert_eq!(planet.root_diameter(), 8.75);

        let ring = spec(0.5, 50, true).dims();
        assert_eq!(ring.pitch_diameter(), 25.0);
        assert_eq!(ring.tip_diameter(), 24.0);
        assert_eq!(ring.root_diameter(), 26.25);
    }

    /// The generated profile must actually reach the analytic tip and root
    /// radii — the check that would have caught the blank-gear ship, where
    /// the "gear" was a pitch cylinder and max == min == pitch radius.
    #[test]
    fn profile_extremes_match_analytic_diameters() {
        for (m, z, internal) in [(0.5, 10, false), (0.5, 20, false), (0.5, 50, true)] {
            let s = spec(m, z, internal);
            let d = s.dims();
            let pts = s.profile().unwrap();
            let rs: Vec<f64> = pts.iter().map(|p| p[0].hypot(p[1])).collect();
            let rmax = rs.iter().cloned().fold(f64::MIN, f64::max);
            let rmin = rs.iter().cloned().fold(f64::MAX, f64::min);
            let (r_tip, r_root) = (d.tip_diameter() / 2.0, d.root_diameter() / 2.0);
            let (want_max, want_min) = if internal {
                (r_root, r_tip)
            } else {
                (r_tip, r_root)
            };
            assert!(
                (rmax - want_max).abs() < 1e-9,
                "m{m} z{z} internal={internal}: rmax {rmax} != {want_max}"
            );
            assert!(
                (rmin - want_min).abs() < 1e-9,
                "m{m} z{z} internal={internal}: rmin {rmin} != {want_min}"
            );
            // Not a blank: the profile must have real radial relief.
            assert!(rmax - rmin > 2.0 * m, "m{m} z{z}: profile has no teeth");
        }
    }

    #[test]
    fn profile_is_simple_and_ccw() {
        let pts = spec(0.5, 20, false).profile().unwrap();
        let area: f64 = (0..pts.len())
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            / 2.0;
        assert!(area > 0.0, "expected CCW winding, area {area}");
        // Angles must advance monotonically around the centre — a sufficient
        // condition for a star-shaped (hence simple) polygon.
        let mut prev = pts[0][1].atan2(pts[0][0]);
        let mut wound = 0.0;
        // Walk every edge including the closing one back to pts[0], or the
        // total falls one root-arc step short of a full turn.
        for p in pts.iter().skip(1).chain(std::iter::once(&pts[0])) {
            let a = p[1].atan2(p[0]);
            let mut step = a - prev;
            while step > PI {
                step -= 2.0 * PI;
            }
            while step < -PI {
                step += 2.0 * PI;
            }
            assert!(step >= -1e-12, "profile doubles back: step {step}");
            wound += step;
            prev = a;
        }
        assert!((wound.abs() - 2.0 * PI).abs() < 1e-6, "wound {wound}");
    }

    /// Centre-distance clearance for the 60c train, measured off the
    /// generated profiles rather than the formula: at cd = 7.5 every tip must
    /// stay at least 0.1 mm off the mating root.
    #[test]
    fn mesh_clearance_at_center_distance_7p5() {
        const CD: f64 = 7.5;
        let r = |s: GearSpec| {
            let pts = s.profile().unwrap();
            let rs: Vec<f64> = pts.iter().map(|p| p[0].hypot(p[1])).collect();
            (
                rs.iter().cloned().fold(f64::MAX, f64::min),
                rs.iter().cloned().fold(f64::MIN, f64::max),
            )
        };
        let (sun_root, sun_tip) = r(spec(0.5, 10, false));
        let (planet_root, planet_tip) = r(spec(0.5, 20, false));
        // Internal ring: min radius is the tip (pointing inward), max the root.
        let (ring_tip, ring_root) = r(spec(0.5, 50, true));

        // sun-planet: external/external, centres CD apart.
        let c1 = CD - sun_tip - planet_root;
        let c2 = CD - planet_tip - sun_root;
        // planet-ring: an internal mesh, centres CD apart. Measured from the
        // ring's centre the planet's root circle reaches out to CD + r_root
        // and its tip circle to CD + r_tip, so both clearances are radial
        // differences in the same direction — the ring's own circles sit
        // outside the planet's.
        let c3 = ring_root - (CD + planet_tip);
        let c4 = ring_tip - (CD + planet_root);

        for (name, c) in [
            ("sun tip / planet root", c1),
            ("planet tip / sun root", c2),
            ("planet tip / ring root", c3),
            ("ring tip / planet root", c4),
        ] {
            assert!(c >= 0.1, "{name}: clearance {c:.4} < 0.1 at cd {CD}");
            // 0.25 module is the standard bottom clearance; anything much
            // larger means the proportions drifted.
            assert!(c < 0.2, "{name}: clearance {c:.4} unexpectedly large");
        }
    }

    #[test]
    fn backlash_thins_the_tooth() {
        let thick = spec(0.5, 20, false);
        let mut thin = thick;
        thin.backlash = 0.02;
        // Half-angle at the pitch circle drops by backlash / d.
        let delta = thick.pitch_half_angle() - thin.pitch_half_angle();
        assert!((delta - 0.02 / 10.0).abs() < 1e-12, "delta {delta}");
        // Tip and root are unaffected by backlash.
        assert_eq!(thick.dims().tip_diameter(), thin.dims().tip_diameter());
    }

    #[test]
    fn degenerate_specs_are_rejected() {
        assert!(spec(0.0, 20, false).validate().is_err());
        assert!(spec(0.5, 3, false).validate().is_err());
        let mut fw = spec(0.5, 20, false);
        fw.face_width = 0.0;
        assert!(fw.validate().is_err());
        let mut bl = spec(0.5, 20, false);
        bl.backlash = 5.0;
        assert!(bl.validate().is_err());
    }

    #[test]
    fn sketch_segments_form_a_closed_loop() {
        let segs = spec(0.5, 10, false).sketch_segments().unwrap();
        assert_eq!(segs.len(), spec(0.5, 10, false).profile().unwrap().len());
        let ends: Vec<(Vec2, Vec2)> = segs
            .iter()
            .map(|s| match s {
                SketchSegment2D::Line { start, end } => (*start, *end),
                _ => panic!("expected lines"),
            })
            .collect();
        for i in 0..ends.len() {
            let next = ends[(i + 1) % ends.len()];
            assert!((ends[i].1.x - next.0.x).abs() < 1e-12);
            assert!((ends[i].1.y - next.0.y).abs() < 1e-12);
        }
    }
}
