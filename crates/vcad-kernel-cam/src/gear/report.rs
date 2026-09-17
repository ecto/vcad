//! The serializable claim: every number that decides whether this gear can be
//! cut, in one struct.
//!
//! Shaped for a future `vcad.cam-claims/1` receipt — predicted values now, a
//! measured over-pins dimension closing them later — so everything is a plain
//! number with a unit in its doc comment and nothing is a formatted string.

use super::{
    GearError, MeshReport, PinMeasurement, PlanetaryMesh, PlanetaryTrain, Reachability,
    RootBinding, SpanMeasurement, SpurGear, ToothSpace,
};
use serde::{Deserialize, Serialize};

/// Everything computed about one gear and the cutter that has to make it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearReport {
    /// Module, mm.
    pub module: f64,
    /// Tooth count.
    pub teeth: u32,
    /// Profile shift, modules.
    pub profile_shift: f64,
    /// Pressure angle, degrees.
    pub pressure_angle_deg: f64,
    /// True for an internal gear.
    pub internal: bool,
    /// Face width, mm.
    pub face_width: f64,
    /// Circumferential backlash thinning at the pitch circle, mm.
    pub backlash_thinning: f64,
    /// Reference pitch radius, mm.
    pub pitch_radius: f64,
    /// Base radius, mm.
    pub base_radius: f64,
    /// Tip radius in force, mm.
    pub tip_radius: f64,
    /// Tip radius the standard addendum would give, mm.
    pub nominal_tip_radius: f64,
    /// Nominal (ideal, unreachable) root radius, mm.
    pub nominal_root_radius: f64,
    /// Circular tooth thickness at the pitch circle, mm.
    pub pitch_tooth_thickness: f64,
    /// Tooth thickness at the tip circle, mm.
    pub tip_land: f64,
    /// True when a rack cutter would undercut this gear.
    pub rack_undercut: bool,
    /// True when the root dives inside the base circle, so part of the flank is
    /// the radial continuation.
    pub root_below_base_circle: bool,
    /// Cutter diameter this report is for, mm.
    pub cutter_diameter: f64,
    /// Which constraint stops the cutter.
    pub root_binding: RootBinding,
    /// Deepest radius the cutter reaches, mm.
    pub effective_root_radius: f64,
    /// Fillet tangency (form) radius, mm.
    pub form_radius: f64,
    /// Radius of the fillet's centre, mm.
    pub fillet_centre_radius: f64,
    /// Half-span of the root arc between the two fillets, radians.
    pub root_arc_half_angle: f64,
    /// Ideal space width at the nominal root, mm.
    pub space_width_at_root: f64,
    /// Ideal space width at the form radius, mm.
    pub space_width_at_form: f64,
    /// Ideal space width at the mouth (tip circle), mm.
    pub mouth_width: f64,
    /// Whether the involute survives the contact range, and by how much.
    pub reachability: Option<Reachability>,
    /// Predicted over/between-pins measurement.
    pub over_pins: Option<PinMeasurement>,
    /// Predicted base tangent measurement.
    pub span: Option<SpanMeasurement>,
    /// The meshes this gear takes part in.
    pub meshes: Vec<MeshReport>,
    /// The planetary train it belongs to, if any.
    pub planetary: Option<PlanetaryMesh>,
}

impl GearReport {
    /// Report a gear against a cutter.
    pub fn new(gear: &SpurGear, cutter_diameter: f64) -> Result<Self, GearError> {
        let space: ToothSpace = gear.tooth_space(cutter_diameter)?;
        Ok(Self {
            module: gear.module,
            teeth: gear.teeth,
            profile_shift: gear.profile_shift,
            pressure_angle_deg: gear.pressure_angle.to_degrees(),
            internal: gear.internal,
            face_width: gear.face_width,
            backlash_thinning: gear.backlash_thinning,
            pitch_radius: gear.r_pitch(),
            base_radius: gear.r_base(),
            tip_radius: gear.r_tip(),
            nominal_tip_radius: gear.nominal_r_tip(),
            nominal_root_radius: gear.r_root(),
            pitch_tooth_thickness: gear.pitch_tooth_thickness(),
            tip_land: gear.tip_land()?,
            rack_undercut: gear.is_rack_undercut(),
            root_below_base_circle: gear.root_below_base_circle(),
            cutter_diameter,
            root_binding: space.binding,
            effective_root_radius: space.effective_root_radius,
            form_radius: space.form_radius,
            fillet_centre_radius: space.fillet_centre_radius,
            root_arc_half_angle: space.root_arc_half_angle,
            space_width_at_root: gear.space_width_at(gear.r_root())?,
            space_width_at_form: space.space_width_at_form,
            mouth_width: space.mouth_width,
            reachability: None,
            over_pins: None,
            span: None,
            meshes: Vec::new(),
            planetary: None,
        })
    }

    /// Add the reachability verdict against a contact limit.
    pub fn with_contact_limit(
        mut self,
        gear: &SpurGear,
        contact_limit: f64,
    ) -> Result<Self, GearError> {
        self.reachability = Some(gear.reachability(self.cutter_diameter, contact_limit)?);
        Ok(self)
    }

    /// Add the predicted pin measurement.
    pub fn with_pin(mut self, gear: &SpurGear, pin_diameter: f64) -> Result<Self, GearError> {
        self.over_pins = Some(gear.over_pins(pin_diameter)?);
        Ok(self)
    }

    /// Add the predicted span measurement, choosing the span that lands on the
    /// live involute. Silently skipped for an internal gear, which has none.
    pub fn with_span(mut self, gear: &SpurGear) -> Result<Self, GearError> {
        if gear.internal {
            return Ok(self);
        }
        let k = gear.recommended_span_teeth(self.form_radius)?;
        self.span = Some(gear.span_measurement(k)?);
        Ok(self)
    }

    /// Attach a mesh this gear takes part in.
    pub fn with_mesh(mut self, mesh: MeshReport) -> Self {
        self.meshes.push(mesh);
        self
    }

    /// Attach the planetary train.
    pub fn with_planetary(mut self, mesh: PlanetaryMesh) -> Self {
        self.planetary = Some(mesh);
        self
    }
}

impl PlanetaryTrain {
    /// Report all three members against one cutter, with their meshes,
    /// reachability verdicts and predicted measurements filled in.
    ///
    /// Order is sun, planet, ring.
    pub fn reports(&self, cutter_diameter: f64) -> Result<Vec<GearReport>, GearError> {
        let mesh = self.mesh()?;
        let sp = self.sun_planet().contact_radii()?;
        let pr = self.planet_ring().contact_radii()?;
        let mut out = Vec::with_capacity(3);
        for (gear, limit, meshes) in [
            (self.sun, sp.pinion_lowest, vec![mesh.sun_planet]),
            (
                self.planet,
                sp.wheel_lowest.min(pr.pinion_lowest),
                vec![mesh.sun_planet, mesh.planet_ring],
            ),
            (self.ring, pr.wheel_highest, vec![mesh.planet_ring]),
        ] {
            let pin = gear.recommended_pin_diameter()?;
            let mut r = GearReport::new(&gear, cutter_diameter)?
                .with_contact_limit(&gear, limit)?
                .with_pin(&gear, pin)?
                .with_span(&gear)?
                .with_planetary(mesh);
            for m in meshes {
                r = r.with_mesh(m);
            }
            out.push(r);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::*;

    #[test]
    fn train_report_round_trips_through_json() {
        let train = PlanetaryTrain::new(sun(), planet(), ring(), 3);
        let reports = train.reports(1.0).unwrap();
        assert_eq!(reports.len(), 3);
        let json = serde_json::to_string(&reports).unwrap();
        let back: Vec<GearReport> = serde_json::from_str(&json).unwrap();
        for (a, b) in back.iter().zip(reports.iter()) {
            assert_eq!(a.teeth, b.teeth);
            assert_eq!(a.root_binding, b.root_binding);
            assert!((a.form_radius - b.form_radius).abs() < 1e-12);
            assert!(
                (a.reachability.unwrap().margin - b.reachability.unwrap().margin).abs() < 1e-12
            );
            assert!(
                (a.over_pins.unwrap().dimension - b.over_pins.unwrap().dimension).abs() < 1e-12
            );
        }
        // The claim the milestone rests on is in there, with its sign.
        assert!(reports[0].reachability.unwrap().ok);
        assert!(reports[1].reachability.unwrap().ok);
        assert!(!reports[2].reachability.unwrap().ok);
        assert!(reports[2].reachability.unwrap().margin < 0.0);
    }

    /// Every field that the fixture also carries agrees with it.
    #[test]
    fn report_carries_the_fixture_numbers() {
        let train = PlanetaryTrain::new(sun(), planet(), ring(), 3);
        let r = train.reports(1.0).unwrap();
        assert!((r[0].base_radius - 4.698463103929543).abs() < 1e-12);
        assert!((r[1].tip_land - 0.8186418694711965).abs() < 1e-12);
        assert!((r[2].nominal_root_radius - 26.72).abs() < 1e-12);
        assert!((r[2].effective_root_radius - 26.52577211671196).abs() < 1e-5);
        assert!((r[0].planetary.unwrap().ratio - 6.0).abs() < 1e-12);
        assert!(
            (r[0].planetary.unwrap().sun_planet.contact_ratio - 1.2365192525876858).abs() < 1e-9
        );
    }
}
