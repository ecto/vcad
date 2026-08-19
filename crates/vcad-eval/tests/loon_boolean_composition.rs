//! End-to-end regression: loon source with chained / nested / `let`-bound
//! `difference` operations must evaluate to geometry with every cut applied.
//!
//! Uses clean axis-aligned cubes (no curved surfaces) so `volume()` is a
//! reliable oracle — the BRep boolean kernel is exact for these. This isolates
//! the *evaluator + loon* composition from any kernel curved-surface robustness
//! concerns: the point under test is that nesting/`let` do not drop cuts.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

fn volume(src: &str) -> f64 {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
            root_cache: None,
            mesh_segments: 0,
        },
    )
    .expect("evaluate_document");
    scene.parts[0].solid.as_ref().expect("root solid").volume()
}

/// 100³ body (1_000_000) minus three disjoint 10³ cubes (−3_000) = 997_000.
const EXPECTED: f64 = 997_000.0;

#[test]
fn chained_nested_difference_applies_all_cuts() {
    // Outer difference's subject is itself a difference, three deep.
    let v = volume(
        "[difference [translate 80 80 80 [cube 10 10 10]] \
           [difference [translate 50 50 50 [cube 10 10 10]] \
             [difference [translate 10 10 10 [cube 10 10 10]] \
               [cube 100 100 100]]]]",
    );
    assert!(
        (v - EXPECTED).abs() < 1.0,
        "chained diff vol={v}, want {EXPECTED}"
    );
}

#[test]
fn pipe_chained_difference_applies_all_cuts() {
    // The documented `pipe` authoring form threads the body through each cut —
    // it lowers to the same nested-subject Difference chain.
    let v = volume(
        "[pipe [cube 100 100 100] \
           [difference [translate 10 10 10 [cube 10 10 10]]] \
           [difference [translate 50 50 50 [cube 10 10 10]]] \
           [difference [translate 80 80 80 [cube 10 10 10]]]]",
    );
    assert!(
        (v - EXPECTED).abs() < 1.0,
        "pipe diff vol={v}, want {EXPECTED}"
    );
}

#[test]
fn let_bound_chained_difference_applies_all_cuts() {
    // Regression: `[let b BODY expr]` used to drop the body expression and
    // return the bare body (vol == 1_000_000, no cuts). It must now apply all.
    let v = volume(
        "[let b [cube 100 100 100] \
           [difference [translate 80 80 80 [cube 10 10 10]] \
             [difference [translate 50 50 50 [cube 10 10 10]] \
               [difference [translate 10 10 10 [cube 10 10 10]] b]]]]",
    );
    assert!(
        (v - EXPECTED).abs() < 1.0,
        "let-bound chained diff vol={v}, want {EXPECTED} (1_000_000 == cuts dropped)"
    );
}

#[test]
fn single_difference_applies_cut() {
    // Baseline: a single cut works (matches reported case 1).
    let v = volume("[difference [translate 10 10 10 [cube 10 10 10]] [cube 100 100 100]]");
    assert!(
        (v - 999_000.0).abs() < 1.0,
        "single diff vol={v}, want 999000"
    );
}
