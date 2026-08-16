#![warn(missing_docs)]

//! Half-edge B-rep topology for the vcad kernel.
//!
//! Arena-based (slotmap) half-edge data structure representing the
//! topology of B-rep solids: vertices, edges, loops, faces, shells,
//! and solids.

use slotmap::{new_key_type, SlotMap};
use vcad_kernel_math::Point3;

new_key_type! {
    /// Handle for a vertex in the topology.
    pub struct VertexId;
    /// Handle for a half-edge in the topology.
    pub struct HalfEdgeId;
    /// Handle for an edge (pair of half-edges) in the topology.
    pub struct EdgeId;
    /// Handle for a loop (closed ring of half-edges) bounding a face.
    pub struct LoopId;
    /// Handle for a face in the topology.
    pub struct FaceId;
    /// Handle for a shell (connected set of faces) in the topology.
    pub struct ShellId;
    /// Handle for a solid in the topology.
    pub struct SolidId;
}

/// A vertex — a point in 3D space.
#[derive(Debug, Clone)]
pub struct Vertex {
    /// 3D position.
    pub point: Point3,
    /// One outgoing half-edge from this vertex (arbitrary choice for traversal).
    pub half_edge: Option<HalfEdgeId>,
}

/// A half-edge — one direction of an edge, bounding a face.
#[derive(Debug, Clone)]
pub struct HalfEdge {
    /// Origin vertex of this half-edge.
    pub origin: VertexId,
    /// Twin (opposite-direction) half-edge sharing the same geometric edge.
    pub twin: Option<HalfEdgeId>,
    /// Next half-edge in the same loop (counterclockwise around the face).
    pub next: Option<HalfEdgeId>,
    /// Previous half-edge in the same loop.
    pub prev: Option<HalfEdgeId>,
    /// The parent edge that this half-edge belongs to.
    pub edge: Option<EdgeId>,
    /// The loop that this half-edge belongs to.
    pub loop_id: Option<LoopId>,
}

/// An edge — a pair of twin half-edges sharing geometry.
#[derive(Debug, Clone)]
pub struct Edge {
    /// One of the two half-edges (the other is accessible via `half_edge.twin`).
    pub half_edge: HalfEdgeId,
}

/// A loop — a closed ring of half-edges bounding a face.
#[derive(Debug, Clone)]
pub struct Loop {
    /// Any half-edge in this loop (traverse via `next` to walk the full ring).
    pub half_edge: HalfEdgeId,
    /// The face this loop bounds.
    pub face: Option<FaceId>,
}

/// Face orientation relative to its surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Face normal matches surface normal.
    Forward,
    /// Face normal is opposite to surface normal.
    Reversed,
}

/// A face — a bounded region of a surface.
#[derive(Debug, Clone)]
pub struct Face {
    /// Outer boundary loop.
    pub outer_loop: LoopId,
    /// Inner boundary loops (holes).
    pub inner_loops: Vec<LoopId>,
    /// Index into the geometry store's surface array.
    pub surface_index: usize,
    /// Whether the face normal agrees with the surface normal.
    pub orientation: Orientation,
    /// The shell this face belongs to.
    pub shell: Option<ShellId>,
}

/// Type of shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellType {
    /// Outer boundary of a solid.
    Outer,
    /// Inner void (cavity) within a solid.
    Void,
}

/// A shell — a connected, closed set of faces forming a boundary.
#[derive(Debug, Clone)]
pub struct Shell {
    /// All faces in this shell.
    pub faces: Vec<FaceId>,
    /// The solid this shell belongs to.
    pub solid: Option<SolidId>,
    /// Whether this is an outer shell or a void.
    pub shell_type: ShellType,
}

/// A solid — the top-level B-rep entity.
#[derive(Debug, Clone)]
pub struct Solid {
    /// The outer shell bounding the solid.
    pub outer_shell: ShellId,
    /// Inner shells (voids/cavities).
    pub void_shells: Vec<ShellId>,
}

/// The topology data structure — arena-based storage for all B-rep entities.
#[derive(Debug, Clone)]
pub struct Topology {
    /// All vertices.
    pub vertices: SlotMap<VertexId, Vertex>,
    /// All half-edges.
    pub half_edges: SlotMap<HalfEdgeId, HalfEdge>,
    /// All edges.
    pub edges: SlotMap<EdgeId, Edge>,
    /// All loops.
    pub loops: SlotMap<LoopId, Loop>,
    /// All faces.
    pub faces: SlotMap<FaceId, Face>,
    /// All shells.
    pub shells: SlotMap<ShellId, Shell>,
    /// All solids.
    pub solids: SlotMap<SolidId, Solid>,
}

impl Topology {
    /// Create an empty topology.
    pub fn new() -> Self {
        Self {
            vertices: SlotMap::with_key(),
            half_edges: SlotMap::with_key(),
            edges: SlotMap::with_key(),
            loops: SlotMap::with_key(),
            faces: SlotMap::with_key(),
            shells: SlotMap::with_key(),
            solids: SlotMap::with_key(),
        }
    }

    // =========================================================================
    // Low-level insertion
    // =========================================================================

    /// Insert a vertex at the given point.
    pub fn add_vertex(&mut self, point: Point3) -> VertexId {
        self.vertices.insert(Vertex {
            point,
            half_edge: None,
        })
    }

    /// Insert a half-edge with the given origin vertex.
    pub fn add_half_edge(&mut self, origin: VertexId) -> HalfEdgeId {
        let he_id = self.half_edges.insert(HalfEdge {
            origin,
            twin: None,
            next: None,
            prev: None,
            edge: None,
            loop_id: None,
        });
        // Set vertex's outgoing half-edge if not already set
        if self.vertices[origin].half_edge.is_none() {
            self.vertices[origin].half_edge = Some(he_id);
        }
        he_id
    }

    /// Create an edge linking two twin half-edges.
    pub fn add_edge(&mut self, he1: HalfEdgeId, he2: HalfEdgeId) -> EdgeId {
        let edge_id = self.edges.insert(Edge { half_edge: he1 });
        self.half_edges[he1].twin = Some(he2);
        self.half_edges[he2].twin = Some(he1);
        self.half_edges[he1].edge = Some(edge_id);
        self.half_edges[he2].edge = Some(edge_id);
        edge_id
    }

    /// Create a loop from a sequence of half-edges (links next/prev in ring).
    pub fn add_loop(&mut self, half_edges: &[HalfEdgeId]) -> LoopId {
        assert!(
            !half_edges.is_empty(),
            "loop must have at least one half-edge"
        );
        let n = half_edges.len();
        let loop_id = self.loops.insert(Loop {
            half_edge: half_edges[0],
            face: None,
        });
        for i in 0..n {
            let next = half_edges[(i + 1) % n];
            let prev = half_edges[(i + n - 1) % n];
            self.half_edges[half_edges[i]].next = Some(next);
            self.half_edges[half_edges[i]].prev = Some(prev);
            self.half_edges[half_edges[i]].loop_id = Some(loop_id);
        }
        loop_id
    }

    /// Create a face with an outer loop and surface index.
    pub fn add_face(
        &mut self,
        outer_loop: LoopId,
        surface_index: usize,
        orientation: Orientation,
    ) -> FaceId {
        let face_id = self.faces.insert(Face {
            outer_loop,
            inner_loops: Vec::new(),
            surface_index,
            orientation,
            shell: None,
        });
        self.loops[outer_loop].face = Some(face_id);
        face_id
    }

    /// Add an inner loop (hole) to a face.
    pub fn add_inner_loop(&mut self, face_id: FaceId, inner_loop: LoopId) {
        self.faces[face_id].inner_loops.push(inner_loop);
        self.loops[inner_loop].face = Some(face_id);
    }

    /// Create a shell from a set of faces.
    pub fn add_shell(&mut self, faces: Vec<FaceId>, shell_type: ShellType) -> ShellId {
        let shell_id = self.shells.insert(Shell {
            faces: faces.clone(),
            solid: None,
            shell_type,
        });
        for &f in &faces {
            self.faces[f].shell = Some(shell_id);
        }
        shell_id
    }

    /// Create a solid with an outer shell.
    pub fn add_solid(&mut self, outer_shell: ShellId) -> SolidId {
        let solid_id = self.solids.insert(Solid {
            outer_shell,
            void_shells: Vec::new(),
        });
        self.shells[outer_shell].solid = Some(solid_id);
        solid_id
    }

    // =========================================================================
    // Euler operators
    // =========================================================================

    /// Make an initial vertex, face (with a degenerate single-vertex loop),
    /// and shell. This is the starting point for building B-rep topology.
    ///
    /// Returns `(vertex, face, shell)`.
    pub fn make_vertex_face_shell(
        &mut self,
        point: Point3,
        surface_index: usize,
    ) -> (VertexId, FaceId, ShellId) {
        let v = self.add_vertex(point);
        let he = self.add_half_edge(v);
        // Self-loop
        self.half_edges[he].next = Some(he);
        self.half_edges[he].prev = Some(he);
        let loop_id = self.loops.insert(Loop {
            half_edge: he,
            face: None,
        });
        self.half_edges[he].loop_id = Some(loop_id);
        let face = self.add_face(loop_id, surface_index, Orientation::Forward);
        let shell = self.add_shell(vec![face], ShellType::Outer);
        (v, face, shell)
    }

    /// Split an edge by inserting a new vertex and a new edge.
    /// Given half-edge `he` from `v1` to `v2`, inserts vertex `v_new` at `point`
    /// and creates edge `v1 -> v_new`, with `he` becoming `v_new -> v2`.
    ///
    /// Returns `(new_vertex, new_edge)`.
    pub fn make_edge_vertex(&mut self, he: HalfEdgeId, point: Point3) -> (VertexId, EdgeId) {
        let v_new = self.add_vertex(point);
        let he_new = self.add_half_edge(v_new);
        let he_twin_new = self.add_half_edge(self.half_edges[he].origin);

        // Link new half-edge into the loop before he
        let prev = self.half_edges[he].prev;
        self.half_edges[he_new].next = Some(he);
        self.half_edges[he_new].prev = prev;
        self.half_edges[he].prev = Some(he_new);
        if let Some(p) = prev {
            self.half_edges[p].next = Some(he_new);
        }
        self.half_edges[he_new].loop_id = self.half_edges[he].loop_id;

        // Handle twin side
        if let Some(twin) = self.half_edges[he].twin {
            let twin_next = self.half_edges[twin].next;
            self.half_edges[he_twin_new].next = twin_next;
            self.half_edges[he_twin_new].prev = Some(twin);
            self.half_edges[twin].next = Some(he_twin_new);
            if let Some(tn) = twin_next {
                self.half_edges[tn].prev = Some(he_twin_new);
            }
            self.half_edges[he_twin_new].loop_id = self.half_edges[twin].loop_id;
            // Update twin's origin to new vertex
            self.half_edges[twin].origin = v_new;
        }

        // Create edge between new half-edges
        let edge = self.add_edge(he_new, he_twin_new);
        // Update he's origin to new vertex
        self.half_edges[he].origin = v_new;
        self.vertices[v_new].half_edge = Some(he);

        (v_new, edge)
    }

    // =========================================================================
    // Adjacency iterators
    // =========================================================================

    /// Iterate half-edges around a loop (following `next` pointers).
    pub fn loop_half_edges(&self, loop_id: LoopId) -> LoopHalfEdgeIter<'_> {
        let start = self.loops[loop_id].half_edge;
        LoopHalfEdgeIter {
            topo: self,
            start,
            current: Some(start),
            started: false,
        }
    }

    /// Iterate half-edges emanating from a vertex (star traversal).
    pub fn vertex_half_edges(&self, vertex_id: VertexId) -> VertexHalfEdgeIter<'_> {
        let start = self.vertices[vertex_id].half_edge;
        VertexHalfEdgeIter {
            topo: self,
            start,
            current: start,
            started: false,
        }
    }

    /// Get the faces adjacent to an edge (the faces of its two half-edges).
    pub fn edge_faces(&self, edge_id: EdgeId) -> (Option<FaceId>, Option<FaceId>) {
        let he1 = self.edges[edge_id].half_edge;
        let he2 = self.half_edges[he1].twin;
        let f1 = self.half_edges[he1]
            .loop_id
            .and_then(|l| self.loops[l].face);
        let f2 = he2
            .and_then(|h| self.half_edges[h].loop_id)
            .and_then(|l| self.loops[l].face);
        (f1, f2)
    }

    /// Get vertices of a loop in order.
    pub fn loop_vertices(&self, loop_id: LoopId) -> Vec<VertexId> {
        self.loop_half_edges(loop_id)
            .map(|he| self.half_edges[he].origin)
            .collect()
    }

    /// Get the destination vertex of a half-edge (origin of next).
    ///
    /// Falls back to the twin's origin when the half-edge is not in a loop.
    /// That case is real, not hypothetical: a STEP import builds both
    /// directions of every edge, but only the half-edges used by a face loop
    /// get `next` wired up — so an edge on a face the reader skipped (or one
    /// bounding a single face) has a loop-less side. Panicking there took down
    /// STEP *export* of any imported solid; the twin's origin is the same
    /// vertex the loop would have named.
    pub fn half_edge_dest(&self, he: HalfEdgeId) -> VertexId {
        self.half_edge_dest_opt(he)
            .expect("half-edge has neither next nor twin")
    }

    /// Destination vertex of a half-edge, or `None` when neither `next` nor a
    /// twin resolves it.
    pub fn half_edge_dest_opt(&self, he: HalfEdgeId) -> Option<VertexId> {
        if let Some(next) = self.half_edges[he].next {
            return Some(self.half_edges[next].origin);
        }
        let twin = self.half_edges[he].twin?;
        Some(self.half_edges[twin].origin)
    }

    /// Count half-edges in a loop.
    pub fn loop_len(&self, loop_id: LoopId) -> usize {
        self.loop_half_edges(loop_id).count()
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator over half-edges in a loop.
pub struct LoopHalfEdgeIter<'a> {
    topo: &'a Topology,
    start: HalfEdgeId,
    current: Option<HalfEdgeId>,
    started: bool,
}

impl<'a> Iterator for LoopHalfEdgeIter<'a> {
    type Item = HalfEdgeId;

    fn next(&mut self) -> Option<HalfEdgeId> {
        let current = self.current?;
        if self.started && current == self.start {
            return None;
        }
        self.started = true;
        self.current = self.topo.half_edges[current].next;
        Some(current)
    }
}

/// Iterator over half-edges emanating from a vertex.
pub struct VertexHalfEdgeIter<'a> {
    topo: &'a Topology,
    start: Option<HalfEdgeId>,
    current: Option<HalfEdgeId>,
    started: bool,
}

impl<'a> Iterator for VertexHalfEdgeIter<'a> {
    type Item = HalfEdgeId;

    fn next(&mut self) -> Option<HalfEdgeId> {
        let start = self.start?;
        let current = self.current?;
        if self.started && current == start {
            return None;
        }
        self.started = true;
        // Move to next outgoing edge: twin of current, then next
        let twin = self.topo.half_edges[current].twin?;
        self.current = self.topo.half_edges[twin].next;
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_vertex() {
        let mut topo = Topology::new();
        let v = topo.add_vertex(Point3::new(1.0, 2.0, 3.0));
        assert_eq!(topo.vertices[v].point, Point3::new(1.0, 2.0, 3.0));
        assert!(topo.vertices[v].half_edge.is_none());
    }

    #[test]
    fn test_add_edge_and_loop() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::new(0.0, 0.0, 0.0));
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));
        let v2 = topo.add_vertex(Point3::new(0.0, 1.0, 0.0));

        // Create half-edges for a triangle
        let he0 = topo.add_half_edge(v0); // v0 -> v1
        let he1 = topo.add_half_edge(v1); // v1 -> v2
        let he2 = topo.add_half_edge(v2); // v2 -> v0

        let loop_id = topo.add_loop(&[he0, he1, he2]);

        // Check loop traversal
        let verts = topo.loop_vertices(loop_id);
        assert_eq!(verts.len(), 3);
        assert_eq!(verts[0], v0);
        assert_eq!(verts[1], v1);
        assert_eq!(verts[2], v2);
    }

    #[test]
    fn test_loop_len() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::origin());
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));
        let v2 = topo.add_vertex(Point3::new(1.0, 1.0, 0.0));
        let v3 = topo.add_vertex(Point3::new(0.0, 1.0, 0.0));

        let he0 = topo.add_half_edge(v0);
        let he1 = topo.add_half_edge(v1);
        let he2 = topo.add_half_edge(v2);
        let he3 = topo.add_half_edge(v3);

        let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
        assert_eq!(topo.loop_len(loop_id), 4);
    }

    #[test]
    fn test_edge_faces() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::origin());
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));
        let v2 = topo.add_vertex(Point3::new(0.0, 1.0, 0.0));
        let v3 = topo.add_vertex(Point3::new(0.0, 0.0, 1.0));

        // Two triangles sharing edge v0-v1
        let he_a0 = topo.add_half_edge(v0); // face A: v0->v1
        let he_a1 = topo.add_half_edge(v1); // face A: v1->v2
        let he_a2 = topo.add_half_edge(v2); // face A: v2->v0

        let he_b0 = topo.add_half_edge(v1); // face B: v1->v0 (twin of he_a0)
        let he_b1 = topo.add_half_edge(v0); // face B: v0->v3
        let he_b2 = topo.add_half_edge(v3); // face B: v3->v1

        let loop_a = topo.add_loop(&[he_a0, he_a1, he_a2]);
        let loop_b = topo.add_loop(&[he_b0, he_b1, he_b2]);

        let face_a = topo.add_face(loop_a, 0, Orientation::Forward);
        let face_b = topo.add_face(loop_b, 1, Orientation::Forward);

        let edge_id = topo.add_edge(he_a0, he_b0);

        let (f1, f2) = topo.edge_faces(edge_id);
        assert_eq!(f1, Some(face_a));
        assert_eq!(f2, Some(face_b));
    }

    #[test]
    fn test_half_edge_dest() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::origin());
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));
        let v2 = topo.add_vertex(Point3::new(0.0, 1.0, 0.0));

        let he0 = topo.add_half_edge(v0);
        let he1 = topo.add_half_edge(v1);
        let he2 = topo.add_half_edge(v2);
        topo.add_loop(&[he0, he1, he2]);

        assert_eq!(topo.half_edge_dest(he0), v1);
        assert_eq!(topo.half_edge_dest(he1), v2);
        assert_eq!(topo.half_edge_dest(he2), v0);
    }
}
