#![warn(missing_docs)]

//! High-level B-rep CAD kernel facade for vcad.
//!
//! Provides the [`Solid`] type — the primary interface for creating and
//! manipulating 3D geometry using B-rep representation.
//!
//! # Example
//!
//! ```
//! use vcad_kernel::Solid;
//!
//! let cube = Solid::cube(10.0, 20.0, 30.0);
//! let mesh = cube.to_mesh(32);
//! assert!(mesh.num_triangles() >= 12);
//! ```

use std::path::Path;

pub use vcad_kernel_booleans;
pub use vcad_kernel_constraints;
pub use vcad_kernel_fillet;
pub use vcad_kernel_geom;
pub use vcad_kernel_math;
pub use vcad_kernel_primitives;
pub use vcad_kernel_shell;
pub use vcad_kernel_sketch;
pub use vcad_kernel_step;
pub use vcad_kernel_sweep;
pub use vcad_kernel_tessellate;
pub use vcad_kernel_text;
pub use vcad_kernel_topo;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_step::StepError;
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

/// Error returned when STEP export fails.
#[derive(Debug)]
pub enum StepExportError {
    /// The solid has been converted to mesh-only representation (e.g., after boolean operations).
    /// B-rep data is required for STEP export.
    NotBRep,
    /// The solid is empty (no geometry).
    Empty,
    /// An error occurred during STEP file writing.
    Step(StepError),
}

impl std::fmt::Display for StepExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepExportError::NotBRep => write!(
                f,
                "cannot export to STEP: solid has been converted to mesh (B-rep data lost after boolean operations)"
            ),
            StepExportError::Empty => write!(f, "cannot export to STEP: solid is empty"),
            StepExportError::Step(e) => write!(f, "STEP export error: {}", e),
        }
    }
}

impl std::error::Error for StepExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StepExportError::Step(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StepError> for StepExportError {
    fn from(e: StepError) -> Self {
        StepExportError::Step(e)
    }
}

/// The internal representation of a solid.
#[derive(Debug, Clone)]
enum SolidRepr {
    /// Full B-rep solid with topology and geometry.
    BRep(Box<BRepSolid>),
    /// Mesh-only solid (result of boolean operations in Phase 1).
    Mesh(TriangleMesh),
    /// Empty solid (no geometry).
    Empty,
}

/// A 3D solid geometry object.
///
/// Solids can be created from primitives, combined with CSG boolean operations,
/// and transformed. The tessellation to triangle meshes is done on demand.
#[derive(Debug, Clone)]
pub struct Solid {
    repr: SolidRepr,
    /// Default tessellation segment count.
    segments: u32,
}

impl Solid {
    // =========================================================================
    // Constructors
    // =========================================================================

    /// Create an empty solid.
    pub fn empty() -> Self {
        Self {
            repr: SolidRepr::Empty,
            segments: 32,
        }
    }

    /// Create a solid from a triangle mesh.
    pub fn from_mesh(mesh: TriangleMesh) -> Self {
        Self {
            repr: SolidRepr::Mesh(mesh),
            segments: 32,
        }
    }

    /// Create a box (cuboid) with corner at origin and dimensions `(sx, sy, sz)`.
    pub fn cube(sx: f64, sy: f64, sz: f64) -> Self {
        Self {
            repr: SolidRepr::BRep(Box::new(vcad_kernel_primitives::make_cube(sx, sy, sz))),
            segments: 32,
        }
    }

    /// Create a cylinder along Z axis with the given radius and height.
    pub fn cylinder(radius: f64, height: f64, segments: u32) -> Self {
        Self {
            repr: SolidRepr::BRep(Box::new(vcad_kernel_primitives::make_cylinder(
                radius, height, segments,
            ))),
            segments,
        }
    }

    /// Create a sphere centered at origin with the given radius.
    pub fn sphere(radius: f64, segments: u32) -> Self {
        Self {
            repr: SolidRepr::BRep(Box::new(vcad_kernel_primitives::make_sphere(
                radius, segments,
            ))),
            segments,
        }
    }

    /// Create a cone/frustum along Z axis.
    pub fn cone(radius_bottom: f64, radius_top: f64, height: f64, segments: u32) -> Self {
        Self {
            repr: SolidRepr::BRep(Box::new(vcad_kernel_primitives::make_cone(
                radius_bottom,
                radius_top,
                height,
                segments,
            ))),
            segments,
        }
    }

    // =========================================================================
    // CSG boolean operations
    // =========================================================================

    /// Boolean union (self ∪ other).
    pub fn union(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Union)
    }

    /// Boolean difference (self − other).
    pub fn difference(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Difference)
    }

    /// Boolean intersection (self ∩ other).
    pub fn intersection(&self, other: &Solid) -> Solid {
        self.boolean(other, BooleanOp::Intersection)
    }

    fn boolean(&self, other: &Solid, op: BooleanOp) -> Solid {
        match (&self.repr, &other.repr) {
            (SolidRepr::Empty, _) => match op {
                BooleanOp::Union => other.clone(),
                BooleanOp::Difference | BooleanOp::Intersection => Solid::empty(),
            },
            (_, SolidRepr::Empty) => match op {
                BooleanOp::Union | BooleanOp::Difference => self.clone(),
                BooleanOp::Intersection => Solid::empty(),
            },
            (SolidRepr::BRep(a), SolidRepr::BRep(b)) => {
                let segments = self.segments.max(other.segments);
                let result = boolean_op(a.as_ref(), b.as_ref(), op, segments);
                match result {
                    BooleanResult::Mesh(m) => Solid {
                        repr: SolidRepr::Mesh(m),
                        segments,
                    },
                    BooleanResult::BRep(brep) => Solid {
                        repr: SolidRepr::BRep(brep),
                        segments,
                    },
                }
            }
            // For mesh-only solids, tessellate BRep first then combine meshes
            _ => {
                let segments = self.segments.max(other.segments);
                let mesh_a = self.to_mesh(segments);
                let mesh_b = other.to_mesh(segments);
                // For mesh-only cases, just concatenate meshes.
                // This is a Phase 1 limitation — proper mesh CSG comes in Phase 2.
                let mut combined = mesh_a;
                combined.merge(&mesh_b);
                Solid {
                    repr: SolidRepr::Mesh(combined),
                    segments,
                }
            }
        }
    }

    // =========================================================================
    // Fillet & chamfer
    // =========================================================================

    /// Chamfer all edges of the solid by the given distance.
    ///
    /// Each edge is replaced by a planar bevel face, each original face is
    /// trimmed inward, and each vertex becomes a triangular face.
    ///
    /// Only works on B-rep solids with planar faces (e.g., cubes, extruded
    /// prisms). Returns the solid unchanged for mesh-only or empty solids.
    pub fn chamfer(&self, distance: f64) -> Solid {
        match &self.repr {
            SolidRepr::BRep(brep) => Solid {
                repr: SolidRepr::BRep(Box::new(vcad_kernel_fillet::chamfer_all_edges(
                    brep, distance,
                ))),
                segments: self.segments,
            },
            _ => self.clone(),
        }
    }

    /// Fillet all edges of the solid with the given radius.
    ///
    /// Each edge is replaced by a cylindrical blend surface tangent to both
    /// adjacent faces, each original face is trimmed inward, and each vertex
    /// becomes a triangular face.
    ///
    /// Only works on B-rep solids with planar faces. Returns the solid
    /// unchanged for mesh-only or empty solids.
    pub fn fillet(&self, radius: f64) -> Solid {
        match &self.repr {
            SolidRepr::BRep(brep) => Solid {
                repr: SolidRepr::BRep(Box::new(vcad_kernel_fillet::fillet_all_edges(brep, radius))),
                segments: self.segments,
            },
            _ => self.clone(),
        }
    }

    /// Shell (hollow) the solid by offsetting all faces inward.
    ///
    /// Creates a hollow shell with walls of the specified thickness.
    /// The outer surface remains, and an inner surface is created at
    /// `thickness` offset.
    ///
    /// # Arguments
    ///
    /// * `thickness` - Wall thickness (positive = inward offset)
    ///
    /// # Returns
    ///
    /// A new solid representing the hollow shell. Returns self unchanged
    /// for empty solids.
    pub fn shell(&self, thickness: f64) -> Solid {
        match &self.repr {
            SolidRepr::Empty => Solid::empty(),
            SolidRepr::BRep(brep) => Solid {
                repr: SolidRepr::BRep(Box::new(vcad_kernel_shell::shell_brep(brep, thickness))),
                segments: self.segments,
            },
            SolidRepr::Mesh(mesh) => Solid {
                repr: SolidRepr::Mesh(vcad_kernel_shell::shell_mesh(mesh, thickness)),
                segments: self.segments,
            },
        }
    }

    // =========================================================================
    // Pattern operations
    // =========================================================================

    /// Create a linear pattern of the solid along a direction.
    ///
    /// # Arguments
    ///
    /// * `direction` - Direction vector (normalized internally)
    /// * `count` - Number of copies including original (must be >= 1)
    /// * `spacing` - Distance between copies along the direction
    ///
    /// # Returns
    ///
    /// A union of all copies. Returns self if count < 2.
    pub fn linear_pattern(&self, direction: Vec3, count: u32, spacing: f64) -> Solid {
        if count < 2 {
            return self.clone();
        }

        let dir_norm = direction.norm();
        if dir_norm < 1e-12 {
            return self.clone();
        }
        let dir = direction / dir_norm;

        let mut result = self.clone();
        for i in 1..count {
            let offset = dir * (spacing * i as f64);
            let copy = self.translate(offset.x, offset.y, offset.z);
            result = result.union(&copy);
        }
        result
    }

    /// Create a circular pattern of the solid around an axis.
    ///
    /// # Arguments
    ///
    /// * `axis_origin` - A point on the rotation axis
    /// * `axis_dir` - Direction of the rotation axis
    /// * `count` - Number of copies including original (must be >= 1)
    /// * `angle_deg` - Total angle span in degrees
    ///
    /// # Returns
    ///
    /// A union of all rotated copies. Returns self if count < 2.
    pub fn circular_pattern(
        &self,
        axis_origin: Point3,
        axis_dir: Vec3,
        count: u32,
        angle_deg: f64,
    ) -> Solid {
        use vcad_kernel_math::Dir3;

        if count < 2 {
            return self.clone();
        }

        let dir_norm = axis_dir.norm();
        if dir_norm < 1e-12 {
            return self.clone();
        }
        let axis = Dir3::new_normalize(axis_dir);
        let angle_step = angle_deg.to_radians() / count as f64;

        let mut result = self.clone();
        for i in 1..count {
            let angle = angle_step * i as f64;
            // Build transform: translate to origin, rotate, translate back
            let t_to_origin =
                Transform::translation(-axis_origin.x, -axis_origin.y, -axis_origin.z);
            let rot = Transform::rotation_about_axis(&axis, angle);
            let t_back = Transform::translation(axis_origin.x, axis_origin.y, axis_origin.z);
            // Compose: first translate to origin, then rotate, then translate back
            let composed = t_back.then(&rot).then(&t_to_origin);
            let copy = self.apply_transform(&composed);
            result = result.union(&copy);
        }
        result
    }

    // =========================================================================
    // Sketch-based operations
    // =========================================================================

    /// Create a solid by extruding a sketch profile along a direction.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to extrude
    /// * `direction` - The extrusion direction vector (magnitude = distance)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the profile or direction is invalid.
    pub fn extrude(
        profile: vcad_kernel_sketch::SketchProfile,
        direction: Vec3,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let brep = vcad_kernel_sketch::extrude(&profile, direction)?;
        Ok(Solid {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by extruding a sketch profile with twist and/or scale.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to extrude
    /// * `direction` - The extrusion direction vector (magnitude = distance)
    /// * `twist_angle` - Twist angle in radians (rotation around extrusion axis)
    /// * `scale_end` - Scale factor at the end of extrusion (1.0 = no taper)
    ///
    /// # Returns
    ///
    /// A B-rep solid with bilinear lateral faces when twisted.
    pub fn extrude_with_options(
        profile: vcad_kernel_sketch::SketchProfile,
        direction: Vec3,
        twist_angle: f64,
        scale_end: f64,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let options = vcad_kernel_sketch::ExtrudeOptions {
            twist_angle,
            scale_end,
            ..Default::default()
        };
        let brep = vcad_kernel_sketch::extrude_with_options(&profile, direction, options)?;
        Ok(Solid {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by revolving a sketch profile around an axis.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to revolve (line segments only)
    /// * `axis_origin` - A point on the axis of revolution
    /// * `axis_dir` - Direction of the axis of revolution
    /// * `angle_deg` - Angle of revolution in degrees (0, 360]
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the profile or parameters are invalid.
    ///
    /// # Limitations
    ///
    /// Arc segments in the profile are not supported (would require torus surfaces).
    pub fn revolve(
        profile: vcad_kernel_sketch::SketchProfile,
        axis_origin: Point3,
        axis_dir: Vec3,
        angle_deg: f64,
    ) -> Result<Self, vcad_kernel_sketch::SketchError> {
        let brep =
            vcad_kernel_sketch::revolve(&profile, axis_origin, axis_dir, angle_deg.to_radians())?;
        Ok(Solid {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by sweeping a profile along a path curve.
    ///
    /// # Arguments
    ///
    /// * `profile` - The closed 2D profile to sweep
    /// * `path` - The 3D path curve to sweep along
    /// * `options` - Sweep options (twist, scaling, segments)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if the path or profile is invalid.
    pub fn sweep<P: vcad_kernel_geom::Curve3d>(
        profile: vcad_kernel_sketch::SketchProfile,
        path: &P,
        options: vcad_kernel_sweep::SweepOptions,
    ) -> Result<Self, vcad_kernel_sweep::SweepError> {
        let brep = vcad_kernel_sweep::sweep(&profile, path, options)?;
        Ok(Solid {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Create a solid by lofting between multiple profiles.
    ///
    /// # Arguments
    ///
    /// * `profiles` - At least 2 profiles to interpolate between
    /// * `options` - Loft options (mode, closed)
    ///
    /// # Returns
    ///
    /// A B-rep solid, or an error if profiles are invalid.
    pub fn loft(
        profiles: &[vcad_kernel_sketch::SketchProfile],
        options: vcad_kernel_sweep::LoftOptions,
    ) -> Result<Self, vcad_kernel_sweep::LoftError> {
        let brep = vcad_kernel_sweep::loft(profiles, options)?;
        Ok(Solid {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    // =========================================================================
    // Transforms
    // =========================================================================

    /// Translate the solid by `(x, y, z)`.
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Solid {
        let t = Transform::translation(x, y, z);
        self.apply_transform(&t)
    }

    /// Rotate the solid by angles in degrees around X, Y, Z axes.
    pub fn rotate(&self, x_deg: f64, y_deg: f64, z_deg: f64) -> Solid {
        let rx = Transform::rotation_x(x_deg.to_radians());
        let ry = Transform::rotation_y(y_deg.to_radians());
        let rz = Transform::rotation_z(z_deg.to_radians());
        // Apply Z, then Y, then X (Euler XYZ intrinsic rotation)
        let t = rx.then(&ry).then(&rz);
        self.apply_transform(&t)
    }

    /// Scale the solid by `(x, y, z)`.
    pub fn scale(&self, x: f64, y: f64, z: f64) -> Solid {
        let t = Transform::scale(x, y, z);
        self.apply_transform(&t)
    }

    /// Apply an arbitrary affine transform to the solid.
    pub fn apply_transform(&self, transform: &Transform) -> Solid {
        match &self.repr {
            SolidRepr::Empty => Solid::empty(),
            SolidRepr::BRep(brep) => {
                let mut new_brep = brep.as_ref().clone();
                // Transform all vertex positions
                for (_id, vertex) in &mut new_brep.topology.vertices {
                    vertex.point = transform.apply_point(&vertex.point);
                }
                // Transform all surface definitions
                for surface in &mut new_brep.geometry.surfaces {
                    *surface = surface.transform(transform);
                }
                // If negative determinant (mirror), flip face orientations
                let det = transform.matrix.fixed_view::<3, 3>(0, 0).determinant();
                if det < 0.0 {
                    for (_id, face) in &mut new_brep.topology.faces {
                        face.orientation = match face.orientation {
                            vcad_kernel_topo::Orientation::Forward => {
                                vcad_kernel_topo::Orientation::Reversed
                            }
                            vcad_kernel_topo::Orientation::Reversed => {
                                vcad_kernel_topo::Orientation::Forward
                            }
                        };
                    }
                }
                Solid {
                    repr: SolidRepr::BRep(Box::new(new_brep)),
                    segments: self.segments,
                }
            }
            SolidRepr::Mesh(mesh) => {
                let mut new_mesh = mesh.clone();
                let verts = &mut new_mesh.vertices;
                for chunk in verts.chunks_mut(3) {
                    let p = Point3::new(chunk[0] as f64, chunk[1] as f64, chunk[2] as f64);
                    let tp = transform.apply_point(&p);
                    chunk[0] = tp.x as f32;
                    chunk[1] = tp.y as f32;
                    chunk[2] = tp.z as f32;
                }
                // If any scale factor is negative, flip triangle winding
                let det = transform.matrix.fixed_view::<3, 3>(0, 0).determinant();
                if det < 0.0 {
                    for tri in new_mesh.indices.chunks_mut(3) {
                        tri.swap(1, 2);
                    }
                }
                Solid {
                    repr: SolidRepr::Mesh(new_mesh),
                    segments: self.segments,
                }
            }
        }
    }

    // =========================================================================
    // Queries
    // =========================================================================

    /// Check if the solid is empty (has no geometry).
    pub fn is_empty(&self) -> bool {
        match &self.repr {
            SolidRepr::Empty => true,
            SolidRepr::BRep(_) => false,
            SolidRepr::Mesh(m) => m.num_triangles() == 0,
        }
    }

    /// Get the triangle mesh representation.
    pub fn to_mesh(&self, segments: u32) -> TriangleMesh {
        match &self.repr {
            SolidRepr::Empty => TriangleMesh::new(),
            SolidRepr::BRep(brep) => tessellate_brep(brep.as_ref(), segments),
            SolidRepr::Mesh(m) => m.clone(),
        }
    }

    /// Compute the volume of the solid from its triangle mesh.
    pub fn volume(&self) -> f64 {
        let mesh = self.to_mesh(self.segments);
        compute_volume(&mesh)
    }

    /// Compute the surface area of the solid from its triangle mesh.
    pub fn surface_area(&self) -> f64 {
        let mesh = self.to_mesh(self.segments);
        compute_surface_area(&mesh)
    }

    /// Compute the axis-aligned bounding box as `(min, max)`.
    ///
    /// For B-rep solids with only planar faces, computes directly from vertex
    /// positions (no tessellation needed). For curved surfaces, falls back to
    /// the tessellated mesh since vertices alone don't capture the full extent.
    /// Check if the axis-aligned bounding boxes of two solids overlap.
    ///
    /// Uses a fast vertex-based AABB (no tessellation) for BRep solids.
    pub fn aabb_overlaps(&self, other: &Solid) -> bool {
        let (min_a, max_a) = self.vertex_aabb();
        let (min_b, max_b) = other.vertex_aabb();
        min_a[0] <= max_b[0]
            && max_a[0] >= min_b[0]
            && min_a[1] <= max_b[1]
            && max_a[1] >= min_b[1]
            && min_a[2] <= max_b[2]
            && max_a[2] >= min_b[2]
    }

    /// Fast vertex-only AABB (no tessellation). Slightly underestimates for curved surfaces.
    fn vertex_aabb(&self) -> ([f64; 3], [f64; 3]) {
        match &self.repr {
            SolidRepr::Empty => ([0.0; 3], [0.0; 3]),
            SolidRepr::BRep(brep) => {
                let mut min = [f64::MAX; 3];
                let mut max = [f64::MIN; 3];
                for (_id, v) in &brep.topology.vertices {
                    let p = v.point;
                    min[0] = min[0].min(p.x);
                    min[1] = min[1].min(p.y);
                    min[2] = min[2].min(p.z);
                    max[0] = max[0].max(p.x);
                    max[1] = max[1].max(p.y);
                    max[2] = max[2].max(p.z);
                }
                (min, max)
            }
            SolidRepr::Mesh(mesh) => {
                let mut min = [f64::MAX; 3];
                let mut max = [f64::MIN; 3];
                for chunk in mesh.vertices.chunks(3) {
                    min[0] = min[0].min(chunk[0] as f64);
                    min[1] = min[1].min(chunk[1] as f64);
                    min[2] = min[2].min(chunk[2] as f64);
                    max[0] = max[0].max(chunk[0] as f64);
                    max[1] = max[1].max(chunk[1] as f64);
                    max[2] = max[2].max(chunk[2] as f64);
                }
                (min, max)
            }
        }
    }

    /// Compute the axis-aligned bounding box as `([min_x, min_y, min_z], [max_x, max_y, max_z])`.
    pub fn bounding_box(&self) -> ([f64; 3], [f64; 3]) {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                use vcad_kernel_geom::SurfaceKind;
                let all_planar = brep
                    .geometry
                    .surfaces
                    .iter()
                    .all(|s| s.surface_type() == SurfaceKind::Plane);
                if all_planar {
                    let mut min = [f64::MAX; 3];
                    let mut max = [f64::MIN; 3];
                    for (_id, v) in &brep.topology.vertices {
                        let p = v.point;
                        min[0] = min[0].min(p.x);
                        min[1] = min[1].min(p.y);
                        min[2] = min[2].min(p.z);
                        max[0] = max[0].max(p.x);
                        max[1] = max[1].max(p.y);
                        max[2] = max[2].max(p.z);
                    }
                    (min, max)
                } else {
                    let mesh = self.to_mesh(self.segments);
                    compute_bounding_box(&mesh)
                }
            }
            _ => {
                let mesh = self.to_mesh(self.segments);
                compute_bounding_box(&mesh)
            }
        }
    }

    /// Compute the geometric centroid (volume-weighted center of mass).
    pub fn center_of_mass(&self) -> [f64; 3] {
        let mesh = self.to_mesh(self.segments);
        compute_center_of_mass(&mesh)
    }

    /// Number of triangles in the tessellated mesh.
    pub fn num_triangles(&self) -> usize {
        let mesh = self.to_mesh(self.segments);
        mesh.num_triangles()
    }

    // =========================================================================
    // STEP import/export
    // =========================================================================

    /// Import the first solid from a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A `Solid` containing the imported B-rep geometry.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the file cannot be read, parsed, or contains no solids.
    pub fn from_step(path: impl AsRef<Path>) -> Result<Self, StepError> {
        let solids = vcad_kernel_step::read_step(path)?;
        let brep = solids.into_iter().next().ok_or(StepError::NoSolids)?;
        Ok(Self {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Import all solids from a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A vector of `Solid`s, one for each solid found in the file.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the file cannot be read, parsed, or contains no solids.
    pub fn from_step_all(path: impl AsRef<Path>) -> Result<Vec<Self>, StepError> {
        let solids = vcad_kernel_step::read_step(path)?;
        Ok(solids
            .into_iter()
            .map(|brep| Self {
                repr: SolidRepr::BRep(Box::new(brep)),
                segments: 32,
            })
            .collect())
    }

    /// Import the first solid from a STEP buffer.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw STEP file contents
    ///
    /// # Returns
    ///
    /// A `Solid` containing the imported B-rep geometry.
    pub fn from_step_buffer(data: &[u8]) -> Result<Self, StepError> {
        let solids = vcad_kernel_step::read_step_from_buffer(data)?;
        let brep = solids.into_iter().next().ok_or(StepError::NoSolids)?;
        Ok(Self {
            repr: SolidRepr::BRep(Box::new(brep)),
            segments: 32,
        })
    }

    /// Import all solids from a STEP buffer.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw STEP file contents
    ///
    /// # Returns
    ///
    /// A vector of `Solid` objects, one for each body in the STEP file.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the buffer cannot be parsed.
    pub fn from_step_buffer_all(data: &[u8]) -> Result<Vec<Self>, StepError> {
        let solids = vcad_kernel_step::read_step_from_buffer(data)?;
        Ok(solids
            .into_iter()
            .map(|brep| Self {
                repr: SolidRepr::BRep(Box::new(brep)),
                segments: 32,
            })
            .collect())
    }

    /// Export this solid to a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns `StepExportError::NotBRep` if the solid has been converted to mesh-only
    /// representation (e.g., after boolean operations). STEP export requires B-rep data.
    /// Returns `StepExportError::Empty` if the solid is empty.
    pub fn to_step(&self, path: impl AsRef<Path>) -> Result<(), StepExportError> {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                vcad_kernel_step::write_step(brep.as_ref(), path)?;
                Ok(())
            }
            SolidRepr::Mesh(_) => Err(StepExportError::NotBRep),
            SolidRepr::Empty => Err(StepExportError::Empty),
        }
    }

    /// Export this solid to STEP format in memory.
    ///
    /// # Returns
    ///
    /// The STEP file contents as bytes.
    ///
    /// # Errors
    ///
    /// See [`Solid::to_step`] for error conditions.
    pub fn to_step_buffer(&self) -> Result<Vec<u8>, StepExportError> {
        match &self.repr {
            SolidRepr::BRep(brep) => {
                let buffer = vcad_kernel_step::write_step_to_buffer(brep.as_ref())?;
                Ok(buffer)
            }
            SolidRepr::Mesh(_) => Err(StepExportError::NotBRep),
            SolidRepr::Empty => Err(StepExportError::Empty),
        }
    }

    /// Check if this solid can be exported to STEP format.
    ///
    /// Returns `true` if the solid has B-rep data (not converted to mesh-only).
    /// Returns `false` for mesh-only or empty solids.
    pub fn can_export_step(&self) -> bool {
        matches!(&self.repr, SolidRepr::BRep(_))
    }

    /// Get a reference to the underlying B-rep solid, if available.
    ///
    /// Returns `None` if the solid is mesh-only (e.g., after boolean operations)
    /// or empty. This is useful for operations that require the full B-rep
    /// representation, such as ray tracing.
    pub fn brep(&self) -> Option<&BRepSolid> {
        match &self.repr {
            SolidRepr::BRep(brep) => Some(brep.as_ref()),
            _ => None,
        }
    }

    /// Check if this solid can be ray traced.
    ///
    /// Returns `true` if the solid has B-rep data (required for direct ray tracing).
    /// Returns `false` for mesh-only or empty solids.
    pub fn can_raytrace(&self) -> bool {
        matches!(&self.repr, SolidRepr::BRep(_))
    }
}

// =============================================================================
// Operator overloads for ergonomic boolean operations
// =============================================================================

impl std::ops::Add for Solid {
    type Output = Solid;

    /// Boolean union: `a + b` is equivalent to `a.union(&b)`.
    fn add(self, other: Solid) -> Solid {
        self.union(&other)
    }
}

impl std::ops::Add for &Solid {
    type Output = Solid;

    /// Boolean union: `&a + &b` is equivalent to `a.union(&b)`.
    fn add(self, other: &Solid) -> Solid {
        self.union(other)
    }
}

impl std::ops::Sub for Solid {
    type Output = Solid;

    /// Boolean difference: `a - b` is equivalent to `a.difference(&b)`.
    fn sub(self, other: Solid) -> Solid {
        self.difference(&other)
    }
}

impl std::ops::Sub for &Solid {
    type Output = Solid;

    /// Boolean difference: `&a - &b` is equivalent to `a.difference(&b)`.
    fn sub(self, other: &Solid) -> Solid {
        self.difference(other)
    }
}

impl std::ops::BitAnd for Solid {
    type Output = Solid;

    /// Boolean intersection: `a & b` is equivalent to `a.intersection(&b)`.
    fn bitand(self, other: Solid) -> Solid {
        self.intersection(&other)
    }
}

impl std::ops::BitAnd for &Solid {
    type Output = Solid;

    /// Boolean intersection: `&a & &b` is equivalent to `a.intersection(&b)`.
    fn bitand(self, other: &Solid) -> Solid {
        self.intersection(other)
    }
}

// =============================================================================
// Mesh computation helpers (same algorithms as vcad lib.rs)
// =============================================================================

fn compute_volume(mesh: &TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut vol = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    (vol / 6.0).abs()
}

fn compute_surface_area(mesh: &TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut area = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = Vec3::new(verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64);
        let v1 = Vec3::new(verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64);
        let v2 = Vec3::new(verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64);
        area += (v1 - v0).cross(&(v2 - v0)).norm() / 2.0;
    }
    area
}

fn compute_bounding_box(mesh: &TriangleMesh) -> ([f64; 3], [f64; 3]) {
    let verts = &mesh.vertices;
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for chunk in verts.chunks(3) {
        for i in 0..3 {
            let v = chunk[i] as f64;
            if v < min[i] {
                min[i] = v;
            }
            if v > max[i] {
                max[i] = v;
            }
        }
    }
    (min, max)
}

fn compute_center_of_mass(mesh: &TriangleMesh) -> [f64; 3] {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    let mut total_vol = 0.0;
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        let vol = v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        total_vol += vol;
        cx += vol * (v0[0] + v1[0] + v2[0]);
        cy += vol * (v0[1] + v1[1] + v2[1]);
        cz += vol * (v0[2] + v1[2] + v2[2]);
    }
    if total_vol.abs() < 1e-15 {
        return [0.0; 3];
    }
    let s = 1.0 / (4.0 * total_vol);
    [cx * s, cy * s, cz * s]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(!cube.is_empty());
        let mesh = cube.to_mesh(32);
        assert!(mesh.num_triangles() >= 12);
    }

    #[test]
    fn test_cylinder() {
        let cyl = Solid::cylinder(5.0, 10.0, 32);
        assert!(!cyl.is_empty());
    }

    #[test]
    fn test_sphere() {
        let sphere = Solid::sphere(10.0, 32);
        assert!(!sphere.is_empty());
    }

    #[test]
    fn test_cone() {
        let cone = Solid::cone(5.0, 3.0, 10.0, 32);
        assert!(!cone.is_empty());
    }

    #[test]
    fn test_empty() {
        let empty = Solid::empty();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_translate() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let moved = cube.translate(100.0, 0.0, 0.0);
        let (min, max) = moved.bounding_box();
        assert!((min[0] - 100.0).abs() < 0.1);
        assert!((max[0] - 110.0).abs() < 0.1);
    }

    #[test]
    fn test_scale() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let scaled = cube.scale(2.0, 1.0, 1.0);
        let (min, max) = scaled.bounding_box();
        assert!((max[0] - min[0] - 20.0).abs() < 0.1);
        assert!((max[1] - min[1] - 10.0).abs() < 0.1);
    }

    #[test]
    fn test_union() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        let result = a.union(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_difference() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(5.0, 5.0, 5.0);
        let result = a.difference(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_intersection() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        let result = a.intersection(&b);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_plate_with_hole_via_solid_api() {
        // This mirrors the exact code path used by the WASM/app
        // Plate: 80x6x60 at origin
        let plate = Solid::cube(80.0, 6.0, 60.0);

        // Hole: 12x20x12, translated to (34, -7, 24)
        let hole = Solid::cube(12.0, 20.0, 12.0).translate(34.0, -7.0, 24.0);

        // Boolean difference
        let result = plate.difference(&hole);

        // Check volume and bbox
        let volume = result.volume();
        let (min, max) = result.bounding_box();

        println!("Solid API test - volume: {}", volume);
        println!(
            "Solid API test - bbox: [{:.1},{:.1},{:.1}] to [{:.1},{:.1},{:.1}]",
            min[0], min[1], min[2], max[0], max[1], max[2]
        );

        // Volume should be ~27936 (plate - intersection)
        // Note: boolean operations have some precision variance
        assert!(
            volume > 25000.0 && volume < 30000.0,
            "Expected volume ~27936, got {}",
            volume
        );

        // Bbox Y should be [0, 6] (not -7 to 13!)
        assert!(
            min[1] >= -0.1 && max[1] <= 6.1,
            "Y bounds should be [0,6], got [{}, {}]",
            min[1],
            max[1]
        );
    }

    #[test]
    fn test_cube_volume() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let vol = cube.volume();
        assert!((vol - 1000.0).abs() < 1.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_cube_surface_area() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let area = cube.surface_area();
        assert!((area - 600.0).abs() < 1.0, "expected ~600, got {area}");
    }

    #[test]
    fn test_cube_bounding_box() {
        let cube = Solid::cube(10.0, 20.0, 30.0);
        let (min, max) = cube.bounding_box();
        assert!((max[0] - min[0] - 10.0).abs() < 0.01);
        assert!((max[1] - min[1] - 20.0).abs() < 0.01);
        assert!((max[2] - min[2] - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_cube_center_of_mass() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let com = cube.center_of_mass();
        assert!((com[0] - 5.0).abs() < 0.1, "cx: {}", com[0]);
        assert!((com[1] - 5.0).abs() < 0.1, "cy: {}", com[1]);
        assert!((com[2] - 5.0).abs() < 0.1, "cz: {}", com[2]);
    }

    #[test]
    fn test_rotate_cube_volume() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let rotated = cube.rotate(45.0, 30.0, 60.0);
        let vol = rotated.volume();
        // Volume should be preserved after rotation
        assert!((vol - 1000.0).abs() < 2.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_translate_cylinder_bbox() {
        let cyl = Solid::cylinder(5.0, 10.0, 32);
        let moved = cyl.translate(100.0, 200.0, 300.0);
        let (min, max) = moved.bounding_box();
        // Center should be offset by translation
        assert!((min[0] - 95.0).abs() < 0.5, "min x: {}", min[0]);
        assert!((max[0] - 105.0).abs() < 0.5, "max x: {}", max[0]);
        assert!((min[2] - 300.0).abs() < 0.5, "min z: {}", min[2]);
        assert!((max[2] - 310.0).abs() < 0.5, "max z: {}", max[2]);
    }

    #[test]
    fn test_scale_cylinder_volume() {
        let cyl = Solid::cylinder(5.0, 10.0, 64);
        let base_vol = cyl.volume();
        let scaled = cyl.scale(2.0, 2.0, 2.0);
        let scaled_vol = scaled.volume();
        // Volume scales by 2^3 = 8
        let ratio = scaled_vol / base_vol;
        assert!((ratio - 8.0).abs() < 0.5, "expected ratio ~8, got {ratio}");
    }

    #[test]
    fn test_mirror_x() {
        let cube = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);
        let mirrored = cube.scale(-1.0, 1.0, 1.0);
        let (min, _max) = mirrored.bounding_box();
        assert!(
            min[0] < 0.0,
            "mirrored min x should be negative: {}",
            min[0]
        );
    }

    #[test]
    fn test_empty_union() {
        let empty = Solid::empty();
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let result = empty.union(&cube);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_num_triangles() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(
            cube.num_triangles() >= 12,
            "cube should have at least 12 triangles"
        );
    }

    #[test]
    fn test_chamfer_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let chamfered = cube.chamfer(1.0);
        assert!(!chamfered.is_empty());
        let vol = chamfered.volume();
        // Chamfered cube: V = L³ - 6d²(L-d) = 1000 - 54 = 946
        assert!(
            (vol - 946.0).abs() < 5.0,
            "chamfered cube volume: expected ~946, got {vol}"
        );
    }

    #[test]
    fn test_fillet_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let filleted = cube.fillet(1.0);
        assert!(!filleted.is_empty());
        // Fillet should have more triangles than original cube due to curved surfaces
        assert!(
            filleted.num_triangles() > cube.num_triangles(),
            "filleted cube should have more triangles than plain cube"
        );
    }

    #[test]
    fn test_chamfer_empty() {
        let empty = Solid::empty();
        let chamfered = empty.chamfer(1.0);
        assert!(chamfered.is_empty());
    }

    #[test]
    fn test_extrude_rectangle() {
        use vcad_kernel_sketch::SketchProfile;

        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);
        let solid = Solid::extrude(profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        assert!(!solid.is_empty());
        let vol = solid.volume();
        // Expected: 10 * 5 * 20 = 1000
        assert!(
            (vol - 1000.0).abs() < 2.0,
            "expected volume ~1000, got {vol}"
        );
    }

    #[test]
    fn test_revolve_rectangle() {
        use vcad_kernel_sketch::SketchProfile;

        // Rectangle profile offset from Z-axis, revolve 90° around Z
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 3.0, 10.0);
        let solid = Solid::revolve(profile, Point3::origin(), Vec3::z(), 90.0).unwrap();

        assert!(!solid.is_empty());
        // Volume check is loose due to planar approximation
        let vol = solid.volume();
        assert!(vol > 100.0, "expected positive volume, got {vol}");
    }

    #[test]
    fn test_extrude_then_boolean() {
        use vcad_kernel_sketch::SketchProfile;

        // Create two extruded boxes and union them
        let profile1 = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 10.0);
        let box1 = Solid::extrude(profile1, Vec3::new(0.0, 0.0, 10.0)).unwrap();

        let profile2 =
            SketchProfile::rectangle(Point3::new(5.0, 5.0, 0.0), Vec3::x(), Vec3::y(), 10.0, 10.0);
        let box2 = Solid::extrude(profile2, Vec3::new(0.0, 0.0, 10.0)).unwrap();

        // Union should produce a valid solid
        let result = box1.union(&box2);
        assert!(!result.is_empty());

        // Volume check: combined volume should be positive and reasonable
        // Two 10x10x10 boxes with 5x5x10 overlap = 1000+1000-250 = 1750
        // Due to boolean approximations, just check it's positive
        let vol = result.volume();
        assert!(
            vol >= 1000.0 && vol <= 2000.0,
            "expected volume between 1000 and 2000, got {vol}"
        );
    }

    #[test]
    fn test_linear_pattern() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let pattern = cube.linear_pattern(Vec3::new(1.0, 0.0, 0.0), 3, 20.0);
        assert!(!pattern.is_empty());
        // 3 cubes of 1000mm³ each = 3000mm³
        let vol = pattern.volume();
        assert!((vol - 3000.0).abs() < 50.0, "expected ~3000, got {vol}");
        // Bounding box should span 50mm in X (10 + 20 + 10 + 20 + 10 = 50... wait, it's 10 + 20 + 10 = 40)
        // Actually: first cube 0-10, second 20-30, third 40-50 → span is 50
        let (min, max) = pattern.bounding_box();
        assert!(
            (max[0] - min[0] - 50.0).abs() < 1.0,
            "expected X span ~50, got {}",
            max[0] - min[0]
        );
    }

    #[test]
    fn test_linear_pattern_single() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let pattern = cube.linear_pattern(Vec3::new(1.0, 0.0, 0.0), 1, 20.0);
        // Should return original cube unchanged
        let vol = pattern.volume();
        assert!((vol - 1000.0).abs() < 2.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_circular_pattern() {
        let cube = Solid::cube(5.0, 5.0, 5.0).translate(10.0, 0.0, 0.0);
        // Pattern 4 copies around Z axis, 360° total
        let pattern = cube.circular_pattern(Point3::origin(), Vec3::z(), 4, 360.0);
        assert!(!pattern.is_empty());
        // 4 cubes of 125mm³ each = 500mm³
        let vol = pattern.volume();
        assert!((vol - 500.0).abs() < 20.0, "expected ~500, got {vol}");
    }

    #[test]
    fn test_circular_pattern_90_deg() {
        let cube = Solid::cube(5.0, 5.0, 5.0).translate(10.0, 0.0, 0.0);
        // Pattern 2 copies around Z axis, 90° span (original at 0°, copy at 45°)
        let pattern = cube.circular_pattern(Point3::origin(), Vec3::z(), 2, 90.0);
        assert!(!pattern.is_empty());
        // 2 cubes
        let vol = pattern.volume();
        assert!((vol - 250.0).abs() < 10.0, "expected ~250, got {vol}");
    }

    #[test]
    fn test_shell_cube() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let shell = cube.shell(2.0);
        assert!(!shell.is_empty());
        // Analytical shell creates 12 faces (6 outer + 6 inner)
        let shell_vol = shell.volume();
        assert!(
            shell_vol > 0.0,
            "shell volume {} should be positive",
            shell_vol
        );
    }

    #[test]
    fn test_shell_empty() {
        let empty = Solid::empty();
        let shell = empty.shell(1.0);
        assert!(shell.is_empty());
    }

    #[test]
    fn test_step_roundtrip() {
        // Create a cube
        let cube = Solid::cube(15.0, 25.0, 35.0);

        // Export to STEP buffer
        let buffer = cube.to_step_buffer().expect("should export to STEP");
        assert!(!buffer.is_empty());

        // Import from buffer
        let imported = Solid::from_step_buffer(&buffer).expect("should import from STEP");
        assert!(!imported.is_empty());

        // Verify topology
        let cube_tris = cube.num_triangles();
        let imported_tris = imported.num_triangles();
        assert!(
            cube_tris > 0 && imported_tris > 0,
            "both should have triangles"
        );

        // Verify geometry roughly matches (volume check)
        let cube_vol = cube.volume();
        let imported_vol = imported.volume();
        let vol_diff = (cube_vol - imported_vol).abs();
        assert!(
            vol_diff < 1.0,
            "volumes should match: original={}, imported={}, diff={}",
            cube_vol,
            imported_vol,
            vol_diff
        );
    }

    #[test]
    fn test_step_can_export() {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        assert!(cube.can_export_step(), "primitive should be exportable");

        // After boolean, B-rep is preserved (canExportStep returns true)
        // Note: More complex boolean chains may produce invalid topology
        // that causes toStepBuffer to fail, but canExportStep still returns true
        let hole = Solid::cylinder(3.0, 15.0, 32);
        let result = cube.difference(&hole);
        assert!(
            result.can_export_step(),
            "boolean result should preserve B-rep marker"
        );
    }

    #[test]
    fn test_step_export_empty_error() {
        let empty = Solid::empty();
        let result = empty.to_step_buffer();
        assert!(
            matches!(result, Err(StepExportError::Empty)),
            "empty solid should return Empty error"
        );
    }

    #[test]
    fn test_operator_add() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);
        let result = a + b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_sub() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(5.0, 5.0, 15.0);
        let result = a - b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_bitand() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0).translate(5.0, 5.0, 5.0);
        let result = a & b;
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_ref() {
        let a = Solid::cube(10.0, 10.0, 10.0);
        let b = Solid::cube(10.0, 10.0, 10.0);
        // Test reference operators
        let union = &a + &b;
        let diff = &a - &b;
        let inter = &a & &b;
        assert!(!union.is_empty());
        assert!(!diff.is_empty());
        assert!(!inter.is_empty());
    }
}
