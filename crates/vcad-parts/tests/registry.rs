//! Registry integration tests: every part builds with defaults, manifest
//! serializes, and unknown paths error cleanly.

use std::collections::HashMap;
use vcad_parts::{all_parts, build_part, find_part, manifest_json};

#[test]
fn every_part_builds_with_default_params() {
    for entry in all_parts() {
        let params: HashMap<String, serde_json::Value> = entry
            .meta
            .params
            .iter()
            .map(|p| (p.name().to_string(), p.default_json()))
            .collect();
        let doc = (entry.build)(&vcad_parts::Params::new(entry.meta.params, &params))
            .unwrap_or_else(|e| panic!("part {} failed to build: {e}", entry.meta.id));
        assert!(
            !doc.nodes.is_empty(),
            "part {} produced an empty document",
            entry.meta.id
        );
        assert!(
            !doc.roots.is_empty(),
            "part {} produced no scene root",
            entry.meta.id
        );
    }
}

#[test]
fn build_part_dispatches_by_path() {
    let mut params = HashMap::new();
    params.insert("size".to_string(), serde_json::json!("M6"));
    params.insert("length".to_string(), serde_json::json!(20.0));
    let doc = build_part("std:fastener.bolt.socket-head", &params).unwrap();
    assert!(doc.nodes.len() >= 4);
}

#[test]
fn build_part_accepts_path_without_std_prefix() {
    let params = HashMap::new();
    let doc = build_part("bearing.608", &params).unwrap();
    assert!(!doc.nodes.is_empty());
}

#[test]
fn unknown_path_errors() {
    let params = HashMap::new();
    let err = build_part("std:does.not.exist", &params).unwrap_err();
    assert!(err.contains("unknown part"));
}

#[test]
fn manifest_json_is_valid_and_contains_all_parts() {
    let json = manifest_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let arr = parsed.as_array().expect("manifest is a JSON array");
    assert_eq!(arr.len(), all_parts().len());
    for entry in all_parts() {
        let found = arr
            .iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(entry.meta.id));
        assert!(found, "manifest is missing {}", entry.meta.id);
    }
}

#[test]
fn find_part_is_case_sensitive_and_stable() {
    assert!(find_part("std:fastener.washer.flat").is_some());
    assert!(find_part("fastener.washer.flat").is_some());
    assert!(find_part("std:fastener.WASHER.flat").is_none());
}

#[test]
fn search_tokens_include_xref_numbers() {
    for entry in all_parts() {
        let manifest = entry.manifest_entry();
        for xref in &manifest.xrefs {
            if let Some(m) = &xref.mcmaster {
                assert!(
                    manifest.search_tokens.iter().any(|t| t == m),
                    "mcmaster number {m} missing from search tokens for {}",
                    entry.meta.id
                );
            }
        }
    }
}
