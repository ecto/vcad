//! Capture loon binding names on scene roots.
//!
//! `[root wheel "steel"]` desugars (in the stdlib) to a `SceneEntry` that
//! takes the solid *by value*, so the name `wheel` never reaches the
//! document — every root exported as an unnamed node. This pass runs over
//! the parsed program before evaluation and rewrites
//!
//! ```text
//! [root wheel "steel"]  ->  [root-named wheel "steel" "wheel"]
//! ```
//!
//! whenever the first argument is a bare symbol. A root built from an
//! expression (`[root [cube 1 1 1] "steel"]`) has no name to capture and is
//! left exactly as it was, and an author who wants a different name can call
//! `root-named` directly.

use loon_lang::ast::{Expr as LExpr, ExprKind};

/// Rewrite every `[root <symbol> <material>]` into a named form, in place.
pub fn capture(exprs: &mut [LExpr]) {
    for e in exprs.iter_mut() {
        capture_expr(e);
    }
}

fn capture_expr(e: &mut LExpr) {
    if let ExprKind::List(items) = &mut e.kind {
        if items.len() == 3 {
            let head_is_root = matches!(&items[0].kind, ExprKind::Symbol(s) if s == "root");
            let name = match &items[1].kind {
                ExprKind::Symbol(s) => Some(s.clone()),
                _ => None,
            };
            if head_is_root {
                if let Some(name) = name {
                    if let ExprKind::Symbol(head) = &mut items[0].kind {
                        *head = "root-named".to_string();
                    }
                    let mut lit = items[1].clone();
                    lit.kind = ExprKind::Str(name);
                    items.push(lit);
                }
            }
        }
    }
    for c in children_mut(e) {
        capture_expr(c);
    }
}

fn children_mut(e: &mut LExpr) -> Vec<&mut LExpr> {
    match &mut e.kind {
        ExprKind::List(v) | ExprKind::Vec(v) | ExprKind::Set(v) | ExprKind::Tuple(v) => {
            v.iter_mut().collect()
        }
        ExprKind::Map(pairs) => pairs.iter_mut().flat_map(|(k, v)| [k, v]).collect(),
        ExprKind::Quote(b) | ExprKind::Unquote(b) | ExprKind::UnquoteSplice(b) => vec![b.as_mut()],
        ExprKind::DotAccess(b, _) => vec![b.as_mut()],
        _ => Vec::new(),
    }
}
