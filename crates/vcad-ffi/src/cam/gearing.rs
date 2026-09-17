//! `vcad_cam_gear`: the geometry the first milestone is measured against.
//!
//! One 20-tooth module-1.0 brass planet, cut with a Ø1 end mill, measured over
//! pins. This entry point hands back the report, the contours (the tooth space
//! the cutter leaves, and the whole profile), the path the cutter's *centre*
//! follows, the over-pins dimension for a pin the shop actually has, and —
//! when a reading is given — the tool offset that reading implies.

use serde::Deserialize;
use serde_json::{json, Value};

use vcad_kernel_cam::gear::contour::FlankTolerance;
use vcad_kernel_cam::{Contour, ContourSegment, GearReport, PlanetaryTrain, SpurGear};

use super::types::{finite, non_negative, positive};

/// One gear.
#[derive(Debug, Clone, Deserialize)]
struct GearReq {
    module: f64,
    teeth: u32,
    #[serde(default)]
    profile_shift: Option<f64>,
    #[serde(default)]
    pressure_angle: Option<f64>,
    #[serde(default)]
    internal: Option<bool>,
    #[serde(default)]
    backlash_thinning: Option<f64>,
    #[serde(default)]
    face_width: Option<f64>,
    #[serde(default)]
    tip_radius: Option<f64>,
}

impl GearReq {
    fn build(&self, what: &str) -> Result<SpurGear, String> {
        let module = positive(&format!("{what}.module"), self.module)?;
        if self.teeth < 3 {
            return Err(format!(
                "{what}.teeth is {}: a gear needs at least three.",
                self.teeth
            ));
        }
        let mut g = if self.internal.unwrap_or(false) {
            SpurGear::internal(module, self.teeth)
        } else {
            SpurGear::external(module, self.teeth)
        };
        if let Some(x) = self.profile_shift {
            g = g.with_profile_shift(finite(&format!("{what}.profile_shift"), x)?);
        }
        if let Some(a) = self.pressure_angle {
            let a = positive(&format!("{what}.pressure_angle"), a)?;
            if a >= 45.0 {
                return Err(format!(
                    "{what}.pressure_angle is {a}°: state it in degrees, below 45."
                ));
            }
            g.pressure_angle = a.to_radians();
        }
        if let Some(b) = self.backlash_thinning {
            g = g.with_backlash_thinning(non_negative(&format!("{what}.backlash_thinning"), b)?);
        }
        if let Some(w) = self.face_width {
            g = g.with_face_width(positive(&format!("{what}.face_width"), w)?);
        }
        if let Some(r) = self.tip_radius {
            g = g.with_tip_radius(positive(&format!("{what}.tip_radius"), r)?);
        }
        g.validate()
            .map_err(|e| format!("{what} is not a usable gear: {e}"))?;
        Ok(g)
    }
}

/// `{ "gear": {…}, "cutter_diameter": 1.0, "pin_diameter": 1.4,
///    "measured": 21.05, "span_teeth": 3,
///    "contours": true, "tooth": 0, "chordal_tolerance": 0.002,
///    "planetary": { "sun": {…}, "planet": {…}, "ring": {…}, "planets": 3 },
///    "flank_tolerance": 0.005 }`
#[derive(Debug, Clone, Deserialize)]
struct GearRequest {
    gear: GearReq,
    #[serde(default)]
    cutter_diameter: Option<f64>,
    #[serde(default)]
    pin_diameter: Option<f64>,
    /// An over-pins reading off the part, for the compensation.
    #[serde(default)]
    measured: Option<f64>,
    /// Add a span-over-teeth measurement, using the widest span whose anvils
    /// still land on usable flank.
    #[serde(default)]
    span: Option<bool>,
    #[serde(default)]
    contours: Option<bool>,
    #[serde(default)]
    tooth: Option<u32>,
    #[serde(default)]
    chordal_tolerance: Option<f64>,
    #[serde(default)]
    flank_tolerance: Option<f64>,
    #[serde(default)]
    planetary: Option<PlanetaryReq>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanetaryReq {
    sun: GearReq,
    planet: GearReq,
    ring: GearReq,
    #[serde(default)]
    planets: Option<u32>,
}

/// Run a gear request.
pub fn run(input: &str) -> Result<Value, String> {
    let req: GearRequest = serde_json::from_str(input)
        .map_err(|e| format!("the gear request could not be read: {e}. Expected a \"gear\"."))?;
    let gear = req.gear.build("gear")?;
    let cutter = match req.cutter_diameter {
        Some(d) => positive("cutter_diameter", d)?,
        None => gear.module,
    };

    let mut report = GearReport::new(&gear, cutter)
        .map_err(|e| format!("this gear cannot be cut as described: {e}"))?;

    if let Some(pin) = req.pin_diameter {
        report = report
            .with_pin(&gear, positive("pin_diameter", pin)?)
            .map_err(|e| format!("the over-pins dimension could not be worked out: {e}"))?;
    }
    if req.span.unwrap_or(false) {
        report = report
            .with_span(&gear)
            .map_err(|e| format!("the span measurement could not be worked out: {e}"))?;
    }

    let mut out = json!({
        "gear": gear,
        "cutter_diameter": cutter,
        "recommended_pin_diameter": gear.recommended_pin_diameter().ok(),
        "report": report,
    });

    // The compensation a measured reading implies: how far, and which way, to
    // move the cutter on the next part.
    if let Some(measured) = req.measured {
        let pin = positive(
            "pin_diameter",
            req.pin_diameter.ok_or(
                "a measured reading needs the pin_diameter it was taken with.".to_string(),
            )?,
        )?;
        let measured = positive("measured", measured)?;
        let nominal = gear
            .over_pins(pin)
            .map_err(|e| format!("the nominal over-pins dimension is unavailable: {e}"))?
            .dimension;
        let comp = gear
            .compensation(pin, measured, nominal)
            .map_err(|e| format!("the compensation could not be worked out: {e}"))?;
        out["compensation"] = json!({
            "pin_diameter": pin,
            "nominal": nominal,
            "measured": measured,
            "detail": comp,
            "note": format!(
                "the teeth came out {:+.4} mm {} than nominal; move the cutter {:+.4} mm along the flank normal ({:+.4} mm radially) on the next part.",
                comp.thickness_error,
                if comp.thickness_error > 0.0 { "thicker" } else { "thinner" },
                comp.tool_normal_offset,
                comp.tool_radial_offset
            ),
        });
    }

    if req.contours.unwrap_or(true) {
        let tol = FlankTolerance::new(match req.chordal_tolerance {
            Some(t) => positive("chordal_tolerance", t)?,
            None => vcad_kernel_cam::gear::contour::DEFAULT_CHORDAL_TOLERANCE,
        })
        .map_err(|e| format!("the chordal tolerance is unusable: {e}"))?;
        let tooth = req.tooth.unwrap_or(0);
        if tooth >= gear.teeth {
            return Err(format!(
                "tooth is {tooth} on a {}-tooth gear: spaces are numbered 0..{}.",
                gear.teeth,
                gear.teeth - 1
            ));
        }
        let space = gear
            .tooth_space_contour(tooth, cutter, tol)
            .map_err(|e| format!("the tooth space could not be drawn: {e}"))?;
        let profile = gear
            .full_profile_contour(cutter, tol)
            .map_err(|e| format!("the full profile could not be drawn: {e}"))?;
        let path = gear
            .tool_centre_path(tooth, cutter, tol)
            .map_err(|e| format!("the cutter path could not be worked out: {e}"))?;
        out["contours"] = json!({
            "tooth": tooth,
            "tooth_space": flatten(&space),
            "full_profile": flatten(&profile),
            "tool_centre_path": flatten(&path),
        });
    }

    if let Some(p) = &req.planetary {
        let train = PlanetaryTrain::new(
            p.sun.build("planetary.sun")?,
            p.planet.build("planetary.planet")?,
            p.ring.build("planetary.ring")?,
            p.planets.unwrap_or(3),
        );
        let mesh = train
            .mesh()
            .map_err(|e| format!("this planetary train does not mesh: {e}"))?;
        out["planetary"] = json!({
            "train": train,
            "mesh": mesh,
            "reports": train
                .reports(cutter)
                .map_err(|e| format!("the train's gears cannot all be cut with Ø{cutter}: {e}"))?,
        });
    }

    if let Some(t) = req.flank_tolerance {
        let limit = positive("flank_tolerance", t)?;
        out["flank_deviation_tolerance"] = json!(limit);
    }

    Ok(out)
}

/// A contour as a point list: the only shape a UI or a post can draw.
fn flatten(contour: &Contour) -> Vec<[f64; 2]> {
    let mut pts = vec![[contour.start.x, contour.start.y]];
    for seg in &contour.segments {
        match seg {
            ContourSegment::Line { to } => pts.push([to.x, to.y]),
            ContourSegment::Arc { to, center, ccw } => {
                // Arcs are sampled so the caller never has to know an arc's
                // sense to draw it; the chordal error is the same 2 µm the
                // flanks were sampled at.
                let from = *pts.last().expect("seeded above");
                let r = (from[0] - center.x).hypot(from[1] - center.y);
                let a0 = (from[1] - center.y).atan2(from[0] - center.x);
                let a1 = (to.y - center.y).atan2(to.x - center.x);
                let mut sweep = if *ccw { a1 - a0 } else { a0 - a1 };
                while sweep <= 1e-12 {
                    sweep += std::f64::consts::TAU;
                }
                let step = if r <= 2e-3 {
                    std::f64::consts::FRAC_PI_2
                } else {
                    2.0 * (1.0 - 2e-3 / r).clamp(-1.0, 1.0).acos()
                };
                let n = ((sweep / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
                for i in 1..=n {
                    if i == n {
                        pts.push([to.x, to.y]);
                    } else {
                        let f = i as f64 / n as f64;
                        let a = if *ccw { a0 + sweep * f } else { a0 - sweep * f };
                        pts.push([center.x + r * a.cos(), center.y + r * a.sin()]);
                    }
                }
            }
        }
    }
    pts
}
