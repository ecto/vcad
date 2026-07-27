//! STEP AP214 assembly import.
//!
//! Reads `PRODUCT_DEFINITION` hierarchy, `NEXT_ASSEMBLY_USAGE_OCCURRENCE`
//! parent-child relationships, and `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION`
//! transforms to build a [`StepAssembly`] containing named parts and instances.

use std::collections::HashMap;
use std::path::Path;

use crate::entities::{parse_any_axis_placement, EntityArgs};
use crate::error::StepError;
use crate::reader::read_solid_from_file;
use stepperoni::{Parser, StepFile};
use vcad_kernel_primitives::BRepSolid;

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

/// A 3-D placement parsed from `AXIS2_PLACEMENT_3D` in the parent frame.
///
/// The placement is given as:
/// - `origin`: translation of the child's local origin in the parent frame
/// - `x_axis`: X-axis direction in the parent frame
/// - `z_axis`: Z-axis direction (normal) in the parent frame
///
/// Y-axis = Z × X (right-hand rule).
#[derive(Debug, Clone)]
pub struct StepPlacement {
    /// Origin of the child coordinate frame in the parent frame (mm).
    pub origin: [f64; 3],
    /// X-axis direction in the parent frame (unit vector).
    pub x_axis: [f64; 3],
    /// Z-axis direction in the parent frame (unit vector).
    pub z_axis: [f64; 3],
}

impl Default for StepPlacement {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        }
    }
}

impl StepPlacement {
    /// Compute the Y-axis from Z × X.
    pub fn y_axis(&self) -> [f64; 3] {
        let z = self.z_axis;
        let x = self.x_axis;
        [
            z[1] * x[2] - z[2] * x[1],
            z[2] * x[0] - z[0] * x[2],
            z[0] * x[1] - z[1] * x[0],
        ]
    }

    /// Build a column-major 4×4 homogeneous transform matrix.
    ///
    /// Column order: `[x_col, y_col, z_col, translation_col]`.
    pub fn to_mat4(&self) -> [[f64; 4]; 4] {
        let x = self.x_axis;
        let y = self.y_axis();
        let z = self.z_axis;
        let o = self.origin;
        [
            [x[0], x[1], x[2], 0.0],
            [y[0], y[1], y[2], 0.0],
            [z[0], z[1], z[2], 0.0],
            [o[0], o[1], o[2], 1.0],
        ]
    }
}

/// A geometry part extracted from a STEP assembly.
///
/// One `StepPart` is created per `PRODUCT_DEFINITION` that has associated
/// `MANIFOLD_SOLID_BREP` geometry.
#[derive(Debug)]
pub struct StepPart {
    /// Identifier of the `PRODUCT_DEFINITION` entity (as a decimal string).
    pub id: String,
    /// Human-readable name from the associated `PRODUCT` entity.
    pub name: String,
    /// B-rep solids that belong to this part definition.
    pub solids: Vec<BRepSolid>,
}

/// An instance of a part placed in a STEP assembly.
///
/// Each `StepInstance` corresponds to one `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION`
/// (or identity-transform `SHAPE_REPRESENTATION_RELATIONSHIP`) that relates
/// the parent and child shape representations.
#[derive(Debug)]
pub struct StepInstance {
    /// Numeric identifier (from `NEXT_ASSEMBLY_USAGE_OCCURRENCE` or synthetic counter).
    pub id: String,
    /// Human-readable name from the NAUO entity.
    pub name: String,
    /// `StepPart.id` of the part being placed.
    pub part_id: String,
    /// `StepPart.id` of the parent part. `None` means this is a root-level instance.
    pub parent_id: Option<String>,
    /// Local-to-parent placement transform.
    ///
    /// When `ITEM_DEFINED_TRANSFORMATION` has a non-identity `transform_item_2`
    /// (child-frame reference), the effective transform is `placement * child_frame^{-1}`.
    /// Most industrial STEP files use an identity child frame.
    pub transform: StepPlacement,
}

/// The result of reading a STEP file as an assembly.
///
/// For single-body part files (no `PRODUCT_DEFINITION` hierarchy), this
/// degenerates to one part per `MANIFOLD_SOLID_BREP` with identity instances.
#[derive(Debug)]
pub struct StepAssembly {
    /// All geometry parts in the assembly.
    pub parts: Vec<StepPart>,
    /// All instances (placements of parts, potentially nested).
    pub instances: Vec<StepInstance>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Read a STEP file as an assembly from a path.
///
/// Falls back to flat part-per-solid mode for files without `PRODUCT_DEFINITION`
/// hierarchy.
pub fn read_step_assembly(path: impl AsRef<Path>) -> Result<StepAssembly, StepError> {
    let data = std::fs::read(path)?;
    read_step_assembly_from_buffer(&data)
}

/// Read a STEP file as an assembly from a byte buffer.
///
/// Falls back to flat part-per-solid mode for files without `PRODUCT_DEFINITION`
/// hierarchy.
pub fn read_step_assembly_from_buffer(data: &[u8]) -> Result<StepAssembly, StepError> {
    let step_file = Parser::parse(data)?;
    let reader = AssemblyReader::new(&step_file);
    reader.read()
}

// ---------------------------------------------------------------------------
// Internal assembly reader
// ---------------------------------------------------------------------------

struct AssemblyReader<'a> {
    file: &'a StepFile,
}

impl<'a> AssemblyReader<'a> {
    fn new(file: &'a StepFile) -> Self {
        Self { file }
    }

    fn read(self) -> Result<StepAssembly, StepError> {
        // ---------------------------------------------------------------
        // Step 1: gather product names
        // PRODUCT(name, id, description, (contexts))
        //   arg[0] = name (string)
        // ---------------------------------------------------------------
        let mut product_name: HashMap<u64, String> = HashMap::new();
        for e in self.file.entities_of_type("PRODUCT") {
            if let Some(name) = e.args.first().and_then(|v| v.as_string()) {
                product_name.insert(e.id, name.to_owned());
            }
        }

        // ---------------------------------------------------------------
        // Step 2: PRODUCT_DEFINITION_FORMATION  →  product
        // PRODUCT_DEFINITION_FORMATION(id, description, product_ref)
        //   arg[2] = product entity ref
        // ---------------------------------------------------------------
        let mut formation_to_product: HashMap<u64, u64> = HashMap::new();
        for e in self.file.entities_of_type("PRODUCT_DEFINITION_FORMATION") {
            if let Ok(prod_id) = e.entity_ref(2) {
                formation_to_product.insert(e.id, prod_id);
            }
        }

        // ---------------------------------------------------------------
        // Step 3: PRODUCT_DEFINITION  →  name (via formation → product)
        // PRODUCT_DEFINITION(id, description, formation_ref, context_ref)
        //   arg[2] = PRODUCT_DEFINITION_FORMATION ref
        // ---------------------------------------------------------------
        let mut pd_name: HashMap<u64, String> = HashMap::new();
        for e in self.file.entities_of_type("PRODUCT_DEFINITION") {
            if let Ok(form_id) = e.entity_ref(2) {
                if let Some(&prod_id) = formation_to_product.get(&form_id) {
                    let name = product_name
                        .get(&prod_id)
                        .cloned()
                        .unwrap_or_else(|| format!("part_{}", e.id));
                    pd_name.insert(e.id, name);
                } else {
                    pd_name.insert(e.id, format!("part_{}", e.id));
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 4: PRODUCT_DEFINITION_SHAPE  →  product_definition
        // PRODUCT_DEFINITION_SHAPE(id, description, product_def_or_relationship)
        //   arg[2] = PRODUCT_DEFINITION ref (or PRODUCT_DEFINITION_RELATIONSHIP)
        // ---------------------------------------------------------------
        let mut pd_shape_to_pd: HashMap<u64, u64> = HashMap::new();
        for e in self.file.entities_of_type("PRODUCT_DEFINITION_SHAPE") {
            if let Ok(pd_id) = e.entity_ref(2) {
                // Only register if the reference points to a PRODUCT_DEFINITION
                if self
                    .file
                    .get(pd_id)
                    .map(|e2| e2.type_name == "PRODUCT_DEFINITION")
                    .unwrap_or(false)
                {
                    pd_shape_to_pd.insert(e.id, pd_id);
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 5: shape representation  →  PRODUCT_DEFINITION
        // SHAPE_DEFINITION_REPRESENTATION(shape_def_prop, shape_rep)
        //   arg[0] = PRODUCT_DEFINITION_SHAPE ref
        //   arg[1] = shape representation ref
        // ---------------------------------------------------------------
        let mut shape_rep_to_pd: HashMap<u64, u64> = HashMap::new();
        let mut pd_to_shape_reps: HashMap<u64, Vec<u64>> = HashMap::new();

        for e in self
            .file
            .entities_of_type("SHAPE_DEFINITION_REPRESENTATION")
        {
            if let (Ok(pds_id), Ok(rep_id)) = (e.entity_ref(0), e.entity_ref(1)) {
                if let Some(&pd_id) = pd_shape_to_pd.get(&pds_id) {
                    shape_rep_to_pd.insert(rep_id, pd_id);
                    pd_to_shape_reps.entry(pd_id).or_default().push(rep_id);
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 6: shape representation  →  MANIFOLD_SOLID_BREP ids
        //
        // ADVANCED_BREP_SHAPE_REPRESENTATION(name, items_list, context)
        //   arg[1] = list of items (MANIFOLD_SOLID_BREP or AXIS2_PLACEMENT_3D refs)
        // SHAPE_REPRESENTATION has the same structure.
        // ---------------------------------------------------------------
        let mut shape_rep_to_solids: HashMap<u64, Vec<u64>> = HashMap::new();

        for type_name in &[
            "ADVANCED_BREP_SHAPE_REPRESENTATION",
            "SHAPE_REPRESENTATION",
            "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
        ] {
            for e in self.file.entities_of_type(type_name) {
                if let Ok(items) = e.entity_ref_list(1) {
                    let solid_ids: Vec<u64> = items
                        .into_iter()
                        .filter(|&item_id| {
                            self.file
                                .get(item_id)
                                .map(|e2| e2.type_name == "MANIFOLD_SOLID_BREP")
                                .unwrap_or(false)
                        })
                        .collect();
                    if !solid_ids.is_empty() {
                        shape_rep_to_solids.insert(e.id, solid_ids);
                    }
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 7: build StepPart for each PD that has geometry
        //
        // Also build a fallback "all solids in one part" for files with no
        // PRODUCT_DEFINITION hierarchy.
        // ---------------------------------------------------------------
        let has_pd_hierarchy = !pd_name.is_empty();

        let mut parts: Vec<StepPart> = Vec::new();
        let mut pd_to_part_idx: HashMap<u64, usize> = HashMap::new();

        if has_pd_hierarchy {
            // Build one part per PD that has solid geometry
            for (&pd_id, name) in &pd_name {
                let shape_reps = pd_to_shape_reps.get(&pd_id).cloned().unwrap_or_default();
                let solid_ids: Vec<u64> = shape_reps
                    .iter()
                    .flat_map(|rep_id| shape_rep_to_solids.get(rep_id).cloned().unwrap_or_default())
                    .collect();

                if solid_ids.is_empty() {
                    // Assembly node (no direct geometry)
                    let idx = parts.len();
                    parts.push(StepPart {
                        id: pd_id.to_string(),
                        name: name.clone(),
                        solids: vec![],
                    });
                    pd_to_part_idx.insert(pd_id, idx);
                    continue;
                }

                let mut solids = Vec::new();
                for solid_id in solid_ids {
                    if let Ok(s) = read_solid_from_file(self.file, solid_id) {
                        solids.push(s);
                    }
                }
                let idx = parts.len();
                parts.push(StepPart {
                    id: pd_id.to_string(),
                    name: name.clone(),
                    solids,
                });
                pd_to_part_idx.insert(pd_id, idx);
            }
        } else {
            // No PD hierarchy: one part per MANIFOLD_SOLID_BREP
            for e in self.file.entities_of_type("MANIFOLD_SOLID_BREP") {
                let name = e
                    .args
                    .first()
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_owned();
                let solid = match read_solid_from_file(self.file, e.id) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let idx = parts.len();
                parts.push(StepPart {
                    id: e.id.to_string(),
                    name: if name.is_empty() {
                        format!("solid_{}", e.id)
                    } else {
                        name
                    },
                    solids: vec![solid],
                });
                // Store e.id as "pd_id" even though it's actually a MANIFOLD_SOLID_BREP id,
                // so we can look it up later.
                pd_to_part_idx.insert(e.id, idx);
            }
        }

        if parts.is_empty() {
            return Err(StepError::NoSolids);
        }

        // ---------------------------------------------------------------
        // Step 8: parse transforms
        //
        // ITEM_DEFINED_TRANSFORMATION(name, description, axis1_ref, axis2_ref)
        //   axis1 = placement in parent frame
        //   axis2 = placement in child frame (usually identity → we ignore it)
        //
        // REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(name, desc, rep1, rep2, transform)
        //   rep1 = parent shape rep
        //   rep2 = child shape rep
        //   transform = ITEM_DEFINED_TRANSFORMATION ref
        // ---------------------------------------------------------------
        let mut transform_map: HashMap<u64, StepPlacement> = HashMap::new();
        for e in self.file.entities_of_type("ITEM_DEFINED_TRANSFORMATION") {
            if let Ok(axis1_id) = e.entity_ref(2) {
                if let Ok(placement) = parse_any_axis_placement(self.file, axis1_id) {
                    let x = placement.x_axis();
                    let z = placement.z_axis();
                    transform_map.insert(
                        e.id,
                        StepPlacement {
                            origin: [
                                placement.location.x,
                                placement.location.y,
                                placement.location.z,
                            ],
                            x_axis: [x.as_ref().x, x.as_ref().y, x.as_ref().z],
                            z_axis: [z.as_ref().x, z.as_ref().y, z.as_ref().z],
                        },
                    );
                }
            }
        }

        // ---------------------------------------------------------------
        // Step 9: build instances from NEXT_ASSEMBLY_USAGE_OCCURRENCE
        //
        // NEXT_ASSEMBLY_USAGE_OCCURRENCE(id, name, description, parent_pd, child_pd, $)
        //   arg[0] = id (string)
        //   arg[1] = name (string)
        //   arg[3] = parent PRODUCT_DEFINITION ref
        //   arg[4] = child PRODUCT_DEFINITION ref
        // ---------------------------------------------------------------
        let mut instances: Vec<StepInstance> = Vec::new();
        let mut nauo_counter = 0u64;

        // Build RRWT lookup: (parent_rep_id, child_rep_id) → transform_id
        let mut rrwt_transforms: HashMap<(u64, u64), u64> = HashMap::new();
        for e in self
            .file
            .entities_of_type("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
        {
            if let (Ok(rep1), Ok(rep2), Ok(xf_id)) =
                (e.entity_ref(2), e.entity_ref(3), e.entity_ref(4))
            {
                rrwt_transforms.insert((rep1, rep2), xf_id);
            }
        }
        // Also handle SHAPE_REPRESENTATION_RELATIONSHIP (identity transform)
        let mut srr_pairs: Vec<(u64, u64)> = Vec::new();
        for e in self
            .file
            .entities_of_type("SHAPE_REPRESENTATION_RELATIONSHIP")
        {
            if let (Ok(rep1), Ok(rep2)) = (e.entity_ref(2), e.entity_ref(3)) {
                srr_pairs.push((rep1, rep2));
            }
        }

        if has_pd_hierarchy {
            for e in self.file.entities_of_type("NEXT_ASSEMBLY_USAGE_OCCURRENCE") {
                let nauo_name = e
                    .args
                    .get(1)
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_owned();
                let parent_pd = match e.entity_ref(3) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let child_pd = match e.entity_ref(4) {
                    Ok(id) => id,
                    Err(_) => continue,
                };

                let parent_part_id = pd_to_part_idx
                    .get(&parent_pd)
                    .map(|&idx| parts[idx].id.clone());
                let child_part_id = match pd_to_part_idx.get(&child_pd) {
                    Some(&idx) => parts[idx].id.clone(),
                    None => continue, // child has no geometry
                };

                // Find a transform by looking for RRWT connecting parent and child shape reps
                let parent_reps = pd_to_shape_reps
                    .get(&parent_pd)
                    .cloned()
                    .unwrap_or_default();
                let child_reps = pd_to_shape_reps.get(&child_pd).cloned().unwrap_or_default();

                let transform = self.find_transform(
                    &parent_reps,
                    &child_reps,
                    &rrwt_transforms,
                    &srr_pairs,
                    &transform_map,
                );

                let instance_id = format!("{}", e.id);
                instances.push(StepInstance {
                    id: instance_id,
                    name: nauo_name,
                    part_id: child_part_id,
                    parent_id: parent_part_id,
                    transform,
                });
                nauo_counter += 1;
            }
        }

        // If no NAUOs found (flat file or pure-geometry STEP), create root instances
        if instances.is_empty() {
            for (i, part) in parts.iter().enumerate() {
                instances.push(StepInstance {
                    id: format!("inst_{i}"),
                    name: part.name.clone(),
                    part_id: part.id.clone(),
                    parent_id: None,
                    transform: StepPlacement::default(),
                });
                nauo_counter += 1;
            }
        }

        let _ = nauo_counter;

        Ok(StepAssembly { parts, instances })
    }

    /// Search for a transform between parent and child shape representations.
    ///
    /// Looks in `rrwt_transforms` for an entry where `parent_reps` × `child_reps` matches,
    /// then resolves the `ITEM_DEFINED_TRANSFORMATION` to a placement.
    ///
    /// Falls back to identity if nothing is found.
    fn find_transform(
        &self,
        parent_reps: &[u64],
        child_reps: &[u64],
        rrwt_transforms: &HashMap<(u64, u64), u64>,
        srr_pairs: &[(u64, u64)],
        transform_map: &HashMap<u64, StepPlacement>,
    ) -> StepPlacement {
        // Try RRWT first
        for &parent_rep in parent_reps {
            for &child_rep in child_reps {
                if let Some(&xf_id) = rrwt_transforms.get(&(parent_rep, child_rep)) {
                    if let Some(placement) = transform_map.get(&xf_id) {
                        return placement.clone();
                    }
                }
            }
        }
        // SRR = identity transform
        for &(rep1, rep2) in srr_pairs {
            if parent_reps.contains(&rep1) && child_reps.contains(&rep2) {
                return StepPlacement::default();
            }
        }
        StepPlacement::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal STEP assembly: two boxes, one as parent, one as child
    /// placed 20 mm along X.
    const ASSEMBLY_STEP: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Assembly test'), '2;1');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;

/* ---- Products ---- */
#1 = PRODUCT('Parent', 'P1', '', (#10));
#2 = PRODUCT('Child', 'P2', '', (#10));
#10 = PRODUCT_CONTEXT('', #11, 'mechanical');
#11 = APPLICATION_CONTEXT('core data for automotive mechanical design processes');

#20 = PRODUCT_DEFINITION_FORMATION('', '', #1);
#21 = PRODUCT_DEFINITION_FORMATION('', '', #2);

#30 = PRODUCT_DEFINITION_CONTEXT('', #11, 'design');
#31 = PRODUCT_DEFINITION('', '', #20, #30);
#32 = PRODUCT_DEFINITION('', '', #21, #30);

/* ---- Shape associations ---- */
#40 = PRODUCT_DEFINITION_SHAPE('', '', #31);
#41 = PRODUCT_DEFINITION_SHAPE('', '', #32);

/* ---- Geometry for child part (a 10mm box) ---- */
#100 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#101 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#102 = CARTESIAN_POINT('', (10.0, 10.0, 0.0));
#103 = CARTESIAN_POINT('', (0.0, 10.0, 0.0));
#104 = CARTESIAN_POINT('', (0.0, 0.0, 10.0));
#105 = CARTESIAN_POINT('', (10.0, 0.0, 10.0));
#106 = CARTESIAN_POINT('', (10.0, 10.0, 10.0));
#107 = CARTESIAN_POINT('', (0.0, 10.0, 10.0));
#110 = VERTEX_POINT('', #100);
#111 = VERTEX_POINT('', #101);
#112 = VERTEX_POINT('', #102);
#113 = VERTEX_POINT('', #103);
#114 = VERTEX_POINT('', #104);
#115 = VERTEX_POINT('', #105);
#116 = VERTEX_POINT('', #106);
#117 = VERTEX_POINT('', #107);
#120 = DIRECTION('', (0.0, 0.0, 1.0));
#121 = DIRECTION('', (1.0, 0.0, 0.0));
#122 = DIRECTION('', (0.0, 0.0, -1.0));
#123 = DIRECTION('', (0.0, 1.0, 0.0));
#124 = DIRECTION('', (0.0, -1.0, 0.0));
#125 = DIRECTION('', (-1.0, 0.0, 0.0));
#130 = AXIS2_PLACEMENT_3D('', #100, #122, #121);
#131 = AXIS2_PLACEMENT_3D('', #104, #120, #121);
#132 = AXIS2_PLACEMENT_3D('', #100, #124, #121);
#133 = AXIS2_PLACEMENT_3D('', #103, #123, #121);
#134 = AXIS2_PLACEMENT_3D('', #100, #125, #123);
#135 = AXIS2_PLACEMENT_3D('', #101, #121, #123);
#140 = PLANE('', #130);
#141 = PLANE('', #131);
#142 = PLANE('', #132);
#143 = PLANE('', #133);
#144 = PLANE('', #134);
#145 = PLANE('', #135);
#150 = DIRECTION('', (1.0, 0.0, 0.0));
#151 = DIRECTION('', (0.0, 1.0, 0.0));
#152 = DIRECTION('', (0.0, 0.0, 1.0));
#153 = DIRECTION('', (-1.0, 0.0, 0.0));
#154 = DIRECTION('', (0.0, -1.0, 0.0));
#155 = DIRECTION('', (0.0, 0.0, -1.0));
#160 = VECTOR('', #150, 10.0);
#161 = VECTOR('', #151, 10.0);
#162 = VECTOR('', #152, 10.0);
#163 = VECTOR('', #153, 10.0);
#164 = VECTOR('', #154, 10.0);
#165 = VECTOR('', #155, 10.0);
#170 = LINE('', #100, #160);
#171 = LINE('', #101, #161);
#172 = LINE('', #102, #163);
#173 = LINE('', #103, #164);
#174 = LINE('', #100, #161);
#175 = LINE('', #104, #160);
#176 = LINE('', #105, #161);
#177 = LINE('', #106, #163);
#178 = LINE('', #107, #164);
#179 = LINE('', #100, #162);
#180 = LINE('', #101, #162);
#181 = LINE('', #102, #162);
#182 = LINE('', #103, #162);
#190 = EDGE_CURVE('', #110, #111, #170, .T.);
#191 = EDGE_CURVE('', #111, #112, #171, .T.);
#192 = EDGE_CURVE('', #112, #113, #172, .T.);
#193 = EDGE_CURVE('', #113, #110, #173, .T.);
#194 = EDGE_CURVE('', #114, #115, #175, .T.);
#195 = EDGE_CURVE('', #115, #116, #176, .T.);
#196 = EDGE_CURVE('', #116, #117, #177, .T.);
#197 = EDGE_CURVE('', #117, #114, #178, .T.);
#198 = EDGE_CURVE('', #110, #114, #179, .T.);
#199 = EDGE_CURVE('', #111, #115, #180, .T.);
#200 = EDGE_CURVE('', #112, #116, #181, .T.);
#201 = EDGE_CURVE('', #113, #117, #182, .T.);
#210 = ORIENTED_EDGE('', *, *, #190, .F.);
#211 = ORIENTED_EDGE('', *, *, #193, .F.);
#212 = ORIENTED_EDGE('', *, *, #192, .F.);
#213 = ORIENTED_EDGE('', *, *, #191, .F.);
#214 = ORIENTED_EDGE('', *, *, #194, .T.);
#215 = ORIENTED_EDGE('', *, *, #195, .T.);
#216 = ORIENTED_EDGE('', *, *, #196, .T.);
#217 = ORIENTED_EDGE('', *, *, #197, .T.);
#220 = ORIENTED_EDGE('', *, *, #190, .T.);
#221 = ORIENTED_EDGE('', *, *, #199, .T.);
#222 = ORIENTED_EDGE('', *, *, #194, .F.);
#223 = ORIENTED_EDGE('', *, *, #198, .F.);
#224 = ORIENTED_EDGE('', *, *, #192, .T.);
#225 = ORIENTED_EDGE('', *, *, #201, .T.);
#226 = ORIENTED_EDGE('', *, *, #196, .F.);
#227 = ORIENTED_EDGE('', *, *, #200, .F.);
#228 = ORIENTED_EDGE('', *, *, #193, .T.);
#229 = ORIENTED_EDGE('', *, *, #198, .T.);
#230 = ORIENTED_EDGE('', *, *, #197, .F.);
#231 = ORIENTED_EDGE('', *, *, #201, .F.);
#232 = ORIENTED_EDGE('', *, *, #191, .T.);
#233 = ORIENTED_EDGE('', *, *, #200, .T.);
#234 = ORIENTED_EDGE('', *, *, #195, .F.);
#235 = ORIENTED_EDGE('', *, *, #199, .F.);
#240 = EDGE_LOOP('', (#210, #211, #212, #213));
#241 = EDGE_LOOP('', (#214, #215, #216, #217));
#242 = EDGE_LOOP('', (#220, #221, #222, #223));
#243 = EDGE_LOOP('', (#224, #225, #226, #227));
#244 = EDGE_LOOP('', (#228, #229, #230, #231));
#245 = EDGE_LOOP('', (#232, #233, #234, #235));
#250 = FACE_OUTER_BOUND('', #240, .T.);
#251 = FACE_OUTER_BOUND('', #241, .T.);
#252 = FACE_OUTER_BOUND('', #242, .T.);
#253 = FACE_OUTER_BOUND('', #243, .T.);
#254 = FACE_OUTER_BOUND('', #244, .T.);
#255 = FACE_OUTER_BOUND('', #245, .T.);
#260 = ADVANCED_FACE('', (#250), #140, .T.);
#261 = ADVANCED_FACE('', (#251), #141, .T.);
#262 = ADVANCED_FACE('', (#252), #142, .T.);
#263 = ADVANCED_FACE('', (#253), #143, .T.);
#264 = ADVANCED_FACE('', (#254), #144, .T.);
#265 = ADVANCED_FACE('', (#255), #145, .T.);
#270 = CLOSED_SHELL('', (#260, #261, #262, #263, #264, #265));
#280 = MANIFOLD_SOLID_BREP('ChildBox', #270);

/* ---- Shape representation for child ---- */
#290 = GEOMETRIC_REPRESENTATION_CONTEXT(3);
#291 = ADVANCED_BREP_SHAPE_REPRESENTATION('', (#280), #290);
#292 = SHAPE_DEFINITION_REPRESENTATION(#41, #291);

/* ---- Shape representation for parent (empty — assembly node) ---- */
#293 = ADVANCED_BREP_SHAPE_REPRESENTATION('', (), #290);
#294 = SHAPE_DEFINITION_REPRESENTATION(#40, #293);

/* ---- Transform: child placed at (20, 0, 0) ---- */
#300 = CARTESIAN_POINT('', (20.0, 0.0, 0.0));
#301 = DIRECTION('', (0.0, 0.0, 1.0));
#302 = DIRECTION('', (1.0, 0.0, 0.0));
#303 = AXIS2_PLACEMENT_3D('', #300, #301, #302);
#304 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#305 = DIRECTION('', (0.0, 0.0, 1.0));
#306 = DIRECTION('', (1.0, 0.0, 0.0));
#307 = AXIS2_PLACEMENT_3D('', #304, #305, #306);
#310 = ITEM_DEFINED_TRANSFORMATION('', '', #303, #307);

/* ---- RRWT ---- */
#320 = REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION('', '', #293, #291, #310);

/* ---- NAUO ---- */
#330 = NEXT_ASSEMBLY_USAGE_OCCURRENCE('U1', 'child_instance', '', #31, #32, $);

ENDSEC;
END-ISO-10303-21;
"#;

    #[test]
    fn test_read_assembly_basic() {
        let asm = read_step_assembly_from_buffer(ASSEMBLY_STEP.as_bytes()).unwrap();
        // Two product definitions → two parts
        assert_eq!(asm.parts.len(), 2);

        // One of the parts is the child with geometry
        let child = asm.parts.iter().find(|p| !p.solids.is_empty()).unwrap();
        assert_eq!(child.solids.len(), 1);
        assert_eq!(child.solids[0].topology.faces.len(), 6);

        // One instance
        assert_eq!(asm.instances.len(), 1);
        let inst = &asm.instances[0];
        assert!(inst.parent_id.is_some());
        // Transform: placed 20 mm along X
        assert!((inst.transform.origin[0] - 20.0).abs() < 1e-6);
        assert!(inst.transform.origin[1].abs() < 1e-6);
        assert!(inst.transform.origin[2].abs() < 1e-6);
    }

    #[test]
    fn test_read_flat_step_as_assembly() {
        // A plain flat STEP file (no PD hierarchy) should be read as an assembly
        // with one root instance per solid.
        let step = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (5.0, 0.0, 0.0));
#3 = CARTESIAN_POINT('', (5.0, 5.0, 0.0));
#4 = CARTESIAN_POINT('', (0.0, 5.0, 0.0));
#5 = CARTESIAN_POINT('', (0.0, 0.0, 5.0));
#6 = CARTESIAN_POINT('', (5.0, 0.0, 5.0));
#7 = CARTESIAN_POINT('', (5.0, 5.0, 5.0));
#8 = CARTESIAN_POINT('', (0.0, 5.0, 5.0));
#11 = VERTEX_POINT('', #1);
#12 = VERTEX_POINT('', #2);
#13 = VERTEX_POINT('', #3);
#14 = VERTEX_POINT('', #4);
#15 = VERTEX_POINT('', #5);
#16 = VERTEX_POINT('', #6);
#17 = VERTEX_POINT('', #7);
#18 = VERTEX_POINT('', #8);
#20 = DIRECTION('', (0.0, 0.0, 1.0));
#21 = DIRECTION('', (1.0, 0.0, 0.0));
#22 = DIRECTION('', (0.0, 0.0, -1.0));
#23 = DIRECTION('', (0.0, 1.0, 0.0));
#24 = DIRECTION('', (0.0, -1.0, 0.0));
#25 = DIRECTION('', (-1.0, 0.0, 0.0));
#30 = AXIS2_PLACEMENT_3D('', #1, #22, #21);
#31 = AXIS2_PLACEMENT_3D('', #5, #20, #21);
#32 = AXIS2_PLACEMENT_3D('', #1, #24, #21);
#33 = AXIS2_PLACEMENT_3D('', #4, #23, #21);
#34 = AXIS2_PLACEMENT_3D('', #1, #25, #23);
#35 = AXIS2_PLACEMENT_3D('', #2, #21, #23);
#40 = PLANE('', #30);
#41 = PLANE('', #31);
#42 = PLANE('', #32);
#43 = PLANE('', #33);
#44 = PLANE('', #34);
#45 = PLANE('', #35);
#50 = DIRECTION('', (1.0, 0.0, 0.0));
#51 = DIRECTION('', (0.0, 1.0, 0.0));
#52 = DIRECTION('', (0.0, 0.0, 1.0));
#53 = DIRECTION('', (-1.0, 0.0, 0.0));
#54 = DIRECTION('', (0.0, -1.0, 0.0));
#55 = DIRECTION('', (0.0, 0.0, -1.0));
#60 = VECTOR('', #50, 5.0);
#61 = VECTOR('', #51, 5.0);
#62 = VECTOR('', #52, 5.0);
#63 = VECTOR('', #53, 5.0);
#64 = VECTOR('', #54, 5.0);
#65 = VECTOR('', #55, 5.0);
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
#100 = EDGE_CURVE('', #11, #12, #70, .T.);
#101 = EDGE_CURVE('', #12, #13, #71, .T.);
#102 = EDGE_CURVE('', #13, #14, #72, .T.);
#103 = EDGE_CURVE('', #14, #11, #73, .T.);
#104 = EDGE_CURVE('', #15, #16, #75, .T.);
#105 = EDGE_CURVE('', #16, #17, #76, .T.);
#106 = EDGE_CURVE('', #17, #18, #77, .T.);
#107 = EDGE_CURVE('', #18, #15, #78, .T.);
#108 = EDGE_CURVE('', #11, #15, #79, .T.);
#109 = EDGE_CURVE('', #12, #16, #80, .T.);
#110 = EDGE_CURVE('', #13, #17, #81, .T.);
#111 = EDGE_CURVE('', #14, #18, #82, .T.);
#120 = ORIENTED_EDGE('', *, *, #100, .F.);
#121 = ORIENTED_EDGE('', *, *, #103, .F.);
#122 = ORIENTED_EDGE('', *, *, #102, .F.);
#123 = ORIENTED_EDGE('', *, *, #101, .F.);
#124 = ORIENTED_EDGE('', *, *, #104, .T.);
#125 = ORIENTED_EDGE('', *, *, #105, .T.);
#126 = ORIENTED_EDGE('', *, *, #106, .T.);
#127 = ORIENTED_EDGE('', *, *, #107, .T.);
#130 = ORIENTED_EDGE('', *, *, #100, .T.);
#131 = ORIENTED_EDGE('', *, *, #109, .T.);
#132 = ORIENTED_EDGE('', *, *, #104, .F.);
#133 = ORIENTED_EDGE('', *, *, #108, .F.);
#134 = ORIENTED_EDGE('', *, *, #102, .T.);
#135 = ORIENTED_EDGE('', *, *, #111, .T.);
#136 = ORIENTED_EDGE('', *, *, #106, .F.);
#137 = ORIENTED_EDGE('', *, *, #110, .F.);
#138 = ORIENTED_EDGE('', *, *, #103, .T.);
#139 = ORIENTED_EDGE('', *, *, #108, .T.);
#140 = ORIENTED_EDGE('', *, *, #107, .F.);
#141 = ORIENTED_EDGE('', *, *, #111, .F.);
#142 = ORIENTED_EDGE('', *, *, #101, .T.);
#143 = ORIENTED_EDGE('', *, *, #110, .T.);
#144 = ORIENTED_EDGE('', *, *, #105, .F.);
#145 = ORIENTED_EDGE('', *, *, #109, .F.);
#150 = EDGE_LOOP('', (#120, #121, #122, #123));
#151 = EDGE_LOOP('', (#124, #125, #126, #127));
#152 = EDGE_LOOP('', (#130, #131, #132, #133));
#153 = EDGE_LOOP('', (#134, #135, #136, #137));
#154 = EDGE_LOOP('', (#138, #139, #140, #141));
#155 = EDGE_LOOP('', (#142, #143, #144, #145));
#160 = FACE_OUTER_BOUND('', #150, .T.);
#161 = FACE_OUTER_BOUND('', #151, .T.);
#162 = FACE_OUTER_BOUND('', #152, .T.);
#163 = FACE_OUTER_BOUND('', #153, .T.);
#164 = FACE_OUTER_BOUND('', #154, .T.);
#165 = FACE_OUTER_BOUND('', #155, .T.);
#170 = ADVANCED_FACE('', (#160), #40, .T.);
#171 = ADVANCED_FACE('', (#161), #41, .T.);
#172 = ADVANCED_FACE('', (#162), #42, .T.);
#173 = ADVANCED_FACE('', (#163), #43, .T.);
#174 = ADVANCED_FACE('', (#164), #44, .T.);
#175 = ADVANCED_FACE('', (#165), #45, .T.);
#180 = CLOSED_SHELL('', (#170, #171, #172, #173, #174, #175));
#190 = MANIFOLD_SOLID_BREP('Box', #180);
ENDSEC;
END-ISO-10303-21;
"#;
        let asm = read_step_assembly_from_buffer(step.as_bytes()).unwrap();
        assert_eq!(asm.parts.len(), 1);
        assert_eq!(asm.parts[0].solids.len(), 1);
        assert_eq!(asm.instances.len(), 1);
        assert!(asm.instances[0].parent_id.is_none());
    }
}
