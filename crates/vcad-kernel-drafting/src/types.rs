//! Core types for 2D drafting and technical drawing generation.

use serde::{Deserialize, Serialize};
use vcad_kernel_math::{Point3, Vec3};

/// A 2D point for serializable drafting output.
///
/// A simple serializable 2D point for drafting output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2D {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl Point2D {
    /// Create a new 2D point.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Origin point (0, 0).
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    /// Distance to another point.
    pub fn distance(&self, other: &Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

impl Default for Point2D {
    fn default() -> Self {
        Self::ORIGIN
    }
}

impl From<vcad_kernel_math::Point2> for Point2D {
    fn from(p: vcad_kernel_math::Point2) -> Self {
        Self { x: p.x, y: p.y }
    }
}

impl From<Point2D> for vcad_kernel_math::Point2 {
    fn from(p: Point2D) -> Self {
        vcad_kernel_math::Point2::new(p.x, p.y)
    }
}

/// Direction for orthographic or isometric projection.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ViewDirection {
    /// Front view: camera on the -Y side looking along +Y (XZ plane visible,
    /// world +X maps to drawing +X).
    #[default]
    Front,
    /// Back view: camera on the +Y side looking along -Y (XZ plane visible,
    /// world -X maps to drawing +X).
    Back,
    /// Top view: camera above (+Z) looking along -Z (XY plane visible,
    /// world +X maps to drawing +X, +Y to drawing +Y).
    Top,
    /// Bottom view: camera below (-Z) looking along +Z (XY plane visible,
    /// world +X maps to drawing +X, -Y to drawing +Y).
    Bottom,
    /// Right view: camera on the +X side looking along -X (YZ plane visible,
    /// world +Y maps to drawing +X).
    Right,
    /// Left view: camera on the -X side looking along +X (YZ plane visible,
    /// world -Y maps to drawing +X).
    Left,
    /// Isometric view with specified azimuth and elevation angles (in radians).
    Isometric {
        /// Azimuth angle in radians (rotation around Z axis).
        azimuth: f64,
        /// Elevation angle in radians (angle from XY plane).
        elevation: f64,
    },
}

impl ViewDirection {
    /// Standard isometric view (30° azimuth, 30° elevation).
    pub const ISOMETRIC_STANDARD: Self = Self::Isometric {
        azimuth: std::f64::consts::FRAC_PI_6,   // 30°
        elevation: std::f64::consts::FRAC_PI_6, // 30°
    };

    /// Dimetric view (common in technical drawings).
    pub const DIMETRIC: Self = Self::Isometric {
        azimuth: 0.46365, // ~26.57° (arctan(0.5))
        elevation: 0.46365,
    };

    /// Get the view direction as a unit vector pointing from the scene
    /// **toward the viewer** (i.e. along the camera position).
    ///
    /// Triangles whose normals have a positive dot product with this vector
    /// face the viewer, and depth measured along it increases toward the
    /// viewer. Until 2026-07 this vector pointed the other way (viewer →
    /// scene) while the projection basis assumed viewer-ward, which mirrored
    /// every view; see `handedness` tests in `projection.rs`.
    pub fn view_vector(&self) -> Vec3 {
        match self {
            ViewDirection::Front => Vec3::new(0.0, -1.0, 0.0),
            ViewDirection::Back => Vec3::new(0.0, 1.0, 0.0),
            ViewDirection::Top => Vec3::new(0.0, 0.0, 1.0),
            ViewDirection::Bottom => Vec3::new(0.0, 0.0, -1.0),
            ViewDirection::Right => Vec3::new(1.0, 0.0, 0.0),
            ViewDirection::Left => Vec3::new(-1.0, 0.0, 0.0),
            ViewDirection::Isometric { azimuth, elevation } => {
                let cos_elev = elevation.cos();
                let sin_elev = elevation.sin();
                let cos_az = azimuth.cos();
                let sin_az = azimuth.sin();
                Vec3::new(cos_elev * sin_az, cos_elev * cos_az, sin_elev)
            }
        }
    }

    /// Get the up vector for this view (used to orient the 2D projection).
    pub fn up_vector(&self) -> Vec3 {
        match self {
            ViewDirection::Front | ViewDirection::Back => Vec3::new(0.0, 0.0, 1.0),
            ViewDirection::Top => Vec3::new(0.0, 1.0, 0.0),
            ViewDirection::Bottom => Vec3::new(0.0, -1.0, 0.0),
            ViewDirection::Right | ViewDirection::Left => Vec3::new(0.0, 0.0, 1.0),
            ViewDirection::Isometric { .. } => Vec3::new(0.0, 0.0, 1.0),
        }
    }
}

/// Visibility of an edge in the projected view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    /// Edge is visible (not occluded by any face).
    Visible,
    /// Edge is hidden (occluded by at least one face).
    Hidden,
}

/// Classification of edge type based on geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeType {
    /// Sharp edge: angle between adjacent faces exceeds threshold.
    Sharp,
    /// Silhouette edge: boundary between front-facing and back-facing faces.
    Silhouette,
    /// Boundary edge: edge with only one adjacent face (mesh boundary).
    Boundary,
}

/// A mesh edge in 3D space (before projection).
#[derive(Debug, Clone)]
pub struct MeshEdge {
    /// Index of the first vertex.
    pub v0: u32,
    /// Index of the second vertex.
    pub v1: u32,
    /// First adjacent triangle index (always present).
    pub tri0: u32,
    /// Second adjacent triangle index (None for boundary edges).
    pub tri1: Option<u32>,
    /// Type of edge based on face normals.
    pub edge_type: EdgeType,
}

impl MeshEdge {
    /// Create a new mesh edge.
    pub fn new(v0: u32, v1: u32, tri0: u32, tri1: Option<u32>, edge_type: EdgeType) -> Self {
        // Canonicalize vertex order for consistent hashing
        let (v0, v1) = if v0 < v1 { (v0, v1) } else { (v1, v0) };
        Self {
            v0,
            v1,
            tri0,
            tri1,
            edge_type,
        }
    }

    /// Check if this is a boundary edge (only one adjacent face).
    pub fn is_boundary(&self) -> bool {
        self.tri1.is_none()
    }
}

/// A 2D projected edge with visibility information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedEdge {
    /// Start point in 2D view coordinates.
    pub start: Point2D,
    /// End point in 2D view coordinates.
    pub end: Point2D,
    /// Visibility classification.
    pub visibility: Visibility,
    /// Type of edge.
    pub edge_type: EdgeType,
    /// Depth of the edge midpoint (for sorting/debugging).
    pub depth: f64,
}

impl ProjectedEdge {
    /// Create a new projected edge.
    pub fn new(
        start: Point2D,
        end: Point2D,
        visibility: Visibility,
        edge_type: EdgeType,
        depth: f64,
    ) -> Self {
        Self {
            start,
            end,
            visibility,
            edge_type,
            depth,
        }
    }

    /// Length of the edge in 2D.
    pub fn length(&self) -> f64 {
        ((self.end.x - self.start.x).powi(2) + (self.end.y - self.start.y).powi(2)).sqrt()
    }

    /// Check if the edge is degenerate (zero length).
    pub fn is_degenerate(&self, tolerance: f64) -> bool {
        self.length() < tolerance
    }
}

/// 2D axis-aligned bounding box.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox2D {
    /// Minimum X coordinate.
    pub min_x: f64,
    /// Minimum Y coordinate.
    pub min_y: f64,
    /// Maximum X coordinate.
    pub max_x: f64,
    /// Maximum Y coordinate.
    pub max_y: f64,
}

impl BoundingBox2D {
    /// Create an empty bounding box.
    pub fn empty() -> Self {
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    /// Expand the bounding box to include a point.
    pub fn include_point(&mut self, p: Point2D) {
        self.min_x = self.min_x.min(p.x);
        self.min_y = self.min_y.min(p.y);
        self.max_x = self.max_x.max(p.x);
        self.max_y = self.max_y.max(p.y);
    }

    /// Width of the bounding box.
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    /// Height of the bounding box.
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    /// Center of the bounding box.
    pub fn center(&self) -> Point2D {
        Point2D::new(
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        )
    }

    /// Check if the bounding box is valid (non-empty).
    pub fn is_valid(&self) -> bool {
        self.min_x <= self.max_x && self.min_y <= self.max_y
    }
}

impl Default for BoundingBox2D {
    fn default() -> Self {
        Self::empty()
    }
}

/// A complete projected view containing all edges.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectedView {
    /// All projected edges.
    pub edges: Vec<ProjectedEdge>,
    /// 2D bounding box of the projected view.
    pub bounds: BoundingBox2D,
    /// View direction used for this projection.
    pub view_direction: ViewDirection,
}

impl ProjectedView {
    /// Create a new empty projected view.
    pub fn new(view_direction: ViewDirection) -> Self {
        Self {
            edges: Vec::new(),
            bounds: BoundingBox2D::empty(),
            view_direction,
        }
    }

    /// Add an edge and update the bounding box.
    pub fn add_edge(&mut self, edge: ProjectedEdge) {
        self.bounds.include_point(edge.start);
        self.bounds.include_point(edge.end);
        self.edges.push(edge);
    }

    /// Get only visible edges.
    pub fn visible_edges(&self) -> impl Iterator<Item = &ProjectedEdge> {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Visible)
    }

    /// Get only hidden edges.
    pub fn hidden_edges(&self) -> impl Iterator<Item = &ProjectedEdge> {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Hidden)
    }

    /// Number of visible edges.
    pub fn num_visible(&self) -> usize {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Visible)
            .count()
    }

    /// Number of hidden edges.
    pub fn num_hidden(&self) -> usize {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Hidden)
            .count()
    }
}

/// A 3D triangle from the mesh, used for occlusion testing.
#[derive(Debug, Clone, Copy)]
pub struct Triangle3D {
    /// First vertex.
    pub v0: Point3,
    /// Second vertex.
    pub v1: Point3,
    /// Third vertex.
    pub v2: Point3,
    /// Face normal (pointing outward).
    pub normal: Vec3,
}

impl Triangle3D {
    /// Create a new triangle and compute its normal.
    pub fn new(v0: Point3, v1: Point3, v2: Point3) -> Self {
        let e1 = v1 - v0;
        let e2 = v2 - v0;
        let normal = e1.cross(e2).normalize();
        Self { v0, v1, v2, normal }
    }

    /// Check if the triangle faces the viewer.
    ///
    /// `view_dir` points from the scene toward the viewer (see
    /// [`ViewDirection::view_vector`]); a triangle is front-facing when its
    /// outward normal has a positive component along it.
    pub fn is_front_facing(&self, view_dir: &Vec3) -> bool {
        self.normal.dot(view_dir) > 0.0
    }

    /// Centroid of the triangle.
    pub fn centroid(&self) -> Point3 {
        Point3::new(
            (self.v0.x + self.v1.x + self.v2.x) / 3.0,
            (self.v0.y + self.v1.y + self.v2.y) / 3.0,
            (self.v0.z + self.v1.z + self.v2.z) / 3.0,
        )
    }
}

// ============================================================================
// Section View Types
// ============================================================================

/// Defines a cutting plane for section views.
///
/// Uses array representation for serialization compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionPlane {
    /// Point on the cutting plane [x, y, z].
    pub origin: [f64; 3],
    /// Plane normal vector (defines the "front" side) [x, y, z].
    pub normal: [f64; 3],
    /// Up direction for 2D projection orientation [x, y, z].
    pub up: [f64; 3],
}

impl SectionPlane {
    /// Create a new section plane.
    pub fn new(origin: Point3, normal: Vec3, up: Vec3) -> Self {
        Self {
            origin: [origin.x, origin.y, origin.z],
            normal: [normal.x, normal.y, normal.z],
            up: [up.x, up.y, up.z],
        }
    }

    /// Create from arrays directly.
    pub fn from_arrays(origin: [f64; 3], normal: [f64; 3], up: [f64; 3]) -> Self {
        Self { origin, normal, up }
    }

    /// Horizontal section at a given Z height (looking down).
    pub fn horizontal(z: f64) -> Self {
        Self {
            origin: [0.0, 0.0, z],
            normal: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    /// Front section at a given Y depth (looking along -Y).
    pub fn front(y: f64) -> Self {
        Self {
            origin: [0.0, y, 0.0],
            normal: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// Right section at a given X position (viewed from +X, looking along
    /// -X — same orientation as [`ViewDirection::Right`]).
    pub fn right(x: f64) -> Self {
        Self {
            origin: [x, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// Get origin as Point3.
    pub fn origin_point(&self) -> Point3 {
        Point3::new(self.origin[0], self.origin[1], self.origin[2])
    }

    /// Get normal as Vec3.
    pub fn normal_vec(&self) -> Vec3 {
        Vec3::new(self.normal[0], self.normal[1], self.normal[2])
    }

    /// Get up as Vec3.
    pub fn up_vec(&self) -> Vec3 {
        Vec3::new(self.up[0], self.up[1], self.up[2])
    }
}

/// One step of an offset (stepped) section.
///
/// The cutting plane jogs to `offset` (along the base plane normal) for the
/// span `u_start..u_end` measured along the base plane's in-plane U axis
/// (`up × normal`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OffsetSectionStep {
    /// Start of the span along the section U axis (mm, in section-view coords).
    pub u_start: f64,
    /// End of the span along the section U axis (mm).
    pub u_end: f64,
    /// Signed offset of the cutting plane from the base plane, along the
    /// base plane normal (mm).
    pub offset: f64,
}

impl OffsetSectionStep {
    /// Create a new offset section step.
    pub fn new(u_start: f64, u_end: f64, offset: f64) -> Self {
        Self {
            u_start,
            u_end,
            offset,
        }
    }
}

/// An offset (stepped) section plane: a base plane plus a sequence of
/// parallel jogs, each covering a span of the section's U axis.
///
/// Steps should be contiguous and non-overlapping in `u`; per drafting
/// convention the jog transitions themselves are not drawn in the section
/// view — every step is flattened onto the base plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetSectionPlane {
    /// The base cutting plane (defines the projection frame).
    pub base: SectionPlane,
    /// The offset steps. An empty list is equivalent to a full section.
    pub steps: Vec<OffsetSectionStep>,
}

impl OffsetSectionPlane {
    /// Create a new offset section plane.
    pub fn new(base: SectionPlane, steps: Vec<OffsetSectionStep>) -> Self {
        Self { base, steps }
    }
}

/// A continuous polyline in the section (intersection result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionCurve {
    /// Ordered vertices of the polyline in 2D.
    pub points: Vec<Point2D>,
    /// Whether it forms a closed loop.
    pub is_closed: bool,
}

impl SectionCurve {
    /// Create a new section curve.
    pub fn new(points: Vec<Point2D>, is_closed: bool) -> Self {
        Self { points, is_closed }
    }

    /// Length of the curve (sum of segment lengths).
    pub fn length(&self) -> f64 {
        if self.points.len() < 2 {
            return 0.0;
        }
        let mut total = 0.0;
        for i in 0..self.points.len() - 1 {
            total += self.points[i].distance(&self.points[i + 1]);
        }
        if self.is_closed && self.points.len() >= 2 {
            total += self.points.last().unwrap().distance(&self.points[0]);
        }
        total
    }
}

/// Cross-hatching pattern for solid regions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HatchPattern {
    /// Distance between hatch lines (mm).
    pub spacing: f64,
    /// Direction in radians (0 = horizontal, π/4 = 45°).
    pub angle: f64,
}

impl HatchPattern {
    /// Create a new hatch pattern.
    pub fn new(spacing: f64, angle: f64) -> Self {
        Self { spacing, angle }
    }

    /// Standard 45-degree hatch at 2mm spacing.
    pub const STANDARD_45: Self = Self {
        spacing: 2.0,
        angle: std::f64::consts::FRAC_PI_4,
    };

    /// Horizontal hatch at specified spacing.
    pub fn horizontal(spacing: f64) -> Self {
        Self {
            spacing,
            angle: 0.0,
        }
    }
}

impl Default for HatchPattern {
    fn default() -> Self {
        Self::STANDARD_45
    }
}

/// A region to be hatched (bounded by curves).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatchRegion {
    /// Closed polygon outline.
    pub boundary: Vec<Point2D>,
    /// Interior holes (each is a closed polygon).
    pub holes: Vec<Vec<Point2D>>,
}

impl HatchRegion {
    /// Create a hatch region with no holes.
    pub fn new(boundary: Vec<Point2D>) -> Self {
        Self {
            boundary,
            holes: Vec::new(),
        }
    }

    /// Create a hatch region with holes.
    pub fn with_holes(boundary: Vec<Point2D>, holes: Vec<Vec<Point2D>>) -> Self {
        Self { boundary, holes }
    }
}

/// Complete section view result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionView {
    /// Intersection polylines (section curves).
    pub curves: Vec<SectionCurve>,
    /// Generated hatch lines as (start, end) pairs.
    pub hatch_lines: Vec<(Point2D, Point2D)>,
    /// 2D bounding box of the view.
    pub bounds: BoundingBox2D,
}

impl SectionView {
    /// Create a new empty section view.
    pub fn new() -> Self {
        Self {
            curves: Vec::new(),
            hatch_lines: Vec::new(),
            bounds: BoundingBox2D::empty(),
        }
    }

    /// Number of closed curves (typically outer boundaries).
    pub fn num_closed_curves(&self) -> usize {
        self.curves.iter().filter(|c| c.is_closed).count()
    }

    /// Number of open curves.
    pub fn num_open_curves(&self) -> usize {
        self.curves.iter().filter(|c| !c.is_closed).count()
    }

    /// Total number of hatch lines.
    pub fn num_hatch_lines(&self) -> usize {
        self.hatch_lines.len()
    }
}

impl Default for SectionView {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Detail View Types
// ============================================================================

/// Parameters for creating a detail view (magnified region).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailViewParams {
    /// Center of the region in the parent view's 2D coordinates.
    pub center: Point2D,
    /// Scale factor (e.g., 2.0 = 2x magnification).
    pub scale: f64,
    /// Width of the region to capture (in parent view units).
    pub width: f64,
    /// Height of the region to capture.
    pub height: f64,
    /// Label for the detail view (e.g., "A", "B").
    pub label: String,
}

impl DetailViewParams {
    /// Create new detail view parameters.
    pub fn new(
        center: Point2D,
        scale: f64,
        width: f64,
        height: f64,
        label: impl Into<String>,
    ) -> Self {
        Self {
            center,
            scale,
            width,
            height,
            label: label.into(),
        }
    }

    /// Half-width of the capture region.
    pub fn half_width(&self) -> f64 {
        self.width / 2.0
    }

    /// Half-height of the capture region.
    pub fn half_height(&self) -> f64 {
        self.height / 2.0
    }

    /// Minimum X of the capture region.
    pub fn min_x(&self) -> f64 {
        self.center.x - self.half_width()
    }

    /// Maximum X of the capture region.
    pub fn max_x(&self) -> f64 {
        self.center.x + self.half_width()
    }

    /// Minimum Y of the capture region.
    pub fn min_y(&self) -> f64 {
        self.center.y - self.half_height()
    }

    /// Maximum Y of the capture region.
    pub fn max_y(&self) -> f64 {
        self.center.y + self.half_height()
    }
}

/// A detail view showing a magnified region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailView {
    /// The magnified edges.
    pub edges: Vec<ProjectedEdge>,
    /// Bounds of the detail view (in magnified coordinates).
    pub bounds: BoundingBox2D,
    /// The parameters used to create this view.
    pub params: DetailViewParams,
}

impl DetailView {
    /// Create a new detail view.
    pub fn new(edges: Vec<ProjectedEdge>, bounds: BoundingBox2D, params: DetailViewParams) -> Self {
        Self {
            edges,
            bounds,
            params,
        }
    }

    /// Number of visible edges.
    pub fn num_visible(&self) -> usize {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Visible)
            .count()
    }

    /// Number of hidden edges.
    pub fn num_hidden(&self) -> usize {
        self.edges
            .iter()
            .filter(|e| e.visibility == Visibility::Hidden)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_direction_vectors() {
        // view_vector points scene -> viewer: the Front camera sits at -Y,
        // the Top camera above at +Z.
        let front = ViewDirection::Front.view_vector();
        assert!((front.y - (-1.0)).abs() < 1e-10);
        assert!(front.x.abs() < 1e-10);
        assert!(front.z.abs() < 1e-10);

        let top = ViewDirection::Top.view_vector();
        assert!((top.z - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_isometric_view() {
        let iso = ViewDirection::ISOMETRIC_STANDARD;
        let v = iso.view_vector();
        // Should have equal X and Y components (30° azimuth)
        assert!(v.norm() > 0.99 && v.norm() < 1.01);
    }

    #[test]
    fn test_bounding_box() {
        let mut bb = BoundingBox2D::empty();
        assert!(!bb.is_valid());

        bb.include_point(Point2D::new(0.0, 0.0));
        bb.include_point(Point2D::new(10.0, 5.0));

        assert!(bb.is_valid());
        assert!((bb.width() - 10.0).abs() < 1e-10);
        assert!((bb.height() - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_projected_edge_length() {
        let edge = ProjectedEdge::new(
            Point2D::new(0.0, 0.0),
            Point2D::new(3.0, 4.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        );
        assert!((edge.length() - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_triangle_normal() {
        let tri = Triangle3D::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        );
        // Normal should point in +Z direction
        assert!(tri.normal.z > 0.9);
    }
}
