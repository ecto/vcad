//! IR import must be lossless: every `.vcad` fixture in the repo imported
//! into the CRDT engine and materialized back keeps its geometry (per-root
//! volume) and every document-level field the CRDT doesn't model.

use std::path::{Path, PathBuf};

use vcad_app::migrate::DOC_EXTRA_FIELDS;
use vcad_app::DocumentApi;
use vcad_crdt::ReplicaId;

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let mut stack = vec![root.join("examples")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "vcad") {
                out.push(p);
            }
        }
    }
    for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "vcad") {
            out.push(e.path());
        }
    }
    out.sort();
    out
}

fn volumes(doc: &vcad_ir::Document) -> Vec<f64> {
    let mut v: Vec<f64> = vcad_eval::evaluate::evaluate_root_solids(doc)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.solid.map(|s| s.volume()))
        .collect();
    v.sort_by(f64::total_cmp);
    v
}

#[test]
fn every_fixture_round_trips() {
    let mut checked = 0;
    for path in fixtures() {
        let text = std::fs::read_to_string(&path).unwrap();
        let Ok(original) = serde_json::from_str::<vcad_ir::Document>(&text) else {
            continue; // CRDT-format or non-IR file
        };
        let mut api = DocumentApi::new(ReplicaId(1));
        api.import_ir(&text);
        let back = api.get_state().document;
        let name = path.display();

        if let Some(defs) = &original.part_defs {
            // Assemblies: geometry roots become part definitions, placed by
            // instances (the engine's assembly model), so compare those.
            let n = |o: &Option<Vec<vcad_ir::Instance>>| o.as_ref().map_or(0, Vec::len);
            assert_eq!(
                back.part_defs.as_ref().map_or(0, |d| d.len()),
                defs.len(),
                "{name}: part defs"
            );
            assert_eq!(
                n(&back.instances),
                n(&original.instances),
                "{name}: instances"
            );
            let mut inertials: Vec<String> = defs
                .values()
                .map(|d| serde_json::to_string(&d.inertial).unwrap())
                .collect();
            let mut got: Vec<String> = back
                .part_defs
                .iter()
                .flatten()
                .map(|(_, d)| serde_json::to_string(&d.inertial).unwrap())
                .collect();
            inertials.sort();
            got.sort();
            assert_eq!(got, inertials, "{name}: part def inertials");
            let colliders = |d: &std::collections::HashMap<String, vcad_ir::PartDef>| -> usize {
                d.values()
                    .map(|p| p.colliders.as_ref().map_or(0, Vec::len))
                    .sum()
            };
            assert_eq!(
                colliders(back.part_defs.as_ref().unwrap()),
                colliders(defs),
                "{name}: colliders"
            );
            assert_eq!(
                back.joints.as_ref().map_or(0, Vec::len),
                original.joints.as_ref().map_or(0, Vec::len),
                "{name}: joints"
            );
        } else {
            assert_eq!(back.roots.len(), original.roots.len(), "{name}: root count");
            let (got, want) = (volumes(&back), volumes(&original));
            assert_eq!(got.len(), want.len(), "{name}: solid count");
            for (g, w) in got.iter().zip(&want) {
                // The transform chain is rebuilt on import, so allow float
                // reassociation noise, not geometry change.
                assert!(
                    (g - w).abs() <= 1e-6 * w.abs().max(1.0),
                    "{name}: volume {g} vs {w}"
                );
            }
        }

        let (a, b) = (
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&back).unwrap(),
        );
        for field in DOC_EXTRA_FIELDS {
            assert_eq!(
                a.get(*field),
                b.get(*field),
                "{name}: field `{field}` lost in import"
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "no IR fixtures found");
    eprintln!("round-tripped {checked} fixtures");
}

#[test]
fn imported_subgraph_parts_can_be_moved() {
    let ir = serde_json::json!({
        "version": "0.1",
        "nodes": {
            "1": { "id": 1, "name": "Bracket mesh", "op": { "type": "mesh_import", "path": "bracket.stl" } }
        },
        "materials": {}, "part_materials": {},
        "roots": [{ "root": 1, "material": "default" }]
    });
    let mut api = DocumentApi::new(ReplicaId(1));
    let state = api
        .apply(vcad_app::DocCommand::ImportIr { document: ir })
        .expect("valid IR imports");
    let part = serde_json::to_value(&state.parts[0]).unwrap();
    assert_eq!(part["kind"], "ir-subgraph");
    assert_eq!(part["name"], "Bracket mesh");
    let id = part["id"].as_str().unwrap().to_string();
    api.apply(vcad_app::DocCommand::SetTranslation {
        id,
        offset: [5.0, 0.0, 0.0],
    })
    .unwrap();
    let doc = api.get_state().document;
    let root = doc.roots[0].root;
    assert!(
        matches!(&doc.nodes[&root].op, vcad_ir::CsgOp::Translate { offset, .. } if offset.x == 5.0),
        "{:?}",
        doc.nodes[&root].op
    );
}
