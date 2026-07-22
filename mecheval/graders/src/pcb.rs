//! ECAD checks — the pcbeval oracle wiring.
//!
//! A `.vcad` document carries PCB data as `PcbBoard` nodes (plus a legacy
//! top-level `pcb` field) and a schematic as `document.schematic`. The
//! checks here run the real kernel oracles against them:
//!
//! - `drc_clean` / `nets_fully_connected` → [`vcad_ecad_pcb::drc::check_drc`]
//! - `erc_clean` → [`vcad_ecad_schematic::erc::check_erc`]
//! - `board_envelope` / `component_count` → direct geometry/BOM reads
//!
//! All checks are fail-closed: a document with no board (or no schematic,
//! for ERC) fails rather than vacuously passing. DEFAULT_CUBE stays dead.

use serde_json::{json, Value};
use vcad_ecad_pcb::drc::{check_drc, DrcRuleType};
use vcad_ecad_schematic::erc::check_erc;
use vcad_ir::ecad::Pcb;
use vcad_ir::{CsgOp, Document};

use crate::blob::CheckOutcome;
use crate::eval::EvalSnapshot;

/// Every PCB in the document: `PcbBoard` nodes first, then the legacy
/// top-level `pcb` field if present.
fn extract_pcbs(doc: &Document) -> Vec<&Pcb> {
    let mut out: Vec<&Pcb> = Vec::new();
    let mut node_ids: Vec<_> = doc.nodes.keys().collect();
    node_ids.sort();
    for id in node_ids {
        if let Some(node) = doc.nodes.get(id) {
            if let CsgOp::PcbBoard { board } = &node.op {
                out.push(board);
            }
        }
    }
    if let Some(pcb) = &doc.pcb {
        out.push(pcb);
    }
    out
}

/// Shared preamble: parsed document with at least one PCB, else the
/// fail-closed outcome.
fn require_pcbs(snapshot: &EvalSnapshot) -> Result<(&Document, Vec<&Pcb>), (CheckOutcome, Value)> {
    let Some(doc) = snapshot.doc.as_ref() else {
        return Err((
            CheckOutcome::Fail,
            json!({ "reason": "document failed to parse", "error": snapshot.fatal }),
        ));
    };
    let pcbs = extract_pcbs(doc);
    if pcbs.is_empty() {
        return Err((
            CheckOutcome::Fail,
            json!({ "reason": "no PCB in document (fail-closed: drc/board checks require a PcbBoard node)" }),
        ));
    }
    Ok((doc, pcbs))
}

fn violation_json(v: &vcad_ecad_pcb::drc::DrcViolation) -> Value {
    json!({
        "rule": format!("{:?}", v.rule),
        "severity": format!("{:?}", v.severity),
        "position": [v.position.x, v.position.y],
        "message": v.message,
        "actual": v.actual,
        "required": v.required,
    })
}

/// Cap the per-check forensic detail so run blobs stay readable.
const MAX_REPORTED: usize = 25;

/// `drc_clean` — full design-rule check across every board in the document.
pub fn check_drc_clean(snapshot: &EvalSnapshot) -> (CheckOutcome, Value) {
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let mut violations = Vec::new();
    for pcb in &pcbs {
        violations.extend(check_drc(pcb));
    }
    let outcome = if violations.is_empty() {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    let detail = json!({
        "boards": pcbs.len(),
        "violations": violations.len(),
        "sample": violations.iter().take(MAX_REPORTED).map(violation_json).collect::<Vec<_>>(),
    });
    (outcome, detail)
}

/// `nets_fully_connected` — the connectivity subset of DRC: no net islands,
/// no unconnected terminals, no shorts between nets. The ECAD analogue of
/// `valid_solid`.
pub fn check_nets_fully_connected(snapshot: &EvalSnapshot) -> (CheckOutcome, Value) {
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let connectivity = |r: &DrcRuleType| {
        matches!(
            r,
            DrcRuleType::NetIslands | DrcRuleType::UnconnectedNet | DrcRuleType::Short
        )
    };
    let mut violations = Vec::new();
    for pcb in &pcbs {
        violations.extend(check_drc(pcb).into_iter().filter(|v| connectivity(&v.rule)));
    }
    let outcome = if violations.is_empty() {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    let detail = json!({
        "boards": pcbs.len(),
        "connectivity_violations": violations.len(),
        "sample": violations.iter().take(MAX_REPORTED).map(violation_json).collect::<Vec<_>>(),
    });
    (outcome, detail)
}

/// `board_envelope` — outline bbox of every board fits inside `max_mm`.
pub fn check_board_envelope(snapshot: &EvalSnapshot, max_mm: [f64; 2]) -> (CheckOutcome, Value) {
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let mut sizes = Vec::new();
    let mut ok = true;
    for pcb in &pcbs {
        let vs = &pcb.outline.vertices;
        if vs.is_empty() {
            ok = false;
            sizes.push(json!({ "error": "empty outline" }));
            continue;
        }
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for v in vs {
            min_x = min_x.min(v.x);
            min_y = min_y.min(v.y);
            max_x = max_x.max(v.x);
            max_y = max_y.max(v.y);
        }
        let (w, h) = (max_x - min_x, max_y - min_y);
        const EPS: f64 = 1e-6;
        if w > max_mm[0] + EPS || h > max_mm[1] + EPS {
            ok = false;
        }
        sizes.push(json!({ "width_mm": w, "height_mm": h }));
    }
    let outcome = if ok {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    (outcome, json!({ "max_mm": max_mm, "boards": sizes }))
}

/// `component_count` — at least `min` placed footprints across all boards.
pub fn check_component_count(snapshot: &EvalSnapshot, min: usize) -> (CheckOutcome, Value) {
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let count: usize = pcbs.iter().map(|p| p.footprints.len()).sum();
    let outcome = if count >= min {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    (
        outcome,
        json!({ "footprints": count, "min": min, "boards": pcbs.len() }),
    )
}

/// `erc_clean` — electrical-rule check on the document's schematic.
pub fn check_erc_clean(snapshot: &EvalSnapshot) -> (CheckOutcome, Value) {
    let Some(doc) = snapshot.doc.as_ref() else {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "document failed to parse", "error": snapshot.fatal }),
        );
    };
    let Some(sheet) = doc.schematic.as_ref() else {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no schematic in document (fail-closed: erc_clean requires document.schematic)" }),
        );
    };
    let violations = check_erc(sheet);
    let outcome = if violations.is_empty() {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    let detail = json!({
        "violations": violations.len(),
        "sample": violations
            .iter()
            .take(MAX_REPORTED)
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>(),
    });
    (outcome, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluate_vcad;
    use vcad_ir::ecad::{
        BoardOutline, DesignRules, LayerStackup, Net, NetClassRules, PcbLayer, StackupLayer, Trace,
    };
    use vcad_ir::Vec2;

    fn minimal_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".into()),
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
            },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn doc_with_pcb(pcb: Pcb) -> EvalSnapshot {
        let doc_json = json!({
            "version": "0.1",
            "nodes": {
                "1": { "id": 1, "name": "board", "op": { "type": "PcbBoard", "board": pcb } }
            },
            "materials": {},
            "part_materials": {},
            "roots": []
        });
        evaluate_vcad(&serde_json::to_string(&doc_json).unwrap())
    }

    #[test]
    fn no_pcb_fails_closed() {
        // DEFAULT_CUBE: a plain solid, no board, no schematic. Every ECAD
        // check must fail, not pass vacuously.
        let snap = evaluate_vcad(
            r#"{"version":"0.1","nodes":{"1":{"id":1,"name":"c","op":{"type":"Cube","size":{"x":50,"y":50,"z":50}}}},"materials":{},"part_materials":{},"roots":[{"root":1,"material":"default"}]}"#,
        );
        assert_eq!(check_drc_clean(&snap).0, CheckOutcome::Fail);
        assert_eq!(check_nets_fully_connected(&snap).0, CheckOutcome::Fail);
        assert_eq!(
            check_board_envelope(&snap, [100.0, 100.0]).0,
            CheckOutcome::Fail
        );
        assert_eq!(check_component_count(&snap, 1).0, CheckOutcome::Fail);
        assert_eq!(check_erc_clean(&snap).0, CheckOutcome::Fail);
    }

    #[test]
    fn bare_board_passes_drc_and_envelope() {
        let snap = doc_with_pcb(minimal_pcb());
        let (out, detail) = check_drc_clean(&snap);
        assert_eq!(out, CheckOutcome::Pass, "detail: {detail}");
        assert_eq!(check_nets_fully_connected(&snap).0, CheckOutcome::Pass);
        assert_eq!(
            check_board_envelope(&snap, [60.0, 40.0]).0,
            CheckOutcome::Pass
        );
        // Envelope tighter than the 50×30 outline → fail.
        assert_eq!(
            check_board_envelope(&snap, [40.0, 40.0]).0,
            CheckOutcome::Fail
        );
        // No footprints → component floor fails.
        assert_eq!(check_component_count(&snap, 1).0, CheckOutcome::Fail);
        assert_eq!(check_component_count(&snap, 0).0, CheckOutcome::Pass);
    }

    #[test]
    fn split_net_fails_connectivity() {
        // One net, two traces that never touch → NetIslands. The flood-fill
        // villain's mirror image: declared connectivity that copper doesn't
        // realize must fail nets_fully_connected.
        let mut pcb = minimal_pcb();
        pcb.nets = vec![Net {
            id: "SIG".into(),
            name: "SIG".into(),
        }];
        pcb.traces = vec![
            Trace {
                net: "SIG".into(),
                layer: PcbLayer::FCu,
                start: Vec2::new(5.0, 5.0),
                end: Vec2::new(10.0, 5.0),
                width: 0.25,
                source: None,
            },
            Trace {
                net: "SIG".into(),
                layer: PcbLayer::FCu,
                start: Vec2::new(30.0, 5.0),
                end: Vec2::new(35.0, 5.0),
                width: 0.25,
                source: None,
            },
        ];
        let snap = doc_with_pcb(pcb);
        let (out, detail) = check_nets_fully_connected(&snap);
        assert_eq!(out, CheckOutcome::Fail, "detail: {detail}");
    }
}

/// `decoupling_proximity` — every qualifying IC power pad has a two-pad
/// capacitor bridging its power net to ground within `max_mm`.
pub fn check_decoupling_proximity(
    snapshot: &EvalSnapshot,
    power_nets: &[String],
    ground_nets: &[String],
    max_mm: f64,
    min_ic_pads: usize,
) -> (CheckOutcome, Value) {
    use vcad_ecad_pcb::geometry::pad_world_position;
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let is_power = |n: &str| power_nets.iter().any(|p| p.eq_ignore_ascii_case(n));
    let is_ground = |n: &str| ground_nets.iter().any(|p| p.eq_ignore_ascii_case(n));

    let mut findings = Vec::new();
    let mut ic_pads_checked = 0usize;
    let mut ok = true;
    for pcb in &pcbs {
        // Candidate decoupling caps: 2-pad footprints bridging power→ground.
        let caps: Vec<_> = pcb
            .footprints
            .iter()
            .filter(|f| f.pads.len() == 2)
            .filter_map(|f| {
                let net = |i: usize| f.pads[i].net.as_deref().unwrap_or("");
                let (p, g) = (net(0), net(1));
                if is_power(p) && is_ground(g) {
                    Some((f, 0usize, p.to_string()))
                } else if is_power(g) && is_ground(p) {
                    Some((f, 1usize, g.to_string()))
                } else {
                    None
                }
            })
            .collect();
        for ic in pcb
            .footprints
            .iter()
            .filter(|f| f.pads.len() >= min_ic_pads)
        {
            for pad in &ic.pads {
                let Some(net) = pad.net.as_deref() else {
                    continue;
                };
                if !is_power(net) {
                    continue;
                }
                ic_pads_checked += 1;
                let ppos = pad_world_position(ic, pad);
                let best = caps
                    .iter()
                    .filter(|(_, _, cnet)| cnet.eq_ignore_ascii_case(net))
                    .map(|(f, pi, _)| {
                        let cpos = pad_world_position(f, &f.pads[*pi]);
                        (
                            ((cpos.x - ppos.x).powi(2) + (cpos.y - ppos.y).powi(2)).sqrt(),
                            f.reference.clone(),
                        )
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                let (dist, cap_ref) = match best {
                    Some((d, r)) => (d, r),
                    None => (f64::INFINITY, "-".to_string()),
                };
                if dist > max_mm {
                    ok = false;
                }
                findings.push(json!({
                    "ic": ic.reference, "pad": pad.number, "net": net,
                    "nearest_cap": cap_ref,
                    "distance_mm": if dist.is_finite() { json!(dist) } else { json!(null) },
                    "ok": dist <= max_mm,
                }));
            }
        }
    }
    if ic_pads_checked == 0 {
        return (
            CheckOutcome::Fail,
            json!({ "reason": format!(
                "no IC (footprint with ≥{min_ic_pads} pads) has a pad on a named power net — fail-closed"
            ) }),
        );
    }
    let outcome = if ok {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    (
        outcome,
        json!({ "max_mm": max_mm, "ic_power_pads": ic_pads_checked, "findings": findings }),
    )
}

/// `fab_ready` — DRC clean AND Gerber serialization of every board
/// succeeds. The pcbeval P5 gate.
pub fn check_fab_ready(snapshot: &EvalSnapshot) -> (CheckOutcome, Value) {
    let (drc_outcome, drc_detail) = check_drc_clean(snapshot);
    if drc_outcome != CheckOutcome::Pass {
        return (drc_outcome, json!({ "stage": "drc", "detail": drc_detail }));
    }
    let (_, pcbs) = match require_pcbs(snapshot) {
        Ok(v) => v,
        Err(out) => return out,
    };
    let mut layers_total = 0usize;
    for pcb in &pcbs {
        match vcad_ecad_export::gerber::generate_gerbers(pcb) {
            Ok(files) => {
                if files.is_empty() {
                    return (
                        CheckOutcome::Fail,
                        json!({ "stage": "gerber", "reason": "gerber generation produced no layers" }),
                    );
                }
                layers_total += files.len();
            }
            Err(e) => {
                return (
                    CheckOutcome::Fail,
                    json!({ "stage": "gerber", "reason": e.to_string() }),
                );
            }
        }
    }
    (
        CheckOutcome::Pass,
        json!({ "drc": "clean", "gerber_layers": layers_total, "boards": pcbs.len() }),
    )
}

/// `netlist_isomorphic` — candidate schematic vs the grader-only golden.
pub fn check_netlist_isomorphic(
    snapshot: &EvalSnapshot,
    golden_raw: Option<&Result<String, String>>,
) -> (CheckOutcome, Value) {
    use crate::netlist;
    let Some(golden_raw) = golden_raw else {
        return (
            CheckOutcome::Error,
            json!({ "reason": "task has a netlist_isomorphic check but no golden_netlist input" }),
        );
    };
    let raw = match golden_raw {
        Ok(r) => r,
        Err(e) => {
            return (
                CheckOutcome::Error,
                json!({ "reason": format!("golden netlist unreadable: {e}") }),
            )
        }
    };
    let golden: netlist::GoldenNetlist = match serde_json::from_str(raw) {
        Ok(g) => g,
        Err(e) => {
            return (
                CheckOutcome::Error,
                json!({ "reason": format!("golden netlist malformed: {e}") }),
            )
        }
    };
    let Some(doc) = snapshot.doc.as_ref() else {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "document failed to parse", "error": snapshot.fatal }),
        );
    };
    let Some(sheet) = doc.schematic.as_ref() else {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no schematic in document (fail-closed)" }),
        );
    };
    let Some(cand) = netlist::candidate_graph(sheet) else {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "schematic has no explicit netlist (sheet.nets) — fail-closed" }),
        );
    };
    let gold = netlist::golden_graph(&golden);
    let iso = netlist::isomorphic(&cand, &gold);
    let detail = json!({
        "isomorphic": iso,
        "candidate": { "components": cand.comps.len(), "nets": cand.net_count },
        "golden": { "components": gold.comps.len(), "nets": gold.net_count },
    });
    (
        if iso {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        detail,
    )
}
