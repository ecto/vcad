//! Recognise features straight from a vendor STEP file.
//!
//! A vendor download is an *assembly*: each component's B-rep is written in
//! its own local frame and placed by the product structure. Reading the solids
//! alone (as [`vcad_kernel_step::read_step`] does) therefore gives geometry in
//! mixed frames — good enough for per-component patterns, wrong for anything
//! that spans components, including the overall length.
//!
//! This module composes the assembly placements, transforms every component
//! into the assembly frame, and recognises both:
//!
//! * per-component reports, which is where bolt circles actually live, and
//! * one assembly-wide report, which is where the envelope lives.

use crate::{recognize, recognize_many, Envelope, FeatureReport};
use serde::Serialize;
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_step::{StepAssembly, StepError, StepPlacement};

/// One component of a STEP assembly, placed in the assembly frame.
#[derive(Debug, Serialize)]
pub struct ComponentReport {
    /// Component name from the STEP product structure.
    pub name: String,
    /// Number of faces in the component's B-rep.
    pub face_count: usize,
    /// Features of this component alone, in the assembly frame.
    pub report: FeatureReport,
}

/// Feature recognition over a whole STEP file.
#[derive(Debug, Serialize)]
pub struct StepFeatureReport {
    /// Per-component reports, largest component first.
    pub components: Vec<ComponentReport>,
    /// One report over every placed component — this is the envelope to quote.
    pub assembly: FeatureReport,
}

impl StepFeatureReport {
    /// The assembly envelope.
    pub fn envelope(&self) -> &Envelope {
        &self.assembly.envelope
    }
}

fn placement_to_transform(p: &StepPlacement) -> Transform {
    let x = Vec3::new(p.x_axis[0], p.x_axis[1], p.x_axis[2]);
    let z = Vec3::new(p.z_axis[0], p.z_axis[1], p.z_axis[2]);
    let y = z.cross(x);
    let o = Point3::new(p.origin[0], p.origin[1], p.origin[2]);
    Transform {
        matrix: tang::Mat4::from_cols(
            tang::Vec4::new(x.x, x.y, x.z, 0.0),
            tang::Vec4::new(y.x, y.y, y.z, 0.0),
            tang::Vec4::new(z.x, z.y, z.z, 0.0),
            tang::Vec4::new(o.x, o.y, o.z, 1.0),
        ),
    }
}

fn transformed(brep: &BRepSolid, t: &Transform) -> BRepSolid {
    let mut out = brep.clone();
    for (_id, v) in &mut out.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut out.geometry.surfaces {
        *s = s.transform(t);
    }
    out
}

/// Place every component of `assembly` into the assembly frame.
///
/// Components reached by no instance are taken as already placed (identity),
/// which is what part files — a single component with no product hierarchy —
/// need.
fn place(assembly: &StepAssembly) -> Vec<(String, BRepSolid)> {
    let mut out = Vec::new();
    let mut placed_parts: Vec<&str> = Vec::new();

    // Walk the instance tree from the roots, composing parent ∘ child.
    let mut stack: Vec<(&str, Transform)> = assembly
        .instances
        .iter()
        .filter(|i| i.parent_id.is_none())
        .map(|i| (i.part_id.as_str(), placement_to_transform(&i.transform)))
        .collect();
    let mut guard = 0usize;
    while let Some((part_id, world)) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            break; // cyclic product structure — refuse to spin
        }
        if let Some(part) = assembly.parts.iter().find(|p| p.id == part_id) {
            for solid in &part.solids {
                out.push((part.name.clone(), transformed(solid, &world)));
            }
            if !part.solids.is_empty() {
                placed_parts.push(part_id);
            }
        }
        for child in assembly
            .instances
            .iter()
            .filter(|i| i.parent_id.as_deref() == Some(part_id))
        {
            stack.push((
                child.part_id.as_str(),
                world.then(&placement_to_transform(&child.transform)),
            ));
        }
    }

    // Anything the instance tree never reached is already in the file frame.
    for part in &assembly.parts {
        if part.solids.is_empty() || placed_parts.contains(&part.id.as_str()) {
            continue;
        }
        for solid in &part.solids {
            out.push((part.name.clone(), solid.clone()));
        }
    }
    out
}

/// Recognise features in a STEP file, honouring the assembly placements.
pub fn recognize_step_file(
    path: impl AsRef<std::path::Path>,
) -> Result<StepFeatureReport, StepError> {
    let assembly = vcad_kernel_step::read_step_assembly(path)?;
    Ok(recognize_step_assembly(&assembly))
}

/// Recognise features in an already-parsed STEP assembly.
pub fn recognize_step_assembly(assembly: &StepAssembly) -> StepFeatureReport {
    let placed = place(assembly);
    let refs: Vec<&BRepSolid> = placed.iter().map(|(_, s)| s).collect();

    let mut components: Vec<ComponentReport> = placed
        .iter()
        .map(|(name, solid)| ComponentReport {
            name: name.clone(),
            face_count: solid.topology.faces.len(),
            report: recognize(solid),
        })
        .collect();
    components.sort_by_key(|c| std::cmp::Reverse(c.face_count));

    StepFeatureReport {
        assembly: recognize_many(&refs),
        components,
    }
}
