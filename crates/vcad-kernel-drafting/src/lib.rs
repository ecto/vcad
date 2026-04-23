#![warn(missing_docs)]

//! 2D drafting and technical drawing generation for the vcad kernel.
//!
//! This crate provides functionality for generating 2D technical drawings
//! from 3D geometry, including:
//!
//! - **Orthographic projection**: Standard views (Front, Top, Right, etc.)
//! - **Isometric projection**: 3D-like views for visualization
//! - **Edge extraction**: Sharp edges, silhouette edges, and boundary edges
//! - **Hidden line removal**: Classification of edges as visible or hidden
//! - **Dimension annotations**: Linear, angular, radial, and ordinate dimensions
//! - **GD&T support**: Feature control frames and datum symbols
//! - **Detail views**: Magnified regions for fine features
//!
//! # Example
//!
//! ```ignore
//! use vcad_kernel_drafting::{project_mesh, ViewDirection};
//! use vcad_kernel_tessellate::TriangleMesh;
//!
//! let mesh: TriangleMesh = /* ... */;
//! let front_view = project_mesh(&mesh, ViewDirection::Front);
//!
//! // Iterate over visible edges
//! for edge in front_view.visible_edges() {
//!     println!("Visible edge: ({}, {}) -> ({}, {})",
//!         edge.start.x, edge.start.y, edge.end.x, edge.end.y);
//! }
//!
//! // Iterate over hidden edges (for dashed lines)
//! for edge in front_view.hidden_edges() {
//!     println!("Hidden edge: ({}, {}) -> ({}, {})",
//!         edge.start.x, edge.start.y, edge.end.x, edge.end.y);
//! }
//! ```

pub mod detail;
pub mod dimension;
pub mod edge_extract;
pub mod hidden_line;
pub mod projection;
pub mod section;
pub mod types;

// Re-export main types and functions for convenience
pub use detail::create_detail_view;
pub use dimension::{
    AngleDefinition, AngularDimension, AnnotationLayer, ArrowType, DatumFeatureSymbol, DatumRef,
    DimensionStyle, FeatureControlFrame, GdtSymbol, GeometryRef, LinearDimension,
    LinearDimensionType, MaterialCondition, OrdinateDimension, RadialDimension, RenderedArc,
    RenderedArrow, RenderedDimension, RenderedText, TextAlignment, TextPlacement, ToleranceMode,
};
pub use edge_extract::{
    extract_drawing_edges, extract_edges, extract_sharp_edges, extract_silhouette_edges,
    DEFAULT_SHARP_ANGLE,
};
pub use hidden_line::{project_mesh, project_mesh_with_options};
pub use projection::{project_point, project_point_with_depth, ViewMatrix};
pub use section::{
    chain_segments, generate_hatch_lines, intersect_mesh_with_plane, project_to_section_plane,
    section_mesh,
};
pub use types::{
    BoundingBox2D, DetailView, DetailViewParams, EdgeType, HatchPattern, HatchRegion, MeshEdge,
    Point2D, ProjectedEdge, ProjectedView, SectionCurve, SectionPlane, SectionView, Triangle3D,
    ViewDirection, Visibility,
};

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_tessellate::TriangleMesh;

    /// Create a unit cube mesh for integration testing.
    fn make_cube() -> TriangleMesh {
        #[rustfmt::skip]
        let vertices: Vec<f32> = vec![
            0.0, 0.0, 0.0,
            1.0, 0.0, 0.0,
            1.0, 1.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
            1.0, 0.0, 1.0,
            1.0, 1.0, 1.0,
            0.0, 1.0, 1.0,
        ];

        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            0, 2, 1, 0, 3, 2,  // Bottom
            4, 5, 6, 4, 6, 7,  // Top
            0, 1, 5, 0, 5, 4,  // Front
            2, 3, 7, 2, 7, 6,  // Back
            0, 4, 7, 0, 7, 3,  // Left
            1, 2, 6, 1, 6, 5,  // Right
        ];

        TriangleMesh {
            vertices,
            indices,
            normals: Vec::new(),
            face_kinds: Vec::new(),
        }
    }

    #[test]
    fn test_full_workflow() {
        let mesh = make_cube();

        // Generate front view
        let view = project_mesh(&mesh, ViewDirection::Front);

        // Should have edges
        assert!(!view.edges.is_empty(), "Should have edges");

        // Should have valid bounds
        assert!(view.bounds.is_valid(), "Bounds should be valid");

        // Cube should project to roughly 1x1 in front view
        let width = view.bounds.width();
        let height = view.bounds.height();
        assert!(width > 0.9 && width < 1.1, "Width should be ~1.0");
        assert!(height > 0.9 && height < 1.1, "Height should be ~1.0");
    }

    #[test]
    fn test_all_standard_views() {
        let mesh = make_cube();

        for view_dir in [
            ViewDirection::Front,
            ViewDirection::Back,
            ViewDirection::Top,
            ViewDirection::Bottom,
            ViewDirection::Right,
            ViewDirection::Left,
        ] {
            let view = project_mesh(&mesh, view_dir);
            assert!(
                !view.edges.is_empty(),
                "View {:?} should have edges",
                view_dir
            );
            assert!(
                view.bounds.is_valid(),
                "View {:?} should have valid bounds",
                view_dir
            );
        }
    }

    #[test]
    fn test_isometric_view() {
        let mesh = make_cube();
        let view = project_mesh(&mesh, ViewDirection::ISOMETRIC_STANDARD);

        // Isometric should show edges (cube edges + face diagonal edges)
        assert!(!view.edges.is_empty(), "Isometric should show edges");

        // Should have at least some visible edges
        assert!(view.num_visible() > 0, "Should have visible edges");
    }

    #[test]
    fn test_edge_types() {
        let mesh = make_cube();
        let view = project_mesh(&mesh, ViewDirection::Front);

        // All cube edges should be either sharp or silhouette
        for edge in &view.edges {
            assert!(
                edge.edge_type == EdgeType::Sharp || edge.edge_type == EdgeType::Silhouette,
                "Cube edges should be sharp or silhouette"
            );
        }
    }
}
