//! STEP file reader: converts parsed STEP data to BRepSolid.

use std::collections::HashMap;
use std::path::Path;

use crate::entities::{
    curves::{parse_curve, StepCurve},
    parse_advanced_face, parse_edge_curve, parse_edge_loop, parse_manifold_solid_brep,
    parse_oriented_edge, parse_shell, parse_surface, parse_vertex_point,
};
use crate::error::StepError;
use stepperoni::{Parser, StepFile};

use vcad_kernel_geom::{Curve3d, GeometryStore};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, HalfEdgeId, LoopId, Orientation, ShellType, Topology, VertexId};

/// Number of intermediate points to sample along a curved edge (arc or B-spline).
const CURVE_SAMPLE_COUNT: usize = 8;

/// Hard ceilings on how much topology we will try to parse from a single
/// STEP solid. They exist purely to keep a malicious file from pinning the
/// reader in a multi-hour allocation loop; legitimate CAD models come
/// nowhere near these numbers.
const MAX_FACES_PER_SOLID: usize = 200_000;
const MAX_EDGES_PER_LOOP: usize = 100_000;

/// Compute the absolute enclosed area of a topology loop using Newell's method.
/// Works in 3D — the magnitude of the cross-product sum gives twice the area.
fn loop_area_3d(topo: &Topology, loop_id: LoopId) -> f64 {
    let verts: Vec<vcad_kernel_math::Point3> = topo
        .loop_half_edges(loop_id)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();
    let n = verts.len();
    if n < 3 {
        return 0.0;
    }
    let mut cross = vcad_kernel_math::Vec3::zeros();
    for i in 0..n {
        let c = verts[i];
        let nx = verts[(i + 1) % n];
        cross.x += (c.y - nx.y) * (c.z + nx.z);
        cross.y += (c.z - nx.z) * (c.x + nx.x);
        cross.z += (c.x - nx.x) * (c.y + nx.y);
    }
    cross.norm() * 0.5
}

/// Compute the angle parameter of a point on a circle.
fn point_angle_on_circle(
    circle: &vcad_kernel_geom::Circle3d,
    pt: &vcad_kernel_math::Point3,
) -> f64 {
    let d = *pt - circle.center;
    let x = d.dot(circle.x_dir.as_ref());
    let y = d.dot(circle.y_dir.as_ref());
    y.atan2(x)
}

/// Read STEP file from a path.
///
/// # Arguments
///
/// * `path` - Path to the STEP file
///
/// # Returns
///
/// A vector of B-rep solids found in the file.
pub fn read_step(path: impl AsRef<Path>) -> Result<Vec<BRepSolid>, StepError> {
    let data = std::fs::read(path)?;
    read_step_from_buffer(&data)
}

/// Read STEP file from a byte buffer.
///
/// # Arguments
///
/// * `data` - Raw STEP file contents
///
/// # Returns
///
/// A vector of B-rep solids found in the file.
pub fn read_step_from_buffer(data: &[u8]) -> Result<Vec<BRepSolid>, StepError> {
    let step_file = Parser::parse(data)?;
    let mut reader = StepReader::new(&step_file);
    reader.read_all_solids()
}

/// Read a single solid by its `MANIFOLD_SOLID_BREP` entity ID.
///
/// Creates a fresh reader state so maps from other solids don't bleed in.
pub(crate) fn read_solid_from_file(file: &StepFile, solid_id: u64) -> Result<BRepSolid, StepError> {
    let mut reader = StepReader::new(file);
    reader.read_solid(solid_id)
}

/// Context for reading STEP files and building B-rep solids.
pub(crate) struct StepReader<'a> {
    pub(crate) file: &'a StepFile,
    /// Maps STEP vertex ID to vcad VertexId.
    vertex_map: HashMap<u64, VertexId>,
    /// Maps STEP edge ID to vcad EdgeId (pair of half-edges).
    edge_map: HashMap<u64, EdgeId>,
    /// Maps (STEP edge ID, orientation) to vcad HalfEdgeId.
    half_edge_map: HashMap<(u64, bool), HalfEdgeId>,
    /// Maps STEP surface ID to vcad geometry store index.
    surface_map: HashMap<u64, usize>,
    /// For subdivided curved edges: maps (STEP edge ID, orientation) to
    /// an ordered chain of half-edge IDs that replace the single original.
    subdivided_edges: HashMap<(u64, bool), Vec<HalfEdgeId>>,
}

impl<'a> StepReader<'a> {
    pub(crate) fn new(file: &'a StepFile) -> Self {
        Self {
            file,
            vertex_map: HashMap::new(),
            edge_map: HashMap::new(),
            half_edge_map: HashMap::new(),
            surface_map: HashMap::new(),
            subdivided_edges: HashMap::new(),
        }
    }

    fn read_all_solids(&mut self) -> Result<Vec<BRepSolid>, StepError> {
        let solid_entities = self.file.entities_of_type("MANIFOLD_SOLID_BREP");
        if solid_entities.is_empty() {
            return Err(StepError::NoSolids);
        }

        let mut solids = Vec::new();
        for entity in solid_entities {
            // Reset maps for each solid
            self.vertex_map.clear();
            self.edge_map.clear();
            self.half_edge_map.clear();
            self.surface_map.clear();
            self.subdivided_edges.clear();

            let solid = self.read_solid(entity.id)?;
            solids.push(solid);
        }

        Ok(solids)
    }

    pub(crate) fn read_solid(&mut self, solid_id: u64) -> Result<BRepSolid, StepError> {
        use std::collections::HashSet;

        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();

        let step_solid = parse_manifold_solid_brep(self.file, solid_id)?;
        let step_shell = parse_shell(self.file, step_solid.outer_shell_id)?;

        if step_shell.face_ids.len() > MAX_FACES_PER_SOLID {
            return Err(StepError::UnsupportedEntity(format!(
                "shell #{} has {} faces (cap {})",
                step_solid.outer_shell_id,
                step_shell.face_ids.len(),
                MAX_FACES_PER_SOLID
            )));
        }

        // Track faces we skip due to unsupported surface types
        let mut skipped_faces: HashSet<u64> = HashSet::new();

        // First pass: collect all vertices and surfaces
        for &face_id in &step_shell.face_ids {
            let step_face = parse_advanced_face(self.file, face_id)?;

            // Parse and store surface - skip face if surface type unsupported
            if !self.surface_map.contains_key(&step_face.surface_id) {
                match parse_surface(self.file, step_face.surface_id) {
                    Ok(surface) => {
                        let idx = geom.add_surface(surface.into_box());
                        self.surface_map.insert(step_face.surface_id, idx);
                    }
                    Err(StepError::UnsupportedEntity(_)) => {
                        // Skip this face - surface type not supported
                        skipped_faces.insert(face_id);
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }

            // Parse vertices from face bounds
            for bound in &step_face.bounds {
                let loop_ = parse_edge_loop(self.file, bound.loop_id)?;
                if loop_.edge_ids.len() > MAX_EDGES_PER_LOOP {
                    return Err(StepError::UnsupportedEntity(format!(
                        "loop #{} has {} edges (cap {})",
                        bound.loop_id,
                        loop_.edge_ids.len(),
                        MAX_EDGES_PER_LOOP
                    )));
                }
                for &oe_id in &loop_.edge_ids {
                    let oe = parse_oriented_edge(self.file, oe_id)?;
                    let edge = parse_edge_curve(self.file, oe.edge_id)?;

                    // Add vertices
                    if !self.vertex_map.contains_key(&edge.start_vertex_id) {
                        let v = parse_vertex_point(self.file, edge.start_vertex_id)?;
                        let vid = topo.add_vertex(v.point);
                        self.vertex_map.insert(edge.start_vertex_id, vid);
                    }
                    if !self.vertex_map.contains_key(&edge.end_vertex_id) {
                        let v = parse_vertex_point(self.file, edge.end_vertex_id)?;
                        let vid = topo.add_vertex(v.point);
                        self.vertex_map.insert(edge.end_vertex_id, vid);
                    }
                }
            }
        }

        // Second pass: create half-edges and edges.
        // For CIRCLE edges, sample intermediate points along the arc.
        // For B-spline/trimmed-curve edges, sample intermediate points too.
        for &face_id in &step_shell.face_ids {
            if skipped_faces.contains(&face_id) {
                continue;
            }
            let step_face = parse_advanced_face(self.file, face_id)?;

            for bound in &step_face.bounds {
                let loop_ = parse_edge_loop(self.file, bound.loop_id)?;
                for &oe_id in &loop_.edge_ids {
                    let oe = parse_oriented_edge(self.file, oe_id)?;
                    let edge = parse_edge_curve(self.file, oe.edge_id)?;

                    // Skip if already processed
                    if self.half_edge_map.contains_key(&(edge.id, oe.orientation)) {
                        continue;
                    }

                    // Determine half-edge direction based on orientation
                    let (start_v, end_v) = if oe.orientation == edge.same_sense {
                        (edge.start_vertex_id, edge.end_vertex_id)
                    } else {
                        (edge.end_vertex_id, edge.start_vertex_id)
                    };

                    // Try to parse the edge curve
                    let curve = parse_curve(self.file, edge.curve_id).ok();

                    // Determine whether this is a curved edge that needs subdivision
                    let needs_subdivision = matches!(
                        &curve,
                        Some(StepCurve::Circle(_))
                            | Some(StepCurve::BSpline(_))
                            | Some(StepCurve::Trimmed(_))
                    );

                    if needs_subdivision {
                        self.subdivide_curved_edge(
                            &mut topo,
                            &edge,
                            &oe,
                            &curve.unwrap(),
                            start_v,
                            end_v,
                        );
                    } else {
                        // Simple edge: single half-edge per direction
                        let start_vid = self.vertex_map[&start_v];
                        let he_id = topo.add_half_edge(start_vid);
                        self.half_edge_map.insert((edge.id, oe.orientation), he_id);

                        let twin_start = self.vertex_map[&end_v];
                        let twin_he_id = topo.add_half_edge(twin_start);
                        self.half_edge_map
                            .insert((edge.id, !oe.orientation), twin_he_id);

                        use std::collections::hash_map::Entry;
                        if let Entry::Vacant(e) = self.edge_map.entry(edge.id) {
                            let he1 = self.half_edge_map[&(edge.id, true)];
                            let he2 = self.half_edge_map[&(edge.id, false)];
                            let edge_id = topo.add_edge(he1, he2);
                            e.insert(edge_id);
                        }
                    }
                }
            }
        }

        // Third pass: create loops, faces, and shell
        let mut vcad_face_ids = Vec::new();

        for &face_id in &step_shell.face_ids {
            if skipped_faces.contains(&face_id) {
                continue;
            }
            let step_face = parse_advanced_face(self.file, face_id)?;
            let surface_idx = self.surface_map[&step_face.surface_id];

            let mut outer_loop: Option<LoopId> = None;
            let mut inner_loops = Vec::new();

            for bound in &step_face.bounds {
                let loop_ = parse_edge_loop(self.file, bound.loop_id)?;

                // Skip empty loops (VERTEX_LOOP returns empty edge list)
                if loop_.edge_ids.is_empty() {
                    continue;
                }

                // Collect half-edges for this loop.
                // For subdivided curved edges, expand to the chain of sub-half-edges.
                let mut loop_hes = Vec::new();
                for &oe_id in &loop_.edge_ids {
                    let oe = parse_oriented_edge(self.file, oe_id)?;
                    let key = (oe.edge_id, oe.orientation);
                    if let Some(chain) = self.subdivided_edges.get(&key) {
                        loop_hes.extend_from_slice(chain);
                    } else {
                        let he_id = self.half_edge_map[&key];
                        loop_hes.push(he_id);
                    }
                }

                let loop_id = topo.add_loop(&loop_hes);

                if bound.is_outer {
                    outer_loop = Some(loop_id);
                } else {
                    inner_loops.push(loop_id);
                }
            }

            // If no explicit FACE_OUTER_BOUND, pick the loop with the largest
            // enclosed area as the outer loop. Many STEP exporters (e.g. Shapr3D)
            // use FACE_BOUND for everything, and the first bound listed is not
            // necessarily the enclosing boundary.
            if outer_loop.is_none() && !inner_loops.is_empty() {
                let mut best_idx = 0;
                let mut best_area = 0.0f64;
                for (i, &lid) in inner_loops.iter().enumerate() {
                    let area = loop_area_3d(&topo, lid);
                    if area > best_area {
                        best_area = area;
                        best_idx = i;
                    }
                }
                outer_loop = Some(inner_loops.remove(best_idx));
            }

            // Create face - skip if no bounds at all
            let outer = match outer_loop {
                Some(l) => l,
                None => continue,
            };

            let orientation = if step_face.same_sense {
                Orientation::Forward
            } else {
                Orientation::Reversed
            };

            let face_id = topo.add_face(outer, surface_idx, orientation);
            for inner in inner_loops {
                topo.add_inner_loop(face_id, inner);
            }
            vcad_face_ids.push(face_id);
        }

        // Create shell and solid
        // Note: vcad doesn't distinguish open/closed shells yet, so we always use Outer
        let _ = step_shell.is_closed; // acknowledged but unused
        let shell_id = topo.add_shell(vcad_face_ids, ShellType::Outer);
        let solid_id = topo.add_solid(shell_id);

        Ok(BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        })
    }

    /// Subdivide a curved edge (circle, B-spline, or trimmed variant) into multiple
    /// segments by sampling intermediate points. This ensures loops with only 2
    /// vertices get enough points for tessellation.
    fn subdivide_curved_edge(
        &mut self,
        topo: &mut Topology,
        edge: &crate::entities::topology::StepEdge,
        oe: &crate::entities::topology::StepOrientedEdge,
        curve: &StepCurve,
        start_v: u64,
        end_v: u64,
    ) {
        // Compute sample parameters [t_start, ..., t_end]
        let mid_pts = self.sample_curve_intermediates(topo, curve, edge, oe, start_v, end_v);

        let start_vid = self.vertex_map[&start_v];
        let end_vid = self.vertex_map[&end_v];
        let n = mid_pts.len();

        // Build forward chain: start → m0 → m1 → ... → mn-1 → (end = next edge's start)
        let mut fwd_origins = vec![start_vid];
        fwd_origins.extend_from_slice(&mid_pts);

        // Reverse chain: end → mn-1 → ... → m0 → (start = next edge's start)
        let mut rev_origins = vec![end_vid];
        for &vid in mid_pts.iter().rev() {
            rev_origins.push(vid);
        }

        let mut fwd_chain = Vec::with_capacity(n + 1);
        let mut rev_chain = Vec::with_capacity(n + 1);

        for &origin in &fwd_origins {
            let he = topo.add_half_edge(origin);
            fwd_chain.push(he);
        }
        for &origin in &rev_origins {
            let he = topo.add_half_edge(origin);
            rev_chain.push(he);
        }

        // Pair forward[i] with reverse[n-i] into edges
        for i in 0..fwd_chain.len() {
            let fwd_he = fwd_chain[i];
            let rev_he = rev_chain[fwd_chain.len() - 1 - i];
            topo.add_edge(fwd_he, rev_he);
        }

        self.subdivided_edges
            .insert((edge.id, oe.orientation), fwd_chain);
        self.subdivided_edges
            .insert((edge.id, !oe.orientation), rev_chain);
    }

    /// Compute intermediate 3D sample points along a curved edge.
    ///
    /// Returns a list of `VertexId`s (not including the start/end vertices)
    /// ordered from start to end.
    fn sample_curve_intermediates(
        &mut self,
        topo: &mut Topology,
        curve: &StepCurve,
        edge: &crate::entities::topology::StepEdge,
        oe: &crate::entities::topology::StepOrientedEdge,
        start_v: u64,
        end_v: u64,
    ) -> Vec<VertexId> {
        let resolved = curve.resolve();
        match resolved {
            StepCurve::Circle(circle) => {
                self.sample_circle_arc(topo, circle, curve, edge, oe, start_v, end_v)
            }
            StepCurve::BSpline(_) => self.sample_bspline(topo, curve, edge, oe),
            // Line or other simple curves don't need intermediate points
            _ => vec![],
        }
    }

    /// Sample a circle arc using either trim parameters (if available via TRIMMED_CURVE)
    /// or vertex endpoint projection.
    #[allow(clippy::too_many_arguments)]
    fn sample_circle_arc(
        &mut self,
        topo: &mut Topology,
        circle: &vcad_kernel_geom::Circle3d,
        curve: &StepCurve,
        edge: &crate::entities::topology::StepEdge,
        oe: &crate::entities::topology::StepOrientedEdge,
        start_v: u64,
        end_v: u64,
    ) -> Vec<VertexId> {
        let start_pt = topo.vertices[self.vertex_map[&start_v]].point;
        let end_pt = topo.vertices[self.vertex_map[&end_v]].point;

        // Prefer explicit trim parameters from TRIMMED_CURVE wrapper
        let (t_start, t_end, forward) = if let StepCurve::Trimmed(tc) = curve {
            let fwd = oe.orientation == edge.same_sense;
            if fwd == tc.sense_agreement {
                (tc.trim1, tc.trim2, true)
            } else {
                (tc.trim2, tc.trim1, false)
            }
        } else {
            // Fall back to projecting vertex endpoints onto the circle
            let ts = point_angle_on_circle(circle, &start_pt);
            let te = point_angle_on_circle(circle, &end_pt);
            (ts, te, oe.orientation == edge.same_sense)
        };

        // Compute the arc sweep (always in the direction of traversal)
        let sweep = if forward {
            let mut s = t_end - t_start;
            if s <= 1e-10 {
                s += std::f64::consts::TAU;
            }
            s
        } else {
            let mut s = t_start - t_end;
            if s <= 1e-10 {
                s += std::f64::consts::TAU;
            }
            -s
        };

        let n = CURVE_SAMPLE_COUNT;
        let mut mid_vids = Vec::with_capacity(n);
        for i in 1..=n {
            let frac = i as f64 / (n + 1) as f64;
            let t = t_start + sweep * frac;
            let pt = circle.evaluate(t);
            let vid = topo.add_vertex(pt);
            mid_vids.push(vid);
        }
        mid_vids
    }

    /// Sample a B-spline (or trimmed B-spline) curve at `CURVE_SAMPLE_COUNT` points.
    fn sample_bspline(
        &mut self,
        topo: &mut Topology,
        curve: &StepCurve,
        _edge: &crate::entities::topology::StepEdge,
        _oe: &crate::entities::topology::StepOrientedEdge,
    ) -> Vec<VertexId> {
        let (t_min, t_max) = curve.domain();
        let n = CURVE_SAMPLE_COUNT;
        let mut mid_vids = Vec::with_capacity(n);
        for i in 1..=n {
            let frac = i as f64 / (n + 1) as f64;
            let t = t_min + (t_max - t_min) * frac;
            let pt = curve.evaluate(t);
            let vid = topo.add_vertex(pt);
            mid_vids.push(vid);
        }
        mid_vids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_simple_box() {
        // A minimal STEP file representing a simple box
        let step_content = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
FILE_NAME('box.step', '2024-01-01', (''), (''), '', '', '');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
/* Points */
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#3 = CARTESIAN_POINT('', (10.0, 10.0, 0.0));
#4 = CARTESIAN_POINT('', (0.0, 10.0, 0.0));
#5 = CARTESIAN_POINT('', (0.0, 0.0, 10.0));
#6 = CARTESIAN_POINT('', (10.0, 0.0, 10.0));
#7 = CARTESIAN_POINT('', (10.0, 10.0, 10.0));
#8 = CARTESIAN_POINT('', (0.0, 10.0, 10.0));

/* Vertices */
#11 = VERTEX_POINT('', #1);
#12 = VERTEX_POINT('', #2);
#13 = VERTEX_POINT('', #3);
#14 = VERTEX_POINT('', #4);
#15 = VERTEX_POINT('', #5);
#16 = VERTEX_POINT('', #6);
#17 = VERTEX_POINT('', #7);
#18 = VERTEX_POINT('', #8);

/* Directions for placements */
#20 = DIRECTION('', (0.0, 0.0, 1.0));
#21 = DIRECTION('', (1.0, 0.0, 0.0));
#22 = DIRECTION('', (0.0, 0.0, -1.0));
#23 = DIRECTION('', (0.0, 1.0, 0.0));
#24 = DIRECTION('', (0.0, -1.0, 0.0));
#25 = DIRECTION('', (-1.0, 0.0, 0.0));

/* Axis placements for surfaces */
#30 = AXIS2_PLACEMENT_3D('', #1, #22, #21);
#31 = AXIS2_PLACEMENT_3D('', #5, #20, #21);
#32 = AXIS2_PLACEMENT_3D('', #1, #24, #21);
#33 = AXIS2_PLACEMENT_3D('', #4, #23, #21);
#34 = AXIS2_PLACEMENT_3D('', #1, #25, #23);
#35 = AXIS2_PLACEMENT_3D('', #2, #21, #23);

/* Surfaces */
#40 = PLANE('', #30);
#41 = PLANE('', #31);
#42 = PLANE('', #32);
#43 = PLANE('', #33);
#44 = PLANE('', #34);
#45 = PLANE('', #35);

/* Directions for line vectors */
#50 = DIRECTION('', (1.0, 0.0, 0.0));
#51 = DIRECTION('', (0.0, 1.0, 0.0));
#52 = DIRECTION('', (0.0, 0.0, 1.0));
#53 = DIRECTION('', (-1.0, 0.0, 0.0));
#54 = DIRECTION('', (0.0, -1.0, 0.0));
#55 = DIRECTION('', (0.0, 0.0, -1.0));

/* Vectors for lines */
#60 = VECTOR('', #50, 10.0);
#61 = VECTOR('', #51, 10.0);
#62 = VECTOR('', #52, 10.0);
#63 = VECTOR('', #53, 10.0);
#64 = VECTOR('', #54, 10.0);
#65 = VECTOR('', #55, 10.0);

/* Lines */
#70 = LINE('', #1, #60);
#71 = LINE('', #2, #61);
#72 = LINE('', #3, #63);
#73 = LINE('', #4, #64);
#74 = LINE('', #1, #61);
#75 = LINE('', #5, #60);
#76 = LINE('', #6, #61);
#77 = LINE('', #7, #63);
#78 = LINE('', #8, #64);
#79 = LINE('', #1, #62);
#80 = LINE('', #2, #62);
#81 = LINE('', #3, #62);
#82 = LINE('', #4, #62);

/* Edges - bottom face */
#100 = EDGE_CURVE('', #11, #12, #70, .T.);
#101 = EDGE_CURVE('', #12, #13, #71, .T.);
#102 = EDGE_CURVE('', #13, #14, #72, .T.);
#103 = EDGE_CURVE('', #14, #11, #73, .T.);

/* Edges - top face */
#104 = EDGE_CURVE('', #15, #16, #75, .T.);
#105 = EDGE_CURVE('', #16, #17, #76, .T.);
#106 = EDGE_CURVE('', #17, #18, #77, .T.);
#107 = EDGE_CURVE('', #18, #15, #78, .T.);

/* Edges - vertical */
#108 = EDGE_CURVE('', #11, #15, #79, .T.);
#109 = EDGE_CURVE('', #12, #16, #80, .T.);
#110 = EDGE_CURVE('', #13, #17, #81, .T.);
#111 = EDGE_CURVE('', #14, #18, #82, .T.);

/* Oriented edges - bottom face (CCW from below = CW from above) */
#120 = ORIENTED_EDGE('', *, *, #100, .F.);
#121 = ORIENTED_EDGE('', *, *, #103, .F.);
#122 = ORIENTED_EDGE('', *, *, #102, .F.);
#123 = ORIENTED_EDGE('', *, *, #101, .F.);

/* Oriented edges - top face (CCW from above) */
#124 = ORIENTED_EDGE('', *, *, #104, .T.);
#125 = ORIENTED_EDGE('', *, *, #105, .T.);
#126 = ORIENTED_EDGE('', *, *, #106, .T.);
#127 = ORIENTED_EDGE('', *, *, #107, .T.);

/* Oriented edges - front face */
#130 = ORIENTED_EDGE('', *, *, #100, .T.);
#131 = ORIENTED_EDGE('', *, *, #109, .T.);
#132 = ORIENTED_EDGE('', *, *, #104, .F.);
#133 = ORIENTED_EDGE('', *, *, #108, .F.);

/* Oriented edges - back face */
#134 = ORIENTED_EDGE('', *, *, #102, .T.);
#135 = ORIENTED_EDGE('', *, *, #111, .T.);
#136 = ORIENTED_EDGE('', *, *, #106, .F.);
#137 = ORIENTED_EDGE('', *, *, #110, .F.);

/* Oriented edges - left face */
#138 = ORIENTED_EDGE('', *, *, #103, .T.);
#139 = ORIENTED_EDGE('', *, *, #108, .T.);
#140 = ORIENTED_EDGE('', *, *, #107, .F.);
#141 = ORIENTED_EDGE('', *, *, #111, .F.);

/* Oriented edges - right face */
#142 = ORIENTED_EDGE('', *, *, #101, .T.);
#143 = ORIENTED_EDGE('', *, *, #110, .T.);
#144 = ORIENTED_EDGE('', *, *, #105, .F.);
#145 = ORIENTED_EDGE('', *, *, #109, .F.);

/* Edge loops */
#150 = EDGE_LOOP('', (#120, #121, #122, #123));
#151 = EDGE_LOOP('', (#124, #125, #126, #127));
#152 = EDGE_LOOP('', (#130, #131, #132, #133));
#153 = EDGE_LOOP('', (#134, #135, #136, #137));
#154 = EDGE_LOOP('', (#138, #139, #140, #141));
#155 = EDGE_LOOP('', (#142, #143, #144, #145));

/* Face bounds */
#160 = FACE_OUTER_BOUND('', #150, .T.);
#161 = FACE_OUTER_BOUND('', #151, .T.);
#162 = FACE_OUTER_BOUND('', #152, .T.);
#163 = FACE_OUTER_BOUND('', #153, .T.);
#164 = FACE_OUTER_BOUND('', #154, .T.);
#165 = FACE_OUTER_BOUND('', #155, .T.);

/* Faces */
#170 = ADVANCED_FACE('', (#160), #40, .T.);
#171 = ADVANCED_FACE('', (#161), #41, .T.);
#172 = ADVANCED_FACE('', (#162), #42, .T.);
#173 = ADVANCED_FACE('', (#163), #43, .T.);
#174 = ADVANCED_FACE('', (#164), #44, .T.);
#175 = ADVANCED_FACE('', (#165), #45, .T.);

/* Shell */
#180 = CLOSED_SHELL('', (#170, #171, #172, #173, #174, #175));

/* Solid */
#190 = MANIFOLD_SOLID_BREP('Box', #180);

ENDSEC;
END-ISO-10303-21;
"#;

        let solids = read_step_from_buffer(step_content.as_bytes()).unwrap();
        assert_eq!(solids.len(), 1);

        let solid = &solids[0];
        assert_eq!(solid.topology.vertices.len(), 8);
        assert_eq!(solid.topology.faces.len(), 6);
        assert_eq!(solid.geometry.surfaces.len(), 6);
    }

    #[test]
    fn test_no_solids() {
        let step_content = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let result = read_step_from_buffer(step_content.as_bytes());
        assert!(matches!(result, Err(StepError::NoSolids)));
    }
}
