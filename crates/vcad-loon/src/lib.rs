//! Bridge between loon programs and vcad-ir Documents.
//!
//! Evaluates `.vcad` loon source files and converts the resulting
//! `Value::Adt` tree into a `vcad_ir::Document`.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use loon_lang::interp::Value;
use loon_lang::module::{MapProvider, ModuleProvider};
use vcad_ir::Document;

mod convert;
pub mod fastener;
pub mod gear;
pub mod modules;
pub mod params;
pub mod recover;
mod rootnames;
pub use convert::{value_to_document, value_to_document_in};
pub use modules::{lib_dirs, LibPathProvider, LIB_PATH_VAR};

/// The bundled vcad loon library source.
pub const VCAD_LIB_SOURCE: &str = include_str!("../../../lib/src/lib.loon");

/// Evaluate a `.vcad` loon source string and produce a Document.
///
/// The `base_dir` is used for module resolution (where `[use ...]` looks for files).
/// The vcad library is automatically available — the source is evaluated with
/// the vcad type definitions and constructors pre-loaded.
pub fn eval_vcad(source: &str, base_dir: Option<&Path>) -> Result<Document, String> {
    Ok(eval_vcad_parametric(source, base_dir, None)?.0)
}

/// Evaluate a `.vcad` loon source string, preserving its declared parameters,
/// datums, and the bindings that connect them to geometry.
///
/// This is [`eval_vcad`] plus provenance. A program that declares parameters
/// with [`params`] forms produces a document whose `parameters`, `datums`, and
/// `bindings` are populated, so the value the author called `pitch_axis_x` is
/// still called that afterwards and `set_parameters` can drive it. A program
/// that declares nothing takes the plain path and pays nothing.
///
/// The returned warnings describe intent that could *not* be preserved — a
/// field whose dependence on a parameter is not affine, a parameter that
/// changes the model's topology. The document is correct either way; the
/// warnings explain why a given parameter will not move a given part.
///
/// Set `VCAD_LOON_NO_PARAM_RECOVERY=1` to declare parameters and datums
/// without recovering bindings. Recovery costs `2n + 2` extra evaluations of
/// the program for `n` independent parameters, which is worth skipping in a
/// hot loop that only needs the geometry.
pub fn eval_vcad_parametric(
    source: &str,
    base_dir: Option<&Path>,
    modules: Option<&HashMap<String, String>>,
) -> Result<(Document, Vec<String>), String> {
    let provider = modules::provider(modules.map(|m| MapProvider::new(m.clone())));
    let exprs = parse_program(source)?;
    let decls = params::scan(&exprs)?;

    let run = |env: &HashMap<String, f64>| -> Result<Document, String> {
        let rewritten = params::rewrite(&exprs, env)?;
        let value = run_program(&rewritten, base_dir, provider.clone())?;
        value_to_document_in(&value, base_dir)
    };

    if decls.is_empty() {
        // Nothing declared: run the program as parsed, with no rewrite pass
        // and no AST clone. This is the path every existing model takes.
        let value = run_program(&exprs, base_dir, provider)?;
        return Ok((value_to_document_in(&value, base_dir)?, Vec::new()));
    }

    let env0 = decls.env()?;
    let mut doc = run(&env0)?;

    let warnings = if std::env::var_os("VCAD_LOON_NO_PARAM_RECOVERY").is_some() {
        Vec::new()
    } else {
        let recovery = recover::recover(&decls, &doc, run);
        doc.bindings = recovery.bindings;
        recovery.warnings
    };
    doc.parameters = decls.params;
    doc.datums = decls.datums;
    Ok((doc, warnings))
}

/// Evaluate a `.vcad` loon source string whose `[use ...]` resolves against an
/// in-memory `name -> source` map instead of (or before) the filesystem.
///
/// This is the resolver for hosts with no filesystem — the browser/WASM
/// kernel and the MCP server, which reads the files itself and hands the
/// kernel a map. A name absent from the map still falls through to
/// `base_dir` when one is given, so the two mechanisms compose.
///
/// The vcad library is available inside imported modules exactly as it is in
/// the root program, and a module's own definitions are what it exports.
pub fn eval_vcad_with_modules(
    source: &str,
    base_dir: Option<&Path>,
    modules: &HashMap<String, String>,
) -> Result<Document, String> {
    Ok(eval_vcad_parametric(source, base_dir, Some(modules))?.0)
}

/// [`eval_vcad_with_modules`], returning the raw loon `Value`.
///
/// See [`eval_vcad_to_value`] for why that `Value` is not serializable.
pub fn eval_vcad_to_value_with_modules(
    source: &str,
    base_dir: Option<&Path>,
    modules: &HashMap<String, String>,
) -> Result<Value, String> {
    let provider = modules::provider(Some(MapProvider::new(modules.clone())));
    eval_with_provider(source, base_dir, provider)
}

/// Evaluate a `.vcad` loon source string and return the raw loon Value.
///
/// Useful for debugging or inspecting the AST before conversion.
///
/// Note the difference from [`eval_vcad`]: the loon `Value` returned here is
/// **not** `serde::Serialize`, so `serde_json::to_string(&value)` fails to
/// compile (E0277). To write a `.vcad` document, use [`eval_vcad`], which
/// returns a serializable [`Document`] — see `examples/loon2vcad.rs`.
pub fn eval_vcad_to_value(source: &str, base_dir: Option<&Path>) -> Result<Value, String> {
    eval_with_provider(source, base_dir, modules::provider(None))
}

/// Parse a `.vcad` program: rewrite multi-value sources and prepend the vcad
/// library so types and constructors are available.
fn parse_program(source: &str) -> Result<Vec<loon_lang::ast::Expr>, String> {
    // A loon program's value is its *last* expression, so a source with
    // several top-level value forms (e.g. two `[root ...]` statements) would
    // silently keep only the final one. Rewrite such programs so every
    // top-level value expression is collected into the document.
    let user_source = collect_top_level_values(source);
    let full_source = format!("{VCAD_LIB_SOURCE}\n\n{user_source}");
    let mut exprs = loon_lang::parser::parse(&full_source).map_err(|e| e.message.clone())?;
    // `[root wheel "steel"]` keeps the binding name on the root node.
    rootnames::capture(&mut exprs);
    Ok(exprs)
}

/// Evaluate a parsed program with the given module resolver.
fn run_program(
    exprs: &[loon_lang::ast::Expr],
    base_dir: Option<&Path>,
    provider: Option<Rc<dyn ModuleProvider>>,
) -> Result<Value, String> {
    // Imported modules get the vcad library too, so `[use ...]` code can
    // speak the same vocabulary as the root program.
    loon_lang::interp::eval_program_with_modules(exprs, base_dir, provider, Some(VCAD_LIB_SOURCE))
        .map_err(|e| format!("{e}"))
}

/// Shared body of the raw-`Value` entry points. Declaration forms are still
/// resolved and substituted — they are part of the language now, so a program
/// using them must run here too — but no provenance is recovered.
fn eval_with_provider(
    source: &str,
    base_dir: Option<&Path>,
    provider: Option<Rc<dyn ModuleProvider>>,
) -> Result<Value, String> {
    let exprs = parse_program(source)?;
    let decls = params::scan(&exprs)?;
    if decls.is_empty() {
        return run_program(&exprs, base_dir, provider);
    }
    let rewritten = params::rewrite(&exprs, &decls.env()?)?;
    run_program(&rewritten, base_dir, provider)
}

/// Top-level statement heads — forms that bind, define, mutate, or print
/// rather than produce a scene value. Everything else at top level is a
/// value expression.
pub(crate) const STATEMENT_HEADS: &[&str] = &[
    "let",
    "use",
    "type",
    "fn",
    "macro",
    "mod",
    "import",
    "def",
    "defn",
    "impl",
    "set!",
    "mut",
    "inspect",
    "pub",
    // Parametric declarations — see [`params::DECL_HEADS`].
    "defparam",
    "datum-plane",
    "datum-axis",
    "datum-point",
    "stack",
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
    let is_value = |f: &TopForm| !STATEMENT_HEADS.contains(&f.head.as_str());
    let value_count = forms.iter().filter(|f| is_value(f)).count();
    // One value form is fine only when it is also the *last* form — otherwise
    // the program's value is a trailing statement (a `let`, or a parametric
    // declaration) and the scene would be lost.
    if value_count == 0 || (value_count == 1 && forms.last().is_some_and(is_value)) {
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
    fn root_captures_binding_name() {
        let source = r#"
[let wheel [cylinder 10.0 5.0]]
[root wheel "steel"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(doc.roots.len(), 1);
        let node = &doc.nodes[&doc.roots[0].root];
        assert_eq!(node.name.as_deref(), Some("wheel"));
    }

    #[test]
    fn root_of_expression_has_no_name() {
        let doc = eval_vcad("[root [cylinder 10.0 5.0] \"steel\"]", None).unwrap();
        assert_eq!(doc.nodes[&doc.roots[0].root].name, None);
    }

    #[test]
    fn root_named_takes_explicit_name() {
        let source = r#"
[let wheel [cylinder 10.0 5.0]]
[root-named wheel "steel" "front-wheel"]
"#;
        let doc = eval_vcad(source, None).unwrap();
        assert_eq!(
            doc.nodes[&doc.roots[0].root].name.as_deref(),
            Some("front-wheel")
        );
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

    // ── module resolution ────────────────────────────────────────────
    //
    // The point of these is the *equivalence*: the same two-file project
    // must produce the same Document whether the modules come off disk or
    // out of an in-memory map.

    const BRACKET_MODULE: &str = r#"
[pub let plate [cube 40.0 20.0 4.0]]
[pub let post [cylinder 3.0 12.0]]
[let internal-scrap [sphere 1.0]]
"#;

    const MAIN_SOURCE: &str = r#"
[use bracket]
[root bracket.plate "aluminum"]
[root [translate 20.0 10.0 4.0 bracket.post] "steel"]
"#;

    fn modules_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn eval_modules_from_disk() {
        let dir = std::env::temp_dir().join(format!("vcad-loon-mod-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bracket.loon"), BRACKET_MODULE).unwrap();
        let doc = eval_vcad(MAIN_SOURCE, Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "aluminum");
        assert_eq!(doc.roots[1].material, "steel");
    }

    #[test]
    fn eval_modules_in_memory_matches_disk() {
        let dir = std::env::temp_dir().join(format!("vcad-loon-eq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bracket.loon"), BRACKET_MODULE).unwrap();
        let from_disk = eval_vcad(MAIN_SOURCE, Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let from_map = eval_vcad_with_modules(
            MAIN_SOURCE,
            None,
            &modules_map(&[("bracket", BRACKET_MODULE)]),
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(&from_disk).unwrap(),
            serde_json::to_value(&from_map).unwrap(),
            "in-memory and filesystem module resolution must agree"
        );
    }

    #[test]
    fn eval_modules_from_lib_path() {
        // `[use bracket]` with no bracket.loon beside the program resolves
        // through $VCAD_LOON_PATH; a file beside the program shadows it.
        let lib = std::env::temp_dir().join(format!("vcad-loon-libpath-{}", std::process::id()));
        let proj = std::env::temp_dir().join(format!("vcad-loon-libproj-{}", std::process::id()));
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(lib.join("bracket.loon"), BRACKET_MODULE).unwrap();
        let prev = std::env::var_os(LIB_PATH_VAR);
        std::env::set_var(LIB_PATH_VAR, &lib);
        let from_lib = eval_vcad(MAIN_SOURCE, Some(&proj));
        std::fs::write(
            proj.join("bracket.loon"),
            "[pub let plate [cube 1.0 1.0 1.0]]\n[pub let post [sphere 1.0]]",
        )
        .unwrap();
        let from_proj = eval_vcad(MAIN_SOURCE, Some(&proj));
        match prev {
            Some(v) => std::env::set_var(LIB_PATH_VAR, v),
            None => std::env::remove_var(LIB_PATH_VAR),
        }
        std::fs::remove_dir_all(&lib).ok();
        std::fs::remove_dir_all(&proj).ok();
        let from_lib = from_lib.unwrap();
        let from_proj = from_proj.unwrap();
        assert_eq!(from_lib.roots.len(), 2);
        assert_eq!(from_proj.roots.len(), 2);
        assert_ne!(
            serde_json::to_value(&from_lib).unwrap(),
            serde_json::to_value(&from_proj).unwrap(),
            "the module beside the program must shadow the lib-path one"
        );
    }

    #[test]
    fn eval_modules_in_memory_keyed_by_filename() {
        let doc = eval_vcad_with_modules(
            MAIN_SOURCE,
            None,
            &modules_map(&[("bracket.loon", BRACKET_MODULE)]),
        )
        .unwrap();
        assert_eq!(doc.roots.len(), 2);
    }

    #[test]
    fn eval_modules_aliased_and_selective() {
        let aliased = eval_vcad_with_modules(
            "[use bracket :as b]\n[root b.plate \"aluminum\"]",
            None,
            &modules_map(&[("bracket", BRACKET_MODULE)]),
        )
        .unwrap();
        assert_eq!(aliased.roots.len(), 1);

        let selective = eval_vcad_with_modules(
            "[use bracket [plate]]\n[root plate \"aluminum\"]",
            None,
            &modules_map(&[("bracket", BRACKET_MODULE)]),
        )
        .unwrap();
        assert_eq!(selective.roots.len(), 1);
    }

    #[test]
    fn eval_modules_pub_hides_non_pub_names() {
        // `internal-scrap` is not `pub`, so a selective import must fail.
        let err = eval_vcad_with_modules(
            "[use bracket [internal-scrap]]\n[root internal-scrap \"steel\"]",
            None,
            &modules_map(&[("bracket", BRACKET_MODULE)]),
        )
        .unwrap_err();
        assert!(
            err.contains("does not export"),
            "expected an export error, got: {err}"
        );
    }

    #[test]
    fn eval_modules_stdlib_visible_inside_module() {
        // The module body calls `cube`, which lives in the vcad library —
        // proof the host prelude reaches imported modules.
        let doc = eval_vcad_with_modules(
            "[use m]\n[root m.thing \"steel\"]",
            None,
            &modules_map(&[("m", "[pub let thing [cube 5.0 5.0 5.0]]")]),
        )
        .unwrap();
        assert_eq!(doc.roots.len(), 1);
    }

    #[test]
    fn eval_modules_nested_use() {
        // b imports a; the root imports b. Nested `[use]` inside an
        // in-memory module resolves against the same map.
        let doc = eval_vcad_with_modules(
            "[use b]\n[root b.stack \"steel\"]",
            None,
            &modules_map(&[
                ("a", "[pub let base [cube 10.0 10.0 2.0]]"),
                (
                    "b",
                    "[use a]\n[pub let stack [translate 0.0 0.0 2.0 a.base]]",
                ),
            ]),
        )
        .unwrap();
        assert_eq!(doc.roots.len(), 1);
    }

    #[test]
    fn eval_modules_circular_import_errors() {
        let err = eval_vcad_with_modules(
            "[use a]\n[root a.x \"steel\"]",
            None,
            &modules_map(&[
                ("a", "[use b]\n[pub let x [cube 1.0 1.0 1.0]]"),
                ("b", "[use a]\n[pub let y [cube 1.0 1.0 1.0]]"),
            ]),
        )
        .unwrap_err();
        assert!(
            err.contains("circular dependency"),
            "expected a cycle error, got: {err}"
        );
    }

    #[test]
    fn eval_modules_missing_module_errors() {
        let err = eval_vcad_with_modules(
            "[use nope]\n[cube 1.0 1.0 1.0]",
            None,
            &modules_map(&[("bracket", BRACKET_MODULE)]),
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn eval_empty_provider_is_a_no_op() {
        // Nothing downstream breaks when no modules are supplied.
        let doc = eval_vcad_with_modules("[cube 10.0 20.0 30.0]", None, &HashMap::new()).unwrap();
        assert_eq!(doc.nodes.len(), 1);
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
