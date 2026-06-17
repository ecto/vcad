//! Regression tests for chained / nested boolean composition in the loon
//! evaluator.
//!
//! Booleans are subject-last: `[difference tool subject]` → `subject − tool`.
//! The cases below mirror real bug reports where multi-feature parts authored
//! by nesting `difference` (the recommended `create_cad_loon` path) lost their
//! cuts:
//!
//!   * the SUBJECT of an outer `difference` is itself a `difference` result, and
//!   * the body is referenced through a `[let name value body]` binding, whose
//!     trailing body used to be silently dropped (returning the bound value).
//!
//! Both reduce to the same IR shape, so we assert the evaluator emits a proper
//! chain of `Difference` nodes with every cut still referenced.

use vcad_ir::{CsgOp, Document, NodeId};
use vcad_loon::eval_vcad;

/// Walk the (single) root and return the chain of `Difference` nodes from the
/// root inward, following the `left` (subject) edge — i.e. the order cuts are
/// composed. Each entry is `(left, right)`.
fn difference_chain(doc: &Document) -> Vec<(NodeId, NodeId)> {
    assert_eq!(doc.roots.len(), 1, "expected exactly one root");
    let mut chain = Vec::new();
    let mut cur = doc.roots[0].root;
    while let Some(node) = doc.nodes.get(&cur) {
        match &node.op {
            CsgOp::Difference { left, right } => {
                chain.push((*left, *right));
                cur = *left;
            }
            _ => break,
        }
    }
    chain
}

fn op_of(doc: &Document, id: NodeId) -> &CsgOp {
    &doc.nodes[&id].op
}

/// The terminal subject of the chain (follow `left` past every Difference) must
/// be the filleted cube body — proof no cut was dropped and the body survives.
fn assert_body_is_filleted_cube(doc: &Document, mut id: NodeId) {
    while let CsgOp::Difference { left, .. } = op_of(doc, id) {
        id = *left;
    }
    match op_of(doc, id) {
        CsgOp::Fillet { child, .. } => {
            assert!(
                matches!(op_of(doc, *child), CsgOp::Cube { .. }),
                "fillet child should be the cube body"
            );
        }
        other => panic!("expected filleted-cube body at chain tail, got {other:?}"),
    }
}

const CASE_3: &str = "[difference [translate 45 42 25 [cylinder 5 4]] \
   [difference [translate 39 -5 6 [fillet 1.5 [cube 12 10 5]]] \
     [difference [translate 45 30 58 [sphere 36]] \
       [fillet 14 [cube 90 60 28]]]]]";

const CASE_4: &str = "[let b [fillet 14 [cube 90 60 28]] \
   [difference [translate 45 42 25 [cylinder 5 4]] \
     [difference [translate 39 -5 6 [fillet 1.5 [cube 12 10 5]]] \
       [difference [translate 45 30 58 [sphere 36]] b]]]]";

/// Case 3: chained differences where each outer `difference`'s subject is the
/// previous `difference` result. All three cuts must compose into a 3-deep
/// Difference chain ending at the body.
#[test]
fn chained_difference_keeps_every_cut() {
    let doc = eval_vcad(CASE_3, None).unwrap();
    let chain = difference_chain(&doc);
    assert_eq!(chain.len(), 3, "expected 3 chained Difference nodes");

    // Each `right` (tool) is, in order: cylinder, small filleted cube, sphere.
    let tool_ops: Vec<&CsgOp> = chain
        .iter()
        .map(|(_, right)| {
            // tools are wrapped in Translate — peel one transform
            match op_of(&doc, *right) {
                CsgOp::Translate { child, .. } => op_of(&doc, *child),
                other => other,
            }
        })
        .collect();
    assert!(
        matches!(tool_ops[0], CsgOp::Cylinder { .. }),
        "outer cut = cylinder"
    );
    assert!(
        matches!(tool_ops[1], CsgOp::Fillet { .. }),
        "middle cut = filleted small cube"
    );
    assert!(
        matches!(tool_ops[2], CsgOp::Sphere { .. }),
        "inner cut = sphere"
    );

    assert_body_is_filleted_cube(&doc, doc.roots[0].root);
}

/// Case 4: the same chain, but the body is bound with `[let b … expr]`. Before
/// the fix the `let` returned `b` (the bare body) and every cut vanished. Now
/// it must produce the identical chain to case 3.
#[test]
fn let_bound_subject_keeps_every_cut() {
    let doc = eval_vcad(CASE_4, None).unwrap();
    let chain = difference_chain(&doc);
    assert_eq!(
        chain.len(),
        3,
        "let-bound body must still compose into 3 Difference nodes (was 0 — cuts dropped)"
    );
    assert_body_is_filleted_cube(&doc, doc.roots[0].root);

    // Case 3 and case 4 are the same part expressed two ways → identical node count.
    let doc3 = eval_vcad(CASE_3, None).unwrap();
    assert_eq!(
        doc.nodes.len(),
        doc3.nodes.len(),
        "let-bound and inline chains must yield the same graph size"
    );
}

/// The `let` body must actually be evaluated and returned. A `let`-bound body
/// with no further use still produces the bare body; with a `difference` around
/// it the difference must win.
#[test]
fn let_body_is_not_dropped() {
    // Plain binding then a cut applied to it via the body expression.
    let doc = eval_vcad(
        "[let body [cube 20 20 20] [difference [translate 5 5 5 [cube 5 5 5]] body]]",
        None,
    )
    .unwrap();
    assert_eq!(doc.roots.len(), 1);
    assert!(
        matches!(op_of(&doc, doc.roots[0].root), CsgOp::Difference { .. }),
        "let body (a difference) must be the root, not the bare bound cube"
    );
}
