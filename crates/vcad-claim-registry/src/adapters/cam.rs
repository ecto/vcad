//! `vcad.cam-claims/1` — the family that exercises the whole registry.
//!
//! CAM is the family with every rung on it: claims that are arithmetic on the
//! program (`job.no_gouge`), claims that are predictions about a part nobody
//! has cut yet (`job.tabs_hold`, `gear.over_pins`), a fingerprint so a claim
//! knows what it rests on, and a measurement loop that turns a reading over
//! pins into the cutter offset for the next part. So it is the family that
//! needs all three registry entry points, and the one the others are checked
//! against.
//!
//! Everything below marshals; none of it decides. `restate`, `bind` and
//! `close_over_pins` are `vcad_kernel_cam::receipt`'s own.

use std::collections::BTreeMap;

use vcad_kernel_cam::gear::SpurGear;
use vcad_kernel_cam::receipt as cam;
use vcad_receipt::ReceiptClaim;

use crate::{BindOutcome, ClaimFamily, RegistryError};

fn parse(report_json: &str) -> Result<cam::ClaimSet, RegistryError> {
    let set: cam::ClaimSet = serde_json::from_str(report_json)
        .map_err(|e| RegistryError::bad_report(cam::CLAIM_SCHEMA, e))?;
    if set.schema != cam::CLAIM_SCHEMA {
        return Err(RegistryError::SchemaMismatch {
            schema: cam::CLAIM_SCHEMA.to_string(),
            found: set.schema.clone(),
        });
    }
    Ok(set)
}

fn render(set: &cam::ClaimSet) -> Result<String, RegistryError> {
    serde_json::to_string(set).map_err(|e| RegistryError::bad_report(cam::CLAIM_SCHEMA, e))
}

fn to_claims(report_json: &str) -> Result<Vec<ReceiptClaim>, RegistryError> {
    Ok(cam::design_claims(&parse(report_json)?))
}

/// Re-state the set against the input digests as they stand now.
///
/// `current` is keyed by CAM's own basis keys (`program`, `outline`, `tool`,
/// `stock`, `gear`, `material`, `solid`). A key a claim depends on that
/// `current` does not carry counts as changed, not as unchanged — a receipt
/// that cannot identify the job it certifies certifies nothing — and that
/// rule is `vcad_kernel_cam::receipt::restate`'s, not ours.
fn restate(report_json: &str, current: &BTreeMap<String, String>) -> Result<String, RegistryError> {
    let set = parse(report_json)?;
    let mut fp = cam::Fingerprint::new();
    for (key, digest) in current {
        fp = fp.with(key.clone(), digest.clone());
    }
    render(&cam::restate(&set, &fp))
}

/// Accept one measurement or a list of them, so a caller with a single
/// reading does not have to wrap it.
fn measurements_of(json: &str) -> Result<Vec<cam::Measurement>, RegistryError> {
    if let Ok(list) = serde_json::from_str::<Vec<cam::Measurement>>(json) {
        return Ok(list);
    }
    let one: cam::Measurement =
        serde_json::from_str(json).map_err(|e| RegistryError::bad_report(cam::CLAIM_SCHEMA, e))?;
    Ok(vec![one])
}

/// Bind measurements of the real part, and derive what they imply.
///
/// Two steps, in this order because the second needs the first's arithmetic:
///
/// 1. `cam::bind` moves every measured claim onto [`Basis::Measured`] and
///    decides `Holds` or `Violated` by whether the reading is inside
///    `tolerance + uncertainty` of the prediction. It refuses — rather than
///    ignoring — a measurement aimed at a computed claim, one with unusable
///    numbers, or one naming a claim that is not in the set.
/// 2. For a measurement over pins, when the deposit recorded the `gear` it
///    was predicted from, `cam::close_over_pins` turns the reading into a
///    tooth-thickness error and the cutter offset that corrects it, and adds
///    `gear.over_pins.compensated` — a *new prediction* for the next part,
///    superseding the one the measurement just closed. Provisional again, on
///    purpose: a compensation is a plan, and the second part has to be
///    measured too.
///
/// No gear in the context means step 2 is skipped and said so in the note —
/// never silently, because a missing compensation is the difference between
/// "cut it again like this" and "cut it again and hope".
fn bind(
    report_json: &str,
    measurements_json: &str,
    context: &serde_json::Value,
) -> Result<BindOutcome, RegistryError> {
    let set = parse(report_json)?;
    let measurements = measurements_of(measurements_json)?;
    let mut bound =
        cam::bind(&set, &measurements).map_err(|e| RegistryError::Refused(e.to_string()))?;

    let gear: Option<SpurGear> = context
        .get("gear")
        .and_then(|g| serde_json::from_value(g.clone()).ok());

    let mut derived = Vec::new();
    let mut skipped_compensation = false;
    for m in &measurements {
        if m.claim != "gear.over_pins" {
            continue;
        }
        let Some(gear) = gear else {
            skipped_compensation = true;
            continue;
        };
        // `close_over_pins` binds the measurement itself, so run it against
        // the ORIGINAL set: handing it the already-bound one would compare
        // the reading against a value that is now the reading.
        let closure = cam::close_over_pins(&gear, &set, m.subject.as_deref(), m)
            .map_err(|e| RegistryError::Refused(e.to_string()))?;
        derived.push(
            serde_json::to_value(closure.compensation)
                .map_err(|e| RegistryError::bad_report(cam::CLAIM_SCHEMA, e))?,
        );
        // The compensated claim is a new claim, not a replacement: the closed
        // one stays in the set as the evidence that the first part missed.
        bound.claims.push(closure.corrected);
    }

    let closed = measurements.len();
    let note = if !derived.is_empty() {
        format!(
            "{closed} measurement(s) bound; a compensated over-pins claim was added, \
             superseding the one the reading closed"
        )
    } else if skipped_compensation {
        format!(
            "{closed} measurement(s) bound. No cutter compensation was derived: the \
             deposit records no `gear` input to work the reading back through. \
             Re-deposit the gear job's claims with its gear definition."
        )
    } else {
        format!("{closed} measurement(s) bound")
    };

    Ok(BindOutcome {
        report: render(&bound)?,
        derived,
        note,
    })
}

/// The CAM family's registry entry.
pub fn family() -> ClaimFamily {
    ClaimFamily {
        schema: cam::CLAIM_SCHEMA,
        domain: cam::RECEIPT_DOMAIN,
        crate_name: "vcad-kernel-cam",
        summary: "milling jobs: gouge, material left, depth, rapids, tabs, loose pieces, \
                  envelope, plunges, cutter fit, gear geometry and the over-pins dimension",
        native_only: false,
        to_claims,
        restate: Some(restate),
        bind: Some(bind),
    }
}
