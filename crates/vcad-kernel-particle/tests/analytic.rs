//! Analytic physics benchmarks: the tracer against closed-form plasma
//! physics, end-to-end through the public API.

use vcad_kernel_particle::device::{Device, WireRing};
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{Fate, TraceOptions, Tracer, DEUTERON};

/// A magnetic bottle: two coaxial coils with the SAME current sign
/// (mirror, not cusp), grounded electrodes (a tiny bias only sets the
/// tracer's internal velocity scale).
fn magnetic_bottle(amp_turns: f64) -> Device {
    let ring = |z: f64| WireRing {
        ring_radius_mm: 40.0,
        z_mm: z,
        wire_radius_mm: 3.0,
        potential_v: -1.0,
        ampere_turns: amp_turns,
    };
    Device {
        chamber_radius_mm: 150.0,
        chamber_half_height_mm: 150.0,
        wall_potential_v: 0.0,
        rings: vec![ring(60.0), ring(-60.0)],
    }
}

/// The mirror criterion: particles inside the loss cone
/// (sin²α < B₀/B_max) stream out the ends; particles outside it are
/// reflected and stay confined.
#[test]
fn magnetic_mirror_confines_by_pitch_angle() {
    let device = magnetic_bottle(150_000.0);
    let sol = solve(&device, 61, 121, &SolveOptions::default()).unwrap();
    let fields = FieldMap::new(&device, &sol);
    // Mirror ratio for this geometry is ~3 (loss cone α ≲ 35°).
    let opts = TraceOptions {
        max_passes: 50,
        time_budget_factor: 0.0023,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &sol, opts);

    // ~1 keV deuteron: r_L ≈ 7 mm at the 0.8 T center field — safely
    // adiabatic against the ~40 mm field scale (a faster ion here is
    // genuinely non-adiabatic and punches through the throat).
    let speed = 3.1e5;
    let launch = |pitch_deg: f64| {
        let a = pitch_deg.to_radians();
        tracer.trace_from(
            DEUTERON,
            [0.0, 0.0, 0.0],
            [speed * a.sin(), 0.0, speed * a.cos()],
        )
    };

    // 10 degrees: deep inside the loss cone -> escapes out the end cap.
    let escaping = launch(10.0);
    assert_eq!(
        escaping.fate,
        Fate::Wall,
        "shallow pitch must escape the bottle: {escaping:?}"
    );

    // 60 degrees: well outside the loss cone -> reflected, still alive at
    // the full budget.
    let confined = launch(60.0);
    assert_eq!(
        confined.fate,
        Fate::Survived,
        "steep pitch must be mirror-confined: {confined:?}"
    );
    assert!(
        confined.time_s > 5.0 * escaping.time_s,
        "confined particle should outlive the escaping one by far: {} vs {}",
        confined.time_s,
        escaping.time_s
    );
}

/// Small-amplitude axial oscillation in the two-ring potential well: the
/// traced period must match 2π/ω with ω² = q·∂²φ/∂z²/m measured from the
/// solved potential itself.
#[test]
fn axial_oscillation_period_matches_the_well_curvature() {
    let device = Device::shielded_two_ring(100.0, 40.0, 20.0, 3.0, -2_000.0, 0.0);
    let sol = solve(&device, 81, 161, &SolveOptions::default()).unwrap();
    let fields = FieldMap::new(&device, &sol);
    // Period is measured over the first 4 periods (8 core entries): long
    // traces slowly leak oscillation amplitude below the core boundary
    // (cell-crossing kicks in the piecewise field pump the orbit — a
    // measured Boris-on-patches artifact, ~single-percent per period),
    // which starves the pass counter, not the physics under test.
    let max_passes = 8;
    let opts = TraceOptions {
        max_passes,
        // Generous budget so the run censors at the pass cap (exactly on
        // a core entry), giving a clean period estimate.
        time_budget_factor: 60.0,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &sol, opts);

    // From rest on the axis, above the core boundary (~15.7 mm) so pass
    // counting sees two core entries per period.
    let z0 = 0.022;
    let out = tracer.trace_from(DEUTERON, [0.0, 0.0, z0], [0.0, 0.0, 0.0]);
    assert_eq!(out.fate, Fate::Survived, "oscillator must not terminate");
    assert_eq!(out.core_passes, max_passes, "expected censoring at the cap");
    let period_traced = 2.0 * out.time_s / f64::from(out.core_passes);

    // Curvature of the well at the origin, from the same solution.
    let h = 0.004;
    let phi = |z: f64| sol.potential_at(0.0, z);
    let curv = (phi(h) - 2.0 * phi(0.0) + phi(-h)) / (h * h);
    assert!(curv > 0.0, "well must be restoring for positive ions");
    let omega = (DEUTERON.charge_c * curv / DEUTERON.mass_kg).sqrt();
    let period_harmonic = 2.0 * std::f64::consts::PI / omega;

    let rel = (period_traced - period_harmonic).abs() / period_harmonic;
    assert!(
        rel < 0.2,
        "period mismatch: traced {period_traced:.4e}, harmonic {period_harmonic:.4e} (rel {rel:.3}) — \
         anharmonicity allows some slack, not this much"
    );
}
