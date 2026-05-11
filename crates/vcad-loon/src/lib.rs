//! Bridge between loon programs and vcad-ir Documents.
//!
//! Evaluates `.vcad` loon source files and converts the resulting
//! `Value::Adt` tree into a `vcad_ir::Document`.

use std::path::Path;

use loon_lang::interp::Value;
use vcad_ir::Document;

mod convert;
pub use convert::value_to_document;

/// The bundled vcad loon library source.
pub const VCAD_LIB_SOURCE: &str = include_str!("../../../lib/src/lib.loon");

/// Evaluate a `.vcad` loon source string and produce a Document.
///
/// The `base_dir` is used for module resolution (where `[use ...]` looks for files).
/// The vcad library is automatically available — the source is evaluated with
/// the vcad type definitions and constructors pre-loaded.
pub fn eval_vcad(source: &str, base_dir: Option<&Path>) -> Result<Document, String> {
    let result = eval_vcad_to_value(source, base_dir)?;
    value_to_document(&result)
}

/// Evaluate a `.vcad` loon source string and return the raw loon Value.
///
/// Useful for debugging or inspecting the AST before conversion.
pub fn eval_vcad_to_value(source: &str, base_dir: Option<&Path>) -> Result<Value, String> {
    // Prepend the vcad library so types and constructors are available
    let full_source = format!("{VCAD_LIB_SOURCE}\n\n{source}");

    let exprs = loon_lang::parser::parse(&full_source).map_err(|e| e.message.clone())?;

    loon_lang::interp::eval_program_with_base_dir(&exprs, base_dir).map_err(|e| format!("{e}"))
}

/// Evaluate a `.vcad` file from disk and produce a Document.
pub fn eval_vcad_file(path: &Path) -> Result<Document, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let base_dir = path.parent();
    eval_vcad(source.trim(), base_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_simple_cube() {
        let doc = eval_vcad("[cube 10.0 20.0 30.0]", None).unwrap();
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.roots.len(), 1);
    }

    #[test]
    fn eval_scene_entry() {
        let doc = eval_vcad("[root [cube 10.0 10.0 10.0] \"steel\"]", None).unwrap();
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.roots[0].material, "steel");
    }

    #[test]
    fn eval_pipe_chain() {
        let source = r#"
[pipe [cube 50.0 30.0 5.0]
  [difference [cylinder 3.0 10.0]]
  [fillet 1.0]
  [translate 0.0 0.0 10.0]]
"#;
        let doc = eval_vcad(source, None).unwrap();
        // cube + cylinder + difference + fillet + translate = 5 nodes
        assert_eq!(doc.nodes.len(), 5);
    }

    #[test]
    fn eval_with_let_bindings() {
        let source = r#"
[let body [cube 50.0 30.0 5.0]]
[let hole [cylinder 3.0 15.0]]
[let part [difference hole body]]
[root part "aluminum"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.roots[0].material, "aluminum");
    }

    #[test]
    fn eval_sketch_extrude() {
        let source = r#"
[let sk [sketch
  0.0 0.0 0.0
  1.0 0.0 0.0
  0.0 1.0 0.0
  #[[line 0.0 0.0 10.0 0.0]
    [line 10.0 0.0 10.0 5.0]
    [line 10.0 5.0 0.0 5.0]
    [line 0.0 5.0 0.0 0.0]]]]
[extrude 0.0 0.0 20.0 sk]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.nodes.len(), 2); // sketch + extrude
    }

    #[test]
    fn eval_vec_of_entries() {
        let source = r#"
#[[root [cube 10.0 10.0 10.0] "steel"]
  [root [sphere 5.0] "glass"]]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
    }

    #[test]
    fn eval_sweep_line() {
        let source = r#"
[let sk [sketch
  0.0 0.0 0.0
  1.0 0.0 0.0
  0.0 1.0 0.0
  #[[line 0.0 0.0 5.0 0.0]
    [line 5.0 0.0 5.0 3.0]
    [line 5.0 3.0 0.0 3.0]
    [line 0.0 3.0 0.0 0.0]]]]
[sweep-line 0.0 0.0 0.0 0.0 0.0 50.0 sk]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.nodes.len(), 2); // sketch + sweep
    }

    #[test]
    fn eval_sweep_helix() {
        let source = r#"
[let sk [sketch
  0.0 0.0 0.0
  1.0 0.0 0.0
  0.0 1.0 0.0
  #[[line 0.0 0.0 2.0 0.0]
    [line 2.0 0.0 2.0 2.0]
    [line 2.0 2.0 0.0 2.0]
    [line 0.0 2.0 0.0 0.0]]]]
[sweep-helix 10.0 5.0 20.0 4.0 sk]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.nodes.len(), 2); // sketch + sweep
    }

    #[test]
    fn eval_loft() {
        let source = r#"
[let sk1 [sketch
  0.0 0.0 0.0  1.0 0.0 0.0  0.0 0.0 1.0
  #[[line 0.0 0.0 10.0 0.0]
    [line 10.0 0.0 10.0 10.0]
    [line 10.0 10.0 0.0 10.0]
    [line 0.0 10.0 0.0 0.0]]]]
[let sk2 [sketch
  0.0 20.0 0.0  1.0 0.0 0.0  0.0 0.0 1.0
  #[[line 2.0 2.0 8.0 2.0]
    [line 8.0 2.0 8.0 8.0]
    [line 8.0 8.0 2.0 8.0]
    [line 2.0 8.0 2.0 2.0]]]]
[loft #[sk1 sk2]]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.nodes.len(), 3); // 2 sketches + loft
    }

    #[test]
    fn eval_assembly() {
        let source = r#"
[assembly
  #[[part "base" [cylinder 40.0 30.0] "steel"]
    [part "arm1" [cube 80.0 20.0 20.0] "aluminum"]]
  #[[instance "base-inst" "base" 0.0 0.0 0.0]
    [instance "arm1-inst" "arm1" 0.0 0.0 30.0]]
  #[[revolute-joint "shoulder" 0.0 1.0 0.0 -90.0 90.0
      "base-inst" 0.0 0.0 25.0
      "arm1-inst" 0.0 0.0 0.0]]
  "base-inst"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert!(doc.part_defs.is_some());
        assert_eq!(doc.part_defs.as_ref().unwrap().len(), 2);
        assert!(doc.instances.is_some());
        assert_eq!(doc.instances.as_ref().unwrap().len(), 2);
        assert!(doc.joints.is_some());
        assert_eq!(doc.joints.as_ref().unwrap().len(), 1);
        assert_eq!(doc.ground_instance_id, Some("base-inst".to_string()));
    }

    #[test]
    fn eval_complex_part() {
        let source = r#"
[let base [cube 100.0 60.0 10.0]]
[let hole1 [translate 20.0 15.0 0.0 [cylinder 5.0 15.0]]]
[let hole2 [translate 80.0 15.0 0.0 [cylinder 5.0 15.0]]]
[let hole3 [translate 20.0 45.0 0.0 [cylinder 5.0 15.0]]]
[let hole4 [translate 80.0 45.0 0.0 [cylinder 5.0 15.0]]]
[let drilled [pipe base
  [difference hole1]
  [difference hole2]
  [difference hole3]
  [difference hole4]]]
[fillet 2.0 drilled]
"#;
        let doc = eval_vcad(source, None).unwrap();
        // Many nodes from the nested operations
        assert!(doc.nodes.len() > 5);
    }
}
