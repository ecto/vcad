//! Example: Generate a technical drawing from a cube.
//!
//! This demonstrates the full drafting workflow:
//! 1. Create a triangle mesh (simulating output from the kernel)
//! 2. Project it to a 2D view
//! 3. Print visible and hidden edges

use vcad_kernel_drafting::{project_mesh, ViewDirection};
use vcad_kernel_tessellate::TriangleMesh;

fn main() {
    // Create a simple cube mesh (10x10x10)
    let mesh = make_cube_mesh(10.0);

    println!(
        "Cube mesh: {} vertices, {} triangles",
        mesh.num_vertices(),
        mesh.num_triangles()
    );

    // Generate views from different directions
    for (name, dir) in [
        ("Front", ViewDirection::Front),
        ("Top", ViewDirection::Top),
        ("Right", ViewDirection::Right),
        ("Isometric", ViewDirection::ISOMETRIC_STANDARD),
    ] {
        let view = project_mesh(&mesh, dir);

        println!("\n{} view:", name);
        println!("  Total edges: {}", view.edges.len());
        println!("  Visible: {}", view.num_visible());
        println!("  Hidden: {}", view.num_hidden());
        println!(
            "  Bounds: ({:.2}, {:.2}) to ({:.2}, {:.2})",
            view.bounds.min_x, view.bounds.min_y, view.bounds.max_x, view.bounds.max_y
        );

        // Print visible edges
        println!("  Visible edges:");
        for edge in view.visible_edges().take(3) {
            println!(
                "    ({:.2}, {:.2}) -> ({:.2}, {:.2})",
                edge.start.x, edge.start.y, edge.end.x, edge.end.y
            );
        }
    }
}

/// Create a cube mesh with given size.
fn make_cube_mesh(size: f64) -> TriangleMesh {
    let s = size as f32;

    #[rustfmt::skip]
    let vertices: Vec<f32> = vec![
        0.0, 0.0, 0.0,
        s,   0.0, 0.0,
        s,   s,   0.0,
        0.0, s,   0.0,
        0.0, 0.0, s,
        s,   0.0, s,
        s,   s,   s,
        0.0, s,   s,
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
    }
}
