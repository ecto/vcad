//! Round-trip: schematic (built the way `create_schematic` does, with
//! connectivity declared as data in the explicit `nets` map) → netlist →
//! [`Circuit`] → DC operating point and AC response, asserted against the
//! same closed forms the validation ladder uses.

use std::collections::BTreeMap;

use vcad_ecad_sim::circuit::netlist::{
    circuit_from_schematic, BlockerReason, ConvertError, ConvertOptions,
};
use vcad_ecad_sim::circuit::{ac, dc, Device};
use vcad_ir::ecad::{PinType, SchematicComponent, SchematicPin, SchematicSheet};
use vcad_ir::Vec2;

fn two_pin(reference: &str, value: &str, pins: [&str; 2]) -> SchematicComponent {
    // Spread components far apart so connectivity comes only from the
    // explicit `nets` map, never from accidental coordinate coincidence.
    let spread: f64 = reference
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
        as f64
        % 10_000.0;
    SchematicComponent {
        reference: reference.to_string(),
        value: value.to_string(),
        footprint_id: String::new(),
        position: Vec2::new(spread * 100.0, spread),
        rotation: 0.0,
        mirror: false,
        pins: pins
            .iter()
            .enumerate()
            .map(|(i, num)| SchematicPin {
                number: num.to_string(),
                name: num.to_string(),
                pin_type: PinType::Passive,
                position: Vec2::new(i as f64 * 10.0, 0.0),
            })
            .collect(),
        pads_override: None,
        properties: std::collections::HashMap::new(),
    }
}

/// Voltage divider + RC filter off the tap:
///
/// V1(5V) — VIN —[R1 7.5k]— MID —[R2 2.5k]— GND, and MID —[R3 1k]— OUT
/// —[C1 100n]— GND. At DC the cap is open: V(MID) = 5·2500/10000 = 1.25 V,
/// V(OUT) = V(MID). At AC (unit drive), the divider Thévenin feeds the RC:
/// H(jω) = k / (1 + jω(R3 + R1∥R2)C).
fn divider_rc_sheet() -> SchematicSheet {
    let mut nets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    nets.insert("VIN".into(), vec!["V1.1".into(), "R1.1".into()]);
    nets.insert(
        "MID".into(),
        vec!["R1.2".into(), "R2.1".into(), "R3.1".into()],
    );
    nets.insert("OUT".into(), vec!["R3.2".into(), "C1.1".into()]);
    nets.insert(
        "GND".into(),
        vec!["V1.2".into(), "R2.2".into(), "C1.2".into()],
    );
    SchematicSheet {
        nets: Some(nets),
        title: Some("divider + RC".into()),
        components: vec![
            two_pin("V1", "5", ["1", "2"]),
            two_pin("R1", "7.5k", ["1", "2"]),
            two_pin("R2", "2k5", ["1", "2"]),
            two_pin("R3", "1k", ["1", "2"]),
            two_pin("C1", "100n", ["1", "2"]),
        ],
        wires: vec![],
        junctions: vec![],
        labels: vec![],
    }
}

#[test]
fn roundtrip_dc_matches_divider_closed_form() {
    let mapped = circuit_from_schematic(&divider_rc_sheet(), &ConvertOptions::default()).unwrap();
    assert_eq!(mapped.ground_nets, vec!["GND".to_string()]);
    assert_eq!(mapped.node_of_net["GND"], 0);

    let sol = dc::operating_point(&mapped.circuit).unwrap();
    let v_mid = sol.node_voltages[mapped.node_of_net["MID"]];
    let v_out = sol.node_voltages[mapped.node_of_net["OUT"]];
    let expect = 5.0 * 2_500.0 / (7_500.0 + 2_500.0);
    assert!((v_mid - expect).abs() < 1e-12, "V(MID) = {v_mid}");
    // Cap is open at DC: no current through R3, so OUT sits at MID.
    assert!((v_out - expect).abs() < 1e-12, "V(OUT) = {v_out}");
}

#[test]
fn roundtrip_ac_matches_rc_closed_form() {
    let mapped = circuit_from_schematic(&divider_rc_sheet(), &ConvertOptions::default()).unwrap();
    let src = mapped.device_of_ref["V1"];
    assert!(matches!(
        mapped.circuit.devices[src],
        Device::VSource { .. }
    ));

    // Thévenin looking back from the cap: k = R2/(R1+R2), Rth = R3 + R1∥R2.
    let (r1, r2, r3, c) = (7_500.0, 2_500.0, 1_000.0, 100e-9);
    let k = r2 / (r1 + r2);
    let rth = r3 + r1 * r2 / (r1 + r2);
    let omega = 1.0 / (rth * c); // corner: |H| = k/√2, phase −45°
    let sol = ac::ac_response(&mapped.circuit, src, omega).unwrap();
    let h = sol.node_voltages[mapped.node_of_net["OUT"]];
    assert!(
        (h.abs() - k / 2f64.sqrt()).abs() < 1e-12,
        "|H| = {} want {}",
        h.abs(),
        k / 2f64.sqrt()
    );
    assert!(
        (h.arg() + std::f64::consts::FRAC_PI_4).abs() < 1e-12,
        "arg H = {}",
        h.arg()
    );
}

#[test]
fn unmappable_components_fail_closed_with_full_list() {
    let mut sheet = divider_rc_sheet();
    sheet.components.push(two_pin("U1", "LM358", ["1", "2"]));
    sheet.components.push(two_pin("J1", "conn", ["1", "2"]));
    sheet.components.push(two_pin("R9", "garbage", ["1", "2"]));

    let err = circuit_from_schematic(&sheet, &ConvertOptions::default()).unwrap_err();
    let ConvertError::Unmappable { blockers } = err else {
        panic!("expected Unmappable, got {err:?}");
    };
    // All three blockers reported, not just the first.
    assert_eq!(blockers.len(), 3);
    assert!(blockers
        .iter()
        .any(|b| b.reference == "U1" && matches!(b.reason, BlockerReason::UnknownKind { .. })));
    assert!(blockers
        .iter()
        .any(|b| b.reference == "J1" && matches!(b.reason, BlockerReason::UnknownKind { .. })));
    assert!(blockers
        .iter()
        .any(|b| b.reference == "R9" && matches!(b.reason, BlockerReason::BadValue { .. })));
}

#[test]
fn stub_allowlist_opens_named_components() {
    let mut sheet = divider_rc_sheet();
    sheet.components.push(two_pin("J1", "conn", ["1", "2"]));
    let options = ConvertOptions {
        stub_as_open: vec!["J1".into()],
    };
    let mapped = circuit_from_schematic(&sheet, &options).unwrap();
    assert_eq!(mapped.stubbed, vec!["J1".to_string()]);
    // The rest of the circuit still simulates.
    let sol = dc::operating_point(&mapped.circuit).unwrap();
    assert!((sol.node_voltages[mapped.node_of_net["MID"]] - 1.25).abs() < 1e-12);
}

#[test]
fn stub_typo_is_rejected() {
    let options = ConvertOptions {
        stub_as_open: vec!["J9".into()],
    };
    let err = circuit_from_schematic(&divider_rc_sheet(), &options).unwrap_err();
    assert!(matches!(err, ConvertError::UnknownStub(ref v) if v == &vec!["J9".to_string()]));
}

#[test]
fn diode_maps_via_anode_cathode_pins() {
    let mut nets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    nets.insert("VIN".into(), vec!["V1.1".into(), "D1.A".into()]);
    nets.insert("MID".into(), vec!["D1.K".into(), "R1.1".into()]);
    nets.insert("GND".into(), vec!["V1.2".into(), "R1.2".into()]);
    let sheet = SchematicSheet {
        nets: Some(nets),
        title: None,
        components: vec![
            two_pin("V1", "5", ["1", "2"]),
            two_pin("D1", "1N4148", ["A", "K"]),
            two_pin("R1", "1k", ["1", "2"]),
        ],
        wires: vec![],
        junctions: vec![],
        labels: vec![],
    };
    let mapped = circuit_from_schematic(&sheet, &ConvertOptions::default()).unwrap();
    let sol = dc::operating_point(&mapped.circuit).unwrap();
    let v_mid = sol.node_voltages[mapped.node_of_net["MID"]];
    // Forward-biased silicon diode: MID ≈ 5 − ~0.6..0.8 V drop.
    assert!(
        (4.0..5.0).contains(&v_mid),
        "diode drop looks wrong: V(MID) = {v_mid}"
    );
}
