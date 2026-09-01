//! Real-world acceptance: the shipped rana `60c` shell must come back clean.
//!
//! The synthetic fixtures prove the checker can fail. This proves it does not
//! cry wolf on a part that was actually printed — a 25k-triangle shell with
//! through-wall J-slots, a chamfered rim, and roofs the author documented as
//! deliberate bridges.
//!
//! The STL lives in the sibling `rana` repo rather than in this one (1.2 MB of
//! binary, and it is that project's artifact, not vcad's), so the test locates
//! it and skips when it is not there. Point `VCAD_PRINTCHECK_SHELL` at a copy
//! to run it elsewhere.

use std::path::PathBuf;

use vcad_printcheck::{check_file, FindingKind, Options};

fn shell() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VCAD_PRINTCHECK_SHELL") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join("Developer/rana/exports/parts-60c/rana-60c-shell.stl");
    p.exists().then_some(p)
}

/// The two whitelisted zones are the shell's documented bridges: the bottom
/// slot roofs (z 1.75..2.65) and the top leg roof wedges (z 25.6..26.25),
/// exactly the ranges `rana tools/support-check.py` allows.
fn opts() -> Options {
    Options {
        allow_bridges: vec![(1.75, 2.65), (25.6, 26.25)],
        ..Default::default()
    }
}

#[test]
fn rana_60c_shell_is_clean() {
    let Some(p) = shell() else {
        eprintln!("skipping: rana-60c-shell.stl not found (set VCAD_PRINTCHECK_SHELL)");
        return;
    };
    let r = check_file(&p, &opts()).unwrap();

    assert_eq!(r.manifold.bad_edges, 0, "the shipped shell is manifold");
    assert!(r.sections.empty_layers.is_empty(), "no empty layers");
    assert!(r.sections.open_sections.is_empty(), "every section closes");
    assert_eq!(r.columns.cracks, 0, "no interior cracks");
    assert_eq!(r.columns.floating_regions, 0, "nothing floats");
    assert!(
        r.walls.min_feature_mm.unwrap() >= 0.4,
        "min feature {:?} should clear a 0.4 nozzle",
        r.walls.min_feature_mm
    );
    assert!(
        r.ok,
        "the shipped shell must pass:\n{}",
        vcad_printcheck::render_text(&r)
    );
}

#[test]
fn rana_60c_shell_slot_roofs_are_reported_as_bridges() {
    let Some(p) = shell() else { return };
    let r = check_file(&p, &opts()).unwrap();
    let roofs: Vec<_> = r.columns.bridges.iter().filter(|b| b.whitelisted).collect();
    assert!(
        !roofs.is_empty(),
        "the documented slot roofs should show up as bridge spans"
    );
    assert!(
        roofs
            .iter()
            .all(|b| (1.75..=2.65).contains(&b.z) || (25.6..=26.25).contains(&b.z)),
        "only the documented heights should be waived: {roofs:#?}"
    );
}

#[test]
fn without_the_whitelist_the_slot_roofs_are_flagged() {
    // The waiver has to be doing real work: with the zones removed, the same
    // roofs must be reported as overlong spans. Otherwise the "pass" above
    // would prove nothing about the bridge check.
    let Some(p) = shell() else { return };
    let r = check_file(&p, &Options::default()).unwrap();
    assert!(
        r.has(FindingKind::OverlongBridge),
        "un-waived slot roofs should exceed the 4 mm bridge convention"
    );
    assert!(!r.ok);
}
