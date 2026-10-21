//! Native leg of the engine parity harness (see `tests/parity/README.md`).

use std::path::PathBuf;

use vcad_app::parity::{replay_native, Fixture};

fn fixtures() -> Vec<(PathBuf, Fixture)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .expect("parity dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let f = serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (p, f)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no parity fixtures");
    out
}

#[test]
fn native_replay_matches_golden() {
    let update = std::env::var_os("UPDATE_PARITY").is_some();
    for (path, mut fixture) in fixtures() {
        let got = replay_native(&fixture);
        if update {
            fixture.expect = got;
            let json = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
            std::fs::write(&path, json).unwrap();
            continue;
        }
        assert_eq!(
            got,
            fixture.expect,
            "{} drifted; rerun with UPDATE_PARITY=1 if intended",
            path.display()
        );
    }
}

#[test]
fn replay_is_deterministic() {
    for (path, fixture) in fixtures() {
        assert_eq!(
            replay_native(&fixture),
            replay_native(&fixture),
            "{}",
            path.display()
        );
    }
}
