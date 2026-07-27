//! Conformance: curved boundaries must export as analytic CIRCLE edges.
//!
//! The regression this pins: every edge used to be written as a LINE, so a
//! circular intersection edge (cylinder ∩ plane) exported as a chain of ~n
//! chord edges deviating from the analytic surfaces by the chord sagitta
//! (r·(1−cos(π/n)) ≈ 0.06 mm at r=50, n=66). With the spec-conventional
//! 1e-6 uncertainty declared, conforming importers (Shapr3D, OCC) failed to
//! sew those edges to the surfaces and DROPPED the curved faces — annular
//! faces and cylinder walls were simply absent, while all-planar boxes
//! rendered perfectly.
//!
//! The assertions here are entity-graph assertions on the STEP text; the
//! real acceptance oracle is a third-party kernel (validated against
//! FreeCAD/OpenCascade: all faces present, sews to a valid closed solid,
//! analytic-exact volume). vcad's own reader is deliberately NOT the oracle
//! — that circular-validation trap has bitten twice before.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder};
use vcad_kernel_step::{read_step_from_buffer, write_step_to_buffer};

/// Minimal STEP entity graph: id -> (keyword, referenced ids).
fn parse_entities(step: &str) -> HashMap<u64, (String, Vec<u64>)> {
    let mut out = HashMap::new();
    for line in step.lines() {
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let Some((id, body)) = rest.split_once(" = ") else {
            continue;
        };
        let Ok(id) = id.parse::<u64>() else { continue };
        let keyword: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let mut refs = Vec::new();
        for (i, c) in body.char_indices() {
            if c == '#' {
                let num: String = body[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(n) = num.parse() {
                    refs.push(n);
                }
            }
        }
        out.insert(id, (keyword, refs));
    }
    out
}

/// For every ADVANCED_FACE on a CYLINDRICAL_SURFACE, count LINE-backed and
/// CIRCLE-backed edges in its bounds.
fn cylindrical_face_edge_stats(step: &str) -> Vec<(usize, usize)> {
    let ent = parse_entities(step);
    let curve_kind = |edge_curve: u64| -> Option<&str> {
        // EDGE_CURVE('', #start, #end, #curve, sense)
        let (kw, refs) = ent.get(&edge_curve)?;
        if kw != "EDGE_CURVE" {
            return None;
        }
        let (ck, _) = ent.get(refs.get(2)?)?;
        Some(ck.as_str())
    };
    let mut stats = Vec::new();
    for (kw, refs) in ent.values() {
        if kw != "ADVANCED_FACE" {
            continue;
        }
        let Some(surf) = refs.last() else { continue };
        if ent.get(surf).map(|(k, _)| k.as_str()) != Some("CYLINDRICAL_SURFACE") {
            continue;
        }
        let (mut lines, mut circles) = (0usize, 0usize);
        for bound in &refs[..refs.len() - 1] {
            let Some((_, loop_refs)) = ent.get(bound) else {
                continue;
            };
            let Some((_, oe_refs)) = loop_refs.first().and_then(|l| ent.get(l)) else {
                continue;
            };
            for oe in oe_refs {
                let Some((_, ec_refs)) = ent.get(oe) else {
                    continue;
                };
                if let Some(kind) = ec_refs.first().and_then(|e| curve_kind(*e)) {
                    match kind {
                        "LINE" => lines += 1,
                        "CIRCLE" => circles += 1,
                        _ => {}
                    }
                }
            }
        }
        stats.push((lines, circles));
    }
    stats
}

fn assert_conformant(step: &str, what: &str) {
    assert!(
        step.contains("CIRCLE("),
        "{what}: no CIRCLE entities — circular edges were not reconstructed"
    );
    for (i, (lines, circles)) in cylindrical_face_edge_stats(step).iter().enumerate() {
        assert!(
            *lines <= 4,
            "{what}: cylindrical face {i} is bounded by {lines} LINE edges \
             (> 4) — a chord-chain polyline survived reconstruction"
        );
        assert!(
            *circles >= 1,
            "{what}: cylindrical face {i} has no CIRCLE boundary edges"
        );
    }
    assert!(
        step.contains("LENGTH_MEASURE(1.0E-6)"),
        "{what}: declared uncertainty is not the spec-conventional 1e-6"
    );
    // The product anchor conforming importers traverse.
    for kw in [
        "PRODUCT(",
        "SHAPE_DEFINITION_REPRESENTATION",
        "ADVANCED_BREP_SHAPE_REPRESENTATION",
    ] {
        assert!(step.contains(kw), "{what}: missing product anchor {kw}");
    }
}

#[test]
fn pristine_cylinder_exports_circles() {
    let buf = write_step_to_buffer(&make_cylinder(10.0, 20.0, 32)).unwrap();
    let step = String::from_utf8_lossy(&buf);
    assert_conformant(&step, "cylinder");
    // Two boundary circles, each split into two semicircular arcs.
    assert_eq!(
        step.matches("CIRCLE(").count(),
        4,
        "expected 2 circles x 2 arcs"
    );
    // Readback sanity (not the acceptance oracle).
    assert_eq!(read_step_from_buffer(&buf).unwrap().len(), 1);
}

#[test]
fn boolean_difference_reconstructs_arcs() {
    let cube = make_cube(20.0, 20.0, 20.0);
    let cyl = make_cylinder(5.0, 30.0, 32);
    let result = boolean_op(&cube, &cyl, BooleanOp::Difference, 32).unwrap();
    let buf = write_step_to_buffer(result.as_brep().unwrap()).unwrap();
    let step = String::from_utf8_lossy(&buf);
    assert_conformant(&step, "cube minus cylinder");
    assert_eq!(read_step_from_buffer(&buf).unwrap().len(), 1);
}

#[test]
fn through_hole_reconstructs_full_circles() {
    // The annulus-style case from the field report: a plate with a round
    // through-hole. The plate faces carry the hole as full-circle inner
    // loops; the hole wall is a cylindrical band.
    let plate = make_cube(40.0, 40.0, 10.0);
    let mut hole = make_cylinder(8.0, 30.0, 64);
    let t = Transform::translation(20.0, 20.0, -5.0);
    for (_, v) in &mut hole.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut hole.geometry.surfaces {
        *s = s.transform(&t);
    }
    let result = boolean_op(&plate, &hole, BooleanOp::Difference, 64).unwrap();
    let buf = write_step_to_buffer(result.as_brep().unwrap()).unwrap();
    let step = String::from_utf8_lossy(&buf);
    assert_conformant(&step, "plate with hole");
    // Hole circles on both plate faces plus both wall boundaries: at least
    // 4 full circles = 8 arcs.
    assert!(
        step.matches("CIRCLE(").count() >= 8,
        "expected >= 8 arcs, got {}",
        step.matches("CIRCLE(").count()
    );
    assert_eq!(read_step_from_buffer(&buf).unwrap().len(), 1);
}
