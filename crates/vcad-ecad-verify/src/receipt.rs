//! Building and re-running verification [`Receipt`]s.
//!
//! A receipt hashes the DRC-relevant board geometry and records a
//! canonicalized DRC summary, so it can be re-run later against the current
//! board. Geometric/DRC drift ([`ReceiptStatus::Stale`]/`Violated`) is reported
//! separately from sourcing drift — a price change never reads as an electrical
//! failure.

use std::collections::BTreeSet;

use vcad_ecad_pcb::analyze_power_integrity;
use vcad_ecad_pcb::drc::{check_drc, DrcViolation};
use vcad_ir::ecad::{
    DrcSummary, PartReceiptLine, Pcb, PowerIntegrityLine, Receipt, ReceiptStatus, RuleCount,
    SourcingSnapshot,
};

/// FNV-1a 64-bit hash of a string, hex-encoded — deterministic and
/// dependency-free, so a receipt's hash is stable across runs and machines.
fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce4_84222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn r(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

/// A canonical, order-stable signature of the DRC-relevant board geometry.
fn canonical_board(pcb: &Pcb) -> String {
    let mut s = String::new();
    s.push_str(&format!("outline:{};", r(pcb.outline.thickness)));
    for v in &pcb.outline.vertices {
        s.push_str(&format!("{},{};", r(v.x), r(v.y)));
    }
    let mut fps: Vec<String> = pcb
        .footprints
        .iter()
        .map(|f| {
            let mut pads: Vec<String> = f
                .pads
                .iter()
                .map(|p| {
                    format!(
                        "{}:{},{}:{}",
                        p.number,
                        r(p.position.x),
                        r(p.position.y),
                        p.net.as_deref().unwrap_or("")
                    )
                })
                .collect();
            pads.sort();
            format!(
                "{}|{}|{},{}|{}|{}",
                f.reference,
                f.footprint_name,
                r(f.position.x),
                r(f.position.y),
                r(f.rotation),
                pads.join(",")
            )
        })
        .collect();
    fps.sort();
    s.push_str(&fps.join(";"));
    let mut routes: Vec<String> = pcb
        .traces
        .iter()
        .map(|t| {
            format!(
                "T{},{}-{},{}:{}:{}",
                r(t.start.x),
                r(t.start.y),
                r(t.end.x),
                r(t.end.y),
                r(t.width),
                t.net
            )
        })
        .chain(
            pcb.vias
                .iter()
                .map(|v| format!("V{},{}:{}", r(v.position.x), r(v.position.y), v.net)),
        )
        .collect();
    routes.sort();
    s.push_str(&routes.join(";"));
    s
}

fn canonical_rules(pcb: &Pcb) -> String {
    let d = &pcb.rules;
    format!(
        "{}/{}/{}/{}/{}/{}/{}",
        r(d.default_rules.trace_width),
        r(d.default_rules.clearance),
        r(d.default_rules.via_diameter),
        r(d.default_rules.via_drill),
        r(d.edge_clearance),
        r(d.hole_to_hole),
        r(d.min_drill),
    )
}

/// Canonical key for a violation: `rule|message|x|y` (rounded).
fn vkey(v: &DrcViolation) -> String {
    format!(
        "{:?}|{}|{}|{}",
        v.rule,
        v.message,
        r(v.position.x),
        r(v.position.y)
    )
}

fn summarize(violations: &[DrcViolation]) -> DrcSummary {
    let mut keys: Vec<String> = violations.iter().map(vkey).collect();
    keys.sort();
    keys.dedup();
    // Per-rule counts.
    let mut by: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for v in violations {
        *by.entry(format!("{:?}", v.rule)).or_insert(0) += 1;
    }
    let by_rule = by
        .into_iter()
        .map(|(rule, count)| RuleCount { rule, count })
        .collect();
    DrcSummary {
        total: violations.len() as u32,
        by_rule,
        violations: keys,
    }
}

/// Build a re-runnable receipt for the current board state.
///
/// Per-part MPNs are read from each footprint's `properties["mpn"]` when
/// present. `sourcing` is attached verbatim (the optional, amputable leaf).
pub fn build_receipt(pcb: &Pcb, sourcing: Option<SourcingSnapshot>) -> Receipt {
    let drc = summarize(&check_drc(pcb));
    let parts = pcb
        .footprints
        .iter()
        .map(|f| PartReceiptLine {
            reference: f.reference.clone(),
            footprint: f.footprint_name.clone(),
            value: f.value.clone(),
            mpn: f.properties.get("mpn").cloned(),
        })
        .collect();
    // Realized-copper continuity for power/plane nets, so the durable proof
    // records whether each plane is electrically continuous — a closed-form
    // PASS never gets to imply a sound plane that isn't there.
    let power_integrity = analyze_power_integrity(pcb)
        .into_iter()
        .map(|c| PowerIntegrityLine {
            net: c.net,
            islands: c.islands as u32,
            continuous: c.continuous,
            coverage: c.coverage,
            connected_pads: c.connected_pads as u32,
            total_pads: c.total_pads as u32,
            vias: c.vias as u32,
        })
        .collect();
    Receipt {
        board_hash: fnv1a(&canonical_board(pcb)),
        design_rules_hash: fnv1a(&canonical_rules(pcb)),
        drc_backend: format!("vcad-ecad-pcb {}", env!("CARGO_PKG_VERSION")),
        drc,
        power_integrity,
        parts,
        sourcing,
    }
}

/// Re-run a receipt against the current board.
///
/// - `Violated` — new DRC violations exist that the receipt did not record.
/// - `Stale` — the board geometry changed but no new violations appeared.
/// - `Holds` — board unchanged and DRC result matches.
///
/// Sourcing drift is intentionally **not** consulted: only geometry/DRC affects
/// the electrical verdict.
pub fn verify_receipt(pcb: &Pcb, receipt: &Receipt) -> ReceiptStatus {
    let current = check_drc(pcb);
    let current_keys: BTreeSet<String> = current.iter().map(vkey).collect();
    let receipt_keys: BTreeSet<String> = receipt.drc.violations.iter().cloned().collect();

    let has_regression = current_keys.difference(&receipt_keys).next().is_some();
    if has_regression {
        return ReceiptStatus::Violated;
    }
    if fnv1a(&canonical_board(pcb)) != receipt.board_hash {
        return ReceiptStatus::Stale;
    }
    ReceiptStatus::Holds
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ecad_parts::resolve;
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;

    fn board_with(parts: Vec<Footprint>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 50.0),
                    Vec2::new(0.0, 50.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                }],
            },
            nets: vec![
                Net {
                    id: "1".into(),
                    name: "N1".into(),
                },
                Net {
                    id: "2".into(),
                    name: "N2".into(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.2,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.3,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: parts,
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn place(value_pkg: &str, reference: &str, at: Vec2) -> Footprint {
        let p = resolve(value_pkg).unwrap();
        let t = &p.derived.footprint;
        let mut fp = Footprint {
            reference: reference.into(),
            value: p.value.clone(),
            footprint_name: t.name.clone(),
            position: at,
            rotation: 0.0,
            front: true,
            pads: t.pads.clone(),
            graphics: t.graphics.clone(),
            model_3d: None,
            properties: std::collections::HashMap::new(),
        };
        for (i, pad) in fp.pads.iter_mut().enumerate() {
            pad.net = Some(((i % 2) + 1).to_string());
        }
        fp
    }

    #[test]
    fn receipt_holds_on_unchanged_board() {
        let pcb = board_with(vec![place("10k 0603", "R1", Vec2::new(25.0, 25.0))]);
        let rcpt = build_receipt(&pcb, None);
        assert_eq!(verify_receipt(&pcb, &rcpt), ReceiptStatus::Holds);
    }

    #[test]
    fn receipt_goes_stale_when_board_moves() {
        let pcb = board_with(vec![place("10k 0603", "R1", Vec2::new(25.0, 25.0))]);
        let rcpt = build_receipt(&pcb, None);
        // Move the part — geometry changes, but still no DRC violations.
        let moved = board_with(vec![place("10k 0603", "R1", Vec2::new(30.0, 30.0))]);
        assert_eq!(verify_receipt(&moved, &rcpt), ReceiptStatus::Stale);
    }

    #[test]
    fn receipt_violated_on_new_collision() {
        let pcb = board_with(vec![place("10k 0402", "R1", Vec2::new(25.0, 25.0))]);
        let rcpt = build_receipt(&pcb, None);
        assert_eq!(rcpt.drc.total, 0, "clean board to start");
        // Add a colliding part → a new CourtyardOverlap the receipt never saw.
        let collided = board_with(vec![
            place("10k 0402", "R1", Vec2::new(25.0, 25.0)),
            place("10k 2512", "R2", Vec2::new(25.5, 25.0)),
        ]);
        assert_eq!(verify_receipt(&collided, &rcpt), ReceiptStatus::Violated);
    }

    #[test]
    fn receipt_records_part_provenance_and_backend() {
        let pcb = board_with(vec![place("10k 0603", "R1", Vec2::new(25.0, 25.0))]);
        let rcpt = build_receipt(&pcb, None);
        assert_eq!(rcpt.parts.len(), 1);
        assert_eq!(rcpt.parts[0].reference, "R1");
        assert!(rcpt.drc_backend.starts_with("vcad-ecad-pcb"));
        assert!(!rcpt.board_hash.is_empty());
    }

    #[test]
    fn sourcing_drift_does_not_change_drc_verdict() {
        let pcb = board_with(vec![place("10k 0603", "R1", Vec2::new(25.0, 25.0))]);
        // Two receipts, identical board, different sourcing snapshots.
        let with_src = build_receipt(
            &pcb,
            Some(SourcingSnapshot {
                lines: vec![SourcingLine {
                    mpn: "RC0603FR-0710KL".into(),
                    stock: Some(100_000),
                    unit_price: Some(0.002),
                    currency: Some("USD".into()),
                }],
            }),
        );
        // Verdict depends only on geometry/DRC, not the sourcing snapshot.
        assert_eq!(verify_receipt(&pcb, &with_src), ReceiptStatus::Holds);
    }

    /// The receipt records realized-copper continuity for power/plane nets, so a
    /// fragmented ground plane is durable proof — not implied by a clean DRC.
    #[test]
    fn receipt_records_fragmented_power_plane() {
        let mut pcb = board_with(vec![]);
        pcb.nets.push(Net {
            id: "GND".into(),
            name: "GND".into(),
        });
        let zone = |v: Vec<Vec2>| Zone {
            outline: v,
            holes: vec![],
            net: "GND".into(),
            layer: PcbLayer::FCu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        };
        // Two non-touching GND pours → an electrically split plane.
        pcb.zones = vec![
            zone(vec![
                Vec2::new(2.0, 2.0),
                Vec2::new(12.0, 2.0),
                Vec2::new(12.0, 12.0),
                Vec2::new(2.0, 12.0),
            ]),
            zone(vec![
                Vec2::new(30.0, 30.0),
                Vec2::new(45.0, 30.0),
                Vec2::new(45.0, 45.0),
                Vec2::new(30.0, 45.0),
            ]),
        ];
        let rcpt = build_receipt(&pcb, None);
        let gnd = rcpt
            .power_integrity
            .iter()
            .find(|p| p.net == "GND")
            .expect("GND continuity recorded in receipt");
        assert_eq!(gnd.islands, 2);
        assert!(!gnd.continuous);
    }
}
