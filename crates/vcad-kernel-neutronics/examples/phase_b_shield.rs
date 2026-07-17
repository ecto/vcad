//! M6 — the application this crate was built for: the shielded-grid IEC
//! experiment's **Phase B dose plan** (docs/shielded-grid-experiment.md).
//!
//! Design point: isotropic 2.45 MeV D-D source at 5×10⁶ n/s (the
//! amateur-record scale the experiment's chain+volume channels aim at —
//! an order above the predicted beam-on-background floor, so this plan
//! is conservative against the machine under-performing). Operator at
//! 2 m. Stated design budget: **2.5 µSv/h at the operator chair** — a
//! design choice (≈1 mSv over 400 h of run time, the general-public
//! annual limit), NOT a regulatory determination; verify local rules
//! before Phase B.
//!
//! Everything runs through the M3 spec seam and prices M4 claims, so
//! the printed table and the receipt come from the same object the
//! experiment will commit alongside the build.
//!
//! Run: `cargo run --release -p vcad-kernel-neutronics --example phase_b_shield`

use std::collections::BTreeMap;

use vcad_kernel_neutronics::receipt::predicted_claims;
use vcad_kernel_neutronics::spec::{d_dose_d_param_via_diffusion, evaluate, ShieldSpec};

const RATE: f64 = 5.0e6;
const BUDGET_USV_H: f64 = 2.5;

fn spec_json(hdpe_mm: f64, borated_mm: f64) -> String {
    let mut layers = vec![r#"{"material": "air", "thickness_mm": 150}"#.to_string()];
    if hdpe_mm > 0.0 {
        layers.push(r#"{"material": "hdpe", "thickness_mm": "hdpe_t"}"#.to_string());
    }
    if borated_mm > 0.0 {
        layers.push(format!(
            r#"{{"material": "borated-hdpe-5", "thickness_mm": {borated_mm}}}"#
        ));
    }
    layers.push(r#"{"material": "air", "thickness_mm": 2350}"#.to_string());
    format!(
        r#"{{
          "layers": [{}],
          "source": {{"rate_n_per_s": {RATE}, "energy_ev": 2.45e6}},
          "detectors": [
            {{"label": "bystander-1m", "radius_mm": 1000}},
            {{"label": "operator-2m", "radius_mm": 2000}}
          ],
          "run": {{"histories_per_batch": 50000, "batches": 20, "seed": 20260717}}
        }}"#,
        layers.join(",")
    )
}

fn main() {
    println!("Phase B dose plan — 2.45 MeV, {RATE:.0e} n/s, budget {BUDGET_USV_H} µSv/h at 2 m");
    println!("(seed 20260717, 1e6 histories/config, M1 exact kinematics)\n");
    println!("| shield | dose @ 1 m (µSv/h) | dose @ 2 m (µSv/h) | budget @ 2 m |");
    println!("|---|---:|---:|---|");

    let mut rows: Vec<(String, f64, f64)> = Vec::new();
    let configs: Vec<(String, f64, f64)> = vec![
        ("bare".to_string(), 0.0, 0.0),
        ("5 cm HDPE".to_string(), 50.0, 0.0),
        ("10 cm HDPE".to_string(), 100.0, 0.0),
        ("15 cm HDPE".to_string(), 150.0, 0.0),
        ("20 cm HDPE".to_string(), 200.0, 0.0),
        ("25 cm HDPE".to_string(), 250.0, 0.0),
        ("15 cm HDPE + 5 cm borated-5%".to_string(), 150.0, 50.0),
    ];
    for (label, hdpe, borated) in &configs {
        let spec: ShieldSpec = serde_json::from_str(&spec_json(*hdpe, *borated)).unwrap();
        let params = BTreeMap::from([("hdpe_t".to_string(), *hdpe)]);
        let params = if *hdpe > 0.0 { params } else { BTreeMap::new() };
        let (doses, result) = evaluate(&spec, &params).unwrap();
        assert_eq!(result.truncated_histories, 0);
        let d1 = &doses[0].dose_usv_per_h;
        let d2 = &doses[1].dose_usv_per_h;
        let verdict = if d2.mean + 2.0 * d2.abs_sigma() < BUDGET_USV_H {
            "PASS (mean + 2σ under)"
        } else if d2.mean < BUDGET_USV_H {
            "marginal (mean under, 2σ over)"
        } else {
            "over budget"
        };
        println!(
            "| {label} | {:.3} ± {:.1}% | {:.3} ± {:.1}% | {verdict} |",
            d1.mean,
            d1.rse * 100.0,
            d2.mean,
            d2.rse * 100.0
        );
        rows.push((label.clone(), d2.mean, d2.abs_sigma()));
    }

    // The chosen design: smallest all-HDPE stack passing with margin.
    println!("\n== chosen design: 15 cm HDPE (+5 cm borated option for capture-gamma economy)");
    let spec: ShieldSpec = serde_json::from_str(&spec_json(150.0, 0.0)).unwrap();
    let params = BTreeMap::from([("hdpe_t".to_string(), 150.0)]);

    // Design compass at the chosen point: how much dose does the next
    // millimeter of HDPE buy?
    let g = d_dose_d_param_via_diffusion(&spec, &params, "hdpe_t", "operator-2m").unwrap();
    println!(
        "compass: d(dose@2m)/d(hdpe_t) = {g:.4} µSv/h per mm (diffusion adjoint — \
         steer with it, price with MC)"
    );

    // The receipt for the chosen design.
    let claims = predicted_claims(&spec, &params).unwrap();
    println!("\n== vcad.neutronics-claims/1 for the chosen design:");
    println!("{}", serde_json::to_string_pretty(&claims).unwrap());

    // NAA feasibility (second customer): thermal flux available at a
    // sample tucked 2.5 cm deep into the shield's inner face.
    let naa_spec: ShieldSpec = serde_json::from_str(&spec_json(150.0, 0.0).replace(
        r#"{"label": "bystander-1m", "radius_mm": 1000}"#,
        r#"{"label": "naa-sample", "radius_mm": 175, "half_width_mm": 10}"#,
    ))
    .unwrap();
    let resolved = naa_spec.resolve(&params).unwrap();
    let naa_region = resolved
        .detector_regions
        .iter()
        .find(|(l, _)| l == "naa-sample")
        .unwrap()
        .1;
    let (_naa_doses, naa_result) = evaluate(&naa_spec, &params).unwrap();
    let th = naa_result.flux_per_source[naa_region][vcad_kernel_neutronics::groups::THERMAL_GROUP];
    println!(
        "\n== NAA feasibility: thermal flux at a sample 2.5 cm into the shield inner face:\n\
         {:.3e} n/cm²/s ± {:.1}% at {RATE:.0e} n/s",
        th.mean * RATE,
        th.rse * 100.0
    );
    println!(
        "(foil-activation detector calibration: workable; trace-element NAA wants \
         ≥1e5–1e7 n/cm²/s — this source is orders below that. Honest answer: \
         calibration yes, assay no.)"
    );
}
