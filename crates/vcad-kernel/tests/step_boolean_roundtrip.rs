//! STEP export of boolean results: BRep must survive to AP214 and back.
//!
//! Regression for "STEP export only supports primitive shapes": real
//! machined parts (an annulus = cylinder minus cylinder) must export to
//! STEP with analytic faces and re-import at matching volume.

use vcad_kernel::Solid;

fn annulus() -> Solid {
    let outer = Solid::cylinder(20.0, 10.0, 64);
    let bore = Solid::cylinder(10.0, 12.0, 64).translate(0.0, 0.0, -1.0);
    outer
        .try_difference(&bore)
        .expect("cylinder-minus-cylinder boolean must succeed")
}

#[test]
fn annulus_step_roundtrip_volume() {
    let ring = annulus();
    assert!(ring.can_export_step(), "boolean result must stay BRep");

    let buffer = ring.to_step_buffer().expect("boolean BRep exports to STEP");
    let content = String::from_utf8_lossy(&buffer);
    assert!(content.contains("MANIFOLD_SOLID_BREP"));
    assert!(
        content.contains("CYLINDRICAL_SURFACE"),
        "annulus must export analytic cylinder faces, not a mesh"
    );

    let reimported = Solid::from_step_buffer_all(&buffer).expect("vcad re-reads its own STEP");
    assert_eq!(reimported.len(), 1);

    let v_orig = ring.volume();
    let v_back = reimported[0].volume();
    let rel = (v_orig - v_back).abs() / v_orig;
    assert!(
        rel < 0.01,
        "round-trip volume drifted {:.3}% (orig {v_orig:.1}, back {v_back:.1})",
        rel * 100.0
    );

    // And against the analytic annulus volume, allowing tessellation error.
    let analytic = std::f64::consts::PI * (20.0f64.powi(2) - 10.0f64.powi(2)) * 10.0;
    let rel_analytic = (v_back - analytic).abs() / analytic;
    assert!(
        rel_analytic < 0.02,
        "re-imported volume {v_back:.1} vs analytic {analytic:.1} ({:.3}%)",
        rel_analytic * 100.0
    );
}

#[test]
fn multi_solid_step_file_has_named_bodies() {
    let a = Solid::cube(10.0, 10.0, 10.0);
    let b = annulus().translate(50.0, 0.0, 0.0);
    let buffer = Solid::solids_to_step_buffer(&[(&a, "plate"), (&b, "ring")])
        .expect("multi-solid export succeeds");
    let content = String::from_utf8_lossy(&buffer);
    assert_eq!(content.matches("MANIFOLD_SOLID_BREP").count(), 2);
    assert!(content.contains("'plate'"));
    assert!(content.contains("'ring'"));

    let reimported = Solid::from_step_buffer_all(&buffer).expect("multi-solid STEP re-reads");
    assert_eq!(reimported.len(), 2);
}

#[test]
fn mesh_only_solid_is_refused() {
    let mesh = Solid::cube(5.0, 5.0, 5.0).to_mesh(8);
    let mesh_solid = Solid::from_mesh(mesh);
    assert!(!mesh_solid.can_export_step());
    assert!(Solid::solids_to_step_buffer(&[(&mesh_solid, "scan")]).is_err());
}

#[test]
fn filleted_boolean_roundtrips_or_refuses_cleanly() {
    // A boolean against a filleted part exercises the NURBS face path.
    // Whether the kernel keeps BRep here or degrades, the exporter must
    // either produce a re-readable file or refuse — never emit garbage.
    let plate = Solid::cube(40.0, 40.0, 10.0).fillet(2.0);
    let bore = Solid::cylinder(5.0, 12.0, 32).translate(20.0, 20.0, -1.0);
    let cut = plate.difference(&bore);
    if cut.can_export_step() {
        let buffer = cut.to_step_buffer().expect("BRep result must serialize");
        let reimported = Solid::from_step_buffer_all(&buffer).expect("exported STEP must re-read");
        assert_eq!(reimported.len(), 1);
    } else {
        assert!(cut.to_step_buffer().is_err());
    }
}
