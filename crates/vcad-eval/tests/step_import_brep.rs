//! A `step_import` node must evaluate to real B-rep — not a tessellation.
//!
//! This is the property the MCP import path depends on: a document that
//! references a STEP file keeps analytic faces, so the imported body can go on
//! to boolean, fillet, and export back out as STEP. The registry route is the
//! one that has to work on wasm, where there is no filesystem.

use std::collections::HashMap;

use vcad_eval::{evaluate_root_solids, step_sources};
use vcad_ir::{CsgOp, Document, Node, SceneEntry};

/// STEP bytes for two distinct bodies, so solid_index selection is testable.
fn two_body_step() -> Vec<u8> {
    let a = vcad_kernel::Solid::cube(10.0, 10.0, 10.0);
    let b = vcad_kernel::Solid::cube(2.0, 2.0, 2.0);
    vcad_kernel::Solid::solids_to_step_buffer(&[(&a, "big"), (&b, "small")])
        .expect("primitives export to STEP")
}

fn doc_with_step_nodes(path: &str, indices: &[Option<u32>]) -> Document {
    let mut nodes = HashMap::new();
    let mut roots = Vec::new();
    for (i, solid_index) in indices.iter().enumerate() {
        let id = i as u64 + 1;
        nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("body_{}", id)),
                op: CsgOp::StepImport {
                    path: path.to_string(),
                    solid_index: *solid_index,
                },
            },
        );
        roots.push(SceneEntry {
            root: id,
            material: "steel".to_string(),
            visible: None,
        });
    }
    Document {
        nodes,
        roots,
        ..Default::default()
    }
}

#[test]
fn registered_step_evaluates_to_brep_and_re_exports() {
    let path = "test://step_import_brep/two_bodies.step";
    step_sources::register(path, two_body_step());

    let doc = doc_with_step_nodes(path, &[None, Some(1)]);
    let roots = evaluate_root_solids(&doc).expect("document evaluates");
    assert_eq!(roots.len(), 2, "one root per step_import node");

    for root in &roots {
        let solid = root.solid.as_ref().expect("root has geometry");
        assert!(
            solid.as_brep().is_some(),
            "import must stay B-rep, not fall back to a mesh"
        );
        assert!(
            solid.can_export_step(),
            "a B-rep import must round-trip back out to STEP"
        );
    }

    // `solid_index` selects the body: without it a multi-body file would
    // collapse to the first solid repeated.
    let v0 = roots[0].solid.as_ref().unwrap().volume();
    let v1 = roots[1].solid.as_ref().unwrap().volume();
    assert!(
        (v0 - 1000.0).abs() < 1.0,
        "solid 0 is the 10mm cube, got {v0}"
    );
    assert!((v1 - 8.0).abs() < 0.1, "solid 1 is the 2mm cube, got {v1}");

    step_sources::unregister(path);
}

#[test]
fn out_of_range_solid_index_errors_rather_than_vanishing() {
    let path = "test://step_import_brep/range.step";
    step_sources::register(path, two_body_step());

    let doc = doc_with_step_nodes(path, &[Some(7)]);
    let err = evaluate_root_solids(&doc).expect_err("index 7 does not exist");
    let msg = err.to_string();
    assert!(msg.contains("out of range"), "unhelpful error: {msg}");
    assert!(msg.contains(path), "error must name the path: {msg}");

    step_sources::unregister(path);
}

#[test]
fn missing_source_errors_rather_than_evaluating_empty() {
    // Neither registered nor a readable file. A silent empty document is the
    // failure mode this error exists to prevent.
    let doc = doc_with_step_nodes("/definitely/not/here.step", &[None]);
    let err = evaluate_root_solids(&doc).expect_err("missing STEP must fail loudly");
    assert!(
        err.to_string().contains("/definitely/not/here.step"),
        "error must name the missing path: {err}"
    );
}
