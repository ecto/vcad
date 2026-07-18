//! Mechanical-domain adapter: mass properties and clearance assertions →
//! receipt claims.
//!
//! Translates measured mass properties (as computed by `inspect_cad` / the
//! kernel) plus an optional spec into claims. The adapter is deliberately
//! dependency-free — callers hand it plain numbers and say which oracle
//! produced them.
//!
//! Fail-closed rule this adapter fixes: `inspect_cad` today silently omits
//! `mass_g` when no material has a density. Here, a mass spec with no
//! measured mass yields an **unverifiable** claim naming the missing input,
//! never a silent skip.

use serde::{Deserialize, Serialize};

use crate::{ClaimQuantity, OracleRef, ReceiptClaim};

/// Domain tag for mechanical claims.
pub const DOMAIN: &str = "mechanical";

/// Measured mass properties. `None` means the oracle could not produce the
/// value (e.g. mass without a material density).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeasuredMassProperties {
    /// Total volume in mm³.
    pub volume_mm3: Option<f64>,
    /// Total surface area in mm².
    pub surface_area_mm2: Option<f64>,
    /// Total mass in grams (requires material densities).
    pub mass_g: Option<f64>,
    /// Axis-aligned bounding-box extents (x, y, z) in mm.
    pub bbox_extents_mm: Option<[f64; 3]>,
}

/// A target value with a relative tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpectedValue {
    /// The claimed value.
    pub target: f64,
    /// Allowed relative deviation, in percent.
    pub tolerance_pct: f64,
}

/// What the design claims about itself. Every populated field produces a
/// claim; missing measured inputs make that claim unverifiable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MassSpec {
    /// Upper bound on mass, grams ("under 50 g").
    pub max_mass_g: Option<f64>,
    /// Expected volume with tolerance.
    pub volume_mm3: Option<ExpectedValue>,
    /// Envelope the part must fit inside (x, y, z extents, mm).
    pub max_envelope_mm: Option<[f64; 3]>,
}

/// A named clearance assertion between two part groups, with its measured
/// outcome. The wire type behind `mech.clearance.*` receipt claims: the
/// full struct rides in the claim's `details` so a stored receipt can be
/// re-verified against a changed document without external context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ClearanceClaim {
    /// Assertion name, e.g. "air-gap".
    pub label: String,
    /// Part ids (stringified root node ids) of the first group.
    pub group_a: Vec<String>,
    /// Part ids of the second group.
    pub group_b: Vec<String>,
    /// Required minimum separation in mm.
    pub required_mm: f64,
    /// Measured minimum distance in mm (negative = penetration depth).
    pub measured_mm: f64,
    /// Whether the measured distance satisfies the requirement.
    pub holds: bool,
    /// Named allowed contact: when true, a measured distance within
    /// [`CONTACT_EPS_MM`] of zero (surfaces touching, e.g. a part bolted
    /// flush to another) satisfies the assertion even below `required_mm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub allow_contact: Option<bool>,
}

/// Tolerance (mm) around zero within which two parts count as "touching"
/// rather than clear or intersecting. Absorbs tessellation noise on flush
/// faces; 1 µm is far below any manufacturable clearance.
pub const CONTACT_EPS_MM: f64 = 1e-3;

impl ClearanceClaim {
    /// Build a claim, deriving `holds` from the measurement.
    pub fn new(
        label: impl Into<String>,
        group_a: Vec<String>,
        group_b: Vec<String>,
        required_mm: f64,
        measured_mm: f64,
    ) -> Self {
        Self {
            label: label.into(),
            group_a,
            group_b,
            required_mm,
            measured_mm: if measured_mm == 0.0 { 0.0 } else { measured_mm },
            holds: measured_mm.is_finite() && measured_mm >= required_mm,
            allow_contact: None,
        }
    }

    /// Mark contact as allowed and re-derive `holds` accordingly: a
    /// measurement within [`CONTACT_EPS_MM`] of zero passes.
    pub fn with_allow_contact(mut self) -> Self {
        self.allow_contact = Some(true);
        self.holds = self.measured_mm.is_finite()
            && (self.measured_mm >= self.required_mm || self.measured_mm.abs() <= CONTACT_EPS_MM);
        self
    }
}

/// Build receipt claims from measured clearance assertions.
///
/// One claim per assertion, id `mech.clearance.<label>`, with the required
/// distance as `predicted` and the measured distance as `measured`. The
/// verdict is re-derived from the numbers (fail-closed): a non-finite
/// measurement is unverifiable, and an inconsistent `holds` flag never
/// upgrades a failing measurement to a pass.
pub fn clearance_claims(assertions: &[ClearanceClaim], oracle: &OracleRef) -> Vec<ReceiptClaim> {
    assertions
        .iter()
        .map(|a| {
            let id = format!("mech.clearance.{}", a.label);
            let description = format!("clearance \"{}\" at least {} mm", a.label, a.required_mm);
            let subject = format!("{} vs {}", a.group_a.join("+"), a.group_b.join("+"));
            if !a.measured_mm.is_finite() || !a.required_mm.is_finite() {
                return ReceiptClaim::unverifiable(
                    id,
                    DOMAIN,
                    description,
                    oracle.clone(),
                    format!(
                        "non-finite clearance measurement ({} mm required, {} mm measured)",
                        a.required_mm, a.measured_mm
                    ),
                )
                .with_subject(subject);
            }
            let contact_ok =
                a.allow_contact.unwrap_or(false) && a.measured_mm.abs() <= CONTACT_EPS_MM;
            let holds = a.holds && (a.measured_mm >= a.required_mm || contact_ok);
            let mut c = ReceiptClaim::pass(id, DOMAIN, description, oracle.clone())
                .with_subject(subject)
                .with_predicted(ClaimQuantity::new(a.required_mm, "mm"))
                .with_measured(ClaimQuantity::new(a.measured_mm, "mm"));
            if !holds {
                c.verdict = crate::ClaimVerdict::Fail;
            }
            if let Ok(json) = serde_json::to_string(a) {
                c = c.with_details(json);
            }
            c
        })
        .collect()
}

fn finite_positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// Build mechanical claims from measured mass properties and a spec.
///
/// Always emits certification claims for whatever was measured (volume,
/// surface area, mass), and pass/fail claims for every spec bound. A spec
/// bound whose measured counterpart is missing produces an unverifiable
/// claim carrying the reason.
pub fn mass_properties_claims(
    measured: &MeasuredMassProperties,
    spec: &MassSpec,
    oracle: &OracleRef,
) -> Vec<ReceiptClaim> {
    let mut claims = Vec::new();
    let o = || oracle.clone();

    // Certification claims: the oracle vouches for the values it computed.
    match measured.volume_mm3 {
        Some(v) if finite_positive(v) => {
            let mut c = ReceiptClaim::pass("mech.volume", DOMAIN, "solid volume", o())
                .with_measured(ClaimQuantity::new(v, "mm^3"));
            if let Some(exp) = spec.volume_mm3 {
                let dev_pct = if exp.target != 0.0 {
                    ((v - exp.target) / exp.target).abs() * 100.0
                } else {
                    f64::INFINITY
                };
                c = c.with_predicted(ClaimQuantity::new(exp.target, "mm^3"));
                if dev_pct > exp.tolerance_pct {
                    c.verdict = crate::ClaimVerdict::Fail;
                    c = c.with_details(format!(
                        "deviation {dev_pct:.2}% exceeds tolerance {:.2}%",
                        exp.tolerance_pct
                    ));
                }
            }
            claims.push(c);
        }
        Some(v) => claims.push(ReceiptClaim::unverifiable(
            "mech.volume",
            DOMAIN,
            "solid volume",
            o(),
            format!("oracle produced a non-physical volume ({v})"),
        )),
        None if spec.volume_mm3.is_some() => claims.push(ReceiptClaim::unverifiable(
            "mech.volume",
            DOMAIN,
            "solid volume",
            o(),
            "volume was not computed",
        )),
        None => {}
    }

    if let Some(a) = measured.surface_area_mm2 {
        if finite_positive(a) {
            claims.push(
                ReceiptClaim::pass("mech.surface_area", DOMAIN, "surface area", o())
                    .with_measured(ClaimQuantity::new(a, "mm^2")),
            );
        }
    }

    // Mass: the flagship fail-closed case. A mass bound with no measured
    // mass (no material density assigned) is unverifiable, never skipped.
    match (measured.mass_g, spec.max_mass_g) {
        (Some(m), Some(max)) if finite_positive(m) => {
            let mut c =
                ReceiptClaim::pass("mech.mass", DOMAIN, format!("mass at most {max} g"), o())
                    .with_predicted(ClaimQuantity::new(max, "g"))
                    .with_measured(ClaimQuantity::new(m, "g"));
            if m > max {
                c.verdict = crate::ClaimVerdict::Fail;
            }
            claims.push(c);
        }
        (Some(m), None) if finite_positive(m) => claims.push(
            ReceiptClaim::pass("mech.mass", DOMAIN, "mass", o())
                .with_measured(ClaimQuantity::new(m, "g")),
        ),
        (Some(m), _) => claims.push(ReceiptClaim::unverifiable(
            "mech.mass",
            DOMAIN,
            "mass",
            o(),
            format!("oracle produced a non-physical mass ({m} g)"),
        )),
        (None, Some(max)) => claims.push(ReceiptClaim::unverifiable(
            "mech.mass",
            DOMAIN,
            format!("mass at most {max} g"),
            o(),
            "mass could not be computed — no material density assigned",
        )),
        (None, None) => {}
    }

    // Envelope fit.
    if let Some(env) = spec.max_envelope_mm {
        match measured.bbox_extents_mm {
            Some(ext) => {
                let fits = ext.iter().zip(env.iter()).all(|(e, m)| e <= m);
                let mut c = ReceiptClaim::pass(
                    "mech.envelope",
                    DOMAIN,
                    format!(
                        "fits within {} x {} x {} mm envelope",
                        env[0], env[1], env[2]
                    ),
                    o(),
                )
                .with_predicted(ClaimQuantity::new(
                    format!("{} x {} x {}", env[0], env[1], env[2]).as_str(),
                    "mm",
                ))
                .with_measured(ClaimQuantity::new(
                    format!("{} x {} x {}", ext[0], ext[1], ext[2]).as_str(),
                    "mm",
                ));
                if !fits {
                    c.verdict = crate::ClaimVerdict::Fail;
                }
                claims.push(c);
            }
            None => claims.push(ReceiptClaim::unverifiable(
                "mech.envelope",
                DOMAIN,
                "fits within envelope",
                o(),
                "bounding box was not computed",
            )),
        }
    }

    claims
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClaimVerdict, DesignReceipt};

    fn oracle() -> OracleRef {
        OracleRef::new("vcad.mass_properties", "test")
    }

    #[test]
    fn mass_bound_without_density_is_unverifiable() {
        let measured = MeasuredMassProperties {
            volume_mm3: Some(1000.0),
            ..Default::default()
        };
        let spec = MassSpec {
            max_mass_g: Some(50.0),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &spec, &oracle());
        let mass = claims.iter().find(|c| c.id == "mech.mass").unwrap();
        assert_eq!(mass.verdict, ClaimVerdict::Unverifiable);
        assert!(mass.details.as_deref().unwrap().contains("density"));
        // and the receipt as a whole cannot read clean
        let r = DesignReceipt::with_claims(claims);
        assert_eq!(r.overall(), ClaimVerdict::Unverifiable);
    }

    #[test]
    fn mass_over_bound_fails() {
        let measured = MeasuredMassProperties {
            mass_g: Some(61.2),
            ..Default::default()
        };
        let spec = MassSpec {
            max_mass_g: Some(50.0),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &spec, &oracle());
        let mass = claims.iter().find(|c| c.id == "mech.mass").unwrap();
        assert_eq!(mass.verdict, ClaimVerdict::Fail);
        assert_eq!(mass.predicted.as_ref().unwrap().unit.as_deref(), Some("g"));
    }

    #[test]
    fn mass_under_bound_passes_with_both_values() {
        let measured = MeasuredMassProperties {
            mass_g: Some(41.3),
            ..Default::default()
        };
        let spec = MassSpec {
            max_mass_g: Some(50.0),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &spec, &oracle());
        let mass = claims.iter().find(|c| c.id == "mech.mass").unwrap();
        assert_eq!(mass.verdict, ClaimVerdict::Pass);
        assert_eq!(
            mass.measured.as_ref().unwrap().value,
            crate::ClaimValue::Number(41.3)
        );
    }

    #[test]
    fn volume_within_tolerance_passes_outside_fails() {
        let measured = MeasuredMassProperties {
            volume_mm3: Some(1002.5),
            ..Default::default()
        };
        let ok = MassSpec {
            volume_mm3: Some(ExpectedValue {
                target: 1000.0,
                tolerance_pct: 1.0,
            }),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &ok, &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Pass);

        let tight = MassSpec {
            volume_mm3: Some(ExpectedValue {
                target: 1000.0,
                tolerance_pct: 0.1,
            }),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &tight, &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
    }

    #[test]
    fn envelope_fit_and_violation() {
        let measured = MeasuredMassProperties {
            bbox_extents_mm: Some([30.0, 20.0, 10.0]),
            ..Default::default()
        };
        let fits = MassSpec {
            max_envelope_mm: Some([40.0, 25.0, 12.0]),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &fits, &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Pass);

        let too_small = MassSpec {
            max_envelope_mm: Some([25.0, 25.0, 12.0]),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &too_small, &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
    }

    #[test]
    fn nan_volume_is_unverifiable_not_pass() {
        let measured = MeasuredMassProperties {
            volume_mm3: Some(f64::NAN),
            ..Default::default()
        };
        let claims = mass_properties_claims(&measured, &MassSpec::default(), &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Unverifiable);
    }

    #[test]
    fn allow_contact_passes_touching_but_not_penetration() {
        let touching = ClearanceClaim::new("bolted", vec!["1".into()], vec!["2".into()], 0.5, 0.0)
            .with_allow_contact();
        assert!(touching.holds);
        let claims = clearance_claims(&[touching], &oracle());
        assert_eq!(claims[0].verdict, crate::ClaimVerdict::Pass);

        let penetrating = ClearanceClaim::new("bad", vec!["1".into()], vec!["2".into()], 0.5, -1.0)
            .with_allow_contact();
        assert!(!penetrating.holds);
        let claims = clearance_claims(&[penetrating], &oracle());
        assert_eq!(claims[0].verdict, crate::ClaimVerdict::Fail);
    }

    #[test]
    fn clearance_that_holds_passes_with_both_values() {
        let a = ClearanceClaim::new("air-gap", vec!["1".into()], vec!["2".into()], 1.0, 1.02);
        assert!(a.holds);
        let claims = clearance_claims(&[a], &oracle());
        assert_eq!(claims.len(), 1);
        let c = &claims[0];
        assert_eq!(c.id, "mech.clearance.air-gap");
        assert_eq!(c.verdict, ClaimVerdict::Pass);
        assert_eq!(c.subject.as_deref(), Some("1 vs 2"));
        assert_eq!(
            c.predicted.as_ref().unwrap().value,
            crate::ClaimValue::Number(1.0)
        );
        assert_eq!(
            c.measured.as_ref().unwrap().value,
            crate::ClaimValue::Number(1.02)
        );
        // details carries the typed claim for lossless re-verification
        let back: ClearanceClaim = serde_json::from_str(c.details.as_deref().unwrap()).unwrap();
        assert_eq!(back.label, "air-gap");
        assert_eq!(back.required_mm, 1.0);
    }

    #[test]
    fn clearance_violation_fails_with_measured_value() {
        let a = ClearanceClaim::new(
            "rotor-screw",
            vec!["1".into()],
            vec!["2".into(), "3".into()],
            0.65,
            0.4,
        );
        assert!(!a.holds);
        let claims = clearance_claims(&[a], &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
        assert_eq!(
            claims[0].measured.as_ref().unwrap().value,
            crate::ClaimValue::Number(0.4)
        );
    }

    #[test]
    fn interference_reports_negative_distance_and_fails() {
        let a = ClearanceClaim::new("fit", vec!["1".into()], vec!["2".into()], 0.05, -1.3);
        let claims = clearance_claims(&[a], &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
        assert_eq!(
            claims[0].measured.as_ref().unwrap().value,
            crate::ClaimValue::Number(-1.3)
        );
    }

    #[test]
    fn inconsistent_holds_flag_never_upgrades_a_failure() {
        let mut a = ClearanceClaim::new("gap", vec!["1".into()], vec!["2".into()], 1.0, 0.5);
        a.holds = true; // lie
        let claims = clearance_claims(&[a], &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
    }

    #[test]
    fn non_finite_clearance_is_unverifiable() {
        let a = ClearanceClaim::new("gap", vec!["1".into()], vec!["2".into()], 1.0, f64::NAN);
        let claims = clearance_claims(&[a], &oracle());
        assert_eq!(claims[0].verdict, ClaimVerdict::Unverifiable);
        // …and it poisons the rollup, never reading clean.
        assert_eq!(
            DesignReceipt::with_claims(claims).overall(),
            ClaimVerdict::Unverifiable
        );
    }

    #[test]
    fn nothing_measured_no_spec_yields_no_claims() {
        let claims = mass_properties_claims(
            &MeasuredMassProperties::default(),
            &MassSpec::default(),
            &oracle(),
        );
        assert!(claims.is_empty());
        // …and an empty claim set rolls up unverifiable, not clean.
        assert_eq!(
            DesignReceipt::with_claims(claims).overall(),
            ClaimVerdict::Unverifiable
        );
    }
}
