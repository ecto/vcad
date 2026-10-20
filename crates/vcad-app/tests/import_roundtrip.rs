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
            let looks = |d: &vcad_ir::Document| {
                let mut v: Vec<(String, Option<bool>)> = d
                    .roots
                    .iter()
                    .map(|r| (r.material.clone(), r.visible.filter(|v| !v)))
                    .collect();
                v.sort();
                v
            };
            assert_eq!(
                looks(&back),
                looks(&original),
                "{name}: root materials/visibility"
            );
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

fn part_ids(api: &DocumentApi) -> Vec<String> {
    let parts = serde_json::to_value(&api.get_state().parts).unwrap();
    parts
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

/// Stable ids of the parts that reach the scene (`partIds` values).
fn scene_part_ids(api: &DocumentApi) -> Vec<String> {
    api.get_state()
        .document
        .part_ids
        .unwrap_or_default()
        .into_values()
        .collect()
}

/// MCP sessions persist the materialized IR and rebuild an engine from it on
/// hydrate, so the stable part ids it hands out must survive that rebuild:
/// same IR → same ids, save → load → same ids, and — because the
/// materializer stamps `partIds` and import honors them — materialized IR
/// re-imported → same ids, even though its node shape differs from the source.
#[test]
fn stable_ids_survive_rebuild_from_ir() {
    let mut checked = 0;
    for path in fixtures() {
        let text = std::fs::read_to_string(&path).unwrap();
        if serde_json::from_str::<vcad_ir::Document>(&text).is_err() {
            continue;
        }
        let name = path.display();
        let mut a = DocumentApi::new(ReplicaId(1));
        a.import_ir(&text);
        let mut b = DocumentApi::new(ReplicaId(7));
        b.import_ir(&text);
        assert_eq!(part_ids(&a), part_ids(&b), "{name}: fresh rebuild");

        let c = DocumentApi::load(&a.save(), ReplicaId(1)).unwrap();
        assert_eq!(part_ids(&a), part_ids(&c), "{name}: save/load");

        let materialized = serde_json::to_string(&a.get_state().document).unwrap();
        let mut d = DocumentApi::new(ReplicaId(1));
        d.import_ir(&materialized);
        let (mut want, mut got) = (scene_part_ids(&a), scene_part_ids(&d));
        want.sort();
        got.sort();
        assert_eq!(got, want, "{name}: re-import");
        checked += 1;
    }
    assert!(checked > 0);
}
