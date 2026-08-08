//! Document sensitivities: derivatives with respect to *named* parameters,
//! carrying units, routes, and a trust radius that was searched for rather
//! than assumed.
//!
//! The headline gate is [`topology_search_finds_the_real_boundary`]. A box
//! with a through-hole has an exact, knowable topology boundary — the
//! radius at which the hole reaches the box wall — and the trust-radius
//! search has to find it without being told it exists.

use vcad_eval::sensitivity::{
    document_sensitivities, rank_parameters, topology_signature, topology_trust_radius, Qoi,
    QoiRequest, SensitivityOptions,
};
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{CsgOp, Document, Expr, MaterialDef, Node, Parameter, SceneEntry, Vec3};
use vcad_kernel_adjoint::{ClaimBasis, Route, TrustLimit};
use vcad_kernel_diff::mass_properties;
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

const BOX: f64 = 20.0;
const THICK: f64 = 10.0;
/// The hole reaches the wall here: the box is 20 wide, centred hole, so a
/// radius of 10 is exactly tangent to the side faces.
const BOUNDARY: f64 = 10.0;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 48,
        height_segments: 2,
        ..Default::default()
    }
}

/// A plate with a centred through-hole. `hole_r` is the differentiable
/// parameter; `plate_t` is a second knob so ranking has something to rank.
fn plate_doc(hole_r: f64, plate_t: f64) -> Document {
    let mut doc = Document::new();
    doc.nodes.insert(
        1,
        Node {
            id: 1,
            name: Some("plate".into()),
            op: CsgOp::Cube {
                size: Vec3::new(BOX, BOX, THICK),
            },
        },
    );
    doc.nodes.insert(
        2,
        Node {
            id: 2,
            name: Some("drill".into()),
            op: CsgOp::Cylinder {
                radius: 1.0,
                height: THICK * 4.0,
                segments: 48,
            },
        },
    );
    doc.nodes.insert(
        3,
        Node {
            id: 3,
            name: Some("drill_at".into()),
            op: CsgOp::Translate {
                child: 2,
                offset: Vec3::new(BOX / 2.0, BOX / 2.0, -THICK),
            },
        },
    );
    doc.nodes.insert(
        4,
        Node {
            id: 4,
            name: Some("drilled".into()),
            op: CsgOp::Difference { left: 1, right: 3 },
        },
    );
    doc.roots.push(SceneEntry {
        root: 4,
        material: "default".into(),
        visible: None,
    });
    doc.materials.insert(
        "default".into(),
        MaterialDef {
            name: "default".into(),
            color: [0.8, 0.8, 0.8],
            metallic: 0.0,
            roughness: 0.5,
            density: None,
            friction: None,
            ..Default::default()
        },
    );
    doc.parameters.insert(
        "hole_r".into(),
        Parameter {
            value: Expr::Number(hole_r),
            unit: Some("mm".into()),
            min: None,
            max: None,
            description: Some("through-hole radius".into()),
        },
    );
    doc.parameters.insert(
        "plate_t".into(),
        Parameter {
            value: Expr::Number(plate_t),
            unit: Some("mm".into()),
            min: None,
            max: None,
            description: Some("plate thickness".into()),
        },
    );
    doc.bindings.bind(
        vcad_ir::BindingKey {
            node_id: 2,
            field_path: "radius".into(),
        },
        Expr::formula("hole_r"),
    );
    doc.bindings.bind(
        vcad_ir::BindingKey {
            node_id: 1,
            field_path: "size.z".into(),
        },
        Expr::formula("plate_t"),
    );
    doc
}

fn opts() -> SensitivityOptions {
    SensitivityOptions {
        density: 1.0,
        probe_step: 1e-4,
        topology_reach: 0.6,
        topology_refinements: 7,
        find_topology_radius: true,
    }
}

/// Volume of the document's first solid part, via a fresh rebuild.
fn rebuild_volume(doc: &Document) -> f64 {
    let scene = evaluate_document(doc, &EvalOptions::default()).expect("evaluate");
    let brep = scene.parts[0]
        .solid
        .as_ref()
        .and_then(|s| s.as_brep())
        .expect("solid");
    let plan = capture_plan(brep, &tess()).expect("capture");
    let seam = vcad_kernel_diff::evaluate_with_sensitivity(
        brep,
        &plan,
        &vcad_kernel_diff::ParamSeeding::new(),
    )
    .expect("seam");
    mass_properties(&seam.positions, &seam.triangles, 1.0).volume
}

/// The search has to rediscover a boundary it was never told about.
#[test]
fn topology_search_finds_the_real_boundary() {
    let theta0 = 7.0;
    let doc = plate_doc(theta0, THICK);

    // Sanity: the signature really does change across the wall.
    let inside = topology_signature(&doc, "hole_r", 7.0).expect("inside");
    let also_inside = topology_signature(&doc, "hole_r", 9.0).expect("inside");
    assert_eq!(
        inside, also_inside,
        "topology must be stable while the hole stays inside the plate"
    );
    let outside = topology_signature(&doc, "hole_r", 12.0);
    assert!(
        outside.is_err() || outside.as_ref().unwrap() != &inside,
        "a hole wider than the plate must change (or break) the topology, got {outside:?}"
    );

    let radius = topology_trust_radius(&doc, "hole_r", theta0, 0.6, 8).expect("a boundary exists");
    println!(
        "trust radius [{:.4}, {:.4}] limited by {:?}",
        radius.lower, radius.upper, radius.limited_by
    );
    assert_eq!(radius.limited_by, TrustLimit::TopologyStable);
    assert!(radius.contains(theta0));
    // The upper edge is the wall, found by bisection to within the search
    // resolution (span 4.2 / 2^8 ≈ 0.016).
    assert!(
        (radius.upper - BOUNDARY).abs() < 0.2,
        "search put the boundary at {:.4}, the plate wall is at {BOUNDARY}",
        radius.upper
    );
    // Nothing breaks going down, so the lower edge is just the reach.
    assert!(radius.lower < theta0);
}

/// A parameter with no nearby boundary reports no topology limit — the
/// search says "not within reach", not a fabricated interval.
#[test]
fn a_stable_parameter_reports_no_topology_limit() {
    let doc = plate_doc(3.0, THICK);
    let radius = topology_trust_radius(&doc, "hole_r", 3.0, 0.3, 6);
    assert!(
        radius.is_none(),
        "a 3 mm hole in a 20 mm plate has no boundary within ±30%, got {radius:?}"
    );
}

/// The exact route must agree with a rebuild finite difference. Note the
/// reference is a *rebuild*, not a closed form: boolean rims are
/// sag-adaptive, so an inscribed-N-gon closed form would be checking the
/// wrong thing.
#[test]
fn exact_volume_sensitivity_matches_a_rebuild_finite_difference() {
    let theta0 = 7.0;
    let doc = plate_doc(theta0, THICK);
    let table = document_sensitivities(
        &doc,
        &["hole_r".into()],
        &[QoiRequest::document(Qoi::Volume)],
        &tess(),
        &opts(),
    )
    .expect("sensitivities");
    assert_eq!(table.len(), 1);
    let row = &table.rows[0];

    let h = 1e-3;
    let fd = (rebuild_volume(&plate_doc(theta0 + h, THICK))
        - rebuild_volume(&plate_doc(theta0 - h, THICK)))
        / (2.0 * h);
    let rel = (row.value - fd).abs() / fd.abs();
    println!(
        "dV/dhole_r seam {:.9e}  rebuild fd {fd:.9e}  rel {rel:.3e}",
        row.value
    );
    assert!(
        rel < 1e-4,
        "dV/dhole_r: seam {:.9e}, rebuild fd {fd:.9e} (rel {rel:.3e})",
        row.value
    );
    // Drilling a bigger hole removes material.
    assert!(row.value < 0.0, "a bigger hole must shrink the volume");
}

/// Every row is fully described: route, unit, basis, trust.
#[test]
fn rows_carry_route_unit_basis_and_trust() {
    let doc = plate_doc(7.0, THICK);
    let table = document_sensitivities(
        &doc,
        &["hole_r".into()],
        &[
            QoiRequest::document(Qoi::Volume),
            QoiRequest::document(Qoi::Mass),
            QoiRequest::document(Qoi::BboxExtent(2)),
        ],
        &tess(),
        &opts(),
    )
    .expect("sensitivities");
    println!("{}", table.render());
    assert_eq!(table.len(), 3);

    let vol = table.rows.iter().find(|r| r.objective == "volume").unwrap();
    assert_eq!(vol.route, Route::Dual);
    assert_eq!(vol.basis, ClaimBasis::Verified, "the seam route is exact");
    assert_eq!(vol.unit, "mm^3/mm");
    assert_eq!(vol.trust.unwrap().limited_by, TrustLimit::TopologyStable);
    assert!(vol.in_trust());

    // The bbox row is a max over vertices, so it may never claim Verified.
    let bbox = table.rows.iter().find(|r| r.objective == "bbox_z").unwrap();
    assert!(matches!(bbox.route, Route::FiniteDifference { .. }));
    assert_eq!(bbox.basis, ClaimBasis::Predicted);
    assert!(bbox.note.as_ref().unwrap().contains("non-smooth"));
    // The hole does not change the plate's z extent.
    assert!(
        bbox.value.abs() < 1e-6,
        "d(bbox_z)/d(hole_r) = {}",
        bbox.value
    );

    assert!(table.all_usable());
}

/// Declared scrub bounds and the searched topology radius compose: the
/// tighter one wins.
#[test]
fn declared_bounds_can_tighten_the_searched_radius() {
    let mut doc = plate_doc(7.0, THICK);
    doc.parameters.get_mut("hole_r").unwrap().min = Some(6.5);
    doc.parameters.get_mut("hole_r").unwrap().max = Some(7.5);
    let table = document_sensitivities(
        &doc,
        &["hole_r".into()],
        &[QoiRequest::document(Qoi::Volume)],
        &tess(),
        &opts(),
    )
    .expect("sensitivities");
    let t = table.rows[0].trust.unwrap();
    assert_eq!(t.limited_by, TrustLimit::ParameterBounds);
    assert!((t.lower - 6.5).abs() < 1e-9 && (t.upper - 7.5).abs() < 1e-9);
}

/// Ranking: which knob actually commands the mass. Thickness moves the
/// whole plate; the hole only removes a disc, so thickness must win.
#[test]
fn ranking_puts_the_dominant_parameter_first() {
    let mut doc = plate_doc(4.0, THICK);
    doc.parameters.get_mut("hole_r").unwrap().min = Some(1.0);
    doc.parameters.get_mut("hole_r").unwrap().max = Some(9.0);
    doc.parameters.get_mut("plate_t").unwrap().min = Some(5.0);
    doc.parameters.get_mut("plate_t").unwrap().max = Some(15.0);

    let ranked = rank_parameters(
        &doc,
        QoiRequest::document(Qoi::Mass),
        &tess(),
        &SensitivityOptions {
            find_topology_radius: false,
            ..opts()
        },
    )
    .expect("ranking");
    for (name, value, influence) in &ranked {
        println!("{name:>10}  dJ/dθ {value:>14.5e}  influence {influence:?}");
    }
    assert_eq!(ranked.len(), 2);
    assert_eq!(
        ranked[0].0, "plate_t",
        "plate thickness should command the mass, got {ranked:?}"
    );
    // Thicker plate, more mass. Bigger hole, less mass.
    assert!(ranked.iter().find(|r| r.0 == "plate_t").unwrap().1 > 0.0);
    assert!(ranked.iter().find(|r| r.0 == "hole_r").unwrap().1 < 0.0);
}
