//! Pins known mass-property values for `compute_mesh_properties` — the
//! canonical (Rust) source of truth consumed by the WASM binding
//! `computeMeshProperties` and every TS inspection path.

use std::f64::consts::PI;
use vcad_kernel::{compute_mesh_properties, Solid};

#[test]
fn unit_cube_mass_properties() {
    let mesh = Solid::cube(1.0, 1.0, 1.0).to_mesh(64);
    let p = compute_mesh_properties(&mesh.vertices, &mesh.indices);
    assert!((p.volume - 1.0).abs() < 1e-6, "volume {}", p.volume);
    assert!((p.area - 6.0).abs() < 1e-6, "area {}", p.area);
    for k in 0..3 {
        assert!((p.center_of_mass[k] - 0.5).abs() < 1e-6, "com axis {k}");
    }
    assert!(p.bbox.min.iter().all(|&v| v.abs() < 1e-6));
    assert!(p.bbox.max.iter().all(|&v| (v - 1.0).abs() < 1e-6));
}

#[test]
fn offset_cylinder_mass_properties() {
    let (r, h, segments) = (5.0, 20.0, 256_u32);
    let solid = Solid::cylinder(r, h, segments).translate(10.0, -3.0, 4.0);
    let mesh = solid.to_mesh(256);
    let p = compute_mesh_properties(&mesh.vertices, &mesh.indices);

    // Tessellated cylinder volume/area converge to the analytic values from
    // below; at 256 segments the polygonal deficit is < 0.05%.
    let vol_exact = PI * r * r * h;
    let area_exact = 2.0 * PI * r * r + 2.0 * PI * r * h;
    assert!((p.volume - vol_exact).abs() / vol_exact < 5e-4);
    assert!((p.area - area_exact).abs() / area_exact < 5e-4);

    // Axis at (10, -3), base at z=4 → COM at (10, -3, 14).
    let expected = [10.0, -3.0, 14.0];
    for k in 0..3 {
        assert!(
            (p.center_of_mass[k] - expected[k]).abs() < 1e-3,
            "com axis {k}: {} vs {}",
            p.center_of_mass[k],
            expected[k]
        );
    }
}
