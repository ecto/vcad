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

use vcad_kernel_geom::{
    BilinearSurface, ConeSurface, CylinderSurface, Plane, SphereSurface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Dir3, Vec3};
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

/// Write several named BRepSolids to a single STEP file on disk.
pub fn write_step_solids(
    solids: &[(&BRepSolid, &str)],
    path: impl AsRef<Path>,
) -> Result<(), StepError> {
    let buffer = write_step_solids_to_buffer(solids)?;
    std::fs::write(path, buffer)?;
    Ok(())
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

        for (edge_id, edge) in &topo.edges {
            // Get the half-edge to determine vertices
            let he = &topo.half_edges[edge.half_edge];
            let start_vid = he.origin;
            let end_vid = topo.half_edge_dest(edge.half_edge);
            let step_edge_id = self.write_line_edge_curve(start_vid, end_vid);
            self.edge_map.insert(edge_id, step_edge_id);
        }

        Ok(())
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
        let topo = &self.solid.topology;
        let start_vertex = self.vertex_map[&start_vid];
        let end_vertex = self.vertex_map[&end_vid];

        let start_point = topo.vertices[start_vid].point;
        let end_point = topo.vertices[end_vid].point;

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

    fn write_loops(&mut self) -> Result<(), StepError> {
        // Collect loops first so we can borrow self mutably inside.
        let loop_ids: Vec<LoopId> = self.solid.topology.loops.keys().collect();

        for loop_id in loop_ids {
            let he_ids: Vec<HalfEdgeId> = self.solid.topology.loop_half_edges(loop_id).collect();

            let mut oriented_edge_ids = Vec::new();

            for he_id in he_ids {
                let he = &self.solid.topology.half_edges[he_id];
                let (step_edge_id, orientation) = match he.edge {
                    Some(edge_id) => {
                        let step_edge_id = self.edge_map[&edge_id];
                        let edge = &self.solid.topology.edges[edge_id];
                        let orientation = edge.half_edge == he_id;
                        (step_edge_id, orientation)
                    }
                    None => {
                        // Orphan half-edge — boolean sewing leaves these when face
                        // boundaries can't be paired (curved-vs-polygonal mismatch).
                        // Synthesize a one-off line edge so the loop can still be
                        // emitted; orientation is forward since this he is the
                        // synthetic edge's only half-edge.
                        let start_vid = he.origin;
                        let end_vid = self.solid.topology.half_edge_dest(he_id);
                        let step_edge_id = self.write_line_edge_curve(start_vid, end_vid);
                        (step_edge_id, true)
                    }
                };

                let oe_id = self.alloc_id();
                let entity = write_oriented_edge("", step_edge_id, orientation);
                self.emit(oe_id, &entity);
                self.oriented_edge_map.insert(he_id, oe_id);

                oriented_edge_ids.push(oe_id);
            }

            let el_id = self.alloc_id();
            let entity = write_edge_loop("", &oriented_edge_ids);
            self.emit(el_id, &entity);
            self.loop_map.insert(loop_id, el_id);
        }

        Ok(())
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
            !content.contains("PLANE("),
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
            content.contains("PLANE("),
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
            !content.contains("PLANE("),
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
