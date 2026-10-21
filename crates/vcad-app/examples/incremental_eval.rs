//! Cold vs incremental evaluation of one edit on a real document.
//!
//! `cargo run --release -p vcad-app --example incremental_eval -- doc.vcad [part-index]`
//!
//! Loads the document into the engine, evaluates it once (cold, priming an
//! `IncrementalCache`), moves one part, then evaluates the edited document
//! both cold and incrementally, checks the two agree, and prints timings and
//! reuse counts.

use std::time::Instant;

use vcad_app::DocumentApi;
use vcad_crdt::ReplicaId;
use vcad_eval::{evaluate::evaluate_document_incremental, incremental::IncrementalCache};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: incremental_eval <doc.vcad> [part-index]");
    let ir = std::fs::read_to_string(&path).expect("read document");
    let mut api = DocumentApi::from_ir(&ir, ReplicaId(1)).expect("load IR");
    let parts = api.get_state().parts;
    let pick = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(parts.len() / 2);
    let id = parts[pick].id().to_string();
    let opts = vcad_eval::EvalOptions {
        skip_clash_detection: true,
        ..Default::default()
    };
    let mut inc = IncrementalCache::new();

    let doc = api.get_state().document;
    let t = Instant::now();
    evaluate_document_incremental(&doc, &opts, &mut inc).expect("eval");
    println!(
        "parts {} | initial (cold, priming): {:?}",
        parts.len(),
        t.elapsed()
    );

    for (name, cmd) in [
        (
            "move one part",
            format!(r#"{{"type":"set_translation","id":"{id}","offset":[3,0,0]}}"#),
        ),
        (
            "rename (no geometry)",
            format!(r#"{{"type":"rename","id":"{id}","name":"Edited"}}"#),
        ),
    ] {
        api.apply_json(&cmd).expect("command");
        let doc = api.get_state().document;
        let t = Instant::now();
        let cold = vcad_eval::evaluate_document(&doc, &opts).expect("cold");
        let t_cold = t.elapsed();
        let t = Instant::now();
        let warm = evaluate_document_incremental(&doc, &opts, &mut inc).expect("incremental");
        let t_warm = t.elapsed();
        let same = serde_json::to_value(cold.parts.iter().map(|p| &p.mesh).collect::<Vec<_>>())
            .unwrap()
            == serde_json::to_value(warm.parts.iter().map(|p| &p.mesh).collect::<Vec<_>>())
                .unwrap()
            && serde_json::to_value(&cold.part_defs).unwrap()
                == serde_json::to_value(&warm.part_defs).unwrap();
        let s = inc.stats();
        println!(
            "{name}: cold {t_cold:?} | incremental {t_warm:?} ({:.1}x) | reused {} solids, evaluated {}, reused {} meshes | equal: {same}",
            t_cold.as_secs_f64() / t_warm.as_secs_f64().max(1e-9),
            s.reused_solids,
            s.evaluated_solids,
            s.reused_meshes,
        );
        assert!(same, "incremental result differs from cold");
    }
}
