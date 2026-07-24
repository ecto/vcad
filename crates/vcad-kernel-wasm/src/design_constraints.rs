//! WASM bindings for the document-level design-constraint solver.
//!
//! Stateless, following the `solveSketchSegments` pattern: the caller sends
//! the whole document as JSON and receives the updated document plus a
//! solve report. Part-edge anchors are resolved by evaluating the
//! referenced part nodes with this crate's own evaluator and querying the
//! kernel's fail-closed topological-naming resolution.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

use vcad_design_constraints::{
    check_design_constraints, solve_design_constraints, AnchorResolver, DesignSolveReport,
    SolveOptions,
};

/// Resolves part-edge anchors by evaluating document nodes on demand
/// (memoized per call) and asking the kernel for the named edge.
struct DocResolver<'a> {
    doc: &'a vcad_ir::Document,
    cache: RefCell<HashMap<vcad_ir::NodeId, Option<crate::Solid>>>,
}

impl AnchorResolver for DocResolver<'_> {
    fn resolve_part_edge(
        &self,
        node: vcad_ir::NodeId,
        face_a: &str,
        face_b: &str,
    ) -> Result<([f64; 3], [f64; 3]), String> {
        let mut cache = self.cache.borrow_mut();
        let solid = cache
            .entry(node)
            .or_insert_with(|| crate::evaluate_node(self.doc, node).ok());
        let Some(solid) = solid else {
            return Err(format!("part node {node} failed to evaluate"));
        };
        let (a, b) = solid
            .inner
            .resolve_named_edge(face_a, face_b)
            .map_err(|e| format!("part edge {face_a}/{face_b} on node {node}: {e}"))?;
        Ok(([a.x, a.y, a.z], [b.x, b.y, b.z]))
    }
}

/// Optional inputs for [`solve_design_constraints_wasm`].
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireOptions {
    /// Footprints to pin at their current position for this solve only:
    /// `[{ node, ref }]`. Used for interactive drag anchoring.
    #[serde(default)]
    extra_fixed: Vec<WireExtraFixed>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireExtraFixed {
    node: u64,
    r#ref: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WireResult<'a> {
    document: &'a vcad_ir::Document,
    report: &'a DesignSolveReport,
}

fn parse_doc(doc_json: &str) -> Result<vcad_ir::Document, JsError> {
    serde_json::from_str(doc_json).map_err(|e| JsError::new(&format!("document JSON: {e}")))
}

fn parse_options(options_json: &str) -> Result<SolveOptions, JsError> {
    let wire: WireOptions = if options_json.trim().is_empty() {
        WireOptions::default()
    } else {
        serde_json::from_str(options_json)
            .map_err(|e| JsError::new(&format!("options JSON: {e}")))?
    };
    Ok(SolveOptions {
        extra_fixed: wire
            .extra_fixed
            .into_iter()
            .map(|f| (f.node, f.r#ref))
            .collect(),
    })
}

/// Solve the document's design constraints and return
/// `{ document, report }` — the updated document (footprint positions and
/// rotations, outline vertices, sketch points, back-annotated driven
/// dimensions) plus the solve report (per-group status, DOF, moved
/// geometry, errors).
#[wasm_bindgen(js_name = solveDesignConstraints)]
pub fn solve_design_constraints_wasm(
    doc_json: &str,
    options_json: &str,
) -> Result<String, JsError> {
    let mut doc = parse_doc(doc_json)?;
    let options = parse_options(options_json)?;
    let snapshot = doc.clone();
    let resolver = DocResolver {
        doc: &snapshot,
        cache: RefCell::new(HashMap::new()),
    };
    let report = solve_design_constraints(&mut doc, &resolver, &options);
    let out = WireResult {
        document: &doc,
        report: &report,
    };
    serde_json::to_string(&out).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Validate and measure the document's constraints without mutating
/// anything. Returns the solve report JSON (dimensional constraints all
/// measured into `drivenValues`).
#[wasm_bindgen(js_name = checkDesignConstraints)]
pub fn check_design_constraints_wasm(doc_json: &str) -> Result<String, JsError> {
    let doc = parse_doc(doc_json)?;
    let resolver = DocResolver {
        doc: &doc,
        cache: RefCell::new(HashMap::new()),
    };
    let report = check_design_constraints(&doc, &resolver);
    serde_json::to_string(&report).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

#[cfg(test)]
mod tests {
    //! Pure-Rust mirror of the wire API (wasm-bindgen entry points can't run
    //! on the host, but the logic underneath can).

    use super::*;

    #[test]
    fn solve_via_wire_api_shapes() {
        // A board with two footprints and one distance constraint, sent
        // through the same JSON path the JS caller uses.
        let doc_json = r#"{
            "version": "0.1",
            "nodes": {
                "1": { "id": 1, "name": "board", "op": { "type": "PcbBoard", "board": {
                    "outline": { "vertices": [{"x":0,"y":0},{"x":50,"y":0},{"x":50,"y":40},{"x":0,"y":40}], "cutouts": [], "thickness": 1.6 },
                    "stackup": { "layers": [] },
                    "nets": [],
                    "rules": { "defaultRules": { "name": "d", "traceWidth": 0.25, "clearance": 0.2, "viaDiameter": 0.6, "viaDrill": 0.3 }, "edgeClearance": 0.5, "holeToHole": 0.5, "minAnnularRing": 0.15, "minDrill": 0.3 },
                    "footprints": [
                        { "ref": "U1", "value": "", "footprintName": "X", "position": {"x": 5, "y": 5}, "pads": [] },
                        { "ref": "U2", "value": "", "footprintName": "X", "position": {"x": 20, "y": 5}, "pads": [] }
                    ],
                    "traces": [], "vias": [], "zones": []
                } } }
            },
            "materials": {},
            "part_materials": {},
            "roots": [],
            "constraints": [
                { "id": "c1", "kind": { "type": "fixed", "a": { "kind": "pcbFootprint", "node": 1, "ref": "U1" } } },
                { "id": "c2", "kind": { "type": "distance",
                    "a": { "kind": "pcbFootprint", "node": 1, "ref": "U1" },
                    "b": { "kind": "pcbFootprint", "node": 1, "ref": "U2" },
                    "value": 10.0 } }
            ]
        }"#;
        let mut doc = parse_doc(doc_json).expect("doc parses");
        let options = parse_options("{}").expect("options parse");
        let snapshot = doc.clone();
        let resolver = DocResolver {
            doc: &snapshot,
            cache: RefCell::new(HashMap::new()),
        };
        let report = solve_design_constraints(&mut doc, &resolver, &options);
        assert!(report.converged, "{report:?}");
        assert_eq!(report.moved_footprints, vec!["U2".to_string()]);
        let json = serde_json::to_string(&WireResult {
            document: &doc,
            report: &report,
        })
        .unwrap();
        assert!(json.contains("\"movedFootprints\":[\"U2\"]"), "{json}");
    }

    #[test]
    fn options_parse_extra_fixed() {
        let opts = parse_options(r#"{ "extraFixed": [{ "node": 1, "ref": "U2" }] }"#).unwrap();
        assert_eq!(opts.extra_fixed, vec![(1, "U2".to_string())]);
    }
}
