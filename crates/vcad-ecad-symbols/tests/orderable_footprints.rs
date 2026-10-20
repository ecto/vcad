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
    for p in parts {
        let id = p["id"].as_str().unwrap_or("?");
        let fp = p["footprint"].as_str().expect("footprint");
        let pins = p["pins"].as_u64().expect("pins") as u32;
        let status = p["footprint_status"].as_str().expect("footprint_status");
        let res = resolve_footprint(fp, pins);
        let pads = res.template.as_ref().map(|t| t.pads.len()).unwrap_or(0);
        match (status, res.matched) {
            ("generated", true) | ("placeholder", false) => {}
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
    assert!(
        failures.is_empty(),
        "footprint mismatches:\n{}",
        failures.join("\n")
    );
}
