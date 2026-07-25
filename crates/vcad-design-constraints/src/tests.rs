//! Unit tests for the document-level constraint solver.

use std::collections::HashMap;

use vcad_ir::constraints::{Anchor, ConstraintKind, DesignConstraint};
use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, NetClassRules, Pcb, PcbLayer, StackupLayer,
};
use vcad_ir::parameters::{Expr, Parameter};
use vcad_ir::{CsgOp, Document, Node, Vec2};

use crate::{
    check_design_constraints, solve_design_constraints, AnchorResolver, NoPartAnchors, SolveOptions,
};

const BOARD: u64 = 1;

fn footprint(reference: &str, x: f64, y: f64) -> Footprint {
    Footprint {
        reference: reference.to_string(),
        value: String::new(),
        footprint_name: "R_0805".to_string(),
        position: Vec2::new(x, y),
        rotation: 0.0,
        front: true,
        pads: vec![],
        graphics: vec![],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn board_doc(footprints: Vec<Footprint>) -> Document {
    let pcb = Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(100.0, 80.0),
                Vec2::new(0.0, 80.0),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: vec![StackupLayer {
                layer: PcbLayer::FCu,
                copper_thickness: Some(0.035),
                dielectric_thickness: None,
                dielectric_er: None,
                material: None,
            }],
        },
        nets: vec![],
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "default".to_string(),
                trace_width: 0.25,
                clearance: 0.2,
                via_diameter: 0.6,
                via_drill: 0.3,
                diff_pair_gap: None,
                diff_pair_width: None,
                target_impedance: None,
                target_diff_impedance: None,
            },
            class_rules: vec![],
            net_class_assignments: HashMap::new(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.3,
        },
        footprints,
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    };
    let mut doc = Document::new();
    doc.nodes.insert(
        BOARD,
        Node {
            id: BOARD,
            name: Some("board".to_string()),
            op: CsgOp::PcbBoard {
                board: Box::new(pcb),
            },
        },
    );
    doc
}

fn fp_anchor(r: &str) -> Anchor {
    Anchor::PcbFootprint {
        node: BOARD,
        r#ref: r.to_string(),
        pad: None,
    }
}

fn constraint(id: &str, kind: ConstraintKind) -> DesignConstraint {
    DesignConstraint {
        id: id.to_string(),
        label: None,
        kind,
        driven: false,
    }
}

fn board_pcb(doc: &Document) -> &Pcb {
    match &doc.nodes[&BOARD].op {
        CsgOp::PcbBoard { board } => board,
        _ => panic!("not a board"),
    }
}

#[test]
fn distance_with_fixed_anchor_converges() {
    let mut doc = board_doc(vec![
        footprint("U1", 10.0, 10.0),
        footprint("U2", 30.0, 10.0),
    ]);
    doc.constraints = vec![
        constraint("c1", ConstraintKind::Fixed { a: fp_anchor("U1") }),
        constraint(
            "c2",
            ConstraintKind::Distance {
                a: fp_anchor("U1"),
                b: fp_anchor("U2"),
                value: Expr::num(10.0),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    assert_eq!(report.moved_footprints, vec!["U2".to_string()]);
    let pcb = board_pcb(&doc);
    let u1 = &pcb.footprints[0].position;
    let u2 = &pcb.footprints[1].position;
    let d = ((u1.x - u2.x).powi(2) + (u1.y - u2.y).powi(2)).sqrt();
    assert!((d - 10.0).abs() < 1e-6, "distance = {d}");
    assert!((u1.x - 10.0).abs() < 1e-9, "U1 must not move");
}

#[test]
fn formula_dimension_resolves_parameters() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0), footprint("U2", 5.0, 0.0)]);
    doc.parameters
        .insert("spacing".to_string(), Parameter::literal(20.0));
    doc.constraints = vec![
        constraint("c1", ConstraintKind::Fixed { a: fp_anchor("U1") }),
        constraint(
            "c2",
            ConstraintKind::Distance {
                a: fp_anchor("U1"),
                b: fp_anchor("U2"),
                value: Expr::formula("spacing / 2 + 5"),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    let pcb = board_pcb(&doc);
    let u2 = &pcb.footprints[1].position;
    let d = (u2.x.powi(2) + u2.y.powi(2)).sqrt();
    assert!((d - 15.0).abs() < 1e-6, "distance = {d}");
}

#[test]
fn outline_edge_length_solves_exact() {
    let mut doc = board_doc(vec![]);
    doc.constraints = vec![
        constraint(
            "c1",
            ConstraintKind::Fixed {
                a: Anchor::PcbOutlineVertex {
                    node: BOARD,
                    index: 0,
                },
            },
        ),
        constraint(
            "c2",
            ConstraintKind::Horizontal {
                a: Anchor::PcbOutlineEdge {
                    node: BOARD,
                    index: 0,
                },
                b: Anchor::PcbOutlineEdge {
                    node: BOARD,
                    index: 0,
                },
            },
        ),
        constraint(
            "c3",
            ConstraintKind::Length {
                a: Anchor::PcbOutlineEdge {
                    node: BOARD,
                    index: 0,
                },
                value: Expr::num(120.0),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    let pcb = board_pcb(&doc);
    let (v0, v1) = (pcb.outline.vertices[0], pcb.outline.vertices[1]);
    assert!((v0.x).abs() < 1e-9 && (v0.y).abs() < 1e-9, "v0 fixed");
    let len = ((v1.x - v0.x).powi(2) + (v1.y - v0.y).powi(2)).sqrt();
    assert!((len - 120.0).abs() < 1e-6, "edge length = {len}");
    assert!((v1.y - v0.y).abs() < 1e-6, "edge horizontal");
}

#[test]
fn rotation_constraint_sets_footprint_rotation() {
    let mut doc = board_doc(vec![footprint("J1", 10.0, 10.0)]);
    doc.constraints = vec![constraint(
        "c1",
        ConstraintKind::Rotation {
            node: BOARD,
            r#ref: "J1".to_string(),
            value: Expr::num(90.0),
        },
    )];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    let pcb = board_pcb(&doc);
    assert!((pcb.footprints[0].rotation - 90.0).abs() < 1e-6);
    assert_eq!(report.moved_footprints, vec!["J1".to_string()]);
}

#[test]
fn driven_dimension_is_measured_not_enforced() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0), footprint("U2", 30.0, 40.0)]);
    doc.constraints = vec![DesignConstraint {
        id: "c1".to_string(),
        label: None,
        kind: ConstraintKind::Distance {
            a: fp_anchor("U1"),
            b: fp_anchor("U2"),
            value: Expr::num(0.0), // stale; should be back-annotated to 50
        },
        driven: true,
    }];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged);
    // Nothing moved (no driving constraints).
    assert!(report.moved_footprints.is_empty());
    assert_eq!(report.driven_values.len(), 1);
    assert!((report.driven_values[0].value - 50.0).abs() < 1e-9);
    // Back-annotated into the document.
    assert_eq!(
        doc.constraints[0].kind.value().unwrap().as_number(),
        Some(50.0)
    );
}

#[test]
fn over_constrained_reports_negative_dof_without_panic() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0)]);
    doc.constraints = vec![
        constraint("c1", ConstraintKind::Fixed { a: fp_anchor("U1") }),
        constraint(
            "c2",
            ConstraintKind::HorizontalDistance {
                a: fp_anchor("U1"),
                value: Expr::num(50.0), // conflicts with Fixed at x=0
            },
        ),
        constraint(
            "c3",
            ConstraintKind::VerticalDistance {
                a: fp_anchor("U1"),
                value: Expr::num(50.0),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert_eq!(report.groups.len(), 1);
    assert!(report.groups[0].dof < 0, "dof = {}", report.groups[0].dof);
    // Conflicting targets: must not converge, and must not write back.
    assert!(!report.converged);
    let pcb = board_pcb(&doc);
    assert!((pcb.footprints[0].position.x).abs() < 1e-9);
}

#[test]
fn bad_reference_is_skipped_and_rest_solves() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0)]);
    doc.constraints = vec![
        constraint(
            "bad",
            ConstraintKind::Fixed {
                a: Anchor::PcbFootprint {
                    node: BOARD,
                    r#ref: "NOPE".to_string(),
                    pad: None,
                },
            },
        ),
        constraint(
            "good",
            ConstraintKind::HorizontalDistance {
                a: fp_anchor("U1"),
                value: Expr::num(25.0),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("NOPE"), "{:?}", report.errors);
    assert!(report.converged);
    assert!((board_pcb(&doc).footprints[0].position.x - 25.0).abs() < 1e-6);
}

#[test]
fn part_edge_anchor_pulls_footprint() {
    struct WallResolver;
    impl AnchorResolver for WallResolver {
        fn resolve_part_edge(
            &self,
            _node: vcad_ir::NodeId,
            face_a: &str,
            _face_b: &str,
        ) -> Result<([f64; 3], [f64; 3]), String> {
            if face_a == "enclosure:front" {
                // A vertical edge at x = 42 in world space.
                Ok(([42.0, 0.0, 0.0], [42.0, 80.0, 0.0]))
            } else {
                Err("lost".to_string())
            }
        }
    }
    let mut doc = board_doc(vec![footprint("J1", 5.0, 40.0)]);
    doc.constraints = vec![constraint(
        "c1",
        ConstraintKind::PointOnEdge {
            point: fp_anchor("J1"),
            edge: Anchor::PartEdge {
                node: 99,
                face_a: "enclosure:front".to_string(),
                face_b: "enclosure:right".to_string(),
                hint: None,
            },
        },
    )];
    let report = solve_design_constraints(&mut doc, &WallResolver, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    let j1 = &board_pcb(&doc).footprints[0].position;
    assert!((j1.x - 42.0).abs() < 1e-6, "J1.x = {}", j1.x);

    // A lost part edge fail-closes with an error, moving nothing.
    let mut doc2 = board_doc(vec![footprint("J1", 5.0, 40.0)]);
    doc2.constraints = vec![constraint(
        "c1",
        ConstraintKind::PointOnEdge {
            point: Anchor::PcbFootprint {
                node: BOARD,
                r#ref: "J1".to_string(),
                pad: None,
            },
            edge: Anchor::PartEdge {
                node: 99,
                face_a: "enclosure:missing".to_string(),
                face_b: "enclosure:right".to_string(),
                hint: None,
            },
        },
    )];
    let report2 = solve_design_constraints(&mut doc2, &WallResolver, &SolveOptions::default());
    assert!(!report2.errors.is_empty(), "{:?}", report2.errors);
    assert!((board_pcb(&doc2).footprints[0].position.x - 5.0).abs() < 1e-9);
}

#[test]
fn extra_fixed_anchors_dragged_footprint() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0), footprint("U2", 10.0, 0.0)]);
    doc.constraints = vec![constraint(
        "c1",
        ConstraintKind::Distance {
            a: fp_anchor("U1"),
            b: fp_anchor("U2"),
            value: Expr::num(10.0),
        },
    )];
    // Simulate a drag: U2 was just moved to (30, 0); pin it for this solve.
    match &mut doc.nodes.get_mut(&BOARD).unwrap().op {
        CsgOp::PcbBoard { board } => board.footprints[1].position = Vec2::new(30.0, 0.0),
        _ => unreachable!(),
    }
    let report = solve_design_constraints(
        &mut doc,
        &NoPartAnchors,
        &SolveOptions {
            extra_fixed: vec![(BOARD, "U2".to_string())],
        },
    );
    assert!(report.converged, "{report:?}");
    let pcb = board_pcb(&doc);
    let (u1, u2) = (&pcb.footprints[0].position, &pcb.footprints[1].position);
    assert!((u2.x - 30.0).abs() < 1e-6, "dragged footprint stays put");
    let d = ((u1.x - u2.x).powi(2) + (u1.y - u2.y).powi(2)).sqrt();
    assert!((d - 10.0).abs() < 1e-6, "U1 followed: d = {d}");
}

#[test]
fn sketch_group_solves_and_writes_back() {
    use vcad_ir::{SketchSegment2D, Vec3};
    let mut doc = Document::new();
    const SKETCH: u64 = 5;
    doc.nodes.insert(
        SKETCH,
        Node {
            id: SKETCH,
            name: None,
            op: CsgOp::Sketch2D {
                origin: Vec3::new(0.0, 0.0, 0.0),
                x_dir: Vec3::new(1.0, 0.0, 0.0),
                y_dir: Vec3::new(0.0, 1.0, 0.0),
                segments: vec![
                    SketchSegment2D::Line {
                        start: Vec2::new(0.0, 0.0),
                        end: Vec2::new(9.0, 0.5),
                    },
                    SketchSegment2D::Line {
                        start: Vec2::new(9.0, 0.5),
                        end: Vec2::new(0.0, 5.0),
                    },
                ],
                holes: None,
            },
        },
    );
    let start0 = Anchor::SketchPoint {
        node: SKETCH,
        segment: 0,
        point: vcad_ir::constraints::SketchPointRef::Start,
    };
    let end0 = Anchor::SketchPoint {
        node: SKETCH,
        segment: 0,
        point: vcad_ir::constraints::SketchPointRef::End,
    };
    doc.constraints = vec![
        constraint("c1", ConstraintKind::Fixed { a: start0.clone() }),
        constraint(
            "c2",
            ConstraintKind::Horizontal {
                a: start0.clone(),
                b: end0.clone(),
            },
        ),
        constraint(
            "c3",
            ConstraintKind::Distance {
                a: start0,
                b: end0,
                value: Expr::num(10.0),
            },
        ),
    ];
    let report = solve_design_constraints(&mut doc, &NoPartAnchors, &SolveOptions::default());
    assert!(report.converged, "{report:?}");
    assert_eq!(report.moved_sketches, vec![SKETCH]);
    let (end, start1) = match &doc.nodes[&SKETCH].op {
        CsgOp::Sketch2D { segments, .. } => match (&segments[0], &segments[1]) {
            (SketchSegment2D::Line { end, .. }, SketchSegment2D::Line { start, .. }) => {
                (*end, *start)
            }
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert!((end.y).abs() < 1e-6, "horizontal: end.y = {}", end.y);
    assert!((end.x - 10.0).abs() < 1e-6, "length: end.x = {}", end.x);
    // Welded shared endpoint moved with it.
    assert!((start1.x - end.x).abs() < 1e-9 && (start1.y - end.y).abs() < 1e-9);
}

#[test]
fn check_measures_without_mutating() {
    let mut doc = board_doc(vec![footprint("U1", 0.0, 0.0), footprint("U2", 30.0, 40.0)]);
    doc.constraints = vec![constraint(
        "c1",
        ConstraintKind::Distance {
            a: fp_anchor("U1"),
            b: fp_anchor("U2"),
            value: Expr::num(10.0),
        },
    )];
    let before = board_pcb(&doc).footprints[1].position;
    let report = check_design_constraints(&doc, &NoPartAnchors);
    assert_eq!(report.driven_values.len(), 1);
    assert!((report.driven_values[0].value - 50.0).abs() < 1e-9);
    assert_eq!(board_pcb(&doc).footprints[1].position, before);
}
