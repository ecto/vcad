//! Every example document in `examples/` opens the way the native app opens
//! it: any `.vcad` format (JSON, loon, VCode) loaded through
//! `vcad_ffi::load_document_file` (mesh paths resolved against the file's
//! directory), opened in the document engine, then evaluated with the app's
//! kernel-direct evaluator, without a panic and with non-empty geometry.
//!
//! New examples are picked up automatically; a file that can't meet the bar
//! fails here instead of in front of a user on the Open Example menu.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use vcad_app::DocumentApi;
use vcad_crdt::ReplicaId;
use vcad_eval::{evaluate_document_with_sheet_metal, EvalOptions};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("vcad" | "loon")
        ) {
            out.push(path);
        }
    }
}

/// Open + evaluate one example; `Ok(triangles)` or a reason.
fn open(path: &Path) -> Result<usize, String> {
    // The app's Open: any .vcad format (JSON, loon, VCode) to IR.
    let doc = vcad_ffi::load_document_file(path)?;
    // The editor opens documents in the engine; a refusal there means a
    // view-only document, which an example must not be. Electronics
    // documents are edited outside the engine by design.
    let electronics = doc.schematic.is_some() || doc.pcb.is_some();
    if !electronics {
        let json = serde_json::to_string(&doc).map_err(|e| format!("serialize: {e}"))?;
        DocumentApi::from_ir(&json, ReplicaId(1)).map_err(|e| format!("engine open: {e}"))?;
    }
    let scene = evaluate_document_with_sheet_metal(&doc, &EvalOptions::default())
        .map_err(|e| format!("evaluate: {e}"))?;
    let parts: usize = scene.parts.iter().map(|p| p.mesh.indices.len() / 3).sum();
    let instances: usize = scene
        .instances
        .iter()
        .flatten()
        .map(|i| i.mesh.indices.len() / 3)
        .sum();
    let triangles = parts + instances;
    if triangles == 0 {
        return Err(format!(
            "no geometry ({} root failure(s): {:?})",
            scene.failures.len(),
            scene.failures.iter().take(3).collect::<Vec<_>>()
        ));
    }
    Ok(triangles)
}

#[test]
fn every_example_opens_with_geometry() {
    let mut files = Vec::new();
    collect(&repo().join("examples"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "no examples found");
    let mut failures = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(repo())
            .unwrap_or(path)
            .display()
            .to_string();
        let started = std::time::Instant::now();
        match catch_unwind(AssertUnwindSafe(|| open(path))) {
            Ok(Ok(tris)) => eprintln!("ok   {rel}: {tris} triangles in {:?}", started.elapsed()),
            Ok(Err(e)) => failures.push(format!("{rel}: {e}")),
            Err(_) => failures.push(format!("{rel}: panicked")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} examples failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
