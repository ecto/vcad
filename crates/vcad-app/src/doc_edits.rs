//! Tool lowerings for the JSON-blob document features: the PCB board
//! (`pcb-board.board`), the schematic sheet (`schematic.sheet`) and the
//! design-constraint set (`design-constraints.constraints`).
//!
//! Each edit reads the current blob, applies one small change, and lowers to
//! a single [`DocCommand::SetParam`] (or [`DocCommand::CreateFeature`] when
//! the singleton feature does not exist yet), so the edit is one undo step
//! and every host runs the same semantics. Out-of-range indices and unknown
//! ids fail closed instead of silently doing nothing.

use serde_json::{json, Value as Json};
use vcad_crdt::Value;

use crate::command::{CommandError, DocCommand};
use crate::DocumentApi;

const PCB_KIND: &str = "pcb-board";
const PCB_KEY: &str = "board";
const SCH_KIND: &str = "schematic";
const SCH_KEY: &str = "sheet";
const DC_KIND: &str = "design-constraints";
const DC_KEY: &str = "constraints";

/// Pad-to-trace-endpoint coincidence tolerance when dragging a footprint (mm).
const PAD_TOL: f64 = 0.08;

/// Lower one blob-edit tool. `None` means `tool` is not handled here.
pub fn lower(
    tool: &str,
    args: &Json,
    doc: &DocumentApi,
) -> Option<Result<DocCommand, CommandError>> {
    let r = match tool {
        "pcb_move_footprint" => edit_pcb(doc, tool, |pcb| move_footprint(pcb, args)),
        "pcb_rotate_footprint" => edit_pcb(doc, tool, |pcb| {
            let fp = item_mut(pcb, "footprints", args, tool)?;
            let angle = num(args, "angle", tool)?;
            let cur = fp.get("rotation").and_then(Json::as_f64).unwrap_or(0.0);
            fp["rotation"] = json!((cur + angle) % 360.0);
            Ok(())
        }),
        "pcb_flip_footprint" => edit_pcb(doc, tool, |pcb| {
            let fp = item_mut(pcb, "footprints", args, tool)?;
            let front = fp.get("front").and_then(Json::as_bool).unwrap_or(true);
            fp["front"] = json!(!front);
            Ok(())
        }),
        "pcb_add_footprint" => edit_pcb(doc, tool, |pcb| {
            push(pcb, "footprints", args, "footprint", tool)
        }),
        "pcb_remove_footprint" => edit_pcb(doc, tool, |pcb| remove(pcb, "footprints", args, tool)),
        "pcb_add_trace" => edit_pcb(doc, tool, |pcb| push(pcb, "traces", args, "trace", tool)),
        "pcb_remove_trace" => edit_pcb(doc, tool, |pcb| remove(pcb, "traces", args, tool)),
        "pcb_add_via" => edit_pcb(doc, tool, |pcb| push(pcb, "vias", args, "via", tool)),
        "pcb_remove_via" => edit_pcb(doc, tool, |pcb| remove(pcb, "vias", args, tool)),
        "sch_add_wire" => edit_sch(doc, tool, |s| push(s, "wires", args, "wire", tool)),
        "sch_remove_wire" => edit_sch(doc, tool, |s| remove(s, "wires", args, tool)),
        "sch_add_label" => edit_sch(doc, tool, |s| push(s, "labels", args, "label", tool)),
        "sch_remove_label" => edit_sch(doc, tool, |s| remove(s, "labels", args, tool)),
        "sch_add_junction" => edit_sch(doc, tool, |s| push(s, "junctions", args, "junction", tool)),
        "sch_move_component" => edit_sch(doc, tool, |s| move_component(s, args)),
        "pcb_set_outline" => edit_pcb(doc, tool, |pcb| {
            let outline = match args.get("outline") {
                Some(o @ Json::Object(_)) => o.clone(),
                _ => return Err(invalid(format!("{tool}: `outline` must be an object"))),
            };
            pcb["outline"] = outline;
            Ok(())
        }),
        "pcb_resize" => edit_pcb(doc, tool, |pcb| {
            // Rectangular resize from the origin corner; thickness and
            // cutouts are kept, custom outlines are rectangularized.
            let w = num(args, "width", tool)?.max(1.0);
            let h = num(args, "height", tool)?.max(1.0);
            if !pcb.get("outline").is_some_and(Json::is_object) {
                pcb["outline"] = json!({});
            }
            pcb["outline"]["vertices"] = json!([
                {"x": 0.0, "y": 0.0}, {"x": w, "y": 0.0}, {"x": w, "y": h}, {"x": 0.0, "y": h}
            ]);
            Ok(())
        }),
        "sch_add_component" => {
            edit_sch_and_pcb(doc, tool, |sch, pcb| add_component(sch, pcb, args))
        }
        "sch_remove_component" => edit_sch_and_pcb(doc, tool, |sch, pcb| {
            let i = index(args, tool)?;
            let comps = array_mut(sch, "components", tool)?;
            if i >= comps.len() {
                return Err(invalid(format!(
                    "{tool}: index {i} out of range ({} components)",
                    comps.len()
                )));
            }
            let removed = comps.remove(i);
            // Its footprint goes with it.
            if let (Some(pcb), Some(r)) = (pcb, removed.get("ref").cloned()) {
                array_mut(pcb, "footprints", tool)?.retain(|fp| fp.get("ref") != Some(&r));
            }
            Ok(())
        }),
        "sch_update_component" => edit_sch_and_pcb(doc, tool, |sch, pcb| {
            let updates = match args.get("updates") {
                Some(Json::Object(o)) => o.clone(),
                _ => return Err(invalid(format!("{tool}: `updates` must be an object"))),
            };
            let comp = item_mut(sch, "components", args, tool)?;
            for (k, v) in &updates {
                comp[k.as_str()] = v.clone();
            }
            // A value change is mirrored onto the matching footprint.
            let r = comp.get("ref").cloned();
            if let (Some(pcb), Some(v), Some(r)) = (pcb, updates.get("value"), r) {
                for fp in array_mut(pcb, "footprints", tool)?.iter_mut() {
                    if fp.get("ref") == Some(&r) {
                        fp["value"] = v.clone();
                    }
                }
            }
            Ok(())
        }),
        "add_design_constraint" => add_constraint(doc, args),
        // Replace the whole set (e.g. sketch constraints merged by the host).
        "set_design_constraints" => match args.get("constraints") {
            Some(Json::Array(cs)) => {
                constraints(doc).map(|(id, _)| write_constraints(id, cs.clone()))
            }
            _ => Err(invalid(format!("{tool}: `constraints` must be an array"))),
        },
        "remove_design_constraint" => edit_constraints(doc, tool, |cs| {
            let id = str_arg(args, "id", tool)?;
            let before = cs.len();
            cs.retain(|c| c.get("id").and_then(Json::as_str) != Some(id));
            if cs.len() == before {
                return Err(CommandError::UnknownId { id: id.to_string() });
            }
            Ok(())
        }),
        "update_design_constraint" => edit_constraints(doc, tool, |cs| {
            let id = str_arg(args, "id", tool)?;
            let c = cs
                .iter_mut()
                .find(|c| c.get("id").and_then(Json::as_str) == Some(id))
                .ok_or_else(|| CommandError::UnknownId { id: id.to_string() })?;
            if let Some(v) = args.get("value").filter(|v| !v.is_null()) {
                // A number, or a string expression bound to a parameter.
                if !(v.is_number() || v.is_string()) {
                    return Err(invalid(format!(
                        "{tool}: `value` must be a number or string"
                    )));
                }
                if let Some(kind) = c.get_mut("kind").and_then(Json::as_object_mut) {
                    if kind.contains_key("value") {
                        kind.insert("value".into(), v.clone());
                    }
                }
            }
            if let Some(d) = args.get("driven").filter(|v| !v.is_null()) {
                let d = d
                    .as_bool()
                    .ok_or_else(|| invalid(format!("{tool}: `driven` must be a boolean")))?;
                c["driven"] = json!(d);
            }
            Ok(())
        }),
        _ => return None,
    };
    Some(r)
}

/// First live feature of `kind`: its stable id and string param `key`.
pub(crate) fn blob(doc: &DocumentApi, kind: &str, key: &str) -> Option<(String, Option<String>)> {
    doc.crdt()
        .ordered_features()
        .into_iter()
        .find_map(|(fid, f)| {
            if f.kind != kind {
                return None;
            }
            let id = doc.stable_ids().stable_id(fid)?.to_string();
            let text = match f.params.get(key).map(|(v, _)| v) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            Some((id, text))
        })
}

pub(crate) fn set_blob(id: String, key: &str, value: &Json) -> DocCommand {
    DocCommand::SetParam {
        id,
        key: key.to_string(),
        value: Value::String(value.to_string()),
    }
}

fn parse(text: Option<String>, what: &str) -> Result<Json, CommandError> {
    let text = text.ok_or_else(|| invalid(format!("document has no {what}")))?;
    serde_json::from_str(&text).map_err(|e| invalid(format!("{what} is not valid JSON: {e}")))
}

fn edit_blob(
    doc: &DocumentApi,
    kind: &str,
    key: &str,
    what: &str,
    tool: &str,
    f: impl FnOnce(&mut Json) -> Result<(), CommandError>,
) -> Result<DocCommand, CommandError> {
    let (id, text) =
        blob(doc, kind, key).ok_or_else(|| invalid(format!("{tool}: document has no {what}")))?;
    let mut value = parse(text, what)?;
    if !value.is_object() {
        return Err(invalid(format!("{tool}: {what} is not an object")));
    }
    f(&mut value)?;
    Ok(set_blob(id, key, &value))
}

fn edit_pcb(
    doc: &DocumentApi,
    tool: &str,
    f: impl FnOnce(&mut Json) -> Result<(), CommandError>,
) -> Result<DocCommand, CommandError> {
    edit_blob(doc, PCB_KIND, PCB_KEY, "PCB board", tool, f)
}

fn edit_sch(
    doc: &DocumentApi,
    tool: &str,
    f: impl FnOnce(&mut Json) -> Result<(), CommandError>,
) -> Result<DocCommand, CommandError> {
    edit_blob(doc, SCH_KIND, SCH_KEY, "schematic", tool, f)
}

/// Edit the schematic and, when the document has one, the PCB board in one
/// batch (one undo step).
fn edit_sch_and_pcb(
    doc: &DocumentApi,
    tool: &str,
    f: impl FnOnce(&mut Json, Option<&mut Json>) -> Result<(), CommandError>,
) -> Result<DocCommand, CommandError> {
    let (sch_id, text) = blob(doc, SCH_KIND, SCH_KEY)
        .ok_or_else(|| invalid(format!("{tool}: document has no schematic")))?;
    let mut sch = parse(text, "schematic")?;
    if !sch.is_object() {
        return Err(invalid(format!("{tool}: schematic is not an object")));
    }
    let mut pcb = match blob(doc, PCB_KIND, PCB_KEY) {
        Some((id, Some(text))) => Some((id, parse(Some(text), "PCB board")?)),
        _ => None,
    }
    .filter(|(_, v)| v.is_object());
    f(&mut sch, pcb.as_mut().map(|(_, v)| v))?;
    let mut commands = vec![set_blob(sch_id, SCH_KEY, &sch)];
    if let Some((id, v)) = pcb {
        commands.push(set_blob(id, PCB_KEY, &v));
    }
    Ok(DocCommand::Batch { commands })
}

/// `sch_add_component {component}`: append it; when it names a footprint
/// with a `footprintTemplate` property and the document has a board, also
/// place that footprint on a 5-wide 10 mm grid.
fn add_component(sch: &mut Json, pcb: Option<&mut Json>, args: &Json) -> Result<(), CommandError> {
    let tool = "sch_add_component";
    let comp = match args.get("component") {
        Some(o @ Json::Object(_)) => o.clone(),
        _ => return Err(invalid(format!("{tool}: `component` must be an object"))),
    };
    array_mut(sch, "components", tool)?.push(comp.clone());
    let (Some(pcb), Some(fp_name)) = (pcb, comp.get("footprintId").filter(|v| !v.is_null())) else {
        return Ok(());
    };
    let Some(template) = comp
        .pointer("/properties/footprintTemplate")
        .and_then(Json::as_str)
        .and_then(|t| serde_json::from_str::<Json>(t).ok())
    else {
        return Ok(());
    };
    let fps = array_mut(pcb, "footprints", tool)?;
    let n = fps.len();
    fps.push(json!({
        "ref": comp.get("ref").cloned().unwrap_or(Json::Null),
        "value": comp.get("value").cloned().unwrap_or(Json::Null),
        "footprintName": fp_name,
        "position": {"x": 10.0 + (n % 5) as f64 * 10.0, "y": 10.0 + (n / 5) as f64 * 10.0},
        "pads": template.get("pads").cloned().unwrap_or_else(|| json!([])),
        "graphics": template.get("graphics").cloned().unwrap_or_else(|| json!([])),
    }));
    Ok(())
}

fn constraints(doc: &DocumentApi) -> Result<(Option<String>, Vec<Json>), CommandError> {
    match blob(doc, DC_KIND, DC_KEY) {
        None => Ok((None, Vec::new())),
        Some((id, None)) => Ok((Some(id), Vec::new())),
        Some((id, text)) => match parse(text, "design constraints")? {
            Json::Array(a) => Ok((Some(id), a)),
            _ => Err(invalid("design constraints are not an array".into())),
        },
    }
}

fn write_constraints(id: Option<String>, cs: Vec<Json>) -> DocCommand {
    let value = Json::Array(cs);
    match id {
        Some(id) => set_blob(id, DC_KEY, &value),
        None => DocCommand::CreateFeature {
            kind: DC_KIND.to_string(),
            params: Some([(DC_KEY.to_string(), Value::String(value.to_string()))].into()),
        },
    }
}

fn edit_constraints(
    doc: &DocumentApi,
    tool: &str,
    f: impl FnOnce(&mut Vec<Json>) -> Result<(), CommandError>,
) -> Result<DocCommand, CommandError> {
    let (id, mut cs) = constraints(doc)?;
    if id.is_none() {
        return Err(invalid(format!(
            "{tool}: document has no design constraints"
        )));
    }
    f(&mut cs)?;
    Ok(write_constraints(id, cs))
}

/// `add_design_constraint {constraint}`: appended with the next free `cN` id.
fn add_constraint(doc: &DocumentApi, args: &Json) -> Result<DocCommand, CommandError> {
    let tool = "add_design_constraint";
    let mut c = match args.get("constraint") {
        Some(Json::Object(o)) => o.clone(),
        _ => return Err(invalid(format!("{tool}: `constraint` must be an object"))),
    };
    let (id, mut cs) = constraints(doc)?;
    let max = cs
        .iter()
        .filter_map(|c| {
            c.get("id")?
                .as_str()?
                .strip_prefix('c')?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    c.insert("id".into(), json!(format!("c{}", max + 1)));
    cs.push(Json::Object(c));
    Ok(write_constraints(id, cs))
}

fn array_mut<'a>(
    v: &'a mut Json,
    key: &str,
    tool: &str,
) -> Result<&'a mut Vec<Json>, CommandError> {
    let obj = v.as_object_mut().expect("checked object");
    obj.entry(key).or_insert_with(|| Json::Array(Vec::new()));
    obj.get_mut(key)
        .and_then(Json::as_array_mut)
        .ok_or_else(|| invalid(format!("{tool}: `{key}` is not an array")))
}

fn index(args: &Json, tool: &str) -> Result<usize, CommandError> {
    args.get("index")
        .and_then(Json::as_u64)
        .map(|i| i as usize)
        .ok_or_else(|| invalid(format!("{tool}: `index` must be a non-negative integer")))
}

fn item_mut<'a>(
    v: &'a mut Json,
    key: &str,
    args: &Json,
    tool: &str,
) -> Result<&'a mut Json, CommandError> {
    let i = index(args, tool)?;
    let arr = array_mut(v, key, tool)?;
    let len = arr.len();
    arr.get_mut(i)
        .ok_or_else(|| invalid(format!("{tool}: index {i} out of range ({len} {key})")))
}

fn push(v: &mut Json, key: &str, args: &Json, item: &str, tool: &str) -> Result<(), CommandError> {
    let value = match args.get(item) {
        Some(o @ Json::Object(_)) => o.clone(),
        _ => return Err(invalid(format!("{tool}: `{item}` must be an object"))),
    };
    array_mut(v, key, tool)?.push(value);
    Ok(())
}

fn remove(v: &mut Json, key: &str, args: &Json, tool: &str) -> Result<(), CommandError> {
    let i = index(args, tool)?;
    let arr = array_mut(v, key, tool)?;
    if i >= arr.len() {
        return Err(invalid(format!(
            "{tool}: index {i} out of range ({} {key})",
            arr.len()
        )));
    }
    arr.remove(i);
    Ok(())
}

fn xy(v: &Json) -> Option<(f64, f64)> {
    Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}

fn xy_arg(args: &Json, key: &str, tool: &str) -> Result<(f64, f64), CommandError> {
    args.get(key)
        .and_then(xy)
        .ok_or_else(|| invalid(format!("{tool}: `{key}` must be {{x, y}}")))
}

/// `pcb_move_footprint {index, position}`: move a footprint, dragging any
/// trace endpoint that sits on one of its pads along by the same delta so
/// routed copper stays attached.
fn move_footprint(pcb: &mut Json, args: &Json) -> Result<(), CommandError> {
    let tool = "pcb_move_footprint";
    let (x, y) = xy_arg(args, "position", tool)?;
    let fp = item_mut(pcb, "footprints", args, tool)?;
    let (fx, fy) = fp.get("position").and_then(xy).unwrap_or((0.0, 0.0));
    let ang = fp
        .get("rotation")
        .and_then(Json::as_f64)
        .unwrap_or(0.0)
        .to_radians();
    let (sin, cos) = ang.sin_cos();
    let pads: Vec<(f64, f64)> = fp
        .get("pads")
        .and_then(Json::as_array)
        .map(|pads| {
            pads.iter()
                .filter_map(|p| p.get("position").and_then(xy))
                .map(|(px, py)| (fx + px * cos - py * sin, fy + px * sin + py * cos))
                .collect()
        })
        .unwrap_or_default();
    fp["position"] = json!({"x": x, "y": y});
    let (dx, dy) = (x - fx, y - fy);
    if dx == 0.0 && dy == 0.0 {
        return Ok(());
    }
    if let Some(traces) = pcb.get_mut("traces").and_then(Json::as_array_mut) {
        for tr in traces {
            for end in ["start", "end"] {
                let Some(p) = tr.get_mut(end) else { continue };
                let Some((ex, ey)) = xy(p) else { continue };
                if pads
                    .iter()
                    .any(|(px, py)| (ex - px).abs() < PAD_TOL && (ey - py).abs() < PAD_TOL)
                {
                    *p = json!({"x": ex + dx, "y": ey + dy});
                }
            }
        }
    }
    Ok(())
}

/// `sch_move_component {index, position, wire_updates?: [{wire_index,
/// endpoint: "start"|"end", position}]}`: move a component and, in the same
/// edit, the wire endpoints attached to it.
fn move_component(sch: &mut Json, args: &Json) -> Result<(), CommandError> {
    let tool = "sch_move_component";
    let (x, y) = xy_arg(args, "position", tool)?;
    let comp = item_mut(sch, "components", args, tool)?;
    comp["position"] = json!({"x": x, "y": y});
    let Some(updates) = args.get("wire_updates").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    let updates = updates
        .as_array()
        .ok_or_else(|| invalid(format!("{tool}: `wire_updates` must be an array")))?;
    let wires = array_mut(sch, "wires", tool)?;
    for u in updates {
        let i = u
            .get("wire_index")
            .and_then(Json::as_u64)
            .ok_or_else(|| invalid(format!("{tool}: wire update needs `wire_index`")))?
            as usize;
        let end = u.get("endpoint").and_then(Json::as_str).unwrap_or("");
        if end != "start" && end != "end" {
            return Err(invalid(format!(
                "{tool}: `endpoint` must be \"start\" or \"end\""
            )));
        }
        let (px, py) = xy_arg(u, "position", tool)?;
        let len = wires.len();
        let wire = wires
            .get_mut(i)
            .ok_or_else(|| invalid(format!("{tool}: wire index {i} out of range ({len} wires)")))?;
        wire[end] = json!({"x": px, "y": py});
    }
    Ok(())
}

fn str_arg<'a>(v: &'a Json, key: &str, tool: &str) -> Result<&'a str, CommandError> {
    v.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| invalid(format!("{tool}: missing string `{key}`")))
}

fn num(v: &Json, key: &str, tool: &str) -> Result<f64, CommandError> {
    v.get(key)
        .and_then(Json::as_f64)
        .ok_or_else(|| invalid(format!("{tool}: missing number `{key}`")))
}

pub(crate) fn invalid(message: String) -> CommandError {
    CommandError::Invalid { message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_crdt::ReplicaId;

    fn api_with(kind: &str, key: &str, value: Json) -> DocumentApi {
        let mut api = DocumentApi::new(ReplicaId(1));
        let params: HashMap<String, Value> =
            [(key.to_string(), Value::String(value.to_string()))].into();
        api.create_feature_raw(kind, params);
        api
    }

    fn blob_json(api: &DocumentApi, kind: &str, key: &str) -> Json {
        serde_json::from_str(&blob(api, kind, key).unwrap().1.unwrap()).unwrap()
    }

    fn apply(api: &mut DocumentApi, tool: &str, args: Json) -> Result<(), CommandError> {
        let cmd = lower(tool, &args, api).expect("handled")?;
        api.apply(cmd).map(|_| ())
    }

    fn board() -> Json {
        json!({
            "footprints": [{
                "ref": "R1",
                "position": {"x": 10.0, "y": 10.0},
                "rotation": 90.0,
                "pads": [{"position": {"x": 1.0, "y": 0.0}}]
            }],
            "traces": [
                {"start": {"x": 10.0, "y": 11.0}, "end": {"x": 30.0, "y": 30.0}},
                {"start": {"x": 50.0, "y": 50.0}, "end": {"x": 60.0, "y": 60.0}}
            ],
            "vias": []
        })
    }

    #[test]
    fn move_footprint_drags_attached_trace_ends() {
        let mut api = api_with(PCB_KIND, PCB_KEY, board());
        // Pad (1,0) rotated 90° sits at (10, 11) — the first trace's start.
        apply(
            &mut api,
            "pcb_move_footprint",
            json!({"index": 0, "position": {"x": 15.0, "y": 12.0}}),
        )
        .unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(
            pcb["footprints"][0]["position"],
            json!({"x": 15.0, "y": 12.0})
        );
        assert_eq!(pcb["traces"][0]["start"], json!({"x": 15.0, "y": 13.0}));
        assert_eq!(pcb["traces"][0]["end"], json!({"x": 30.0, "y": 30.0}));
        assert_eq!(pcb["traces"][1], board()["traces"][1]);
    }

    #[test]
    fn rotate_flip_add_remove_on_board() {
        let mut api = api_with(PCB_KIND, PCB_KEY, board());
        apply(
            &mut api,
            "pcb_rotate_footprint",
            json!({"index": 0, "angle": 300.0}),
        )
        .unwrap();
        apply(&mut api, "pcb_flip_footprint", json!({"index": 0})).unwrap();
        apply(
            &mut api,
            "pcb_add_via",
            json!({"via": {"position": {"x": 1, "y": 2}, "diameter": 0.6}}),
        )
        .unwrap();
        apply(&mut api, "pcb_remove_trace", json!({"index": 1})).unwrap();
        apply(
            &mut api,
            "pcb_add_trace",
            json!({"trace": {"start": {"x": 0, "y": 0}, "end": {"x": 1, "y": 1}}}),
        )
        .unwrap();
        apply(
            &mut api,
            "pcb_add_footprint",
            json!({"footprint": {"ref": "C1"}}),
        )
        .unwrap();
        apply(&mut api, "pcb_remove_footprint", json!({"index": 0})).unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(pcb["footprints"].as_array().unwrap().len(), 1);
        assert_eq!(pcb["footprints"][0]["ref"], "C1");
        assert_eq!(pcb["vias"].as_array().unwrap().len(), 1);
        assert_eq!(pcb["traces"].as_array().unwrap().len(), 2);
        assert_eq!(pcb["traces"][1]["end"], json!({"x": 1, "y": 1}));
        // Rotation wraps and flip toggles (checked before the removal).
        let mut api = api_with(PCB_KIND, PCB_KEY, board());
        apply(
            &mut api,
            "pcb_rotate_footprint",
            json!({"index": 0, "angle": 300.0}),
        )
        .unwrap();
        apply(&mut api, "pcb_flip_footprint", json!({"index": 0})).unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(pcb["footprints"][0]["rotation"], json!(30.0));
        assert_eq!(pcb["footprints"][0]["front"], json!(false));
    }

    #[test]
    fn board_edits_fail_closed() {
        let mut api = api_with(PCB_KIND, PCB_KEY, board());
        let before = blob_json(&api, PCB_KIND, PCB_KEY);
        for (tool, args) in [
            ("pcb_remove_trace", json!({"index": 5})),
            ("pcb_rotate_footprint", json!({"index": 3, "angle": 1.0})),
            ("pcb_move_footprint", json!({"index": 0})),
            ("pcb_add_via", json!({"via": 1})),
            ("pcb_remove_via", json!({"index": -1})),
        ] {
            assert!(
                matches!(
                    apply(&mut api, tool, args),
                    Err(CommandError::Invalid { .. })
                ),
                "{tool}"
            );
        }
        assert_eq!(blob_json(&api, PCB_KIND, PCB_KEY), before);
        // No board at all.
        let empty = DocumentApi::new(ReplicaId(1));
        assert!(lower("pcb_flip_footprint", &json!({"index": 0}), &empty)
            .unwrap()
            .is_err());
        assert!(lower("not_a_blob_tool", &json!({}), &empty).is_none());
    }

    #[test]
    fn schematic_edits() {
        let sheet = json!({
            "components": [{"ref": "R1", "position": {"x": 0, "y": 0}}],
            "wires": [{"start": {"x": 0, "y": 0}, "end": {"x": 5, "y": 0}}],
            "labels": [],
            "junctions": []
        });
        let mut api = api_with(SCH_KIND, SCH_KEY, sheet);
        apply(&mut api, "sch_move_component", json!({
            "index": 0,
            "position": {"x": 2, "y": 3},
            "wire_updates": [{"wire_index": 0, "endpoint": "start", "position": {"x": 2, "y": 3}}]
        })).unwrap();
        apply(
            &mut api,
            "sch_add_wire",
            json!({"wire": {"start": {"x": 1, "y": 1}, "end": {"x": 2, "y": 2}}}),
        )
        .unwrap();
        apply(&mut api, "sch_remove_wire", json!({"index": 1})).unwrap();
        apply(&mut api, "sch_add_label", json!({"label": {"text": "VCC"}})).unwrap();
        apply(
            &mut api,
            "sch_add_junction",
            json!({"junction": {"position": {"x": 1, "y": 1}}}),
        )
        .unwrap();
        let s = blob_json(&api, SCH_KIND, SCH_KEY);
        assert_eq!(s["components"][0]["position"], json!({"x": 2.0, "y": 3.0}));
        assert_eq!(s["wires"][0]["start"], json!({"x": 2.0, "y": 3.0}));
        assert_eq!(s["wires"].as_array().unwrap().len(), 1);
        assert_eq!(s["labels"][0]["text"], "VCC");
        apply(&mut api, "sch_remove_label", json!({"index": 0})).unwrap();
        assert!(blob_json(&api, SCH_KIND, SCH_KEY)["labels"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(apply(&mut api, "sch_move_component", json!({
            "index": 0, "position": {"x": 0, "y": 0},
            "wire_updates": [{"wire_index": 9, "endpoint": "start", "position": {"x": 0, "y": 0}}]
        })).is_err());
    }

    #[test]
    fn board_outline_and_resize() {
        let mut api = api_with(
            PCB_KIND,
            PCB_KEY,
            json!({"outline": {"vertices": [], "thickness": 1.6}}),
        );
        apply(
            &mut api,
            "pcb_resize",
            json!({"width": 40.0, "height": 0.2}),
        )
        .unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(pcb["outline"]["thickness"], json!(1.6));
        assert_eq!(pcb["outline"]["vertices"][2], json!({"x": 40.0, "y": 1.0}));
        apply(
            &mut api,
            "pcb_set_outline",
            json!({"outline": {"vertices": [{"x": 1, "y": 1}]}}),
        )
        .unwrap();
        assert_eq!(
            blob_json(&api, PCB_KIND, PCB_KEY)["outline"],
            json!({"vertices": [{"x": 1, "y": 1}]})
        );
        assert!(apply(&mut api, "pcb_set_outline", json!({"outline": []})).is_err());
    }

    #[test]
    fn schematic_component_edits_follow_onto_the_board() {
        let mut api = api_with(SCH_KIND, SCH_KEY, json!({"components": [], "wires": []}));
        let params: HashMap<String, Value> = [(
            PCB_KEY.to_string(),
            Value::String(json!({"footprints": [{"ref": "X9"}]}).to_string()),
        )]
        .into();
        api.create_feature_raw(PCB_KIND, params);
        let template = json!({"pads": [{"n": 1}]}).to_string();
        apply(
            &mut api,
            "sch_add_component",
            json!({"component": {
                "ref": "R1", "value": "10k", "footprintId": "0603",
                "properties": {"footprintTemplate": template}
            }}),
        )
        .unwrap();
        // No template → schematic only.
        apply(
            &mut api,
            "sch_add_component",
            json!({"component": {"ref": "R2", "footprintId": "0603"}}),
        )
        .unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(pcb["footprints"].as_array().unwrap().len(), 2);
        assert_eq!(pcb["footprints"][1]["ref"], "R1");
        assert_eq!(
            pcb["footprints"][1]["position"],
            json!({"x": 20.0, "y": 10.0})
        );
        assert_eq!(pcb["footprints"][1]["pads"], json!([{"n": 1}]));
        apply(
            &mut api,
            "sch_update_component",
            json!({"index": 0, "updates": {"value": "22k"}}),
        )
        .unwrap();
        assert_eq!(
            blob_json(&api, SCH_KIND, SCH_KEY)["components"][0]["value"],
            "22k"
        );
        assert_eq!(
            blob_json(&api, PCB_KIND, PCB_KEY)["footprints"][1]["value"],
            "22k"
        );
        apply(&mut api, "sch_remove_component", json!({"index": 0})).unwrap();
        let pcb = blob_json(&api, PCB_KIND, PCB_KEY);
        assert_eq!(pcb["footprints"].as_array().unwrap().len(), 1);
        assert_eq!(
            blob_json(&api, SCH_KIND, SCH_KEY)["components"][0]["ref"],
            "R2"
        );
        assert!(apply(&mut api, "sch_remove_component", json!({"index": 4})).is_err());
        // Schematic-only documents work too.
        let mut api = api_with(SCH_KIND, SCH_KEY, json!({"components": []}));
        apply(
            &mut api,
            "sch_add_component",
            json!({"component": {"ref": "U1"}}),
        )
        .unwrap();
        assert_eq!(
            blob_json(&api, SCH_KIND, SCH_KEY)["components"][0]["ref"],
            "U1"
        );
    }

    #[test]
    fn design_constraints_crud() {
        let mut api = DocumentApi::new(ReplicaId(1));
        // First add creates the singleton feature.
        apply(
            &mut api,
            "add_design_constraint",
            json!({"constraint": {"kind": {"type": "distance", "value": 5.0}}}),
        )
        .unwrap();
        apply(
            &mut api,
            "add_design_constraint",
            json!({"constraint": {"kind": {"type": "parallel"}}}),
        )
        .unwrap();
        let cs = blob_json(&api, DC_KIND, DC_KEY);
        assert_eq!(cs[0]["id"], "c1");
        assert_eq!(cs[1]["id"], "c2");
        apply(
            &mut api,
            "update_design_constraint",
            json!({"id": "c1", "value": 7.5, "driven": true}),
        )
        .unwrap();
        // `value` only applies to kinds that carry one.
        apply(
            &mut api,
            "update_design_constraint",
            json!({"id": "c2", "value": 1.0}),
        )
        .unwrap();
        let cs = blob_json(&api, DC_KIND, DC_KEY);
        assert_eq!(cs[0]["kind"]["value"], json!(7.5));
        assert_eq!(cs[0]["driven"], json!(true));
        assert!(cs[1]["kind"].get("value").is_none());
        apply(&mut api, "remove_design_constraint", json!({"id": "c1"})).unwrap();
        apply(
            &mut api,
            "add_design_constraint",
            json!({"constraint": {"kind": {"type": "x"}}}),
        )
        .unwrap();
        let cs = blob_json(&api, DC_KIND, DC_KEY);
        assert_eq!(cs.as_array().unwrap().len(), 2);
        assert_eq!(cs[1]["id"], "c3");
        assert!(matches!(
            apply(&mut api, "remove_design_constraint", json!({"id": "c9"})),
            Err(CommandError::UnknownId { .. })
        ));
        assert!(apply(&mut api, "add_design_constraint", json!({"constraint": 3})).is_err());
        // Whole-set replacement, and creation when the feature is missing.
        let mut fresh = DocumentApi::new(ReplicaId(1));
        apply(
            &mut fresh,
            "set_design_constraints",
            json!({"constraints": [{"id": "c7"}]}),
        )
        .unwrap();
        assert_eq!(blob_json(&fresh, DC_KIND, DC_KEY), json!([{"id": "c7"}]));
        apply(
            &mut fresh,
            "set_design_constraints",
            json!({"constraints": []}),
        )
        .unwrap();
        assert_eq!(blob_json(&fresh, DC_KIND, DC_KEY), json!([]));
        assert!(apply(
            &mut fresh,
            "set_design_constraints",
            json!({"constraints": {}})
        )
        .is_err());
        // String values (parameter expressions) are accepted; other types are not.
        apply(
            &mut api,
            "add_design_constraint",
            json!({"constraint": {"kind": {"type": "distance", "value": 1.0}}}),
        )
        .unwrap();
        apply(
            &mut api,
            "update_design_constraint",
            json!({"id": "c4", "value": "width / 2"}),
        )
        .unwrap();
        assert_eq!(
            blob_json(&api, DC_KIND, DC_KEY)[2]["kind"]["value"],
            "width / 2"
        );
        assert!(apply(
            &mut api,
            "update_design_constraint",
            json!({"id": "c4", "value": [1]})
        )
        .is_err());
    }
}
