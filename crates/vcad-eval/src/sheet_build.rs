//! Build a sheet-metal IR document from a `sheet_metal_create` request:
//! a base flange (rectangle or polygon outline with holes) followed by
//! ordered edge flanges, hems, jogs and an optional bend relief, each node
//! parented to the previous. Hosts pass the agent's JSON straight through.

use serde_json::{json, Map, Value};
use vcad_ir::{CsgOp, Document, Node, SceneEntry};

/// Build the document. Malformed engravings or a non-numeric required field
/// are errors (a silently dropped part label is worse than a refusal).
pub fn build_sheet_metal_document(args: &Value) -> Result<Document, String> {
    let mut ops: Vec<(String, Value)> = Vec::new();
    let num = |v: Option<&Value>| v.and_then(Value::as_f64);
    let thickness = num(args.get("thickness")).ok_or("`thickness` must be a number")?;
    let material = args
        .get("material")
        .and_then(Value::as_str)
        .unwrap_or("al-soft");
    let mut base = Map::new();
    base.insert("thickness".into(), json!(thickness));
    base.insert("material".into(), json!(material));
    if let Some(shop) = args
        .get("shop_profile")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        base.insert("shop_profile".into(), json!(shop));
    }
    if let Some(raw) = args.get("engravings").and_then(Value::as_array) {
        let engravings = raw
            .iter()
            .enumerate()
            .map(|(i, e)| engraving(i, e))
            .collect::<Result<Vec<_>, _>>()?;
        if !engravings.is_empty() {
            base.insert("engravings".into(), Value::Array(engravings));
        }
    }
    if let Some(outline) = args.get("outline").and_then(Value::as_array) {
        base.insert("type".into(), json!("SheetMetalBaseFlangePolygon"));
        base.insert("outline".into(), Value::Array(loop_points(outline)));
        if let Some(holes) = args.get("holes").and_then(Value::as_array) {
            let holes: Vec<Value> = holes
                .iter()
                .map(|h| Value::Array(h.as_array().map(|l| loop_points(l)).unwrap_or_default()))
                .collect();
            base.insert("holes".into(), Value::Array(holes));
        }
        ops.push(("Base flange (polygon)".into(), Value::Object(base)));
    } else {
        base.insert("type".into(), json!("SheetMetalBaseFlangeRect"));
        base.insert("width".into(), json!(num(args.get("width")).unwrap_or(0.0)));
        base.insert("depth".into(), json!(num(args.get("depth")).unwrap_or(0.0)));
        ops.push(("Base flange".into(), Value::Object(base)));
    }

    let list = |k: &str| {
        args.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let dir = |s: &Value| s.get("direction").cloned().unwrap_or(json!("Up"));
    let panel = |s: &Value| s.get("panel_id").cloned().unwrap_or(json!(0));
    let mut chain: Vec<(String, Map<String, Value>)> = Vec::new();
    for f in list("flanges") {
        let mut o = Map::new();
        o.insert("type".into(), json!("SheetMetalEdgeFlange"));
        o.insert("panel_id".into(), panel(&f));
        o.insert(
            "edge_index".into(),
            f.get("edge_index").cloned().unwrap_or(Value::Null),
        );
        o.insert(
            "length".into(),
            f.get("length").cloned().unwrap_or(Value::Null),
        );
        o.insert(
            "angle".into(),
            f.get("angle")
                .cloned()
                .unwrap_or(json!(std::f64::consts::FRAC_PI_2)),
        );
        // Radius omitted → the kernel defaults to thickness (or the shop's).
        copy(&f, &mut o, "radius", "radius");
        o.insert("direction".into(), dir(&f));
        copy(&f, &mut o, "manual_k", "manual_k");
        chain.push(("Edge flange".into(), o));
    }
    for h in list("hems") {
        let mut o = Map::new();
        o.insert("type".into(), json!("SheetMetalHem"));
        o.insert("panel_id".into(), panel(&h));
        o.insert(
            "edge_index".into(),
            h.get("edge_index").cloned().unwrap_or(Value::Null),
        );
        o.insert(
            "kind".into(),
            h.get("kind").cloned().unwrap_or(json!("Closed")),
        );
        o.insert(
            "length".into(),
            h.get("length").cloned().unwrap_or(Value::Null),
        );
        o.insert("gap".into(), h.get("gap").cloned().unwrap_or(json!(0)));
        o.insert("direction".into(), dir(&h));
        chain.push(("Hem".into(), o));
    }
    for j in list("jogs") {
        let mut o = Map::new();
        o.insert("type".into(), json!("SheetMetalJog"));
        o.insert("panel_id".into(), panel(&j));
        o.insert(
            "edge_index".into(),
            j.get("edge_index").cloned().unwrap_or(Value::Null),
        );
        o.insert(
            "offset".into(),
            j.get("offset").cloned().unwrap_or(Value::Null),
        );
        o.insert(
            "length".into(),
            j.get("length").cloned().unwrap_or(Value::Null),
        );
        copy(&j, &mut o, "radius", "radius");
        o.insert("direction".into(), dir(&j));
        chain.push(("Jog".into(), o));
    }
    // `bend_relief: true` → default sizing; an object → explicit sizing.
    let relief = match args.get("bend_relief") {
        Some(Value::Bool(true)) => Some(Value::Object(Map::new())),
        Some(o @ Value::Object(_)) => Some(o.clone()),
        _ => None,
    };
    if let Some(r) = relief {
        let mut o = Map::new();
        o.insert("type".into(), json!("SheetMetalBendRelief"));
        copy(&r, &mut o, "width_mm", "width");
        copy(&r, &mut o, "depth_mm", "depth");
        chain.push(("Bend relief".into(), o));
    }

    let mut doc = Document::default();
    let mut add = |id: u64, name: String, op: Value| -> Result<(), String> {
        let op: CsgOp = serde_json::from_value(op).map_err(|e| format!("{name}: {e}"))?;
        doc.nodes.insert(
            id,
            Node {
                id,
                name: Some(name),
                op,
            },
        );
        Ok(())
    };
    let (name, op) = ops.remove(0);
    add(0, name, op)?;
    let mut parent = 0u64;
    for (i, (label, mut o)) in chain.into_iter().enumerate() {
        let id = i as u64 + 1;
        o.insert("parent".into(), json!(parent));
        let name = if label == "Bend relief" {
            label
        } else {
            format!("{label} {id}")
        };
        add(id, name, Value::Object(o))?;
        parent = id;
    }
    doc.roots = vec![SceneEntry {
        root: parent,
        material: "default".into(),
        visible: None,
    }];
    Ok(doc)
}

fn copy(from: &Value, to: &mut Map<String, Value>, src: &str, dst: &str) {
    if let Some(v) = from.get(src).filter(|v| !v.is_null()) {
        to.insert(dst.into(), v.clone());
    }
}

/// `[x, y]` pairs and `{x, y}` objects both accepted; others dropped.
fn point(p: &Value) -> Option<Value> {
    let (x, y) = match p {
        Value::Array(a) if a.len() >= 2 => (a[0].as_f64()?, a[1].as_f64()?),
        Value::Object(_) => (p.get("x")?.as_f64()?, p.get("y")?.as_f64()?),
        _ => return None,
    };
    Some(json!({"x": x, "y": y}))
}

fn loop_points(l: &[Value]) -> Vec<Value> {
    l.iter().filter_map(point).collect()
}

fn engraving(i: usize, e: &Value) -> Result<Value, String> {
    let ty = e.get("type").and_then(Value::as_str);
    let has_text = e.get("text").is_some_and(Value::is_string);
    if ty == Some("Text") || (ty.is_none() && has_text) {
        let text = e
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty());
        let text = text.ok_or(format!("engravings[{i}]: Text needs a non-empty `text`"))?;
        let height = e.get("height").and_then(Value::as_f64).filter(|h| *h > 0.0);
        let height = height.ok_or(format!(
            "engravings[{i}]: Text needs `height` > 0 (cap height, mm)"
        ))?;
        let f = |k: &str| e.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        return Ok(
            json!({"type": "Text", "text": text, "x": f("x"), "y": f("y"), "height": height, "angle": f("angle")}),
        );
    }
    if ty == Some("Polyline") || e.get("points").is_some_and(Value::is_array) {
        let points = e
            .get("points")
            .and_then(Value::as_array)
            .map(|p| loop_points(p))
            .unwrap_or_default();
        if points.len() < 2 {
            return Err(format!("engravings[{i}]: Polyline needs >= 2 points"));
        }
        return Ok(json!({"type": "Polyline", "points": points}));
    }
    Err(format!(
        "engravings[{i}]: expected {{type: \"Text\", text, x, y, height}} or {{type: \"Polyline\", points}}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_base_with_chain_of_features() {
        let doc = build_sheet_metal_document(&json!({
            "width": 100, "depth": 50, "thickness": 1.5, "material": "steel",
            "shop_profile": "sendcutsend",
            "flanges": [{"edge_index": 0, "length": 20}, {"edge_index": 2, "length": 15, "radius": 2, "direction": "Down"}],
            "hems": [{"edge_index": 1, "length": 8}],
            "jogs": [{"edge_index": 3, "offset": 5, "length": 10}],
            "bend_relief": true
        }))
        .unwrap();
        assert_eq!(doc.nodes.len(), 6);
        assert_eq!(doc.roots[0].root, 5);
        match &doc.nodes[&0].op {
            CsgOp::SheetMetalBaseFlangeRect {
                width,
                depth,
                material,
                shop_profile,
                ..
            } => {
                assert_eq!((*width, *depth), (100.0, 50.0));
                assert_eq!(material, "steel");
                assert_eq!(shop_profile.as_deref(), Some("sendcutsend"));
            }
            other => panic!("{other:?}"),
        }
        let v = serde_json::to_value(&doc).unwrap();
        let op = |id: &str| v["nodes"][id]["op"].clone();
        assert_eq!(op("1")["parent"], json!(0));
        assert!((op("1")["angle"].as_f64().unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(op("1").get("radius").is_none_or(Value::is_null));
        assert_eq!(op("2")["direction"], "Down");
        assert_eq!(op("3")["type"], "SheetMetalHem");
        assert_eq!(op("3")["kind"], "Closed");
        assert_eq!(op("4")["parent"], json!(3));
        assert_eq!(op("5")["type"], "SheetMetalBendRelief");
        assert_eq!(v["nodes"]["5"]["name"], "Bend relief");
        assert_eq!(v["nodes"]["2"]["name"], "Edge flange 2");
    }

    #[test]
    fn polygon_base_points_and_engravings() {
        let doc = build_sheet_metal_document(&json!({
            "thickness": 1,
            "outline": [[0, 0], {"x": 10, "y": 0}, [10, 10], "junk"],
            "holes": [[[2, 2], [3, 2], [3, 3]]],
            "engravings": [{"text": "P1", "x": 1, "y": 2, "height": 3}, {"points": [[0, 0], [1, 1]]}],
            "bend_relief": {"width_mm": 1.2}
        }))
        .unwrap();
        let v = serde_json::to_value(&doc).unwrap();
        let base = &v["nodes"]["0"]["op"];
        assert_eq!(base["type"], "SheetMetalBaseFlangePolygon");
        assert_eq!(base["outline"].as_array().unwrap().len(), 3);
        assert_eq!(base["holes"][0].as_array().unwrap().len(), 3);
        assert_eq!(base["material"], "al-soft");
        assert_eq!(base["engravings"].as_array().unwrap().len(), 2);
        assert_eq!(v["nodes"]["1"]["op"]["width"], json!(1.2));
        assert_eq!(doc.roots[0].root, 1);
    }

    #[test]
    fn malformed_input_is_refused() {
        for bad in [
            json!({}),
            json!({"thickness": 1, "engravings": [{"type": "Text", "text": "", "height": 1}]}),
            json!({"thickness": 1, "engravings": [{"text": "A", "height": 0}]}),
            json!({"thickness": 1, "engravings": [{"points": [[0, 0]]}]}),
            json!({"thickness": 1, "engravings": [{"type": "Blob"}]}),
            json!({"thickness": 1, "flanges": [{"length": 5}]}),
        ] {
            assert!(build_sheet_metal_document(&bad).is_err(), "{bad}");
        }
    }
}
