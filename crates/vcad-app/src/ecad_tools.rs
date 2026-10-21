//! Board-level ECAD tool lowerings: create a PCB board (`pcb_init`,
//! `pcb_import`), reconcile the board against its schematic
//! (`pcb_sync_schematic`), and commit a design-constraint solve
//! (`apply_design_solve`). Like [`crate::doc_edits`], each lowers to one
//! command over the `pcb-board` / `design-constraints` blobs.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value as Json};
use vcad_crdt::Value;

use crate::command::{CommandError, DocCommand};
use crate::doc_edits::{blob, invalid, set_blob};
use crate::DocumentApi;

/// Highest copper count a new board may have (FCu + BCu + In1Cu..In30Cu).
pub const MAX_COPPER_LAYERS: u32 = 32;

/// Lower one board-level tool. `None` means `tool` is not handled here.
pub fn lower(
    tool: &str,
    args: &Json,
    doc: &DocumentApi,
) -> Option<Result<DocCommand, CommandError>> {
    Some(match tool {
        "pcb_init" => pcb_init(args),
        "pcb_import" => match args.get("board") {
            Some(b @ Json::Object(_)) => Ok(create_board(
                b,
                args.get("name")
                    .and_then(Json::as_str)
                    .unwrap_or("Imported PCB"),
            )),
            _ => Err(invalid("pcb_import: `board` must be an object".into())),
        },
        "pcb_sync_schematic" => sync_schematic(args, doc),
        "apply_design_solve" => apply_design_solve(args, doc),
        _ => return None,
    })
}

fn create_board(board: &Json, name: &str) -> DocCommand {
    let params: HashMap<String, Value> = [
        ("name".to_string(), Value::String(name.to_string())),
        ("board".to_string(), Value::String(board.to_string())),
        ("material".to_string(), Value::String("__pcb_fr4__".into())),
    ]
    .into();
    DocCommand::CreateFeature {
        kind: "pcb-board".into(),
        params: Some(params),
    }
}

/// Ordered copper layer names (top → bottom) for `count` copper layers.
pub fn copper_layer_names(count: u32) -> Vec<String> {
    match count {
        0 => Vec::new(),
        1 => vec!["FCu".into()],
        n => std::iter::once("FCu".to_string())
            .chain((1..n - 1).map(|i| format!("In{i}Cu")))
            .chain(std::iter::once("BCu".to_string()))
            .collect(),
    }
}

fn opt_f64(args: &Json, key: &str, default: f64) -> Result<f64, CommandError> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .ok_or_else(|| invalid(format!("pcb_init: `{key}` must be a number"))),
    }
}

/// `pcb_init {width?, height?, layers?, thickness?, trace_width?,
/// clearance?, name?}`: a rectangular board with a uniform FR4 stackup and
/// default design rules.
fn pcb_init(args: &Json) -> Result<DocCommand, CommandError> {
    let w = opt_f64(args, "width", 50.0)?;
    let h = opt_f64(args, "height", 30.0)?;
    let layers = opt_f64(args, "layers", 2.0)?;
    let thickness = opt_f64(args, "thickness", 1.6)?;
    let trace_width = opt_f64(args, "trace_width", 0.15)?;
    let clearance = opt_f64(args, "clearance", 0.15)?;
    let name = args
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or("PCB Board");
    let ok = layers.fract() == 0.0
        && layers >= 1.0
        && (layers == 1.0 || (layers as u32).is_multiple_of(2))
        && layers <= MAX_COPPER_LAYERS as f64;
    if !ok {
        return Err(invalid(format!(
            "unsupported PCB layer count {layers}: expected 1 or an even number from 2 to {MAX_COPPER_LAYERS}"
        )));
    }
    let count = layers as u32;
    let dielectric = if count > 1 {
        thickness / f64::from(count - 1)
    } else {
        thickness
    };
    let stackup: Vec<Json> = copper_layer_names(count)
        .into_iter()
        .enumerate()
        .map(|(i, layer)| {
            if i == 0 {
                json!({"layer": layer, "copperThickness": 0.035})
            } else {
                json!({
                    "layer": layer,
                    "copperThickness": 0.035,
                    "dielectricThickness": dielectric,
                    "dielectricEr": 4.5,
                    "material": "FR4",
                })
            }
        })
        .collect();
    let board = json!({
        "outline": {
            "vertices": [
                {"x": 0.0, "y": 0.0}, {"x": w, "y": 0.0}, {"x": w, "y": h}, {"x": 0.0, "y": h}
            ],
            "thickness": thickness,
        },
        "stackup": {"layers": stackup},
        "nets": [],
        "rules": {
            "defaultRules": {
                "name": "Default",
                "traceWidth": trace_width,
                "clearance": clearance,
                "viaDiameter": 0.6,
                "viaDrill": 0.3,
            },
            "edgeClearance": 0.25,
            "holeToHole": 0.25,
            "minAnnularRing": 0.13,
            "minDrill": 0.2,
            // KiCad's microvia defaults (uvia_min_drill / uvia_min_size).
            "minMicroviaDrill": 0.1,
            "minMicroviaDiameter": 0.2,
            "inferredFloors": [],
        },
        "footprints": [],
        "traces": [],
        "vias": [],
        "zones": [],
    });
    Ok(create_board(&board, name))
}

/// One SMD pad per schematic pin on a centred 2.54 mm single row — used when
/// a component carries no `footprintTemplate`, so every placed part is
/// visible and routable.
pub fn pads_from_pins(pins: &[Json]) -> Vec<Json> {
    let n = pins.len() as f64;
    pins.iter()
        .enumerate()
        .map(|(i, pin)| {
            json!({
                "number": pin.get("number").cloned().unwrap_or(Json::Null),
                "padType": "SMD",
                "shape": {"type": "RoundRect", "width": 1.5, "height": 1.5, "cornerRatio": 0.25},
                "position": {"x": (i as f64 - (n - 1.0) / 2.0) * 2.54, "y": 0.0},
                "layers": ["FCu", "FPaste", "FMask"],
            })
        })
        .collect()
}

/// Reconcile `pcb` against `schematic`; returns whether anything changed.
///
/// 1. Unless `place_unplaced` is false, place each schematic component that
///    has no footprint (pads from its `footprintTemplate`, else its pins),
///    staggered on a 5-wide 10 mm grid.
/// 2. With a netlist, set `pad.net` for every connected pad, clear stale
///    assignments, and union new net names into `pcb.nets` (never removing,
///    so routed nets keep their identity). Runs over all footprints so a
///    re-sync repairs drift.
pub fn sync_schematic_data(
    pcb: &mut Json,
    schematic: &Json,
    netlist: Option<&Json>,
    place_unplaced: bool,
) -> bool {
    let mut changed = false;
    if !pcb.get("footprints").is_some_and(Json::is_array) {
        pcb["footprints"] = json!([]);
    }
    if !pcb.get("nets").is_some_and(Json::is_array) {
        pcb["nets"] = json!([]);
    }
    let empty = Vec::new();
    let components = schematic
        .get("components")
        .and_then(Json::as_array)
        .unwrap_or(&empty);
    if place_unplaced {
        let fps = pcb["footprints"].as_array_mut().expect("array");
        let mut refs: HashSet<Json> = fps.iter().filter_map(|f| f.get("ref").cloned()).collect();
        for comp in components {
            let r = comp.get("ref").cloned().unwrap_or(Json::Null);
            if refs.contains(&r) {
                continue;
            }
            let template = comp
                .pointer("/properties/footprintTemplate")
                .and_then(Json::as_str)
                .and_then(|t| serde_json::from_str::<Json>(t).ok());
            let mut pads = template
                .as_ref()
                .and_then(|t| t.get("pads"))
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();
            let graphics = template
                .as_ref()
                .and_then(|t| t.get("graphics"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            if pads.is_empty() {
                pads = pads_from_pins(comp.get("pins").and_then(Json::as_array).unwrap_or(&empty));
            }
            let k = fps.len();
            let name = comp
                .get("footprintId")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| r.clone());
            fps.push(json!({
                "ref": r,
                "value": comp.get("value").cloned().unwrap_or(Json::Null),
                "footprintName": name,
                "position": {"x": 10.0 + (k % 5) as f64 * 10.0, "y": 10.0 + (k / 5) as f64 * 10.0},
                "pads": pads,
                "graphics": graphics,
            }));
            refs.insert(r);
            changed = true;
        }
    }
    let Some(nets) = netlist.and_then(|n| n.get("nets")).and_then(Json::as_array) else {
        return changed;
    };
    let mut pad_net: HashMap<(String, String), String> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    for net in nets {
        let Some(name) = net.get("name").and_then(Json::as_str) else {
            continue;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        for c in net
            .get("connections")
            .and_then(Json::as_array)
            .unwrap_or(&empty)
        {
            let (Some(r), Some(p)) = (
                c.get("component_ref").and_then(Json::as_str),
                c.get("pin_number").and_then(Json::as_str),
            ) else {
                continue;
            };
            pad_net.insert((r.to_string(), p.to_string()), name.to_string());
        }
    }
    for fp in pcb["footprints"].as_array_mut().expect("array") {
        let r = fp
            .get("ref")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let Some(pads) = fp.get_mut("pads").and_then(Json::as_array_mut) else {
            continue;
        };
        for pad in pads {
            let num = pad
                .get("number")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let wanted = pad_net.get(&(r.clone(), num));
            let cur = pad.get("net").and_then(Json::as_str);
            if wanted.map(String::as_str) != cur {
                let Some(obj) = pad.as_object_mut() else {
                    continue;
                };
                match wanted {
                    Some(w) => obj.insert("net".into(), json!(w)),
                    None => obj.remove("net"),
                };
                changed = true;
            }
        }
    }
    let pcb_nets = pcb["nets"].as_array_mut().expect("array");
    let known: HashSet<String> = pcb_nets
        .iter()
        .filter_map(|n| n.get("name").and_then(Json::as_str).map(str::to_string))
        .collect();
    for name in names {
        if !known.contains(&name) {
            pcb_nets.push(json!({"id": name, "name": name}));
            changed = true;
        }
    }
    changed
}

/// `pcb_sync_schematic {netlist?, place_unplaced?}`. An already-in-sync board
/// lowers to an empty batch (no change).
fn sync_schematic(args: &Json, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    let tool = "pcb_sync_schematic";
    let (pcb_id, pcb_text) = blob(doc, "pcb-board", "board")
        .ok_or_else(|| invalid(format!("{tool}: document has no PCB board")))?;
    let (_, sch_text) = blob(doc, "schematic", "sheet")
        .ok_or_else(|| invalid(format!("{tool}: document has no schematic")))?;
    let parse = |t: Option<String>, what: &str| -> Result<Json, CommandError> {
        let t = t.ok_or_else(|| invalid(format!("{tool}: {what} is empty")))?;
        serde_json::from_str(&t)
            .map_err(|e| invalid(format!("{tool}: {what} is not valid JSON: {e}")))
    };
    let mut pcb = parse(pcb_text, "PCB board")?;
    let sch = parse(sch_text, "schematic")?;
    if !pcb.is_object() {
        return Err(invalid(format!("{tool}: PCB board is not an object")));
    }
    let netlist = args.get("netlist").filter(|v| !v.is_null());
    let place = args
        .get("place_unplaced")
        .and_then(Json::as_bool)
        .unwrap_or(true);
    if !sync_schematic_data(&mut pcb, &sch, netlist, place) {
        return Ok(DocCommand::Batch {
            commands: Vec::new(),
        });
    }
    Ok(set_blob(pcb_id, "board", &pcb))
}

/// `apply_design_solve {document, report}`: commit the output of the
/// kernel's design-constraint solve. A converged group that solved a PCB
/// writes that board back when the solve moved footprints or outline
/// vertices; back-annotated driven values write the constraint set.
fn apply_design_solve(args: &Json, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    let tool = "apply_design_solve";
    let solved = args
        .get("document")
        .filter(|d| d.is_object())
        .ok_or_else(|| invalid(format!("{tool}: `document` must be an object")))?;
    let report = args
        .get("report")
        .filter(|r| r.is_object())
        .ok_or_else(|| invalid(format!("{tool}: `report` must be an object")))?;
    let len = |k: &str| report.get(k).and_then(Json::as_array).map_or(0, Vec::len);
    let mut commands = Vec::new();
    let moved = len("movedFootprints") > 0 || len("movedVertices") > 0;
    let board = report
        .get("groups")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|g| g.get("converged").and_then(Json::as_bool) == Some(true))
        .find_map(|g| node_pcb(solved, g.get("node")?));
    if let (true, Some(board)) = (moved, board) {
        let (id, _) = blob(doc, "pcb-board", "board")
            .ok_or_else(|| invalid(format!("{tool}: document has no PCB board")))?;
        commands.push(set_blob(id, "board", board));
    }
    if len("drivenValues") > 0 {
        let cs = solved
            .get("constraints")
            .cloned()
            .unwrap_or_else(|| json!([]));
        match blob(doc, "design-constraints", "constraints") {
            Some((id, _)) => commands.push(set_blob(id, "constraints", &cs)),
            None => commands.push(DocCommand::CreateFeature {
                kind: "design-constraints".into(),
                params: Some([("constraints".to_string(), Value::String(cs.to_string()))].into()),
            }),
        }
    }
    Ok(DocCommand::Batch { commands })
}

/// The PCB a solved document holds for board node `node`: the node's own
/// `PcbBoard` op, else the materializer's top-level `pcb`.
fn node_pcb<'a>(doc: &'a Json, node: &Json) -> Option<&'a Json> {
    let key = match node {
        Json::Number(n) => n.to_string(),
        Json::String(s) => s.clone(),
        _ => return None,
    };
    let op = doc.get("nodes")?.get(&key)?.get("op")?;
    if op.get("type").and_then(Json::as_str) == Some("PcbBoard") {
        return op.get("board");
    }
    doc.get("pcb").filter(|p| p.is_object())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_crdt::ReplicaId;

    fn apply(
        api: &mut DocumentApi,
        tool: &str,
        args: Json,
    ) -> Result<Option<String>, CommandError> {
        let cmd = lower(tool, &args, api).expect("handled")?;
        api.apply(cmd).map(|r| r.created_feature_id)
    }

    fn board_of(api: &DocumentApi) -> Json {
        serde_json::from_str(&blob(api, "pcb-board", "board").unwrap().1.unwrap()).unwrap()
    }

    fn comp(r: &str, pins: &[&str]) -> Json {
        json!({
            "ref": r, "value": "10k",
            "pins": pins.iter().map(|p| json!({"number": p})).collect::<Vec<_>>(),
        })
    }

    fn sheet(comps: Vec<Json>) -> Json {
        json!({"components": comps, "wires": [], "junctions": [], "labels": []})
    }

    fn empty_pcb() -> Json {
        json!({"footprints": [], "nets": [], "traces": [], "vias": []})
    }

    #[test]
    fn pcb_init_builds_a_stackup_and_rules() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let id = apply(
            &mut api,
            "pcb_init",
            json!({"width": 40, "height": 20, "layers": 4, "name": "Main"}),
        )
        .unwrap()
        .expect("created");
        assert!(api.stable_ids().resolve(&id).is_some());
        let b = board_of(&api);
        let layers: Vec<&str> = b["stackup"]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["layer"].as_str().unwrap())
            .collect();
        assert_eq!(layers, ["FCu", "In1Cu", "In2Cu", "BCu"]);
        assert_eq!(b["stackup"]["layers"][0].get("dielectricThickness"), None);
        assert!(
            (b["stackup"]["layers"][1]["dielectricThickness"]
                .as_f64()
                .unwrap()
                - 1.6 / 3.0)
                .abs()
                < 1e-12
        );
        assert_eq!(b["outline"]["vertices"][2], json!({"x": 40.0, "y": 20.0}));
        assert_eq!(b["rules"]["defaultRules"]["traceWidth"], json!(0.15));
        assert_eq!(copper_layer_names(1), ["FCu"]);
        for bad in [3, 0, 34] {
            assert!(
                apply(&mut api, "pcb_init", json!({"layers": bad})).is_err(),
                "{bad}"
            );
        }
        apply(
            &mut api,
            "pcb_import",
            json!({"board": {"footprints": [{"ref": "Q1"}]}}),
        )
        .unwrap();
        assert!(apply(&mut api, "pcb_import", json!({"board": 5})).is_err());
    }

    // Ported from the retired TS ecad-sync tests (same cases, engine path).
    #[test]
    fn pads_from_pins_centred_row() {
        let pads = pads_from_pins(&[
            json!({"number": "1"}),
            json!({"number": "2"}),
            json!({"number": "3"}),
        ]);
        let nums: Vec<&str> = pads.iter().map(|p| p["number"].as_str().unwrap()).collect();
        assert_eq!(nums, ["1", "2", "3"]);
        assert_eq!(pads[1]["position"]["x"], json!(0.0));
        assert!((pads[0]["position"]["x"].as_f64().unwrap() + 2.54).abs() < 1e-9);
        assert!((pads[2]["position"]["x"].as_f64().unwrap() - 2.54).abs() < 1e-9);
        assert!(pads
            .iter()
            .all(|p| p["layers"].as_array().unwrap().contains(&json!("FCu"))));
    }

    #[test]
    fn sync_places_unplaced_components() {
        let mut pcb = empty_pcb();
        let sch = sheet(vec![comp("R1", &["1", "2"])]);
        assert!(sync_schematic_data(&mut pcb, &sch, None, true));
        assert_eq!(pcb["footprints"][0]["ref"], "R1");
        assert_eq!(pcb["footprints"][0]["pads"].as_array().unwrap().len(), 2);
        // Idempotent: no duplicate, no change.
        assert!(!sync_schematic_data(&mut pcb, &sch, None, true));
        assert_eq!(pcb["footprints"].as_array().unwrap().len(), 1);
        // Template pads win over pin-derived ones.
        let mut pcb = empty_pcb();
        let mut c = comp("J1", &["1"]);
        c["properties"] = json!({"footprintTemplate":
            json!({"pads": [{"number": "1", "padType": "THT", "position": {"x": -2, "y": 0}}]}).to_string()});
        sync_schematic_data(&mut pcb, &sheet(vec![c]), None, true);
        assert_eq!(pcb["footprints"][0]["pads"][0]["padType"], "THT");
        // place_unplaced = false leaves the board alone.
        let mut pcb = empty_pcb();
        assert!(!sync_schematic_data(
            &mut pcb,
            &sheet(vec![comp("R1", &["1"])]),
            None,
            false
        ));
        assert!(pcb["footprints"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sync_maps_nets_onto_pads() {
        let netlist = json!({"nets": [
            {"name": "GND", "connections": [{"component_ref": "R1", "pin_number": "1"}]},
            {"name": "NET-001", "connections": [
                {"component_ref": "R1", "pin_number": "2"},
                {"component_ref": "R2", "pin_number": "1"}
            ]}
        ]});
        let sch = sheet(vec![comp("R1", &["1", "2"]), comp("R2", &["1", "2"])]);
        // Placed first without nets (the auto-place flow), netted afterwards.
        let mut pcb = json!({"footprints": [], "nets": [{"id": "VBAT", "name": "VBAT"}]});
        sync_schematic_data(&mut pcb, &sch, None, true);
        assert!(sync_schematic_data(&mut pcb, &sch, Some(&netlist), true));
        let pad =
            |pcb: &Json, f: usize, p: usize| pcb["footprints"][f]["pads"][p].get("net").cloned();
        assert_eq!(pad(&pcb, 0, 0), Some(json!("GND")));
        assert_eq!(pad(&pcb, 0, 1), Some(json!("NET-001")));
        assert_eq!(pad(&pcb, 1, 0), Some(json!("NET-001")));
        assert_eq!(pad(&pcb, 1, 1), None);
        let mut names: Vec<&str> = pcb["nets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["GND", "NET-001", "VBAT"]); // additive, never removes
                                                       // A stale assignment is cleared when its net disappears.
        assert!(sync_schematic_data(
            &mut pcb,
            &sch,
            Some(&json!({"nets": []})),
            true
        ));
        assert!(pcb["footprints"][0]["pads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p.get("net").is_none()));
        // Net names register even with nothing to place.
        let mut pcb = empty_pcb();
        let only = json!({"nets": [{"name": "NET-001", "connections": []}]});
        assert!(sync_schematic_data(
            &mut pcb,
            &sheet(vec![]),
            Some(&only),
            true
        ));
        assert_eq!(pcb["nets"], json!([{"id": "NET-001", "name": "NET-001"}]));
    }

    #[test]
    fn sync_tool_commits_once_then_noops() {
        let mut api = DocumentApi::new(ReplicaId(1));
        apply(&mut api, "pcb_init", json!({})).unwrap();
        let params: HashMap<String, Value> = [(
            "sheet".to_string(),
            Value::String(sheet(vec![comp("U1", &["1"])]).to_string()),
        )]
        .into();
        api.create_feature_raw("schematic", params);
        apply(&mut api, "pcb_sync_schematic", json!({})).unwrap();
        assert_eq!(board_of(&api)["footprints"][0]["ref"], "U1");
        let cmd = lower("pcb_sync_schematic", &json!({}), &api)
            .unwrap()
            .unwrap();
        assert!(matches!(cmd, DocCommand::Batch { ref commands } if commands.is_empty()));
        let empty = DocumentApi::new(ReplicaId(1));
        assert!(lower("pcb_sync_schematic", &json!({}), &empty)
            .unwrap()
            .is_err());
    }

    #[test]
    fn apply_design_solve_writes_board_and_driven_values() {
        let mut api = DocumentApi::new(ReplicaId(1));
        apply(&mut api, "pcb_init", json!({})).unwrap();
        let solved_board = json!({"footprints": [{"ref": "R1", "position": {"x": 9, "y": 9}}]});
        let solved = json!({
            "nodes": {"4": {"op": {"type": "Empty"}}},
            "pcb": solved_board,
            "constraints": [{"id": "c1", "kind": {"value": 12.0}}]
        });
        let report = json!({
            "groups": [{"node": 4, "converged": true}],
            "movedFootprints": ["R1"], "movedVertices": [], "drivenValues": [{"id": "c1"}]
        });
        apply(
            &mut api,
            "apply_design_solve",
            json!({"document": solved, "report": report}),
        )
        .unwrap();
        assert_eq!(board_of(&api), solved_board);
        let cs: Json = serde_json::from_str(
            &blob(&api, "design-constraints", "constraints")
                .unwrap()
                .1
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cs[0]["kind"]["value"], json!(12.0));
        // Nothing moved, nothing driven → empty batch; non-converged groups never write.
        let quiet = json!({"groups": [{"node": 4, "converged": false}], "movedFootprints": ["R1"]});
        let cmd = lower(
            "apply_design_solve",
            &json!({"document": solved, "report": quiet}),
            &api,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(cmd, DocCommand::Batch { ref commands } if commands.is_empty()));
        assert!(lower("apply_design_solve", &json!({"report": {}}), &api)
            .unwrap()
            .is_err());
    }
}
