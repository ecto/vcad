//! Test section view generation.
//!
//! Run with: cargo run -p vcad-kernel-drafting --example section_test

use vcad_kernel_drafting::{section_mesh, HatchPattern, SectionPlane};
use vcad_kernel_tessellate::TriangleMesh;

/// Create a cube mesh for testing.
fn make_cube(size: f64) -> TriangleMesh {
    #[rustfmt::skip]
    let vertices: Vec<f32> = vec![
        0.0, 0.0, 0.0,           // 0
        size as f32, 0.0, 0.0,   // 1
        size as f32, size as f32, 0.0,  // 2
        0.0, size as f32, 0.0,   // 3
        0.0, 0.0, size as f32,   // 4
        size as f32, 0.0, size as f32,  // 5
        size as f32, size as f32, size as f32, // 6
        0.0, size as f32, size as f32, // 7
    ];

    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        // Bottom (-Z)
        0, 2, 1, 0, 3, 2,
        // Top (+Z)
        4, 5, 6, 4, 6, 7,
        // Front (-Y)
        0, 1, 5, 0, 5, 4,
        // Back (+Y)
        2, 3, 7, 2, 7, 6,
        // Left (-X)
        0, 4, 7, 0, 7, 3,
        // Right (+X)
        1, 2, 6, 1, 6, 5,
    ];

    TriangleMesh {
        vertices,
        indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    }
}

fn main() {
    println!("Section View Test\n");

    // Create a 10x10x10 cube
    let mesh = make_cube(10.0);
    println!("Created cube mesh: {} triangles", mesh.indices.len() / 3);

    // Test 1: Horizontal section at z=5 (middle of cube)
    println!("\n--- Horizontal Section at Z=5 ---");
    let plane = SectionPlane::horizontal(5.0);
    let section = section_mesh(&mesh, &plane, None);

    println!("Section curves: {}", section.curves.len());
    println!("Closed curves: {}", section.num_closed_curves());
    println!("Open curves: {}", section.num_open_curves());
    for (i, curve) in section.curves.iter().enumerate() {
        println!(
            "  Curve {}: {} points, closed: {}",
            i,
            curve.points.len(),
            curve.is_closed
        );
    }
    println!(
        "Bounds: ({:.2}, {:.2}) to ({:.2}, {:.2})",
        section.bounds.min_x, section.bounds.min_y, section.bounds.max_x, section.bounds.max_y
    );

    // Test 2: Same section with hatching
    println!("\n--- Horizontal Section with 45° Hatch ---");
    let pattern = HatchPattern::new(2.0, std::f64::consts::FRAC_PI_4);
    let section_hatched = section_mesh(&mesh, &plane, Some(&pattern));

    println!("Section curves: {}", section_hatched.curves.len());
    println!("Hatch lines: {}", section_hatched.hatch_lines.len());

    // Test 3: Front section at y=5
    println!("\n--- Front Section at Y=5 ---");
    let front_plane = SectionPlane::front(5.0);
    let front_section = section_mesh(&mesh, &front_plane, None);

    println!("Section curves: {}", front_section.curves.len());
    for (i, curve) in front_section.curves.iter().enumerate() {
        println!(
            "  Curve {}: {} points, closed: {}",
            i,
            curve.points.len(),
            curve.is_closed
        );
    }

    // Test 4: Section outside the mesh (should be empty)
    println!("\n--- Section Outside Mesh (Z=20) ---");
    let outside_plane = SectionPlane::horizontal(20.0);
    let outside_section = section_mesh(&mesh, &outside_plane, None);

    println!(
        "Section curves: {} (expected: 0)",
        outside_section.curves.len()
    );

    // Test 5: Horizontal hatch pattern
    println!("\n--- Horizontal Hatch (0°) ---");
    let horiz_pattern = HatchPattern::horizontal(1.5);
    let horiz_hatched = section_mesh(&mesh, &plane, Some(&horiz_pattern));
    println!("Hatch lines: {}", horiz_hatched.hatch_lines.len());

    println!("\n=== All tests completed ===");
}
