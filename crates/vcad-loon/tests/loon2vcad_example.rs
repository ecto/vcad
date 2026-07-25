//! The documented `.loon` → `.vcad` workflow (`examples/loon2vcad.rs`) must
//! emit a document the rest of the toolchain — `vcad-render` in particular —
//! can consume: parsed JSON with the expected partDefs/instances/joints and a
//! *string* `version`.

use std::path::Path;
use std::process::Command;

const SRC: &str = r#"
[let link [cube 40 10 10]]
[assembly
  #[[part "link" link "aluminum"]]
  #[[instance "link1" "link" 0 0 0]
    [instance "link2" "link" 40 0 0]]
  #[[revolute-joint "j1" 0 0 1 -90 90 "link1" 40 5 5 "link2" 0 5 5]]
  "link1"]
"#;

/// Run the example on a temp `.loon` file and parse its stdout as JSON.
fn run_example(source: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join("vcad-loon2vcad-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let input = dir.join("part.loon");
    std::fs::write(&input, source).expect("write source");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(cargo)
        .args(["run", "-q", "--example", "loon2vcad", "--"])
        .arg(&input)
        .current_dir(manifest)
        .output()
        .expect("spawn loon2vcad");
    assert!(
        out.status.success(),
        "loon2vcad failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is not valid .vcad JSON")
}

#[test]
fn example_round_trips_a_small_assembly() {
    let doc = run_example(SRC);

    let len = |key: &str| doc.get(key).and_then(|v| v.as_array()).map_or(0, Vec::len);
    // partDefs is a map keyed by part id; instances and joints are arrays.
    let part_defs = doc
        .get("partDefs")
        .and_then(|v| v.as_object())
        .map_or(0, serde_json::Map::len);
    assert_eq!(part_defs, 1, "partDefs, doc: {doc}");
    assert_eq!(len("instances"), 2);
    assert_eq!(len("joints"), 1);

    // `.vcad` `version` must serialize as a string, not a number.
    assert!(
        doc.get("version").is_some_and(serde_json::Value::is_string),
        "version must be a string, got {:?}",
        doc.get("version")
    );
}
