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
    // A loon program's value is its *last* expression, so a source with
    // several top-level value forms (e.g. two `[root ...]` statements) would
    // silently keep only the final one. Rewrite such programs so every
    // top-level value expression is collected into the document.
    let user_source = collect_top_level_values(source);

    // Prepend the vcad library so types and constructors are available
    let full_source = format!("{VCAD_LIB_SOURCE}\n\n{user_source}");

    let exprs = loon_lang::parser::parse(&full_source).map_err(|e| e.message.clone())?;

    loon_lang::interp::eval_program_with_base_dir(&exprs, base_dir).map_err(|e| format!("{e}"))
}

/// Top-level statement heads — forms that bind, define, mutate, or print
/// rather than produce a scene value. Everything else at top level is a
/// value expression.
const STATEMENT_HEADS: &[&str] = &[
    "let", "use", "type", "fn", "macro", "mod", "import", "def", "defn", "impl", "set!", "mut",
    "inspect", "pub",
];

/// One top-level form in the source: its byte span and head symbol
/// (empty for bare atoms, strings, and vector literals).
struct TopForm {
    start: usize,
    end: usize,
    head: String,
}

/// Split source into top-level forms with a lightweight bracket scanner
/// (skips `;` line comments and string literals). All control decisions are
/// made on ASCII bytes; multibyte UTF-8 sequences are consumed opaquely, so
/// form spans always fall on char boundaries. Returns `None` for anything
/// the scanner doesn't model — unbalanced brackets, unterminated strings,
/// top-level `{`/`(`/backtick/`#`-non-vector forms, postfix `?` — and the
/// caller then leaves the source untouched (preserving last-expression
/// semantics and letting the real parser report any error).
fn split_top_level(source: &str) -> Option<Vec<TopForm>> {
    let bytes = source.as_bytes();
    let mut forms = Vec::new();
    let mut i = 0usize;
    // Consume an escape-aware string literal starting at bytes[at] == b'"';
    // returns the index just past the closing quote, or None if unterminated.
    let scan_string = |mut at: usize| -> Option<usize> {
        at += 1;
        while at < bytes.len() && bytes[at] != b'"' {
            if bytes[at] == b'\\' {
                at += 1;
            }
            at += 1;
        }
        if at >= bytes.len() {
            None
        } else {
            Some(at + 1)
        }
    };
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b';' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Top-level syntax the scanner doesn't model — bail.
        if c == b'{' || c == b'(' || c == b'`' {
            return None;
        }
        if c == b'#' && !(i + 1 < bytes.len() && bytes[i + 1] == b'[') {
            return None;
        }
        let start = i;
        if c == b'"' {
            // Top-level string literal — a value form with no head.
            i = scan_string(i)?;
            forms.push(TopForm {
                start,
                end: i,
                head: String::new(),
            });
        } else if c == b'[' || c == b'#' {
            if c == b'#' {
                i += 1;
            }
            let mut depth = 0i32;
            let mut closed = false;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => {
                        i = scan_string(i)? - 1;
                    }
                    b';' => {
                        while i < bytes.len() && bytes[i] != b'\n' {
                            i += 1;
                        }
                        continue;
                    }
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            closed = true;
                            break;
                        }
                        if depth < 0 {
                            return None;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if !closed {
                return None;
            }
            // Head symbol: strip at most one '#' and one '[' so a nested
            // list head (e.g. an IIFE `[[fn ...] arg]`) yields no symbol
            // and the form counts as a value expression.
            let inner = &source[start..i];
            let head = inner
                .strip_prefix('#')
                .unwrap_or(inner)
                .strip_prefix('[')
                .unwrap_or(inner)
                .trim_start()
                .split(|ch: char| ch.is_ascii_whitespace() || ch == '[' || ch == ']')
                .next()
                .unwrap_or("")
                .to_string();
            forms.push(TopForm {
                start,
                end: i,
                head,
            });
        } else {
            // Bare atom (e.g. a symbol referencing an earlier let binding).
            // Terminate only on ASCII structure bytes; multibyte chars are
            // atom content.
            while i < bytes.len() {
                let ch = bytes[i];
                if ch.is_ascii_whitespace() || ch == b'[' || ch == b';' || ch == b'"' {
                    break;
                }
                i += 1;
            }
            let atom = &source[start..i];
            // Postfix `?` binds to the preceding form — semantics the
            // splitter can't reproduce; bail.
            if atom == "?" {
                return None;
            }
            forms.push(TopForm {
                start,
                end: i,
                head: String::new(),
            });
        }
    }
    Some(forms)
}

/// Rewrite a program with several top-level value expressions so all of
/// them reach the document: each value form becomes a `[let __vcad_top_N …]`
/// binding (statements pass through in order) and a final `#[…]` vector
/// collects the bindings. Programs with zero or one value form are returned
/// unchanged.
fn collect_top_level_values(source: &str) -> String {
    let Some(forms) = split_top_level(source) else {
        return source.to_string();
    };
    let value_count = forms
        .iter()
        .filter(|f| !STATEMENT_HEADS.contains(&f.head.as_str()))
        .count();
    if value_count < 2 {
        return source.to_string();
    }
    // Pick a binding prefix that can't shadow (or be shadowed by) anything
    // in the user's source.
    let mut prefix = String::from("__vcad_top");
    while source.contains(&prefix) {
        prefix.push('_');
    }
    let mut out = String::with_capacity(source.len() + 32 * value_count);
    let mut names: Vec<String> = Vec::new();
    for form in &forms {
        let text = &source[form.start..form.end];
        if STATEMENT_HEADS.contains(&form.head.as_str()) {
            out.push_str(text);
            out.push('\n');
        } else {
            let name = format!("{prefix}_{}", names.len());
            out.push_str("[let ");
            out.push_str(&name);
            out.push(' ');
            out.push_str(text);
            out.push_str("]\n");
            names.push(name);
        }
    }
    out.push_str("#[");
    out.push_str(&names.join(" "));
    out.push_str("]\n");
    out
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
    fn eval_multiple_top_level_roots() {
        // Two bare [root ...] statements — both must survive, not just the last.
        let source = r#"
[root [cube 10.0 10.0 10.0] "steel"]
[root [sphere 5.0] "glass"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "steel");
        assert_eq!(doc.roots[1].material, "glass");
    }

    #[test]
    fn eval_multiple_roots_with_lets_and_comments() {
        let source = r#"
; base plate
[let plate [cube 100.0 60.0 5.0]]
[root plate "aluminum"]
[let post [cylinder 5.0 40.0]] ; a post
[root [translate 50.0 30.0 5.0 post] "steel"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "aluminum");
        assert_eq!(doc.roots[1].material, "steel");
    }

    #[test]
    fn eval_multiple_bare_solids() {
        // Bare solids (no [root]) at top level each become a default-material part.
        let source = r#"
[cube 10.0 10.0 10.0]
[sphere 5.0]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
    }

    #[test]
    fn eval_single_root_unchanged() {
        // The multi-root rewrite must not fire for single-value programs.
        let source = r#"
[let body [cube 50.0 30.0 5.0]]
[root body "steel"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.roots[0].material, "steel");
    }

    #[test]
    fn eval_multiple_roots_with_string_brackets() {
        // Brackets and semicolons inside strings must not confuse the splitter.
        let source = r#"
[root [cube 10.0 10.0 10.0] "st;[eel]"]
[root [sphere 5.0] "glass"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "st;[eel]");
    }

    #[test]
    fn eval_multiple_roots_with_multibyte_chars() {
        // Multibyte UTF-8 in comments and strings must not panic the
        // byte-level splitter (NBSP after the comment marker, é in a string).
        let source = "; gehäuse\u{a0}teil\n[root [cube 10.0 10.0 10.0] \"stähl\"]\n[root [sphere 5.0] \"glass\"]\n";
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "stähl");
    }

    #[test]
    fn eval_multiple_roots_with_top_level_string() {
        // A stray top-level string (docstring habit) contributes nothing
        // but must not break evaluation.
        let source = r#"
"two parts: a cube and a sphere"
[root [cube 10.0 10.0 10.0] "steel"]
[root [sphere 5.0] "glass"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
    }

    #[test]
    fn eval_vec_alongside_bare_root() {
        // A #[...] scene vector mixed with a bare root merges recursively.
        let source = r#"
#[[root [cube 10.0 10.0 10.0] "steel"]
  [root [cylinder 3.0 8.0] "brass"]]
[root [sphere 5.0] "glass"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 3);
    }

    #[test]
    fn eval_multiple_roots_no_name_collision() {
        // A user binding that contains the generated prefix must not be
        // shadowed or shadow the synthetic let-bindings.
        let source = r#"
[let __vcad_top_0 [cube 1.0 1.0 1.0]]
[root __vcad_top_0 "steel"]
[root [sphere 5.0] "glass"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "steel");
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
