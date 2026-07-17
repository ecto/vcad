//! M0 headline physics: magnetic self-shielding of a two-ring cathode.
//!
//! The claim under test (Hedditch/Bowden-Reid/Khachan 2015, qualitatively):
//! running current through the cathode rings wraps them in a magnetic
//! sheath that deflects incoming ions, so wire interception falls and
//! recirculation rises relative to the zero-current control at identical
//! geometry and bias.

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::stats;
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};

fn run(ampere_turns: f64) -> vcad_kernel_particle::fom::EnsembleStats {
    // Low bias keeps ions slow (easy to deflect) so the effect is decisive
    // at test-friendly grid resolution and particle counts.
    let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -500.0, ampere_turns);
    let sol = solve(&device, 81, 161, &SolveOptions::default()).expect("poisson");
    let fields = FieldMap::new(&device, &sol);
    let opts = TraceOptions {
        max_passes: 15,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &sol, opts);
    let outcomes = tracer.launch_ensemble(DEUTERON, 24);
    stats(&outcomes)
}

#[test]
fn shield_current_cuts_interception_and_boosts_recirculation() {
    let control = run(0.0);
    let shielded = run(40_000.0);

    // The control must actually exercise the loss channel.
    assert!(
        control.interception_fraction > 0.3,
        "control fusor barely intercepts: {control:?}"
    );
    assert!(
        shielded.interception_fraction < control.interception_fraction,
        "shielding did not reduce interception: control {control:?}, shielded {shielded:?}"
    );
    assert!(
        shielded.mean_passes > control.mean_passes,
        "shielding did not improve recirculation: control {control:?}, shielded {shielded:?}"
    );
}
