//! Persistent topological naming — kernel-level regression tests.
//!
//! M0: names survive boolean propagation and are deterministic across
//! rebuilds. M1/M2: a named-edge reference resolves after an upstream
//! parameter change and drives a blend onto the intended edge.

use vcad_kernel::vcad_kernel_fillet::{BlendKey, BlendSection, EdgeQuery};
use vcad_kernel::vcad_kernel_math::Point3;
use vcad_kernel::Solid;

fn sorted_names(s: &Solid) -> Vec<String> {
    let mut v: Vec<String> = s
        .names()
        .expect("solid should carry names")
        .faces
        .values()
        .map(|n| n.to_string())
        .collect();
    v.sort();
    v
}

/// A through-hole difference keeps the cube's face names and imports the
/// cylinder wall's name; two rebuilds at different radii agree on the name
/// set (determinism does not depend on boolean iteration order).
#[test]
fn boolean_propagates_names_deterministically() {
    let build = |r: f64| {
        let cube = Solid::cube(20.0, 20.0, 20.0);
        let hole = Solid::cylinder(r, 40.0, 16).translate(10.0, 10.0, -10.0);
        cube.difference(&hole)
    };
    let a = build(5.0);
    let names = sorted_names(&a);
    // All six cube faces survive under their seeded names.
    for tag in ["bottom", "top", "front", "back", "left", "right"] {
        assert!(
            names.contains(&format!("cube:{tag}")),
            "missing cube:{tag} in {names:?}"
        );
    }
    // The hole wall carries the cylinder's name.
    assert!(
        names.iter().any(|n| n.starts_with("cylinder:side")),
        "missing cylinder wall name in {names:?}"
    );

    let b = build(6.0);
    assert_eq!(names, sorted_names(&b), "name set must be rebuild-stable");
}

/// Scoping rewrites every name's scope — the DAG-evaluator hook.
#[test]
fn rescoping_names() {
    let mut cube = Solid::cube(10.0, 10.0, 10.0);
    cube.set_name_scope("n7");
    assert!(sorted_names(&cube).contains(&"n7:top".to_string()));
    assert!(cube.resolve_named_edge("n7:top", "n7:right").is_ok());
    assert!(cube.resolve_named_edge("cube:top", "cube:right").is_err());
}

/// The M2 core: a named-edge fillet stays on the intended edge when the
/// parent primitive's dimension changes.
#[test]
fn named_edge_blend_survives_parameter_change() {
    let keys = [BlendKey {
        t: 0.0,
        section: BlendSection {
            size: 2.0,
            shape: 1.0,
        },
    }];
    for sx in [10.0, 14.0] {
        let cube = Solid::cube(sx, 10.0, 10.0);
        // The reference resolves to the x = sx, z = 10 edge at either size.
        let (a, b) = cube
            .resolve_named_edge("cube:top", "cube:right")
            .expect("edge resolves");
        for p in [a, b] {
            assert!((p.x - sx).abs() < 1e-9, "endpoint x at {sx}: {p:?}");
            assert!((p.z - 10.0).abs() < 1e-9, "endpoint z: {p:?}");
        }

        let blended = cube
            .edge_blend_named("cube:top", "cube:right", &keys)
            .expect("blend applies");
        let brep = blended.as_brep().expect("brep result");
        // The intended corner line (x = sx, z = 10) is gone...
        let on_target_corner = brep
            .topology
            .vertices
            .iter()
            .filter(|(_, v)| (v.point.x - sx).abs() < 1e-9 && (v.point.z - 10.0).abs() < 1e-9)
            .count();
        assert_eq!(on_target_corner, 0, "target edge must be blended at {sx}");
        // ...while the opposite top edge (x = 0, z = 10) stays sharp.
        let on_opposite_corner = brep
            .topology
            .vertices
            .iter()
            .filter(|(_, v)| v.point.x.abs() < 1e-9 && (v.point.z - 10.0).abs() < 1e-9)
            .count();
        assert!(
            on_opposite_corner >= 2,
            "opposite edge must remain sharp at {sx}"
        );
    }
}

/// Fail-closed: unresolvable references error out instead of guessing, and
/// an `Endpoints` query that matches nothing blends nothing.
#[test]
fn unresolvable_references_fail_closed() {
    let cube = Solid::cube(10.0, 10.0, 10.0);
    assert!(cube.resolve_named_edge("cube:top", "cube:nope").is_err());
    assert!(cube.resolve_named_edge("garbage", "cube:top").is_err());
    // A name map is dropped by ops without a propagation rule.
    let filleted = cube.fillet(1.0);
    assert!(filleted.names().is_none());
    assert!(filleted
        .resolve_named_edge("cube:top", "cube:right")
        .is_err());

    // Endpoints query matching nothing → solid unchanged.
    let keys = [BlendKey {
        t: 0.0,
        section: BlendSection {
            size: 2.0,
            shape: 1.0,
        },
    }];
    let noop = cube.edge_blend(
        &EdgeQuery::Endpoints {
            a: Point3::new(99.0, 99.0, 99.0),
            b: Point3::new(99.0, 99.0, 90.0),
        },
        &keys,
    );
    let before = cube.as_brep().unwrap().topology.faces.len();
    let after = noop.as_brep().unwrap().topology.faces.len();
    assert_eq!(before, after);
}
