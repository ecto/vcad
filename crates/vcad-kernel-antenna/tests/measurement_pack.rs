//! The M6 loop, end to end in software: predicted claims for the 915 MHz
//! PCB monopole, a synthetic NanoVNA sweep generated from the same model,
//! and the fail-closed compare that will meet the real board.

use vcad_kernel_antenna::ecad::add_trace_as_wire;
use vcad_kernel_antenna::nanovna::{measurements_from_s1p, parse_s1p, NanoVnaTolerances};
use vcad_kernel_antenna::receipt::{
    compare, predicted_claims, ClaimSet, FrequencyBand, Measurement, Verdict,
};
use vcad_kernel_antenna::{mom, solve_driven, AntennaError, Mesh, SolveOptions, WireGrid};

const OPTS: SolveOptions = SolveOptions {
    quad_outer: 6,
    quad_inner: 6,
};

/// The measurement-pack antenna: a 78 mm × 1.6 mm PCB monopole trace over
/// ground, modeled through the ecad adapter.
fn monopole() -> (Mesh, usize) {
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    add_trace_as_wire(&mut g, &[[0.0, 0.0, 0.0], [0.0, 0.0, 78.0]], 1.6, &[12]).unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    (mesh, feed)
}

fn band() -> FrequencyBand {
    FrequencyBand {
        f_lo_hz: 700e6,
        f_hi_hz: 1100e6,
        points: 81,
    }
}

fn monopole_claims() -> (Mesh, usize, ClaimSet) {
    let (mesh, feed) = monopole();
    let claims = predicted_claims(&mesh, feed, band(), 50.0, &OPTS).unwrap();
    (mesh, feed, claims)
}

/// Synthesize a NanoVNA .s1p export from the model itself, with an
/// optional frequency scale (0.72 ≈ what FR-4 will do to the real board).
fn synthetic_s1p(mesh: &Mesh, feed: usize, f_scale: f64) -> String {
    let mut out = String::from("! synthetic NanoVNA sweep (model-generated)\n# HZ S RI R 50\n");
    let n = 161;
    for i in 0..n {
        let f_model = 700e6 + 400e6 * i as f64 / (n - 1) as f64;
        let sol = solve_driven(mesh, feed, f_model, &OPTS).unwrap();
        let s = mom::s11(sol.z_in, 50.0);
        out.push_str(&format!(
            "{:.0} {:.8} {:.8}\n",
            f_model * f_scale,
            s.re,
            s.im
        ));
    }
    out
}

/// Perfect board (no substrate): every VNA-measurable claim Holds, and
/// the claims a one-port instrument cannot see read Unmeasured — so the
/// report does NOT fully verify. Fail-closed means exactly this.
#[test]
fn model_generated_sweep_holds_all_measurable_claims() {
    let (mesh, feed, claims) = monopole_claims();
    let sweep = parse_s1p(&synthetic_s1p(&mesh, feed, 1.0)).unwrap();
    let ms = measurements_from_s1p(&sweep, &claims, &NanoVnaTolerances::default()).unwrap();
    let report = compare(&claims, &ms).unwrap();

    for row in &report.rows {
        match row.claim.as_str() {
            "gain_dbi" | "radiation_efficiency" => {
                assert_eq!(row.verdict, Verdict::Unmeasured, "{}", row.claim);
            }
            _ => {
                assert_eq!(
                    row.verdict,
                    Verdict::Holds,
                    "{}: predicted {:.4e}, measured {:?}",
                    row.claim,
                    row.predicted,
                    row.measured
                );
            }
        }
    }
    assert_eq!(report.violated, 0);
    assert_eq!(report.unmeasured, 2);
    assert!(
        !report.fully_verified,
        "a report with unmeasured claims must never fully verify"
    );
}

/// The FR-4 rehearsal: shift the synthetic sweep down 28% (the ε_eff
/// downshift a real board will show). The frequency-bearing claims must
/// read Violated — loudly, because that violation IS the M1.5
/// measurement — and the report must not verify.
#[test]
fn substrate_downshift_reads_violated() {
    let (mesh, feed, claims) = monopole_claims();
    let sweep = parse_s1p(&synthetic_s1p(&mesh, feed, 0.72)).unwrap();
    let ms = measurements_from_s1p(&sweep, &claims, &NanoVnaTolerances::default()).unwrap();
    let report = compare(&claims, &ms).unwrap();

    let verdict_of = |name: &str| {
        report
            .rows
            .iter()
            .find(|r| r.claim == name)
            .unwrap()
            .verdict
    };
    assert_eq!(verdict_of("s11_min_freq"), Verdict::Violated);
    assert_eq!(verdict_of("resonance_in_band"), Verdict::Violated);
    assert!(report.violated >= 3, "violated = {}", report.violated);
    assert!(!report.fully_verified);
}

/// A measurement naming no claim is an error — never silently ignored.
#[test]
fn stray_measurement_fails_closed() {
    let (_, _, claims) = monopole_claims();
    let stray = Measurement {
        claim: "swr_at_band".into(),
        value: 1.5,
        tolerance: 0.1,
    };
    match compare(&claims, &[stray]) {
        Err(AntennaError::UnknownMeasurement { name }) => assert_eq!(name, "swr_at_band"),
        other => panic!("expected unknown-measurement error, got {other:?}"),
    }
}

/// The monopole claim set itself: resonance where the free-space model
/// puts it (~920 MHz for 78 mm), a −10 dB dip, and the substrate caveat
/// present on the numbers FR-4 will move.
#[test]
fn monopole_claims_have_the_free_space_story() {
    let (_, _, claims) = monopole_claims();
    let f_res = claims.claim("resonant_frequency").unwrap().value;
    assert!(
        (900e6..940e6).contains(&f_res),
        "free-space 78 mm quarter-wave resonance ≈ 921 MHz, got {:.1} MHz",
        f_res / 1e6
    );
    assert!(claims.claim("s11_db_at_band").unwrap().value < -12.0);
    assert!(claims.claim("bandwidth_10db").unwrap().value > 20e6);
    assert!(claims
        .claim("resonant_frequency")
        .unwrap()
        .note
        .contains("M1.5"));
}
