//! Every footprint named in the orderable parts DB (`lib/parts/orderable.json`)
//! must match its recorded `footprint_status`: `generated` parts resolve to a
//! real land pattern with the DB's pad count; `placeholder` parts are ones
//! vcad can't generate yet (you can order them but not place them faithfully).
//! When a generator lands for a placeholder this test fails, forcing the DB
//! to be updated, so the sourcing DB and the footprint generator stay in sync.

use vcad_ecad_symbols::footprint::resolve_footprint;

#[test]
fn orderable_db_footprints_resolve() {
    let raw = include_str!("../../../lib/parts/orderable.json");
    let db: serde_json::Value = serde_json::from_str(raw).expect("orderable.json parses");
    let parts = db["parts"].as_array().expect("parts array");
    assert!(!parts.is_empty());
    let mut failures = Vec::new();
    let mut placeholders = Vec::new();
    for p in parts {
        let id = p["id"].as_str().unwrap_or("?");
        let fp = p["footprint"].as_str().expect("footprint");
        // `pads` (optional) = distinct electrical pad numbers on the land
        // pattern, when the vendor footprint numbers by terminal rather than
        // by package lead (power packages: G/S/D = 1/2/3, diode K/A = 1/2).
        let pins = p["pads"]
            .as_u64()
            .or_else(|| p["pins"].as_u64())
            .expect("pins") as u32;
        let status = p["footprint_status"].as_str().expect("footprint_status");
        let res = resolve_footprint(fp, pins);
        // Distinct numbered pads: vendor land patterns split one pin into
        // several fingers (TDSON/TSDSON drain combs share a number) and add
        // unnumbered NPTH pegs, so raw pad count overstates the pin count.
        let pads = res
            .template
            .as_ref()
            .map(|t| {
                t.pads
                    .iter()
                    .filter(|p| !p.number.is_empty())
                    .map(|p| p.number.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            })
            .unwrap_or(0);
        match (status, res.matched) {
            ("generated", true) => {}
            ("placeholder", false) => {
                placeholders.push(format!("{id} ({fp})"));
            }
            ("placeholder", true) => {
                failures.push(format!(
                    "{id}: '{fp}' now generates a real land pattern — set footprint_status to \"generated\""
                ));
                continue;
            }
            _ => {
                failures.push(format!(
                    "{id}: '{fp}' status {status} but matched={}",
                    res.matched
                ));
                continue;
            }
        }
        if !res.matched {
            continue;
        }
        if pads as u32 != pins {
            failures.push(format!("{id}: '{fp}' has {pads} pads, DB says {pins}"));
        }
    }
    // Recorded so the remaining gap is visible in `--nocapture` output.
    eprintln!(
        "orderable parts still on placeholder footprints ({}):\n  {}",
        placeholders.len(),
        placeholders.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "footprint mismatches:\n{}",
        failures.join("\n")
    );
}
