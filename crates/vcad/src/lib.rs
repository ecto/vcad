#![warn(missing_docs)]

//! vcad — Parametric CAD in Rust
//!
//! CSG modeling with multi-format export (STL, glTF, USD, DXF).
//!
//! # Status
//!
//! This crate is the legacy manifold-based CSG API. It is preserved as the
//! ergonomic Rust-facing way to script models, but the implementation will
//! be migrated to sit on top of the new BRep kernel (`vcad-kernel`) rather
//! than its own mesh-CSG backend.
//!
//! Tracked in [issue #10](https://github.com/ecto/vcad/issues/10).
//!
//! # Example
//!
//! ```rust,no_run
//! use vcad::{centered_cube, Part};
//!
//! let cube = centered_cube("block", 20.0, 10.0, 5.0);
//! let hole = Part::cylinder("hole", 3.0, 10.0, 32).translate(0.0, 0.0, -2.5);
//! let result = cube.difference(&hole);
//! result.write_stl("block_with_hole.stl").unwrap();
//! ```

type Vec3 = tang::Vec3<f64>;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3 as IrVec3};

pub mod export;
pub mod step;

pub use export::{Material, Materials};

/// Errors returned by CAD operations.
#[derive(Error, Debug)]
pub enum CadError {
    /// An I/O error occurred during export.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// The geometry is empty (no vertices or triangles).
    #[error("Empty geometry")]
    EmptyGeometry,
}

/// Global atomic counter for unique IR node IDs.
static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a globally unique [`NodeId`].
fn alloc_node_id() -> NodeId {
    NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed)
}

// =============================================================================
// Mesh adaptor — provides a common interface for the B-rep kernel
// =============================================================================

/// A triangle mesh with vertices and indices.
///
/// This is the common mesh type used by all export formats.
#[derive(Clone)]
pub struct PartMesh {
    verts: Vec<f32>,
    idxs: Vec<u32>,
}

impl PartMesh {
    /// Flat array of vertex positions `[x0, y0, z0, x1, y1, z1, ...]`.
    pub fn vertices(&self) -> Vec<f32> {
        self.verts.clone()
    }

    /// Flat array of triangle indices `[i0, i1, i2, ...]`.
    pub fn indices(&self) -> Vec<u32> {
        self.idxs.clone()
    }

    fn from_kernel_mesh(mesh: vcad_kernel::vcad_kernel_tessellate::TriangleMesh) -> Self {
        Self {
            verts: mesh.vertices,
            idxs: mesh.indices,
        }
    }
}

/// A named part with geometry.
///
/// Parts are the primary building block in vcad. Create primitives with
/// [`Part::cube`], [`Part::cylinder`], [`Part::sphere`], etc., then combine
/// them with CSG operations ([`Part::union`], [`Part::difference`],
/// [`Part::intersection`]) or the operator shorthands (`+`, `-`, `&`).
///
/// Each Part carries an IR subtree recording its parametric construction
/// history. Extract it with [`Part::to_document`].
#[derive(Clone)]
pub struct Part {
    /// Human-readable name for this part (used in export filenames and scene graphs).
    pub name: String,
    solid: vcad_kernel::Solid,
    ir_node_id: NodeId,
    ir_nodes: HashMap<NodeId, Node>,
}

impl Part {
    // =========================================================================
    // Internal constructors
    // =========================================================================

    fn with_ir(
        name: String,
        solid: vcad_kernel::Solid,
        ir_node_id: NodeId,
        ir_nodes: HashMap<NodeId, Node>,
    ) -> Self {
        Self {
            name,
            solid,
            ir_node_id,
            ir_nodes,
        }
    }

    /// Create a leaf IR node (primitive or empty) and return `(id, nodes)`.
    fn make_leaf(name: &str, op: CsgOp) -> (NodeId, HashMap<NodeId, Node>) {
        let id = alloc_node_id();
        let mut nodes = HashMap::new();
        nodes.insert(
            id,
            Node {
                id,
                name: Some(name.to_string()),
                op,
            },
        );
        (id, nodes)
    }

    /// Build a binary CSG node, merging both children's IR maps.
    fn make_binary(
        name: &str,
        left: &Part,
        right: &Part,
        op_fn: impl FnOnce(NodeId, NodeId) -> CsgOp,
    ) -> (NodeId, HashMap<NodeId, Node>) {
        let id = alloc_node_id();
        let mut nodes = left.ir_nodes.clone();
        nodes.extend(right.ir_nodes.iter().map(|(&k, v)| (k, v.clone())));
        nodes.insert(
            id,
            Node {
                id,
                name: Some(name.to_string()),
                op: op_fn(left.ir_node_id, right.ir_node_id),
            },
        );
        (id, nodes)
    }

    /// Build a unary transform node, cloning the child's IR map.
    fn make_unary(
        name: &str,
        child: &Part,
        op_fn: impl FnOnce(NodeId) -> CsgOp,
    ) -> (NodeId, HashMap<NodeId, Node>) {
        let id = alloc_node_id();
        let mut nodes = child.ir_nodes.clone();
        nodes.insert(
            id,
            Node {
                id,
                name: Some(name.to_string()),
                op: op_fn(child.ir_node_id),
            },
        );
        (id, nodes)
    }

    // =========================================================================
    // Public constructors
    // =========================================================================

    /// Create an empty part.
    pub fn empty(name: impl Into<String>) -> Self {
        let name = name.into();
        let (id, nodes) = Self::make_leaf(&name, CsgOp::Empty);
        Self::with_ir(name, vcad_kernel::Solid::empty(), id, nodes)
    }

    /// Create a cube/box centered at origin.
    pub fn cube(name: impl Into<String>, x: f64, y: f64, z: f64) -> Self {
        let name = name.into();
        let (id, nodes) = Self::make_leaf(
            &name,
            CsgOp::Cube {
                size: IrVec3::new(x, y, z),
            },
        );
        Self::with_ir(name, vcad_kernel::Solid::cube(x, y, z), id, nodes)
    }

    /// Create a cylinder along Z axis, centered at origin.
    pub fn cylinder(name: impl Into<String>, radius: f64, height: f64, segments: u32) -> Self {
        let name = name.into();
        let (id, nodes) = Self::make_leaf(
            &name,
            CsgOp::Cylinder {
                radius,
                height,
                segments,
            },
        );
        Self::with_ir(
            name,
            vcad_kernel::Solid::cylinder(radius, height, segments),
            id,
            nodes,
        )
    }

    /// Create a cone/tapered cylinder.
    pub fn cone(
        name: impl Into<String>,
        radius_bottom: f64,
        radius_top: f64,
        height: f64,
        segments: u32,
    ) -> Self {
        let name = name.into();
        let (id, nodes) = Self::make_leaf(
            &name,
            CsgOp::Cone {
                radius_bottom,
                radius_top,
                height,
                segments,
            },
        );
        Self::with_ir(
            name,
            vcad_kernel::Solid::cone(radius_bottom, radius_top, height, segments),
            id,
            nodes,
        )
    }

    /// Create a sphere centered at origin.
    pub fn sphere(name: impl Into<String>, radius: f64, segments: u32) -> Self {
        let name = name.into();
        let (id, nodes) = Self::make_leaf(&name, CsgOp::Sphere { radius, segments });
        Self::with_ir(
            name,
            vcad_kernel::Solid::sphere(radius, segments),
            id,
            nodes,
        )
    }

    // =========================================================================
    // CSG operations
    // =========================================================================

    /// Boolean difference (self - other).
    pub fn difference(&self, other: &Part) -> Self {
        let result_name = format!("{}-diff", self.name);
        let (id, nodes) = Self::make_binary(&result_name, self, other, |l, r| CsgOp::Difference {
            left: l,
            right: r,
        });
        Self::with_ir(result_name, self.solid.difference(&other.solid), id, nodes)
    }

    /// Boolean union (self + other).
    pub fn union(&self, other: &Part) -> Self {
        let result_name = format!("{}-union", self.name);
        let (id, nodes) = Self::make_binary(&result_name, self, other, |l, r| CsgOp::Union {
            left: l,
            right: r,
        });
        Self::with_ir(result_name, self.solid.union(&other.solid), id, nodes)
    }

    /// Boolean intersection.
    pub fn intersection(&self, other: &Part) -> Self {
        let result_name = format!("{}-intersect", self.name);
        let (id, nodes) = Self::make_binary(&result_name, self, other, |l, r| {
            CsgOp::Intersection { left: l, right: r }
        });
        Self::with_ir(
            result_name,
            self.solid.intersection(&other.solid),
            id,
            nodes,
        )
    }

    // =========================================================================
    // Transforms
    // =========================================================================

    /// Translate the part.
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Self {
        let (id, nodes) = Self::make_unary(&self.name, self, |child| CsgOp::Translate {
            child,
            offset: IrVec3::new(x, y, z),
        });
        Self::with_ir(self.name.clone(), self.solid.translate(x, y, z), id, nodes)
    }

    /// Translate by vector.
    pub fn translate_vec(&self, v: Vec3) -> Self {
        self.translate(v.x, v.y, v.z)
    }

    /// Rotate the part (angles in degrees).
    pub fn rotate(&self, x_deg: f64, y_deg: f64, z_deg: f64) -> Self {
        let (id, nodes) = Self::make_unary(&self.name, self, |child| CsgOp::Rotate {
            child,
            angles: IrVec3::new(x_deg, y_deg, z_deg),
        });
        Self::with_ir(
            self.name.clone(),
            self.solid.rotate(x_deg, y_deg, z_deg),
            id,
            nodes,
        )
    }

    /// Scale the part.
    pub fn scale(&self, x: f64, y: f64, z: f64) -> Self {
        let (id, nodes) = Self::make_unary(&self.name, self, |child| CsgOp::Scale {
            child,
            factor: IrVec3::new(x, y, z),
        });
        Self::with_ir(self.name.clone(), self.solid.scale(x, y, z), id, nodes)
    }

    /// Uniform scale.
    pub fn scale_uniform(&self, s: f64) -> Self {
        self.scale(s, s, s)
    }

    // =========================================================================
    // Queries
    // =========================================================================

    /// Check if geometry is empty.
    pub fn is_empty(&self) -> bool {
        self.solid.is_empty()
    }

    /// Get the mesh representation.
    pub fn to_mesh(&self) -> PartMesh {
        PartMesh::from_kernel_mesh(self.solid.to_mesh(32))
    }

    /// Export to binary STL bytes (delegates to [`export::stl::to_stl_bytes`]).
    pub fn to_stl(&self) -> Result<Vec<u8>, CadError> {
        export::stl::to_stl_bytes(self)
    }

    /// Write STL to file (delegates to [`export::stl::export_stl`]).
    pub fn write_stl(&self, path: impl AsRef<std::path::Path>) -> Result<(), CadError> {
        export::stl::export_stl(self, path)
    }

    /// Extract the IR document for this part.
    ///
    /// The document contains all nodes in this part's construction DAG
    /// with this part's root node as the single scene entry.
    pub fn to_document(&self) -> Document {
        let mut doc = Document::new();
        doc.nodes = self.ir_nodes.clone();
        doc.roots.push(SceneEntry {
            root: self.ir_node_id,
            material: "default".to_string(),
            visible: None,
        });
        doc
    }

    // =========================================================================
    // STEP import/export
    // =========================================================================

    /// Import a part from a STEP file.
    ///
    /// # Arguments
    ///
    /// * `name` - Name for the imported part
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A `Part` containing the imported B-rep geometry.
    ///
    /// # Errors
    ///
    /// Returns a `StepError` if the file cannot be read, parsed, or contains no solids.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use vcad::Part;
    ///
    /// let imported = Part::from_step("bracket", "bracket.step").unwrap();
    /// imported.write_stl("bracket.stl").unwrap();
    /// ```
    #[cfg(feature = "step")]
    pub fn from_step(
        name: impl Into<String>,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, step::StepError> {
        let solid = vcad_kernel::Solid::from_step(&path)?;
        let name = name.into();
        let (id, nodes) = Self::make_leaf(
            &name,
            CsgOp::StepImport {
                path: path.as_ref().to_string_lossy().into_owned(),
            },
        );
        Ok(Self::with_ir(name, solid, id, nodes))
    }

    /// Import all solids from a STEP file as separate parts.
    ///
    /// # Arguments
    ///
    /// * `base_name` - Base name for the imported parts (will be suffixed with _0, _1, etc.)
    /// * `path` - Path to the STEP file
    ///
    /// # Returns
    ///
    /// A vector of `Part`s, one for each solid found in the file.
    #[cfg(feature = "step")]
    pub fn from_step_all(
        base_name: impl Into<String>,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Vec<Self>, step::StepError> {
        let solids = vcad_kernel::Solid::from_step_all(&path)?;
        let base_name = base_name.into();
        let path_str = path.as_ref().to_string_lossy().into_owned();

        Ok(solids
            .into_iter()
            .enumerate()
            .map(|(i, solid)| {
                let name = format!("{}_{}", base_name, i);
                let (id, nodes) = Self::make_leaf(
                    &name,
                    CsgOp::StepImport {
                        path: path_str.clone(),
                    },
                );
                Self::with_ir(name, solid, id, nodes)
            })
            .collect())
    }

    /// Export this part to a STEP file.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns `StepExportError::NotBRep` if the part has been through boolean operations
    /// that converted it to mesh-only representation. STEP export requires B-rep data.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use vcad::Part;
    ///
    /// // Primitives can be exported to STEP
    /// let cube = Part::cube("box", 10.0, 20.0, 30.0);
    /// cube.write_step("box.step").unwrap();
    ///
    /// // After boolean operations, STEP export may fail
    /// let hole = Part::cylinder("hole", 3.0, 40.0, 32);
    /// let result = cube.difference(&hole);
    /// // result.write_step("box_with_hole.step"); // May return StepExportError::NotBRep
    /// ```
    #[cfg(feature = "step")]
    pub fn write_step(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), step::StepExportError> {
        self.solid.to_step(path)
    }

    /// Check if this part can be exported to STEP format.
    ///
    /// Returns `true` if the part has B-rep data (typically primitives and
    /// transforms). Returns `false` after boolean operations that convert
    /// the geometry to mesh representation.
    #[cfg(feature = "step")]
    pub fn can_export_step(&self) -> bool {
        self.solid.can_export_step()
    }
}

/// Helper to create a centered cube (cubes are corner-aligned at origin by default)
pub fn centered_cube(name: impl Into<String>, x: f64, y: f64, z: f64) -> Part {
    Part::cube(name, x, y, z).translate(-x / 2.0, -y / 2.0, -z / 2.0)
}

/// Helper to create a centered cylinder
pub fn centered_cylinder(name: impl Into<String>, radius: f64, height: f64, segments: u32) -> Part {
    Part::cylinder(name, radius, height, segments).translate(0.0, 0.0, -height / 2.0)
}

/// Create a counterbore hole (through hole + larger shallow hole for bolt head)
pub fn counterbore_hole(
    through_diameter: f64,
    counterbore_diameter: f64,
    counterbore_depth: f64,
    total_depth: f64,
    segments: u32,
) -> Part {
    let through = Part::cylinder("through", through_diameter / 2.0, total_depth, segments);
    let counterbore = Part::cylinder(
        "counterbore",
        counterbore_diameter / 2.0,
        counterbore_depth,
        segments,
    )
    .translate(0.0, 0.0, total_depth - counterbore_depth);
    through.union(&counterbore)
}

/// Create a bolt pattern (circle of holes)
pub fn bolt_pattern(
    num_holes: usize,
    bolt_circle_diameter: f64,
    hole_diameter: f64,
    depth: f64,
    segments: u32,
) -> Part {
    let radius = bolt_circle_diameter / 2.0;
    let mut result = Part::empty("bolt_pattern");

    for i in 0..num_holes {
        let angle = 2.0 * PI * (i as f64) / (num_holes as f64);
        let x = radius * angle.cos();
        let y = radius * angle.sin();
        let hole =
            Part::cylinder("hole", hole_diameter / 2.0, depth, segments).translate(x, y, 0.0);
        result = result.union(&hole);
    }

    result
}

// =============================================================================
// Operator overloads for ergonomic CSG
// =============================================================================

/// Union: `&a + &b`
impl std::ops::Add for &Part {
    type Output = Part;
    fn add(self, rhs: &Part) -> Part {
        self.union(rhs)
    }
}

/// Union: `a + b`
impl std::ops::Add for Part {
    type Output = Part;
    fn add(self, rhs: Part) -> Part {
        self.union(&rhs)
    }
}

/// Difference: `&a - &b`
impl std::ops::Sub for &Part {
    type Output = Part;
    fn sub(self, rhs: &Part) -> Part {
        self.difference(rhs)
    }
}

/// Difference: `a - b`
impl std::ops::Sub for Part {
    type Output = Part;
    fn sub(self, rhs: Part) -> Part {
        self.difference(&rhs)
    }
}

/// Intersection: `&a & &b`
impl std::ops::BitAnd for &Part {
    type Output = Part;
    fn bitand(self, rhs: &Part) -> Part {
        self.intersection(rhs)
    }
}

/// Intersection: `a & b`
impl std::ops::BitAnd for Part {
    type Output = Part;
    fn bitand(self, rhs: Part) -> Part {
        self.intersection(&rhs)
    }
}

// =============================================================================
// Mesh inspection
// =============================================================================

impl Part {
    /// Signed volume of the mesh (uses the divergence theorem).
    ///
    /// Returns a positive value for well-formed closed meshes.
    pub fn volume(&self) -> f64 {
        let mesh = self.to_mesh();
        let verts = mesh.vertices();
        let indices = mesh.indices();
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
            // Signed volume of tetrahedron formed with origin
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }

    /// Total surface area of the mesh.
    pub fn surface_area(&self) -> f64 {
        let mesh = self.to_mesh();
        let verts = mesh.vertices();
        let indices = mesh.indices();
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
            area += (v1 - v0).cross(v2 - v0).norm() / 2.0;
        }
        area
    }

    /// Axis-aligned bounding box as `(min, max)`.
    pub fn bounding_box(&self) -> ([f64; 3], [f64; 3]) {
        let mesh = self.to_mesh();
        let verts = mesh.vertices();
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

    /// Geometric centroid (volume-weighted center of mass assuming uniform density).
    pub fn center_of_mass(&self) -> [f64; 3] {
        let mesh = self.to_mesh();
        let verts = mesh.vertices();
        let indices = mesh.indices();
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
            let vol = v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
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

    /// Number of triangles in the mesh.
    pub fn num_triangles(&self) -> usize {
        let mesh = self.to_mesh();
        mesh.indices().len() / 3
    }
}

// =============================================================================
// Mirror and pattern transforms
// =============================================================================

impl Part {
    /// Mirror across the YZ plane (negate X).
    pub fn mirror_x(&self) -> Part {
        self.scale(-1.0, 1.0, 1.0)
    }

    /// Mirror across the XZ plane (negate Y).
    pub fn mirror_y(&self) -> Part {
        self.scale(1.0, -1.0, 1.0)
    }

    /// Mirror across the XY plane (negate Z).
    pub fn mirror_z(&self) -> Part {
        self.scale(1.0, 1.0, -1.0)
    }

    /// Union of `count` copies spaced by `(dx, dy, dz)`.
    ///
    /// The first copy is at the original position; each subsequent copy
    /// is offset by an additional `(dx, dy, dz)`.
    pub fn linear_pattern(&self, dx: f64, dy: f64, dz: f64, count: usize) -> Part {
        let mut result = self.translate(0.0, 0.0, 0.0); // clone
        for i in 1..count {
            let n = i as f64;
            result = result.union(&self.translate(dx * n, dy * n, dz * n));
        }
        result
    }

    /// Union of `count` copies rotated evenly around the Z axis.
    ///
    /// Each copy is rotated by `360° / count` increments. An optional
    /// `radius` translates each copy outward along X before rotating.
    pub fn circular_pattern(&self, radius: f64, count: usize) -> Part {
        let mut result = Part::empty("circular_pattern");
        for i in 0..count {
            let angle = 360.0 * (i as f64) / (count as f64);
            let copy = self.translate(radius, 0.0, 0.0).rotate(0.0, 0.0, angle);
            result = result.union(&copy);
        }
        result
    }
}

// =============================================================================
// Scene (multi-part assembly with materials)
// =============================================================================

/// A scene node containing a part with its material assignment.
pub struct SceneNode {
    /// The geometry for this node.
    pub part: Part,
    /// Key into the [`Materials`] database for this node's material.
    pub material_key: String,
}

impl SceneNode {
    /// Create a new scene node with a part and material key.
    pub fn new(part: Part, material_key: impl Into<String>) -> Self {
        Self {
            part,
            material_key: material_key.into(),
        }
    }
}

/// A scene containing multiple parts with different materials
///
/// Unlike Part.union() which merges geometry into a single mesh,
/// Scene preserves individual parts for multi-material rendering.
pub struct Scene {
    /// Name of the scene (used as root node name in exports).
    pub name: String,
    /// Ordered list of parts with their material assignments.
    pub nodes: Vec<SceneNode>,
}

impl Scene {
    /// Create a new empty scene.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: Vec::new(),
        }
    }

    /// Add a part with its material key
    pub fn add(&mut self, part: Part, material_key: impl Into<String>) {
        self.nodes.push(SceneNode::new(part, material_key));
    }

    /// Add a part with default material
    pub fn add_default(&mut self, part: Part) {
        self.nodes.push(SceneNode::new(part, "default"));
    }

    /// Get total number of nodes
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if scene is empty
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Extract the IR document for the full scene (multi-root).
    ///
    /// Each scene node becomes a root entry in the document with its
    /// assigned material key. All IR nodes from all parts are merged.
    pub fn to_document(&self) -> Document {
        let mut doc = Document::new();
        for scene_node in &self.nodes {
            doc.nodes.extend(
                scene_node
                    .part
                    .ir_nodes
                    .iter()
                    .map(|(&k, v)| (k, v.clone())),
            );
            doc.roots.push(SceneEntry {
                root: scene_node.part.ir_node_id,
                material: scene_node.material_key.clone(),
                visible: None,
            });
        }
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cube_creation() {
        let cube = Part::cube("test", 10.0, 10.0, 10.0);
        assert!(!cube.is_empty());
    }

    #[test]
    fn test_cylinder_creation() {
        let cyl = Part::cylinder("test", 5.0, 10.0, 32);
        assert!(!cyl.is_empty());
    }

    #[test]
    fn test_difference() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        let hole = Part::cylinder("hole", 3.0, 15.0, 32).translate(5.0, 5.0, -1.0);
        let result = cube.difference(&hole);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_operator_overloads() {
        let a = Part::cube("a", 10.0, 10.0, 10.0);
        let b = Part::cube("b", 10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);

        // Owned operators
        let union = Part::cube("a", 10.0, 10.0, 10.0) + Part::cube("b", 10.0, 10.0, 10.0);
        assert!(!union.is_empty());

        let diff = Part::cube("a", 10.0, 10.0, 10.0)
            - Part::cube("b", 5.0, 5.0, 5.0).translate(2.5, 2.5, 2.5);
        assert!(!diff.is_empty());

        let isect = Part::cube("a", 10.0, 10.0, 10.0)
            & Part::cube("b", 10.0, 10.0, 10.0).translate(5.0, 5.0, 5.0);
        assert!(!isect.is_empty());

        // Reference operators
        let union_ref = &a + &b;
        assert!(!union_ref.is_empty());

        let diff_ref = &a - &b;
        assert!(!diff_ref.is_empty());

        let isect_ref = &a & &b;
        assert!(!isect_ref.is_empty());
    }

    #[test]
    fn test_volume() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        let vol = cube.volume();
        assert!((vol - 1000.0).abs() < 1.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_surface_area() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        let area = cube.surface_area();
        assert!((area - 600.0).abs() < 1.0, "expected ~600, got {area}");
    }

    #[test]
    fn test_bounding_box() {
        let cube = Part::cube("cube", 10.0, 20.0, 30.0);
        let (min, max) = cube.bounding_box();
        assert!((max[0] - min[0] - 10.0).abs() < 0.01);
        assert!((max[1] - min[1] - 20.0).abs() < 0.01);
        assert!((max[2] - min[2] - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_center_of_mass() {
        // Cube at origin should have centroid at (5,5,5) since cubes are corner-aligned
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        let com = cube.center_of_mass();
        assert!((com[0] - 5.0).abs() < 0.1, "cx: {}", com[0]);
        assert!((com[1] - 5.0).abs() < 0.1, "cy: {}", com[1]);
        assert!((com[2] - 5.0).abs() < 0.1, "cz: {}", com[2]);
    }

    #[test]
    fn test_num_triangles() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        assert!(
            cube.num_triangles() >= 12,
            "cube should have at least 12 triangles"
        );
    }

    #[test]
    fn test_mirror() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0).translate(5.0, 0.0, 0.0);
        let mirrored = cube.mirror_x();
        let (min, _max) = mirrored.bounding_box();
        // Original is at x=[5,15], mirrored should be at x=[-15,-5]
        assert!(
            min[0] < 0.0,
            "mirrored min x should be negative: {}",
            min[0]
        );
    }

    #[test]
    fn test_linear_pattern() {
        let cube = Part::cube("cube", 5.0, 5.0, 5.0);
        let pattern = cube.linear_pattern(10.0, 0.0, 0.0, 3);
        let (min, max) = pattern.bounding_box();
        // 3 copies at x=0, x=10, x=20 each 5 wide → spans 0..25
        assert!((max[0] - min[0] - 25.0).abs() < 0.1);
    }

    #[test]
    fn test_circular_pattern() {
        let cube = Part::cube("cube", 2.0, 2.0, 2.0);
        let pattern = cube.circular_pattern(10.0, 4);
        assert!(!pattern.is_empty());
        // Should span roughly -12..12 in both X and Y
        let (min, max) = pattern.bounding_box();
        assert!(max[0] > 10.0);
        assert!(min[0] < -10.0 + 2.0); // at least close to -10
    }

    // =========================================================================
    // IR recording tests
    // =========================================================================

    #[test]
    fn test_ir_primitive() {
        let cube = Part::cube("box", 10.0, 20.0, 30.0);
        let doc = cube.to_document();
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.roots.len(), 1);
        let root = &doc.nodes[&doc.roots[0].root];
        assert_eq!(root.name, Some("box".to_string()));
        match &root.op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            other => panic!("expected Cube, got {other:?}"),
        }
    }

    #[test]
    fn test_ir_csg_dag() {
        let cube = Part::cube("box", 10.0, 10.0, 10.0);
        let cyl = Part::cylinder("hole", 3.0, 15.0, 32);
        let result = cube.difference(&cyl);
        let doc = result.to_document();
        // 3 nodes: Cube, Cylinder, Difference
        assert_eq!(doc.nodes.len(), 3);
        assert_eq!(doc.roots.len(), 1);
        let root = &doc.nodes[&doc.roots[0].root];
        match &root.op {
            CsgOp::Difference { left, right } => {
                assert!(matches!(doc.nodes[left].op, CsgOp::Cube { .. }));
                assert!(matches!(doc.nodes[right].op, CsgOp::Cylinder { .. }));
            }
            other => panic!("expected Difference, got {other:?}"),
        }
    }

    #[test]
    fn test_ir_transform_chain() {
        let cube = Part::cube("box", 5.0, 5.0, 5.0);
        let moved = cube.translate(1.0, 2.0, 3.0);
        let rotated = moved.rotate(0.0, 0.0, 45.0);
        let doc = rotated.to_document();
        // 3 nodes: Cube, Translate, Rotate
        assert_eq!(doc.nodes.len(), 3);
        let root = &doc.nodes[&doc.roots[0].root];
        match &root.op {
            CsgOp::Rotate { child, angles } => {
                assert_eq!(angles.z, 45.0);
                match &doc.nodes[child].op {
                    CsgOp::Translate {
                        child: inner,
                        offset,
                    } => {
                        assert_eq!(offset.x, 1.0);
                        assert_eq!(offset.y, 2.0);
                        assert_eq!(offset.z, 3.0);
                        assert!(matches!(doc.nodes[inner].op, CsgOp::Cube { .. }));
                    }
                    other => panic!("expected Translate, got {other:?}"),
                }
            }
            other => panic!("expected Rotate, got {other:?}"),
        }
    }

    #[test]
    fn test_ir_roundtrip() {
        let cube = Part::cube("box", 10.0, 20.0, 30.0);
        let hole = Part::cylinder("hole", 3.0, 40.0, 32);
        let result = cube.difference(&hole);
        let doc = result.to_document();
        let json = doc.to_json().expect("serialize");
        let restored = Document::from_json(&json).expect("deserialize");
        assert_eq!(doc, restored);
        assert_eq!(restored.nodes.len(), 3);
    }

    #[test]
    fn test_clone() {
        let cube = Part::cube("cube", 10.0, 10.0, 10.0);
        let cloned = cube.clone();
        assert_eq!(cloned.name, cube.name);
        assert!(!cloned.is_empty());
        // Cloned part is independent — operations on clone don't affect original
        let moved = cloned.translate(100.0, 0.0, 0.0);
        let (orig_min, _) = cube.bounding_box();
        let (moved_min, _) = moved.bounding_box();
        assert!(moved_min[0] > orig_min[0] + 50.0);
    }

    #[test]
    fn test_ir_scene() {
        let body = Part::cube("body", 20.0, 10.0, 5.0);
        let wheel = Part::cylinder("wheel", 3.0, 2.0, 32);

        let mut scene = Scene::new("car");
        scene.add(body, "steel");
        scene.add(wheel, "rubber");

        let doc = scene.to_document();
        // 2 root entries
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "steel");
        assert_eq!(doc.roots[1].material, "rubber");
        // 2 nodes total (one per primitive)
        assert_eq!(doc.nodes.len(), 2);
    }
}
