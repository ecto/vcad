//! A KiCad THT pad (`(layers "*.Cu" "*.Mask")`) must import onto every copper
//! layer of the board so DRC connectivity sees it. The importer used to drop
//! the wildcard, leaving `layers == []`, and DRC skipped the pad entirely.
use vcad_ecad_pcb::drc::{check_drc, DrcRuleType};
use vcad_ecad_symbols::parse_kicad_pcb;

const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator test)
  (general (thickness 1.6))
  (layers
    (0 "F.Cu" signal)
    (1 "In1.Cu" signal)
    (2 "In2.Cu" signal)
    (31 "B.Cu" signal)
    (38 "B.Mask" user)
    (39 "F.Mask" user)
    (44 "Edge.Cuts" user)
  )
  (net 0 "")
  (net 1 "SIG")
  (gr_line (start 0 0) (end 40 0) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 40 0) (end 40 20) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 40 20) (end 0 20) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 0 20) (end 0 0) (layer "Edge.Cuts") (width 0.05))
  (footprint "Conn_1x01" (layer "F.Cu") (at 10 10)
    (property "Reference" "J1")
    (pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SIG"))
  )
  (footprint "Conn_1x01" (layer "F.Cu") (at 30 10)
    (property "Reference" "J2")
    (pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SIG"))
  )
  (segment (start 10 10) (end 30 10) (width 0.25) (layer "In2.Cu") (net 1))
)"#;

fn connectivity(pcb: &vcad_ir::ecad::Pcb) -> Vec<String> {
    check_drc(pcb)
        .into_iter()
        .filter(|v| {
            matches!(
                v.rule,
                DrcRuleType::UnconnectedNet | DrcRuleType::NetIslands
            )
        })
        .map(|v| v.message)
        .collect()
}

#[test]
fn tht_pads_joined_by_inner_trace_are_connected() {
    let pcb = parse_kicad_pcb(BOARD).unwrap();
    assert!(pcb.footprints.iter().all(|f| f.pads[0].layers.len() == 6));
    let v = connectivity(&pcb);
    assert!(v.is_empty(), "THT pads should connect via In2.Cu: {v:?}");

    // Control: pads on outer copper only never meet the In2.Cu trace, so the
    // net reads as split — proves the inner-layer membership is what joins it.
    // (With `layers == []`, the old import, DRC skipped the pads outright and
    // could not even report the net.)
    let mut outer_only = pcb.clone();
    for fp in &mut outer_only.footprints {
        fp.pads[0].layers = vec![vcad_ir::ecad::PcbLayer::FCu, vcad_ir::ecad::PcbLayer::BCu];
    }
    assert!(!connectivity(&outer_only).is_empty());
}
