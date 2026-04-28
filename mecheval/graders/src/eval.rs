//! Shared `.vcad` → kernel evaluation pipeline used by every grader check.
//!
//! Every kernel-backed check needs the same prefix: parse the `.vcad`,
//! evaluate it, collect the resulting [`Solid`]s. Doing that once per check
//! would be wasteful and would make the per-check dispatch in `grader.rs`
//! re-do the parse + eval N times. Instead the dispatch builds an
//! [`EvalSnapshot`] up front and hands it to each check.

use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::file_io::parse_vcad_file;
use vcad_kernel::Solid;

/// Result of loading + evaluating a candidate `.vcad`.
pub struct EvalSnapshot {
    /// Solids successfully produced by visible roots.
    pub solids: Vec<Solid>,
    /// Number of visible roots in the document (whether they evaluated or not).
    pub root_count: usize,
    /// Per-root failures, formatted as `"root[i]: <msg>"`.
    pub root_failures: Vec<String>,
    /// Top-level error, if the document failed to parse or eval at all.
    pub fatal: Option<String>,
}

impl EvalSnapshot {
    /// True iff the document parsed, evaluated, and produced at least one
    /// non-empty solid with positive volume.
    pub fn has_any_valid_solid(&self) -> bool {
        if self.fatal.is_some() {
            return false;
        }
        self.solids.iter().any(|s| {
            // Catch panics inside volume/num_triangles for defense in depth —
            // a malformed candidate shouldn't crash the grader.
            catch_unwind(AssertUnwindSafe(|| {
                s.num_triangles() > 0 && s.volume() > 0.0
            }))
            .unwrap_or(false)
        })
    }

    /// Aggregate bounding box across every evaluated solid: per-axis
    /// minimum of mins and maximum of maxes. Returns `None` if no solid
    /// produced a valid AABB.
    pub fn aggregate_bbox(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut acc: Option<([f64; 3], [f64; 3])> = None;
        for s in &self.solids {
            let bb = catch_unwind(AssertUnwindSafe(|| s.bounding_box()));
            let (lo, hi) = match bb {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Skip degenerate (kernel returns sentinel f64::MAX/MIN for
            // empty solids). A real solid will have lo[i] <= hi[i].
            if (0..3).any(|i| lo[i] > hi[i]) {
                continue;
            }
            acc = Some(match acc {
                None => (lo, hi),
                Some((al, ah)) => (
                    [al[0].min(lo[0]), al[1].min(lo[1]), al[2].min(lo[2])],
                    [ah[0].max(hi[0]), ah[1].max(hi[1]), ah[2].max(hi[2])],
                ),
            });
        }
        acc
    }

    /// Diagnostic JSON for use in a check's `details` field.
    pub fn summary_json(&self) -> serde_json::Value {
        json!({
            "fatal": self.fatal,
            "root_count": self.root_count,
            "solids_produced": self.solids.len(),
            "root_failures": self.root_failures,
        })
    }
}

/// Load and evaluate a `.vcad` JSON string. Never panics — kernel panics
/// inside per-root evaluation are already trapped by `vcad-eval`'s
/// `catch_unwind`; here we just guard the top-level parse and call.
pub fn evaluate_vcad(raw_vcad: &str) -> EvalSnapshot {
    // Parse via the shared loader (handles both legacy and current formats,
    // detects bare-IR vs. wrapped, etc.).
    let parsed = match parse_vcad_file(raw_vcad) {
        Ok(p) => p,
        Err(e) => {
            return EvalSnapshot {
                solids: Vec::new(),
                root_count: 0,
                root_failures: Vec::new(),
                fatal: Some(format!("parse: {}", e)),
            };
        }
    };

    let doc = parsed.document;
    let root_count = doc.roots.iter().filter(|r| r.visible != Some(false)).count();

    let scene = match catch_unwind(AssertUnwindSafe(|| {
        evaluate_document(&doc, &EvalOptions::default())
    })) {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return EvalSnapshot {
                solids: Vec::new(),
                root_count,
                root_failures: Vec::new(),
                fatal: Some(format!("eval: {}", e)),
            };
        }
        Err(_) => {
            return EvalSnapshot {
                solids: Vec::new(),
                root_count,
                root_failures: Vec::new(),
                fatal: Some("eval panicked".into()),
            };
        }
    };

    let mut solids = Vec::new();
    for part in scene.parts.iter() {
        if let Some(s) = part.solid.clone() {
            solids.push(s);
        }
    }

    let root_failures: Vec<String> = scene
        .failures
        .iter()
        .map(|f| format!("{}: {}", f.scope, f.error))
        .collect();

    EvalSnapshot {
        solids,
        root_count,
        root_failures,
        fatal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written minimal .vcad with a single 10mm cube root.
    fn cube_vcad(size: f64) -> String {
        format!(
            r#"{{
                "version": "0.1",
                "nodes": {{
                    "1": {{
                        "id": 1,
                        "name": "cube",
                        "op": {{ "type": "Cube", "size": {{ "x": {s}, "y": {s}, "z": {s} }} }}
                    }}
                }},
                "materials": {{}},
                "part_materials": {{}},
                "roots": [{{ "root": 1, "material": "default" }}]
            }}"#,
            s = size
        )
    }

    #[test]
    fn evaluates_a_simple_cube() {
        let snap = evaluate_vcad(&cube_vcad(10.0));
        assert!(snap.fatal.is_none(), "fatal: {:?}", snap.fatal);
        assert_eq!(snap.solids.len(), 1);
        assert!(snap.has_any_valid_solid());
    }

    #[test]
    fn handles_empty_document() {
        let snap = evaluate_vcad(
            r#"{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}"#,
        );
        // Empty doc parses fine but has no solids.
        assert!(snap.fatal.is_none());
        assert_eq!(snap.solids.len(), 0);
        assert!(!snap.has_any_valid_solid());
    }

    #[test]
    fn handles_garbage_input() {
        let snap = evaluate_vcad("this is not json");
        assert!(snap.fatal.is_some());
        assert!(!snap.has_any_valid_solid());
    }
}
