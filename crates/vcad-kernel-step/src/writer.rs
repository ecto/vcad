//! STEP file writer: converts BRepSolid to STEP format.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::entities::{
    bilinear_planar_to_placement, compress_knots, cylinder_to_placement, plane_to_placement,
    sphere_to_placement, torus_to_placement, write_advanced_face, write_axis2_placement_3d,
    write_bspline_control_points, write_bspline_surface_with_knots, write_cartesian_point,
    write_closed_shell, write_conical_surface, write_cylindrical_surface, write_direction,
    write_edge_curve, write_edge_loop, write_face_bound, write_manifold_solid_brep,
    write_oriented_edge, write_plane, write_spherical_surface, write_toroidal_surface,
    write_vertex_point, AxisPlacement,
};
use crate::error::StepError;
use crate::reconstruct::{
    canonical_split, classify_chain, fit_tol, plan_edge_merges, ChainGeom, ChainPlan,
    EdgeMergePlan, SegEnd,
};

use vcad_kernel_geom::{
    BilinearSurface, ConeSurface, CylinderSurface, Plane, SphereSurface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_nurbs::BSplineSurface;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, LoopId, Orientation, VertexId};

/// Write a BRepSolid to a STEP file.
///
/// # Arguments
///
/// * `solid` - The B-rep solid to write
/// * `path` - Output file path
pub fn write_step(solid: &BRepSolid, path: impl AsRef<Path>) -> Result<(), StepError> {
    let buffer = write_step_to_buffer(solid)?;
    std::fs::write(path, buffer)?;
    Ok(())
}

/// Write a BRepSolid to a STEP format byte buffer.
///
/// # Arguments
///
/// * `solid` - The B-rep solid to write
///
/// # Returns
///
/// The STEP file contents as bytes.
pub fn write_step_to_buffer(solid: &BRepSolid) -> Result<Vec<u8>, StepError> {
    write_step_solids_to_buffer(&[(solid, "Solid")])
}

/// Write several BRepSolids into a single STEP file.
///
/// Each entry is `(solid, name)`; the name lands on the
/// `MANIFOLD_SOLID_BREP` entity so downstream CAD shows one body per part.
///
/// # Arguments
///
/// * `solids` - The B-rep solids to write, with per-body names
///
/// # Returns
///
/// The STEP file contents as bytes.
pub fn write_step_solids_to_buffer(solids: &[(&BRepSolid, &str)]) -> Result<Vec<u8>, StepError> {
    if solids.is_empty() {
        return Err(StepError::InvalidGeometry(
            "no solids to write to STEP".into(),
        ));
    }
    let mut entities: Vec<String> = Vec::new();
    let mut next_id = 1;
    let mut solid_ids: Vec<u64> = Vec::new();
    for (solid, name) in solids {
        let mut writer = StepWriter::with_start_id(solid, next_id);
        solid_ids.push(writer.write_entities(name)?);
        next_id = writer.next_id;
        entities.extend(writer.output);
    }
    // Product/representation anchor: without this layer (context with UNITS,
    // ADVANCED_BREP_SHAPE_REPRESENTATION, PRODUCT chain, and the
    // SHAPE_DEFINITION_REPRESENTATION that ties them together) conforming
    // importers traverse PRODUCT -> SDR -> representation, find nothing, and
    // report "no 3D geometry" even though the MANIFOLD_SOLID_BREPs are in the
    // file. vcad's own reader scans for solids directly, which is why a
    // round-trip through vcad alone cannot catch the omission.
    write_product_anchor(&mut entities, &mut next_id, &solid_ids);
    assemble_file(&entities)
}

/// Append the AP214 product structure + representation context that anchor
/// the solids for conforming importers: SI units (mm), uncertainty, one
/// PRODUCT, and an ADVANCED_BREP_SHAPE_REPRESENTATION holding every solid.
fn write_product_anchor(entities: &mut Vec<String>, next_id: &mut u64, solid_ids: &[u64]) {
    let mut id = || {
        let v = *next_id;
        *next_id += 1;
        v
    };
    let app = id();
    entities.push(format!(
        "#{app} = APPLICATION_CONTEXT('automotive design');"
    ));
    let apd = id();
    entities.push(format!(
        "#{apd} = APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2010,#{app});"
    ));
    let pctx = id();
    entities.push(format!(
        "#{pctx} = PRODUCT_CONTEXT('',#{app},'mechanical');"
    ));
    let prod = id();
    entities.push(format!(
        "#{prod} = PRODUCT('vcad_part','vcad_part','',(#{pctx}));"
    ));
    let prpc = id();
    entities.push(format!(
        "#{prpc} = PRODUCT_RELATED_PRODUCT_CATEGORY('part','',(#{prod}));"
    ));
    let pdf = id();
    entities.push(format!(
        "#{pdf} = PRODUCT_DEFINITION_FORMATION('','',#{prod});"
    ));
    let pdctx = id();
    entities.push(format!(
        "#{pdctx} = PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design');"
    ));
    let pd = id();
    entities.push(format!(
        "#{pd} = PRODUCT_DEFINITION('design','',#{pdf},#{pdctx});"
    ));
    let pds = id();
    entities.push(format!("#{pds} = PRODUCT_DEFINITION_SHAPE('','',#{pd});"));
    let len_u = id();
    entities.push(format!(
        "#{len_u} = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );"
    ));
    let ang_u = id();
    entities.push(format!(
        "#{ang_u} = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );"
    ));
    let sol_u = id();
    entities.push(format!(
        "#{sol_u} = ( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );"
    ));
    // 1e-6 is the spec-conventional distance accuracy, and it is honest again:
    // circular boundaries are reconstructed as CIRCLE edges before writing
    // (see reconstruct.rs), so edges lie on their faces' analytic surfaces
    // exactly instead of deviating by the tessellation chord sagitta.
    let unc = id();
    entities.push(format!(
        "#{unc} = UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.0E-6),#{len_u},'distance_accuracy_value','');"
    ));
    let ctx = id();
    entities.push(format!(
        "#{ctx} = ( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{len_u},#{ang_u},#{sol_u})) REPRESENTATION_CONTEXT('','3D') );"
    ));
    let items: Vec<String> = solid_ids.iter().map(|s| format!("#{s}")).collect();
    let absr = id();
    entities.push(format!(
        "#{absr} = ADVANCED_BREP_SHAPE_REPRESENTATION('',({}),#{ctx});",
        items.join(",")
    ));
    let sdr = id();
    entities.push(format!(
        "#{sdr} = SHAPE_DEFINITION_REPRESENTATION(#{pds},#{absr});"
    ));
}

/// Write several named BRepSolids to a single STEP file on disk.
pub fn write_step_solids(
    solids: &[(&BRepSolid, &str)],
    path: impl AsRef<Path>,
) -> Result<(), StepError> {
    let buffer = write_step_solids_to_buffer(solids)?;
    std::fs::write(path, buffer)?;
    Ok(())
}

/// Wrap entity lines in the ISO-10303-21 header/footer.
fn assemble_file(entities: &[String]) -> Result<Vec<u8>, StepError> {
    let mut buffer = Vec::new();
    writeln!(buffer, "ISO-10303-21;")?;
    writeln!(buffer, "HEADER;")?;
    writeln!(
        buffer,
        "FILE_DESCRIPTION(('STEP file generated by vcad'), '2;1');"
    )?;
    writeln!(
        buffer,
        "FILE_NAME('model.step', '{}', ('vcad'), ('vcad'), 'vcad-kernel-step', 'vcad', '');",
        chrono_lite_date()
    )?;
    writeln!(
        buffer,
        "FILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 3 1 1 }}'));"
    )?;
    writeln!(buffer, "ENDSEC;")?;
    writeln!(buffer, "DATA;")?;
    for line in entities {
        writeln!(buffer, "{}", line)?;
    }
    writeln!(buffer, "ENDSEC;")?;
    writeln!(buffer, "END-ISO-10303-21;")?;
    Ok(buffer)
}

/// Context for writing STEP files.
struct StepWriter<'a> {
    solid: &'a BRepSolid,
    next_id: u64,
    output: Vec<String>,
    /// Maps vcad VertexId to STEP point ID.
    point_map: HashMap<VertexId, u64>,
    /// Maps vcad VertexId to STEP vertex ID.
    vertex_map: HashMap<VertexId, u64>,
    /// Maps vcad EdgeId to STEP edge ID.
    edge_map: HashMap<EdgeId, u64>,
    /// Maps vcad HalfEdgeId to STEP oriented edge ID.
    oriented_edge_map: HashMap<HalfEdgeId, u64>,
    /// Maps vcad surface index to STEP surface ID.
    surface_map: HashMap<usize, u64>,
    /// Maps vcad LoopId to STEP edge loop ID.
    loop_map: HashMap<LoopId, u64>,
    /// Maps vcad FaceId to STEP face bound ID.
    face_bound_map: HashMap<FaceId, u64>,
    /// Maps vcad FaceId to STEP face ID.
    face_map: HashMap<FaceId, u64>,
    /// Analytic edge reconstruction plan (chord chains → LINE/CIRCLE edges).
    plan: EdgeMergePlan,
    /// STEP edge IDs for each chain's segments, in chain-forward order.
    chain_step: Vec<Vec<u64>>,
}

impl<'a> StepWriter<'a> {
    fn with_start_id(solid: &'a BRepSolid, start_id: u64) -> Self {
        Self {
            solid,
            next_id: start_id,
            output: Vec::new(),
            point_map: HashMap::new(),
            vertex_map: HashMap::new(),
            edge_map: HashMap::new(),
            oriented_edge_map: HashMap::new(),
            surface_map: HashMap::new(),
            loop_map: HashMap::new(),
            face_bound_map: HashMap::new(),
            face_map: HashMap::new(),
            plan: EdgeMergePlan::default(),
            chain_step: Vec::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn emit(&mut self, id: u64, entity: &str) {
        self.output.push(format!("#{} = {};", id, entity));
    }

    fn write_entities(&mut self, name: &str) -> Result<u64, StepError> {
        // Reconstruct analytic edges (chord chains → LINE/CIRCLE) up front so
        // write_edges/write_loops can consult the plan.
        self.plan = plan_edge_merges(self.solid);
        // Write all geometry and topology
        self.write_points()?;
        self.write_surfaces()?;
        self.write_vertices()?;
        self.write_edges()?;
        self.write_loops()?;
        self.write_faces()?;
        let shell_id = self.write_shell()?;
        let solid_id = self.write_solid(shell_id, name)?;
        Ok(solid_id)
    }

    fn write_points(&mut self) -> Result<(), StepError> {
        let topo = &self.solid.topology;
        for (vid, vertex) in &topo.vertices {
            let id = self.alloc_id();
            let entity = write_cartesian_point(&vertex.point, "");
            self.emit(id, &entity);
            self.point_map.insert(vid, id);
        }
        Ok(())
    }

    fn write_vertices(&mut self) -> Result<(), StepError> {
        for (vid, _) in &self.solid.topology.vertices {
            let point_id = self.point_map[&vid];
            let id = self.alloc_id();
            let entity = write_vertex_point("", point_id);
            self.emit(id, &entity);
            self.vertex_map.insert(vid, id);
        }
        Ok(())
    }

    fn write_surfaces(&mut self) -> Result<(), StepError> {
        let geom = &self.solid.geometry;

        for (idx, surface) in geom.surfaces.iter().enumerate() {
            let surf_id = self.alloc_id();

            // Handle each surface type, extracting placement and entity in one pass
            let (placement, entity) = match surface.surface_type() {
                SurfaceKind::Plane => {
                    let plane = surface.as_any().downcast_ref::<Plane>().ok_or_else(|| {
                        StepError::InvalidGeometry("failed to downcast Plane surface".into())
                    })?;
                    let placement_id = self.write_axis_placement(&plane_to_placement(plane))?;
                    (placement_id, write_plane("", placement_id))
                }
                SurfaceKind::Cylinder => {
                    let cyl = surface
                        .as_any()
                        .downcast_ref::<CylinderSurface>()
                        .ok_or_else(|| {
                            StepError::InvalidGeometry("failed to downcast Cylinder surface".into())
                        })?;
                    let placement_id = self.write_axis_placement(&cylinder_to_placement(cyl))?;
                    (
                        placement_id,
                        write_cylindrical_surface(cyl.radius, "", placement_id),
                    )
                }
                SurfaceKind::Sphere => {
                    let sphere = surface
                        .as_any()
                        .downcast_ref::<SphereSurface>()
                        .ok_or_else(|| {
                            StepError::InvalidGeometry("failed to downcast Sphere surface".into())
                        })?;
                    let placement_id = self.write_axis_placement(&sphere_to_placement(sphere))?;
                    (
                        placement_id,
                        write_spherical_surface(sphere.radius, "", placement_id),
                    )
                }
                SurfaceKind::Cone => {
                    let cone = surface
                        .as_any()
                        .downcast_ref::<ConeSurface>()
                        .ok_or_else(|| {
                            StepError::InvalidGeometry("failed to downcast Cone surface".into())
                        })?;
                    let placement = AxisPlacement {
                        location: cone.apex,
                        axis: Some(cone.axis),
                        ref_direction: Some(cone.ref_dir),
                    };
                    let placement_id = self.write_axis_placement(&placement)?;
                    // For STEP, we need the radius at the reference position
                    // Since apex is at the placement location, radius is 0 there
                    (
                        placement_id,
                        write_conical_surface(0.0, cone.half_angle, "", placement_id),
                    )
                }
                SurfaceKind::Torus => {
                    let torus =
                        surface
                            .as_any()
                            .downcast_ref::<TorusSurface>()
                            .ok_or_else(|| {
                                StepError::InvalidGeometry(
                                    "failed to downcast Torus surface".into(),
                                )
                            })?;
                    let placement_id = self.write_axis_placement(&torus_to_placement(torus))?;
                    (
                        placement_id,
                        write_toroidal_surface(
                            torus.major_radius,
                            torus.minor_radius,
                            "",
                            placement_id,
                        ),
                    )
                }
                SurfaceKind::BSpline => {
                    let bspline = surface
                        .as_any()
                        .downcast_ref::<BSplineSurface>()
                        .ok_or_else(|| {
                            StepError::InvalidGeometry("failed to downcast BSpline surface".into())
                        })?;
                    let entity = self.write_bspline_surface_entity(bspline);
                    self.emit(surf_id, &entity);
                    self.surface_map.insert(idx, surf_id);
                    continue;
                }
                SurfaceKind::Bilinear => {
                    let bilinear = surface
                        .as_any()
                        .downcast_ref::<BilinearSurface>()
                        .ok_or_else(|| {
                            StepError::InvalidGeometry("failed to downcast Bilinear surface".into())
                        })?;
                    if bilinear.is_planar() {
                        let placement_id =
                            self.write_axis_placement(&bilinear_planar_to_placement(bilinear))?;
                        (placement_id, write_plane("", placement_id))
                    } else {
                        let entity = self.write_bilinear_as_bspline(bilinear);
                        self.emit(surf_id, &entity);
                        self.surface_map.insert(idx, surf_id);
                        continue;
                    }
                }
            };

            let _ = placement; // placement_id already used in entity construction
            self.emit(surf_id, &entity);
            self.surface_map.insert(idx, surf_id);
        }

        Ok(())
    }

    fn write_axis_placement(&mut self, placement: &AxisPlacement) -> Result<u64, StepError> {
        // Write location point
        let loc_id = self.alloc_id();
        self.emit(loc_id, &write_cartesian_point(&placement.location, ""));

        // Write axis direction if present
        let axis_id = if let Some(axis) = placement.axis {
            let id = self.alloc_id();
            self.emit(id, &write_direction(&axis, ""));
            Some(id)
        } else {
            None
        };

        // Write ref direction if present
        let ref_id = if let Some(ref_dir) = placement.ref_direction {
            let id = self.alloc_id();
            self.emit(id, &write_direction(&ref_dir, ""));
            Some(id)
        } else {
            None
        };

        // Write placement
        let placement_id = self.alloc_id();
        let entity = write_axis2_placement_3d(placement, "", loc_id, axis_id, ref_id);
        self.emit(placement_id, &entity);

        Ok(placement_id)
    }

    /// Write a BSplineSurface as a B_SPLINE_SURFACE_WITH_KNOTS entity.
    fn write_bspline_surface_entity(&mut self, bspline: &BSplineSurface) -> String {
        // Write control points and collect their IDs
        let cp_ids = write_bspline_control_points(
            &bspline.control_points,
            bspline.n_u,
            bspline.n_v,
            &mut |entity_str| {
                let id = self.alloc_id();
                self.emit(id, entity_str);
                id
            },
        );

        // Compress knot vectors
        let (u_knots, u_mults) = compress_knots(&bspline.knots_u);
        let (v_knots, v_mults) = compress_knots(&bspline.knots_v);

        write_bspline_surface_with_knots(
            "",
            bspline.degree_u,
            bspline.degree_v,
            &cp_ids,
            &u_knots,
            &u_mults,
            &v_knots,
            &v_mults,
        )
    }

    /// Write a non-planar BilinearSurface as a degree-1 B_SPLINE_SURFACE_WITH_KNOTS.
    fn write_bilinear_as_bspline(&mut self, bilinear: &BilinearSurface) -> String {
        // A bilinear surface is a degree-1 B-spline with 2x2 control points.
        // Control points in row-major (v-major) order:
        // row 0 (v=0): p00, p10
        // row 1 (v=1): p01, p11
        let points = [bilinear.p00, bilinear.p10, bilinear.p01, bilinear.p11];
        let cp_ids = write_bspline_control_points(&points, 2, 2, &mut |entity_str| {
            let id = self.alloc_id();
            self.emit(id, entity_str);
            id
        });

        // Degree-1 with 2 control points: knots = [0, 0, 1, 1] → compressed = ([0, 1], [2, 2])
        let knots = vec![0.0, 1.0];
        let mults = vec![2, 2];

        write_bspline_surface_with_knots("", 1, 1, &cp_ids, &knots, &mults, &knots, &mults)
    }

    fn write_edges(&mut self) -> Result<(), StepError> {
        let topo = &self.solid.topology;

        let edge_ids: Vec<EdgeId> = topo.edges.keys().collect();
        for edge_id in edge_ids {
            if self.plan.edge_chain.contains_key(&edge_id) {
                continue; // replaced by a reconstructed chain segment
            }
            let he = self.solid.topology.edges[edge_id].half_edge;
            let start_vid = self.solid.topology.half_edges[he].origin;
            let end_vid = self.solid.topology.half_edge_dest(he);
            let step_edge_id = self.write_line_edge_curve(start_vid, end_vid);
            self.edge_map.insert(edge_id, step_edge_id);
        }

        // Emit one LINE or CIRCLE edge per reconstructed chain segment.
        let plan = std::mem::take(&mut self.plan);
        for chain in &plan.chains {
            let mut seg_ids = Vec::with_capacity(chain.segments.len());
            // Synthesized split vertices (closed single-edge circles) are
            // minted once per chain and shared between its segments.
            let mut synth_vertex_ids: Vec<Option<u64>> = vec![None; chain.synth_points.len()];
            for seg in &chain.segments {
                let (start_vertex, start_pt) =
                    self.resolve_seg_end(seg.start, chain, &mut synth_vertex_ids);
                let (end_vertex, _end_pt) =
                    self.resolve_seg_end(seg.end, chain, &mut synth_vertex_ids);
                let id = match &chain.geom {
                    ChainGeom::Line => {
                        let end_pt = match seg.end {
                            SegEnd::Topo(v) => self.solid.topology.vertices[v].point,
                            SegEnd::Synth(i) => chain.synth_points[i],
                        };
                        self.write_line_edge_between(start_vertex, end_vertex, start_pt, end_pt)
                    }
                    ChainGeom::Arc {
                        center,
                        radius,
                        normal,
                    } => self.write_arc_edge_curve(
                        start_vertex,
                        end_vertex,
                        start_pt,
                        *center,
                        *radius,
                        *normal,
                    ),
                };
                seg_ids.push(id);
            }
            self.chain_step.push(seg_ids);
        }
        self.plan = plan;

        Ok(())
    }

    /// Resolve a segment endpoint to a STEP vertex ID and its 3D point,
    /// minting a VERTEX_POINT for synthesized split vertices on first use.
    fn resolve_seg_end(
        &mut self,
        end: SegEnd,
        chain: &ChainPlan,
        synth_ids: &mut [Option<u64>],
    ) -> (u64, Point3) {
        match end {
            SegEnd::Topo(v) => (self.vertex_map[&v], self.solid.topology.vertices[v].point),
            SegEnd::Synth(i) => {
                let p = chain.synth_points[i];
                if let Some(id) = synth_ids[i] {
                    return (id, p);
                }
                let point_id = self.alloc_id();
                self.emit(point_id, &write_cartesian_point(&p, ""));
                let vertex_id = self.alloc_id();
                self.emit(vertex_id, &write_vertex_point("", point_id));
                synth_ids[i] = Some(vertex_id);
                (vertex_id, p)
            }
        }
    }

    /// Emit a straight EDGE_CURVE between two STEP vertices at given points.
    fn write_line_edge_between(
        &mut self,
        start_vertex: u64,
        end_vertex: u64,
        start_point: Point3,
        end_point: Point3,
    ) -> u64 {
        let dir_vec = end_point - start_point;
        let magnitude = dir_vec.norm();
        let dir = if magnitude > 1e-15 {
            Dir3::new_normalize(dir_vec)
        } else {
            Dir3::new_normalize(Vec3::x())
        };

        let line_point_id = self.alloc_id();
        self.emit(line_point_id, &write_cartesian_point(&start_point, ""));

        let dir_id = self.alloc_id();
        self.emit(dir_id, &write_direction(&dir, ""));

        let vec_id = self.alloc_id();
        self.emit(
            vec_id,
            &format!("VECTOR('', #{}, {:.15E})", dir_id, magnitude),
        );

        let line_id = self.alloc_id();
        self.emit(
            line_id,
            &format!("LINE('', #{}, #{})", line_point_id, vec_id),
        );

        let step_edge_id = self.alloc_id();
        let entity = write_edge_curve("", start_vertex, end_vertex, line_id, true);
        self.emit(step_edge_id, &entity);
        step_edge_id
    }

    /// Emit an EDGE_CURVE over a CIRCLE from `start` counterclockwise about
    /// `normal` to the end vertex. The circle's ref direction is placed at the
    /// start point, so the edge parameterization begins at the start vertex.
    fn write_arc_edge_curve(
        &mut self,
        start_vertex: u64,
        end_vertex: u64,
        start_point: Point3,
        center: Point3,
        radius: f64,
        normal: Dir3,
    ) -> u64 {
        let ref_dir = Dir3::new_normalize(start_point - center);
        let placement = AxisPlacement {
            location: center,
            axis: Some(normal),
            ref_direction: Some(ref_dir),
        };
        let placement_id = self
            .write_axis_placement(&placement)
            .expect("axis placement emission is infallible");

        let circle_id = self.alloc_id();
        self.emit(
            circle_id,
            &format!("CIRCLE('', #{}, {:.15E})", placement_id, radius),
        );

        let step_edge_id = self.alloc_id();
        let entity = write_edge_curve("", start_vertex, end_vertex, circle_id, true);
        self.emit(step_edge_id, &entity);
        step_edge_id
    }

    /// Emit a straight EDGE_CURVE between two vcad vertices and return its ID.
    ///
    /// Used by `write_edges` for proper edges and by `write_loops` to synthesize
    /// edges for orphan half-edges (those whose `parent_edge` link was severed
    /// by the boolean sewing pipeline — see crates/vcad-kernel-booleans/src/sew.rs).
    /// The geometry is approximated as a line segment regardless of the
    /// underlying surface's curvature; this matches `write_edges`'s existing
    /// limitation and keeps the writer's contract simple.
    fn write_line_edge_curve(&mut self, start_vid: VertexId, end_vid: VertexId) -> u64 {
        let start_vertex = self.vertex_map[&start_vid];
        let end_vertex = self.vertex_map[&end_vid];
        let start_point = self.solid.topology.vertices[start_vid].point;
        let end_point = self.solid.topology.vertices[end_vid].point;
        self.write_line_edge_between(start_vertex, end_vertex, start_point, end_point)
    }

    fn write_loops(&mut self) -> Result<(), StepError> {
        // Collect loops first so we can borrow self mutably inside.
        let loop_ids: Vec<LoopId> = self.solid.topology.loops.keys().collect();
        let plan = std::mem::take(&mut self.plan);

        let chain_of = |he_id: HalfEdgeId, topo: &vcad_kernel_topo::Topology| -> Option<usize> {
            topo.half_edges[he_id]
                .edge
                .and_then(|e| plan.edge_chain.get(&e).copied())
        };

        for loop_id in loop_ids {
            let he_ids: Vec<HalfEdgeId> = self.solid.topology.loop_half_edges(loop_id).collect();
            let n = he_ids.len();

            // Degenerate "slit" loop on a cylindrical face: exactly two
            // half-edges that are twins of one axis-parallel seam edge. The
            // boolean sewing pipeline loses the circular boundaries of a full
            // cylindrical band, leaving a zero-area loop; rebuild the two
            // circles from the surface so the face is properly bounded.
            if n == 2 {
                if let Some(el_id) = self.try_write_slit_cylinder_loop(loop_id, &he_ids) {
                    self.loop_map.insert(loop_id, el_id);
                    continue;
                }
            }

            // Rotate so the walk never starts in the middle of a chain run
            // (a chain's edges appear as one cyclic block — validated during
            // planning) or an orphan half-edge run. If the whole loop is one
            // chain or all-orphan, 0 is fine (handled as a closed run).
            let is_orphan = |he: HalfEdgeId, topo: &vcad_kernel_topo::Topology| {
                topo.half_edges[he].edge.is_none()
            };
            let mut start = 0;
            for i in 0..n {
                let cur = chain_of(he_ids[i], &self.solid.topology);
                let prev = chain_of(he_ids[(i + n - 1) % n], &self.solid.topology);
                let mid_chain = cur.is_some() && cur == prev;
                let mid_orphan = is_orphan(he_ids[i], &self.solid.topology)
                    && is_orphan(he_ids[(i + n - 1) % n], &self.solid.topology);
                if !(mid_chain || mid_orphan) {
                    start = i;
                    break;
                }
            }
            let rotated: Vec<HalfEdgeId> = (0..n).map(|i| he_ids[(start + i) % n]).collect();

            let mut oriented_edge_ids = Vec::new();
            let mut i = 0;
            while i < n {
                let he_id = rotated[i];
                if let Some(ci) = chain_of(he_id, &self.solid.topology) {
                    // Consume the whole chain run and emit its segments.
                    let chain = &plan.chains[ci];
                    let run_len = chain.edges.len();
                    let forward = self.chain_run_is_forward(chain, &rotated[i..], run_len);
                    let seg_ids = self.chain_step[ci].clone();
                    if forward {
                        for sid in seg_ids {
                            let oe_id = self.alloc_id();
                            self.emit(oe_id, &write_oriented_edge("", sid, true));
                            oriented_edge_ids.push(oe_id);
                        }
                    } else {
                        for sid in seg_ids.into_iter().rev() {
                            let oe_id = self.alloc_id();
                            self.emit(oe_id, &write_oriented_edge("", sid, false));
                            oriented_edge_ids.push(oe_id);
                        }
                    }
                    i += run_len;
                    continue;
                }

                if self.solid.topology.half_edges[he_id].edge.is_none() {
                    // Orphan half-edge — boolean sewing leaves these when face
                    // boundaries can't be paired (curved-vs-polygonal mismatch).
                    // Gather the maximal consecutive orphan run and try to
                    // reconstruct it as one LINE or CIRCLE; fall back to
                    // per-half-edge line segments.
                    let mut run_len = 1;
                    while i + run_len < n
                        && self.solid.topology.half_edges[rotated[i + run_len]]
                            .edge
                            .is_none()
                    {
                        run_len += 1;
                    }
                    let run = &rotated[i..i + run_len];
                    let edge_ids = self.write_orphan_run(run, run_len == n);
                    for sid in edge_ids {
                        let oe_id = self.alloc_id();
                        self.emit(oe_id, &write_oriented_edge("", sid, true));
                        oriented_edge_ids.push(oe_id);
                    }
                    i += run_len;
                    continue;
                }

                let he = &self.solid.topology.half_edges[he_id];
                let edge_id = he.edge.expect("checked above");
                let step_edge_id = self.edge_map[&edge_id];
                let orientation = self.solid.topology.edges[edge_id].half_edge == he_id;

                let oe_id = self.alloc_id();
                let entity = write_oriented_edge("", step_edge_id, orientation);
                self.emit(oe_id, &entity);
                self.oriented_edge_map.insert(he_id, oe_id);

                oriented_edge_ids.push(oe_id);
                i += 1;
            }

            let el_id = self.alloc_id();
            let entity = write_edge_loop("", &oriented_edge_ids);
            self.emit(el_id, &entity);
            self.loop_map.insert(loop_id, el_id);
        }

        self.plan = plan;
        Ok(())
    }

    /// Whether a loop's run of half-edges traverses the chain in its forward
    /// (as-emitted) direction.
    fn chain_run_is_forward(&self, chain: &ChainPlan, run: &[HalfEdgeId], run_len: usize) -> bool {
        let topo = &self.solid.topology;
        if chain.edges.len() == 1 {
            // Single closed edge: direction is the canonical half-edge's, by
            // the same convention the plan used to orient the circle.
            return topo.edges[chain.edges[0]].half_edge == run[0];
        }
        let first_origin = topo.half_edges[run[0]].origin;
        if !chain.closed {
            return first_origin == chain.vertices[0];
        }
        // Closed multi-edge chain: compare consecutive origins to chain order.
        let k = chain.vertices.len();
        let j = chain
            .vertices
            .iter()
            .position(|v| *v == first_origin)
            .expect("run origin must be a chain vertex");
        debug_assert!(run_len >= 2);
        let second_origin = topo.half_edges[run[1]].origin;
        second_origin == chain.vertices[(j + 1) % k]
    }

    /// Emit STEP edges for a maximal run of orphan half-edges, reconstructing
    /// the run as one LINE or CIRCLE when the whole run verifies as such
    /// (falling back to per-half-edge line segments otherwise). Returns the
    /// STEP edge IDs in traversal order; all are traversal-forward.
    ///
    /// Orphan half-edges have no twins, so the two loops sharing a boundary
    /// each emit their own edges (as they always have); classification is
    /// purely point-based and the closed-circle split vertices are canonical,
    /// so both sides reconstruct identical geometry for importers to sew.
    fn write_orphan_run(&mut self, run: &[HalfEdgeId], closed: bool) -> Vec<u64> {
        // Chords along a tessellated smooth curve turn by a small angle per
        // step; corners (tangent discontinuities) turn sharply. Splitting at
        // sharp turns is symmetric under traversal reversal, keeping the two
        // sides of a boundary consistent. cos(40°): full circles tessellated
        // with ≥ 9 segments survive as one piece.
        const COS_SHARP: f64 = 0.766;

        let vids: Vec<VertexId> = {
            let topo = &self.solid.topology;
            let mut v: Vec<VertexId> = run.iter().map(|he| topo.half_edges[*he].origin).collect();
            if !closed {
                v.push(topo.half_edge_dest(*run.last().unwrap()));
            }
            v
        };
        let points: Vec<Point3> = vids
            .iter()
            .map(|v| self.solid.topology.vertices[*v].point)
            .collect();
        let n_v = vids.len();

        let sharp = |prev: Point3, cur: Point3, next: Point3| -> bool {
            let a = cur - prev;
            let b = next - cur;
            let (na, nb) = (a.norm(), b.norm());
            na < 1e-12 || nb < 1e-12 || a.dot(b) / (na * nb) < COS_SHARP
        };

        if closed {
            // A closed run is the entire loop (maximality), so cyclic
            // rotation of the emitted edges is harmless.
            let corners: Vec<usize> = (0..n_v)
                .filter(|&i| {
                    sharp(
                        points[(i + n_v - 1) % n_v],
                        points[i],
                        points[(i + 1) % n_v],
                    )
                })
                .collect();
            if corners.is_empty() {
                if let Some(ChainGeom::Arc {
                    center,
                    radius,
                    normal,
                }) = classify_chain(&points, true)
                {
                    // Full circle: split at canonical (direction-independent)
                    // vertices so both sides of the boundary agree.
                    let (a, b) = canonical_split(&points);
                    let arc = |w: &mut Self, s: usize, e: usize| {
                        let sv = w.vertex_map[&vids[s]];
                        let ev = w.vertex_map[&vids[e]];
                        w.write_arc_edge_curve(sv, ev, points[s], center, radius, normal)
                    };
                    // Traversal order is a → b → a (a < b or not, the two
                    // arcs form the same cycle).
                    return vec![arc(self, a, b), arc(self, b, a)];
                }
                return self.write_orphan_fallback(run);
            }
            // Segment cyclically between corners, starting at the first one.
            let mut out = Vec::new();
            for (k, &c) in corners.iter().enumerate() {
                let next_c = corners[(k + 1) % corners.len()];
                let span = if next_c > c {
                    next_c - c
                } else {
                    n_v - c + next_c
                };
                let piece_vids: Vec<VertexId> = (0..=span).map(|j| vids[(c + j) % n_v]).collect();
                let piece_pts: Vec<Point3> = (0..=span).map(|j| points[(c + j) % n_v]).collect();
                let piece_hes: Vec<HalfEdgeId> = (0..span).map(|j| run[(c + j) % n_v]).collect();
                out.extend(self.write_orphan_piece(&piece_hes, &piece_vids, &piece_pts));
            }
            return out;
        }

        // Open run: pieces between the endpoints and any sharp corners.
        let mut bounds = vec![0usize];
        for i in 1..n_v - 1 {
            if sharp(points[i - 1], points[i], points[i + 1]) {
                bounds.push(i);
            }
        }
        bounds.push(n_v - 1);
        let mut out = Vec::new();
        for w in bounds.windows(2) {
            let (s, e) = (w[0], w[1]);
            out.extend(self.write_orphan_piece(&run[s..e], &vids[s..=e], &points[s..=e]));
        }
        out
    }

    /// Rebuild a degenerate slit loop (a seam edge and its twin as the only
    /// boundary of a full cylindrical band face) by inserting the band's two
    /// boundary circles, each split at a synthesized antipodal vertex.
    /// Returns the STEP EDGE_LOOP id on success.
    fn try_write_slit_cylinder_loop(
        &mut self,
        loop_id: LoopId,
        he_ids: &[HalfEdgeId],
    ) -> Option<u64> {
        let topo = &self.solid.topology;
        let (a, b) = (he_ids[0], he_ids[1]);
        if topo.half_edges[a].twin != Some(b) || topo.half_edges[a].edge.is_none() {
            return None;
        }
        let face_id = topo.loops[loop_id].face?;
        let face = &topo.faces[face_id];
        let surface = &self.solid.geometry.surfaces[face.surface_index];
        if surface.surface_type() != SurfaceKind::Cylinder {
            return None;
        }
        let cyl = surface.as_any().downcast_ref::<CylinderSurface>()?;
        let axis = *cyl.axis.as_ref();

        let p_a0 = topo.vertices[topo.half_edges[a].origin].point;
        let p_a1 = topo.vertices[topo.half_edge_dest(a)].point;
        let seam = p_a1 - p_a0;
        let seam_len = seam.norm();
        if seam_len < 1e-9 || seam.cross(axis).norm() > fit_tol(seam_len) {
            return None; // not an axis-parallel seam
        }

        // Sign conventions: the loop winds CCW about the face's outward
        // normal. At the band end reached along +axis the boundary circle
        // runs CCW about (-outward_radial x sign) — combined: the traversal
        // normal is axis * (-s_n * s_d) where s_n = +1 for an outward-radial
        // face normal and s_d = +1 at the +axis end.
        let s_n: f64 = if face.orientation == Orientation::Forward {
            1.0
        } else {
            -1.0
        };

        let edge_id = topo.half_edges[a].edge.expect("checked above");
        let step_line_id = self.edge_map[&edge_id];
        let canonical_is_a = self.solid.topology.edges[edge_id].half_edge == a;

        let mut oriented: Vec<u64> = Vec::new();
        // Emit: a, circle at dest(a), b, circle at dest(b).
        for this_he in [a, b] {
            let orientation = if this_he == a {
                canonical_is_a
            } else {
                !canonical_is_a
            };
            let oe_id = self.alloc_id();
            self.emit(oe_id, &write_oriented_edge("", step_line_id, orientation));
            oriented.push(oe_id);

            let v_end = self.solid.topology.half_edge_dest(this_he);
            let p_end = self.solid.topology.vertices[v_end].point;
            let p_other =
                self.solid.topology.vertices[self.solid.topology.half_edges[this_he].origin].point;
            let s_d = (p_end - p_other).dot(axis).signum();
            let normal = Dir3::new_normalize(axis * (-s_n * s_d));

            let center = cyl.center + axis * (p_end - cyl.center).dot(axis);
            let radius = (p_end - center).norm();
            if (radius - cyl.radius).abs() > fit_tol(cyl.radius) {
                return None;
            }
            let antipode = center + (center - p_end);

            let sv = self.vertex_map[&v_end];
            let anti_point_id = self.alloc_id();
            self.emit(anti_point_id, &write_cartesian_point(&antipode, ""));
            let anti_vertex_id = self.alloc_id();
            self.emit(anti_vertex_id, &write_vertex_point("", anti_point_id));

            let arc1 = self.write_arc_edge_curve(sv, anti_vertex_id, p_end, center, radius, normal);
            let arc2 =
                self.write_arc_edge_curve(anti_vertex_id, sv, antipode, center, radius, normal);
            for arc in [arc1, arc2] {
                let oe_id = self.alloc_id();
                self.emit(oe_id, &write_oriented_edge("", arc, true));
                oriented.push(oe_id);
            }
        }

        let el_id = self.alloc_id();
        self.emit(el_id, &write_edge_loop("", &oriented));
        Some(el_id)
    }

    /// Emit one smooth orphan piece: a single LINE or CIRCLE arc if the whole
    /// piece verifies as one, else per-half-edge lines.
    fn write_orphan_piece(
        &mut self,
        hes: &[HalfEdgeId],
        vids: &[VertexId],
        points: &[Point3],
    ) -> Vec<u64> {
        if hes.len() >= 2 {
            match classify_chain(points, false) {
                Some(ChainGeom::Line) => {
                    return vec![
                        self.write_line_edge_curve(*vids.first().unwrap(), *vids.last().unwrap())
                    ];
                }
                Some(ChainGeom::Arc {
                    center,
                    radius,
                    normal,
                }) => {
                    let sv = self.vertex_map[vids.first().unwrap()];
                    let ev = self.vertex_map[vids.last().unwrap()];
                    return vec![
                        self.write_arc_edge_curve(sv, ev, points[0], center, radius, normal)
                    ];
                }
                None => {}
            }
        }
        self.write_orphan_fallback(hes)
    }

    /// One line edge per orphan half-edge (the pre-reconstruction behavior).
    fn write_orphan_fallback(&mut self, hes: &[HalfEdgeId]) -> Vec<u64> {
        hes.iter()
            .map(|he_id| {
                let start_vid = self.solid.topology.half_edges[*he_id].origin;
                let end_vid = self.solid.topology.half_edge_dest(*he_id);
                self.write_line_edge_curve(start_vid, end_vid)
            })
            .collect()
    }

    fn write_faces(&mut self) -> Result<(), StepError> {
        let topo = &self.solid.topology;

        for (face_id, face) in &topo.faces {
            let surface_id = self.surface_map[&face.surface_index];

            // Write outer bound
            let outer_loop_id = self.loop_map[&face.outer_loop];
            let outer_bound_id = self.alloc_id();
            let entity = write_face_bound("", outer_loop_id, true, true);
            self.emit(outer_bound_id, &entity);

            let mut bound_ids = vec![outer_bound_id];

            // Write inner bounds (holes)
            for inner_loop in &face.inner_loops {
                let inner_loop_id = self.loop_map[inner_loop];
                let inner_bound_id = self.alloc_id();
                let entity = write_face_bound("", inner_loop_id, true, false);
                self.emit(inner_bound_id, &entity);
                bound_ids.push(inner_bound_id);
            }

            self.face_bound_map.insert(face_id, outer_bound_id);

            // Write face
            let same_sense = face.orientation == Orientation::Forward;
            let step_face_id = self.alloc_id();
            let entity = write_advanced_face("", &bound_ids, surface_id, same_sense);
            self.emit(step_face_id, &entity);
            self.face_map.insert(face_id, step_face_id);
        }

        Ok(())
    }

    fn write_shell(&mut self) -> Result<u64, StepError> {
        let topo = &self.solid.topology;
        let solid = &topo.solids[self.solid.solid_id];
        let shell = &topo.shells[solid.outer_shell];

        let face_ids: Vec<u64> = shell.faces.iter().map(|fid| self.face_map[fid]).collect();

        let shell_id = self.alloc_id();
        let entity = write_closed_shell("", &face_ids);
        self.emit(shell_id, &entity);

        Ok(shell_id)
    }

    fn write_solid(&mut self, shell_id: u64, name: &str) -> Result<u64, StepError> {
        let solid_id = self.alloc_id();
        // STEP strings are single-quote delimited; strip quotes from names.
        let name = name.replace('\'', "");
        let entity = write_manifold_solid_brep(&name, shell_id);
        self.emit(solid_id, &entity);
        Ok(solid_id)
    }
}

/// Simple date string without external chrono dependency.
fn chrono_lite_date() -> String {
    // Just return a placeholder - real implementation would use system time
    "2024-01-01T00:00:00".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_step_from_buffer;
    use vcad_kernel_geom::{BilinearSurface, GeometryStore, Surface};
    use vcad_kernel_math::Point3;
    use vcad_kernel_nurbs::BSplineSurface;
    use vcad_kernel_primitives::make_cube;
    use vcad_kernel_topo::{Orientation, ShellType, Topology};

    #[test]
    fn test_write_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let buffer = write_step_to_buffer(&cube).unwrap();
        let content = String::from_utf8_lossy(&buffer);

        // Check essential content
        assert!(content.contains("ISO-10303-21"));
        assert!(content.contains("MANIFOLD_SOLID_BREP"));
        assert!(content.contains("CLOSED_SHELL"));
        assert!(content.contains("ADVANCED_FACE"));
        assert!(content.contains("PLANE("));
        assert!(content.contains("CARTESIAN_POINT"));
    }

    /// Parse the DATA section into `id -> entity body` (text after `=`,
    /// trailing `;` stripped). Deliberately independent of vcad's own STEP
    /// parser: these conformance tests must not share an oracle with the
    /// reader, or a layer both sides skip goes untested (the round-trip
    /// tests passed for months on files every conforming importer rejected).
    fn parse_entities(content: &str) -> HashMap<u64, String> {
        let mut map = HashMap::new();
        for line in content.lines() {
            let Some(rest) = line.strip_prefix('#') else {
                continue;
            };
            let Some((id, body)) = rest.split_once('=') else {
                continue;
            };
            let Ok(id) = id.trim().parse::<u64>() else {
                continue;
            };
            map.insert(id, body.trim().trim_end_matches(';').trim().to_string());
        }
        map
    }

    fn ids_of_type(map: &HashMap<u64, String>, ty: &str) -> Vec<u64> {
        let mut ids: Vec<u64> = map
            .iter()
            .filter(|(_, body)| {
                body.strip_prefix(ty)
                    .map(|rest| rest.trim_start().starts_with('('))
                    .unwrap_or(false)
            })
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Entity ids referenced (as `#N`) inside a body string.
    fn refs_in(body: &str) -> Vec<u64> {
        let mut refs = Vec::new();
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
                if end > start {
                    refs.push(body[start..end].parse().unwrap());
                }
                i = end;
            } else {
                i += 1;
            }
        }
        refs
    }

    /// Assert the product/representation anchor a conforming importer
    /// traverses: PRODUCT -> ... -> SDR -> ABSR(all solids, unit context).
    fn assert_anchor_graph(content: &str) {
        let map = parse_entities(content);

        // Header: FILE_SCHEMA must carry the AP214 object identifier.
        assert!(
            content.contains("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 3 1 1 }'));"),
            "FILE_SCHEMA must include the AP214 object-identifier suffix"
        );

        // Exactly one SDR per file.
        let sdrs = ids_of_type(&map, "SHAPE_DEFINITION_REPRESENTATION");
        assert_eq!(sdrs.len(), 1, "expected exactly one SDR, got {sdrs:?}");
        let sdr_refs = refs_in(&map[&sdrs[0]]);
        assert_eq!(
            sdr_refs.len(),
            2,
            "SDR must reference (PDS, representation)"
        );

        // SDR arg 0 chains PDS -> PD -> PDF -> PRODUCT.
        let pds = &map[&sdr_refs[0]];
        assert!(
            pds.starts_with("PRODUCT_DEFINITION_SHAPE"),
            "SDR arg0: {pds}"
        );
        let pd = &map[refs_in(pds).last().unwrap()];
        assert!(pd.starts_with("PRODUCT_DEFINITION"), "PDS ref: {pd}");
        let pdf = &map[&refs_in(pd)[0]];
        assert!(
            pdf.starts_with("PRODUCT_DEFINITION_FORMATION"),
            "PD arg2: {pdf}"
        );
        let product = &map[refs_in(pdf).last().unwrap()];
        assert!(product.starts_with("PRODUCT"), "PDF ref: {product}");

        // SDR arg 1 is the ABSR; it must reference every MANIFOLD_SOLID_BREP.
        let absr = &map[&sdr_refs[1]];
        assert!(
            absr.starts_with("ADVANCED_BREP_SHAPE_REPRESENTATION"),
            "SDR arg1: {absr}"
        );
        let absr_refs = refs_in(absr);
        let solids = ids_of_type(&map, "MANIFOLD_SOLID_BREP");
        assert!(!solids.is_empty(), "no MANIFOLD_SOLID_BREP entities");
        for solid in &solids {
            assert!(
                absr_refs.contains(solid),
                "ABSR does not reference solid #{solid}: {absr}"
            );
        }

        // The ABSR's last ref is the geometric context; it must be the
        // complex instance carrying GLOBAL_UNIT_ASSIGNED_CONTEXT, and that
        // context must reference a LENGTH_UNIT.
        let ctx = &map[absr_refs.last().unwrap()];
        assert!(
            ctx.contains("GEOMETRIC_REPRESENTATION_CONTEXT")
                && ctx.contains("GLOBAL_UNIT_ASSIGNED_CONTEXT")
                && ctx.contains("GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT"),
            "ABSR context is not a unit-assigned representation context: {ctx}"
        );
        let has_length_unit = refs_in(ctx)
            .iter()
            .any(|r| map[r].contains("LENGTH_UNIT()") && map[r].contains("SI_UNIT"));
        assert!(
            has_length_unit,
            "context does not reference an SI LENGTH_UNIT: {ctx}"
        );
    }

    #[test]
    fn test_anchor_graph_single_body() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let buffer = write_step_to_buffer(&cube).unwrap();
        assert_anchor_graph(&String::from_utf8_lossy(&buffer));
    }

    #[test]
    fn test_anchor_graph_multi_body() {
        let a = make_cube(10.0, 10.0, 10.0);
        let b = make_cube(5.0, 5.0, 20.0);
        let buffer = write_step_solids_to_buffer(&[(&a, "A"), (&b, "B")]).unwrap();
        let content = String::from_utf8_lossy(&buffer);
        assert_anchor_graph(&content);
        let map = parse_entities(&content);
        assert_eq!(ids_of_type(&map, "MANIFOLD_SOLID_BREP").len(), 2);
    }

    #[test]
    fn test_roundtrip_cube() {
        // Create a cube
        let original = make_cube(10.0, 20.0, 30.0);

        // Write to STEP
        let buffer = write_step_to_buffer(&original).unwrap();

        // Read back
        let solids = read_step_from_buffer(&buffer).unwrap();
        assert_eq!(solids.len(), 1);

        let imported = &solids[0];

        // Verify topology matches
        assert_eq!(
            original.topology.vertices.len(),
            imported.topology.vertices.len()
        );
        assert_eq!(original.topology.faces.len(), imported.topology.faces.len());
        assert_eq!(original.topology.edges.len(), imported.topology.edges.len());

        // Verify geometry matches
        assert_eq!(
            original.geometry.surfaces.len(),
            imported.geometry.surfaces.len()
        );
    }

    /// Helper: build a minimal BRepSolid with a single face using the given surface.
    fn make_single_face_solid(surface: Box<dyn Surface>, corners: [Point3; 4]) -> BRepSolid {
        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();

        let surf_idx = geom.add_surface(surface);

        let v0 = topo.add_vertex(corners[0]);
        let v1 = topo.add_vertex(corners[1]);
        let v2 = topo.add_vertex(corners[2]);
        let v3 = topo.add_vertex(corners[3]);

        let he0 = topo.add_half_edge(v0);
        let he1 = topo.add_half_edge(v1);
        let he2 = topo.add_half_edge(v2);
        let he3 = topo.add_half_edge(v3);

        let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
        let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

        // Create twin half-edges for edge pairing
        let he0t = topo.add_half_edge(v1);
        let he1t = topo.add_half_edge(v2);
        let he2t = topo.add_half_edge(v3);
        let he3t = topo.add_half_edge(v0);

        topo.add_edge(he0, he0t);
        topo.add_edge(he1, he1t);
        topo.add_edge(he2, he2t);
        topo.add_edge(he3, he3t);

        let shell_id = topo.add_shell(vec![face_id], ShellType::Outer);
        let solid_id = topo.add_solid(shell_id);

        BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        }
    }

    #[test]
    fn test_write_bspline_surface() {
        // Create a simple bicubic B-spline surface (4x4 control points)
        let mut control_points = Vec::new();
        for v in 0..4 {
            for u in 0..4 {
                control_points.push(Point3::new(
                    u as f64 * 10.0,
                    v as f64 * 10.0,
                    if u == 1 && v == 1 { 5.0 } else { 0.0 },
                ));
            }
        }
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let bspline = BSplineSurface::new(control_points, 4, 4, knots.clone(), knots, 3, 3);

        let corners = [
            bspline.eval(0.0, 0.0),
            bspline.eval(1.0, 0.0),
            bspline.eval(1.0, 1.0),
            bspline.eval(0.0, 1.0),
        ];

        let solid = make_single_face_solid(Box::new(bspline), corners);
        let buffer = write_step_to_buffer(&solid).unwrap();
        let content = String::from_utf8_lossy(&buffer);

        // Must contain B_SPLINE_SURFACE_WITH_KNOTS, not PLANE
        assert!(
            content.contains("B_SPLINE_SURFACE_WITH_KNOTS"),
            "B-spline surface should be written as B_SPLINE_SURFACE_WITH_KNOTS, got:\n{}",
            content
        );
        assert!(
            !content.contains("PLANE('"),
            "B-spline surface should NOT be written as PLANE"
        );
    }

    #[test]
    fn test_write_bilinear_planar() {
        // A planar bilinear surface should export as PLANE
        let bilinear = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        );
        assert!(bilinear.is_planar());

        let corners = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        let solid = make_single_face_solid(Box::new(bilinear), corners);
        let buffer = write_step_to_buffer(&solid).unwrap();
        let content = String::from_utf8_lossy(&buffer);

        assert!(
            content.contains("PLANE('"),
            "Planar bilinear should be written as PLANE"
        );
        assert!(
            !content.contains("B_SPLINE_SURFACE_WITH_KNOTS"),
            "Planar bilinear should NOT be written as B_SPLINE_SURFACE_WITH_KNOTS"
        );
    }

    #[test]
    fn test_write_bilinear_nonplanar() {
        // A non-planar bilinear surface should export as B_SPLINE degree 1
        let bilinear = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 5.0), // Not coplanar
        );
        assert!(!bilinear.is_planar());

        let corners = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 5.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        let solid = make_single_face_solid(Box::new(bilinear), corners);
        let buffer = write_step_to_buffer(&solid).unwrap();
        let content = String::from_utf8_lossy(&buffer);

        assert!(
            content.contains("B_SPLINE_SURFACE_WITH_KNOTS"),
            "Non-planar bilinear should be written as B_SPLINE_SURFACE_WITH_KNOTS"
        );
        assert!(
            !content.contains("PLANE('"),
            "Non-planar bilinear should NOT be written as PLANE"
        );
    }

    #[test]
    fn test_roundtrip_bspline_surface() {
        // Create a B-spline surface, write to STEP, read back, verify geometry preserved
        let mut control_points = Vec::new();
        for v in 0..4 {
            for u in 0..4 {
                control_points.push(Point3::new(
                    u as f64 * 10.0,
                    v as f64 * 10.0,
                    if (u == 1 || u == 2) && (v == 1 || v == 2) {
                        5.0
                    } else {
                        0.0
                    },
                ));
            }
        }
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let bspline = BSplineSurface::new(
            control_points.clone(),
            4,
            4,
            knots.clone(),
            knots.clone(),
            3,
            3,
        );

        let corners = [
            bspline.eval(0.0, 0.0),
            bspline.eval(1.0, 0.0),
            bspline.eval(1.0, 1.0),
            bspline.eval(0.0, 1.0),
        ];

        let solid = make_single_face_solid(Box::new(bspline), corners);
        let buffer = write_step_to_buffer(&solid).unwrap();

        // Read back
        let solids = read_step_from_buffer(&buffer).unwrap();
        assert_eq!(solids.len(), 1);

        let imported = &solids[0];

        // Find the B-spline surface in imported geometry
        let imported_bspline = imported
            .geometry
            .surfaces
            .iter()
            .find(|s| s.surface_type() == SurfaceKind::BSpline)
            .expect("Should have a BSpline surface after roundtrip");

        let imported_bspline = imported_bspline
            .as_any()
            .downcast_ref::<BSplineSurface>()
            .unwrap();

        // Verify control points match
        assert_eq!(imported_bspline.n_u, 4);
        assert_eq!(imported_bspline.n_v, 4);
        assert_eq!(imported_bspline.degree_u, 3);
        assert_eq!(imported_bspline.degree_v, 3);
        assert_eq!(imported_bspline.control_points.len(), 16);

        for (orig, imported) in control_points
            .iter()
            .zip(imported_bspline.control_points.iter())
        {
            assert!(
                (orig.x - imported.x).abs() < 1e-6
                    && (orig.y - imported.y).abs() < 1e-6
                    && (orig.z - imported.z).abs() < 1e-6,
                "Control point mismatch: {:?} vs {:?}",
                orig,
                imported
            );
        }

        // Verify surface evaluates to same points
        for &(u, v) in &[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.75)] {
            let orig_pt = {
                let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
                let orig =
                    BSplineSurface::new(control_points.clone(), 4, 4, knots.clone(), knots, 3, 3);
                orig.eval(u, v)
            };
            let imported_pt = imported_bspline.eval(u, v);
            assert!(
                (orig_pt.x - imported_pt.x).abs() < 1e-6
                    && (orig_pt.y - imported_pt.y).abs() < 1e-6
                    && (orig_pt.z - imported_pt.z).abs() < 1e-6,
                "Surface eval mismatch at ({}, {}): {:?} vs {:?}",
                u,
                v,
                orig_pt,
                imported_pt
            );
        }
    }
}
