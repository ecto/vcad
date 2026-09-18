//! The batching budget must not change the solid, and must not need a wall
//! clock the platform does not have.
//!
//! A chain of `difference` nodes is cut in ONE difference against the union
//! of its tools when the batching budget allows, and one tool at a time when
//! it does not. Both are supposed to be the same part. Before this test the
//! budget was read from `std::time::Instant::now()`, which panics on
//! `wasm32-unknown-unknown` and traps the module, so this exact document —
//! a plate with a bore and two holes, authored as a chain — killed the
//! browser kernel and fell back to the TypeScript evaluator.
//!
//! The two paths are driven here by the clock alone: the default (no clock
//! supplied) budget batches, and a clock that jumps a day per reading spends
//! the budget on the first charge, so the same document is also cut as
//! authored. The volumes must agree with each other and with the closed form.

use std::sync::atomic::{AtomicU64, Ordering};

use vcad_eval::{budget, evaluate_document, EvalOptions, EvaluatedScene};
use vcad_loon::eval_vcad;

/// A plate, a central bore and two corner holes, cut as a CHAIN of
/// differences — `[difference tool subject]` in loon is `subject - tool`, so
/// this is `plate - bore - hole_a - hole_b`. One difference against the union
/// of the three tools would not reach the batching path at all.
const REPRODUCER: &str = "\
[let plate [cube 80 50 6]]
[let bore [translate 40 25 -1 [cylinder-n 8 8 96]]]
[let hole-a [translate 12 12 -1 [cylinder-n 2.5 8 64]]]
[let hole-b [translate 68 38 -1 [cylinder-n 2.5 8 64]]]
[difference hole-b [difference hole-a [difference bore plate]]]";

/// Relative slack against [`closed_form_volume`], for the rim discretisation
/// that `Solid::volume()` — but not the model — has.
///
/// It admits 2.3 mm³, seven times the measured rim deficit, and the smallest
/// defect it has to catch — one cut silently skipped — is 118 mm³.
const CLOSED_FORM_TOLERANCE: f64 = 1e-4;

/// `80 · 50 · 6 − π · 8² · 6 − 2 · π · 2.5² · 6` = 22558.008972 mm³.
///
/// **The circles are circles.** `cylinder-n`'s count is a fidelity hint for
/// the boolean and seam machinery, not a face count: the surface stays a true
/// analytic cylinder, and `vcad-loon` refuses a count below 3 precisely
/// because a reader would otherwise take `[cylinder-n r h 6]` for a hex boss
/// (see `convert.rs::segments_val`). So `πr²` is the oracle, not the
/// inscribed-96-gon area — and [`ngon_volume`] measures the difference rather
/// than asserting it: this part comes back at 22558.332996 mm³, which is
/// 1.44e-5 from the circular figure and 4.06e-5 from the n-gon one, so the
/// circular form is nearer by 2.8×, and on the correct side.
///
/// The residual is discretisation in the *measurement*, not in the model:
/// `Solid::volume()` integrates the tessellation, and a boolean's rims are
/// sag-adaptive rather than the authored 96/64 — the 0.324 mm³ excess is the
/// inscribed-polygon deficit of rims of roughly 199 points at r=8 and 112 at
/// r=2.5 (0.0584 mm² of missing bore area against the 0.0540 mm² measured).
/// Hence [`CLOSED_FORM_TOLERANCE`].
fn closed_form_volume() -> f64 {
    let plate = 80.0 * 50.0 * 6.0;
    let bore = std::f64::consts::PI * 8.0 * 8.0 * 6.0;
    let hole = std::f64::consts::PI * 2.5 * 2.5 * 6.0;
    plate - bore - 2.0 * hole
}

/// The same part if `cylinder-n`'s count *were* a facet count: regular
/// n-gons of area `n · r² · sin(2π/n) / 2`, at the authored 96 and 64.
fn ngon_volume() -> f64 {
    let ngon = |n: f64, r: f64| n * r * r * (std::f64::consts::TAU / n).sin() / 2.0;
    80.0 * 50.0 * 6.0 - ngon(96.0, 8.0) * 6.0 - 2.0 * ngon(64.0, 2.5) * 6.0
}

/// A clock that jumps a full day every time it is read, so the very first
/// charge against a 20 s budget is refused.
struct JumpingClock(AtomicU64);

impl JumpingClock {
    fn new() -> Self {
        JumpingClock(AtomicU64::new(0))
    }
}

impl vcad_eval::Clock for JumpingClock {
    fn now_ms(&self) -> f64 {
        86_400_000.0 * self.0.fetch_add(1, Ordering::Relaxed) as f64
    }
}

fn evaluate(clock: Option<Box<dyn vcad_eval::Clock>>) -> EvaluatedScene {
    let doc = eval_vcad(REPRODUCER, None).expect("eval_vcad");
    evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock,
            root_cache: None,
            mesh_segments: 0,
            on_root: None,
        },
    )
    .expect("evaluate_document")
}

fn root_volume(scene: &EvaluatedScene) -> f64 {
    scene.parts[0]
        .solid
        .as_ref()
        .expect("the root came back without a B-rep")
        .volume()
}

#[test]
fn a_difference_chain_evaluates_the_same_with_and_without_a_clock() {
    // Batched: no clock supplied, so the budget runs on the platform default
    // (native `Instant`, and nothing at all on wasm32 — where it counts
    // operations instead). The budget is never spent on a chain this short.
    budget::reset_exhaustions();
    let batched = evaluate(None);
    let batched_volume = root_volume(&batched);
    assert_eq!(
        budget::exhaustions(),
        0,
        "a three-tool chain exhausted the default budget; it is meant to batch"
    );
    // A refusal count of zero is also what a document that never reached the
    // batching path reports, and this whole test is the comparison of two
    // paths — so say positively that this one batched. `cut_chain` charges
    // once per tool past the first while fusing them (2), then once more
    // before committing to the single difference against the fusion (3).
    assert!(
        budget::charges() >= 3,
        "the chain took only {} charges, so it never reached the batched \
         difference and this test is comparing one path with itself",
        budget::charges()
    );

    // Cut as authored: the clock is past the deadline before the first
    // union, so `cut_chain` abandons the batch and differences one tool at a
    // time. The refusal is what makes this the OTHER path, so assert it.
    budget::reset_exhaustions();
    let chained = evaluate(Some(Box::new(JumpingClock::new())));
    let chained_volume = root_volume(&chained);
    assert!(
        budget::exhaustions() >= 1,
        "the jumped clock never exhausted a budget, so both runs took the batched path"
    );
    assert_eq!(
        budget::charges(),
        1,
        "the batching kept charging after the budget refused it"
    );

    // Measured at 4.7e-11 mm³ apart — the two paths reassociate the same
    // booleans, they do not reproduce each other bit for bit.
    assert!(
        (batched_volume - chained_volume).abs() < 1e-6,
        "batched {batched_volume} vs chained {chained_volume}: the budget changed the solid"
    );

    let want = closed_form_volume();
    for (name, got) in [("batched", batched_volume), ("chained", chained_volume)] {
        let rel = (got - want).abs() / want;
        assert!(
            rel < CLOSED_FORM_TOLERANCE,
            "{name} volume {got}, closed form {want} (relative error {rel:.3e})"
        );

        // …and the circular oracle is the right one: `cylinder-n`'s count is
        // a fidelity hint, so the bores are true circles, not inscribed
        // 96-/64-gons. A change that turned the count into real facets would
        // still pass the tolerance above — it would fail here.
        let ngon = ngon_volume();
        assert!(
            (got - want).abs() < (got - ngon).abs(),
            "{name} volume {got} sits closer to the inscribed-n-gon figure {ngon} \
             than to the circular one {want}: `cylinder-n` has started faceting"
        );
    }
}
