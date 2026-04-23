//! Example: Generate a 2D technical drawing of a bracket with dimensions.
//!
//! This demonstrates the full drafting workflow:
//! 1. Create 3D geometry (a bracket)
//! 2. Project to 2D front view
//! 3. Add dimension annotations
//! 4. Render to graphical primitives
//!
//! Run with: cargo run -p vcad-kernel-drafting --example bracket_drawing

use vcad_kernel_drafting::dimension::{AnnotationLayer, DimensionStyle, GeometryRef};
use vcad_kernel_drafting::{project_mesh, Point2D, ViewDirection};
use vcad_kernel_tessellate::TriangleMesh;

/// Create a simple bracket mesh (L-shaped with a hole).
fn make_bracket_mesh() -> TriangleMesh {
    // Simplified bracket: 100x50x10mm base with 50x50x10mm upright
    // This is a simplified mesh - in practice you'd use the vcad crate

    // Base plate vertices (100x10x50)
    #[rustfmt::skip]
    let vertices: Vec<f32> = vec![
        // Base plate (at Y=0, extending in X and Z)
        // Bottom face
        0.0, 0.0, 0.0,      // 0
        100.0, 0.0, 0.0,    // 1
        100.0, 0.0, 50.0,   // 2
        0.0, 0.0, 50.0,     // 3
        // Top face
        0.0, 10.0, 0.0,     // 4
        100.0, 10.0, 0.0,   // 5
        100.0, 10.0, 50.0,  // 6
        0.0, 10.0, 50.0,    // 7
        // Upright (at X=0, extending in Y and Z)
        // Back face
        0.0, 10.0, 0.0,     // 8 (same as 4)
        10.0, 10.0, 0.0,    // 9
        10.0, 60.0, 0.0,    // 10
        0.0, 60.0, 0.0,     // 11
        // Front face
        0.0, 10.0, 50.0,    // 12 (same as 7)
        10.0, 10.0, 50.0,   // 13
        10.0, 60.0, 50.0,   // 14
        0.0, 60.0, 50.0,    // 15
    ];

    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        // Base bottom
        0, 2, 1, 0, 3, 2,
        // Base top
        4, 5, 6, 4, 6, 7,
        // Base front (Z=50)
        3, 6, 2, 3, 7, 6,
        // Base back (Z=0)
        0, 1, 5, 0, 5, 4,
        // Base left (X=0)
        0, 4, 7, 0, 7, 3,
        // Base right (X=100)
        1, 2, 6, 1, 6, 5,
        // Upright back (Z=0)
        8, 9, 10, 8, 10, 11,
        // Upright front (Z=50)
        12, 14, 13, 12, 15, 14,
        // Upright top (Y=60)
        11, 10, 14, 11, 14, 15,
        // Upright right (X=10)
        9, 13, 14, 9, 14, 10,
        // Upright left (X=0)
        8, 11, 15, 8, 15, 12,
    ];

    TriangleMesh {
        vertices,
        indices,
        normals: Vec::new(),
            face_kinds: Vec::new(),
    }
}

fn main() {
    println!("=== Bracket Technical Drawing Demo ===\n");

    // Step 1: Create the 3D bracket geometry
    println!("1. Creating 3D bracket geometry...");
    let mesh = make_bracket_mesh();
    println!(
        "   Mesh: {} vertices, {} triangles",
        mesh.vertices.len() / 3,
        mesh.indices.len() / 3
    );

    // Step 2: Project to 2D front view (looking along -Y)
    println!("\n2. Projecting to 2D front view...");
    let view = project_mesh(&mesh, ViewDirection::Front);
    println!(
        "   Projected: {} edges ({} visible, {} hidden)",
        view.edges.len(),
        view.num_visible(),
        view.num_hidden()
    );
    println!(
        "   Bounds: ({:.1}, {:.1}) to ({:.1}, {:.1})",
        view.bounds.min_x, view.bounds.min_y, view.bounds.max_x, view.bounds.max_y
    );

    // Step 3: Add dimension annotations
    println!("\n3. Adding dimension annotations...");
    let mut annotations = AnnotationLayer::with_style(DimensionStyle::new().with_precision(0));

    // Overall width (base plate)
    annotations.add_horizontal_dimension(
        Point2D::new(0.0, 0.0),
        Point2D::new(100.0, 0.0),
        -15.0, // Below the part
    );

    // Base height
    annotations.add_vertical_dimension(
        Point2D::new(100.0, 0.0),
        Point2D::new(100.0, 10.0),
        15.0, // Right side
    );

    // Upright height
    annotations.add_vertical_dimension(
        Point2D::new(0.0, 10.0),
        Point2D::new(0.0, 60.0),
        -15.0, // Left side
    );

    // Upright width
    annotations.add_horizontal_dimension(
        Point2D::new(0.0, 60.0),
        Point2D::new(10.0, 60.0),
        10.0, // Above
    );

    // Add a diameter dimension for a hypothetical hole
    annotations.add_diameter_dimension(
        GeometryRef::Circle {
            center: Point2D::new(50.0, 5.0),
            radius: 3.0,
        },
        std::f64::consts::FRAC_PI_4,
    );

    // Add GD&T: flatness on base
    annotations.add_flatness_tolerance(0.05, Point2D::new(50.0, -25.0));

    // Add GD&T: perpendicularity on upright
    annotations.add_perpendicularity_tolerance(0.1, Point2D::new(-25.0, 35.0), 'A');

    // Add datum symbol
    annotations.add_datum_symbol('A', Point2D::new(110.0, 5.0));

    println!("   Added {} annotations", annotations.annotation_count());

    // Step 4: Render all annotations
    println!("\n4. Rendering annotations to primitives...");
    let rendered = annotations.render_all(Some(&view));

    let total_lines: usize = rendered.iter().map(|r| r.lines.len()).sum();
    let total_arcs: usize = rendered.iter().map(|r| r.arcs.len()).sum();
    let total_arrows: usize = rendered.iter().map(|r| r.arrows.len()).sum();
    let total_texts: usize = rendered.iter().map(|r| r.texts.len()).sum();

    println!("   Rendered primitives:");
    println!("     Lines: {}", total_lines);
    println!("     Arcs: {}", total_arcs);
    println!("     Arrows: {}", total_arrows);
    println!("     Texts: {}", total_texts);

    // Step 5: Display dimension text values
    println!("\n5. Dimension values:");
    for (i, dim) in rendered.iter().enumerate() {
        for text in &dim.texts {
            println!(
                "   [{:2}] {} at ({:.1}, {:.1})",
                i, text.text, text.position.x, text.position.y
            );
        }
    }

    // Step 6: Summary of what would be exported
    println!("\n6. Export summary:");
    println!("   The rendered primitives can be exported to DXF using:");
    println!("   - Lines → DXF LINE entities");
    println!("   - Arcs → DXF ARC entities");
    println!("   - Arrows → DXF SOLID (filled) or LINE (open)");
    println!("   - Texts → DXF TEXT entities");
    println!("   - Basic dims → DXF with box around text");

    println!("\nDone!");
}
