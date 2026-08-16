//! Drift guard for the boolean representation-fidelity matrix.
//!
//! The matrix is only useful if it stays current. This test re-runs it and
//! compares the fidelity class of every cell against the checked-in
//! baseline, so a change that quietly widens (or narrows) the mesh-CSG
//! fallback shows up in review as a baseline diff rather than as a surprise
//! at STEP-export time.
//!
//! Only the fidelity *class* is compared — not volumes or face counts,
//! which are known to differ between x86_64 and aarch64.
//!
//! Re-bless after an intentional change:
//!
//! ```text
//! VCAD_FIDELITY_BLESS=1 cargo test -p vcad-torture --test fidelity_baseline
//! ```
//!
//! and regenerate the human-readable report:
//!
//! ```text
//! cargo run -p vcad-torture -- fidelity \
//!   --md docs/boolean-fidelity-matrix.md \
//!   --json crates/vcad-torture/fidelity-baseline.json
//! ```

use std::collections::BTreeMap;

use vcad_torture::fidelity::FidelityMatrix;

const BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fidelity-baseline.json");

#[test]
fn fidelity_matches_baseline() {
    let matrix = FidelityMatrix::run();
    let current = matrix.classes();

    if std::env::var("VCAD_FIDELITY_BLESS").is_ok() {
        let json = serde_json::to_string_pretty(&current).expect("serialise baseline");
        std::fs::write(BASELINE, format!("{json}\n")).expect("write baseline");
        eprintln!("blessed {} cells into {BASELINE}", current.len());
        return;
    }

    let raw = std::fs::read_to_string(BASELINE)
        .unwrap_or_else(|e| panic!("reading {BASELINE}: {e} — run with VCAD_FIDELITY_BLESS=1"));
    let expected: BTreeMap<String, String> =
        serde_json::from_str(&raw).expect("baseline is a JSON object of id -> fidelity");

    let mut improved = Vec::new();
    let mut regressed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();

    for (id, now) in &current {
        match expected.get(id) {
            None => added.push(format!("  + {id}: {now}")),
            Some(was) if was != now => {
                // "analytic" is the good end; anything else is a loss.
                let line = format!("  ~ {id}: {was} -> {now}");
                if now == "analytic" {
                    improved.push(line);
                } else {
                    regressed.push(line);
                }
            }
            Some(_) => {}
        }
    }
    for id in expected.keys() {
        if !current.contains_key(id) {
            removed.push(format!("  - {id}"));
        }
    }

    if improved.is_empty() && regressed.is_empty() && added.is_empty() && removed.is_empty() {
        return;
    }

    let mut msg = String::from(
        "boolean fidelity drifted from the checked-in baseline.\n\
         If this is intentional, re-bless with \
         VCAD_FIDELITY_BLESS=1 and regenerate docs/boolean-fidelity-matrix.md.\n",
    );
    for (label, lines) in [
        ("REGRESSED (lost analytic representation)", &regressed),
        ("improved (gained analytic representation)", &improved),
        ("new cells", &added),
        ("cells no longer in the corpus", &removed),
    ] {
        if !lines.is_empty() {
            msg.push_str(&format!("\n{label}:\n{}\n", lines.join("\n")));
        }
    }
    panic!("{msg}");
}
