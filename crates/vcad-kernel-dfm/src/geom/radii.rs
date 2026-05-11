//! Detect cylindrical features and their radii.
//!
//! v1 walks `BRepSolid.geometry.surfaces` looking for [`CylinderSurface`]s.
//! Their radius is the canonical small-radius feature signal — too
//! small means "smaller than the smallest cutter / nozzle", which is
//! exactly what most processes flag.

use vcad_kernel_geom::{CylinderSurface, SurfaceKind};
use vcad_kernel_primitives::BRepSolid;

/// One cylindrical feature.
#[derive(Debug, Clone, Copy)]
pub struct CylinderSample {
    /// Index of the face that uses this surface (first occurrence).
    pub face: usize,
    /// Cylinder radius in mm.
    pub radius_mm: f64,
}

/// Find every cylindrical face and report its radius.
pub fn cylinders(brep: &BRepSolid) -> Vec<CylinderSample> {
    let mut out = Vec::new();
    for (idx, (_id, face)) in brep.topology.faces.iter().enumerate() {
        let Some(surface) = brep.geometry.surfaces.get(face.surface_index) else {
            continue;
        };
        if surface.surface_type() != SurfaceKind::Cylinder {
            continue;
        }
        if let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() {
            out.push(CylinderSample {
                face: idx,
                radius_mm: cyl.radius,
            });
        }
    }
    out
}
