//! IR-to-kernel type conversions.

use vcad_ir::{SketchSegment2D, Vec3 as IrVec3};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_sketch::{SketchError, SketchProfile, SketchSegment};

/// Convert IR Vec3 to kernel Vec3.
pub fn to_vec3(v: &IrVec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// Convert IR Vec3 to kernel Point3.
pub fn to_point3(v: &IrVec3) -> Point3 {
    Point3::new(v.x, v.y, v.z)
}

/// Convert IR sketch segment to kernel SketchSegment.
fn convert_segment(seg: &SketchSegment2D) -> SketchSegment {
    match seg {
        SketchSegment2D::Line { start, end } => SketchSegment::Line {
            start: Point2::new(start.x, start.y),
            end: Point2::new(end.x, end.y),
        },
        SketchSegment2D::Arc {
            start,
            end,
            center,
            ccw,
        } => SketchSegment::Arc {
            start: Point2::new(start.x, start.y),
            end: Point2::new(end.x, end.y),
            center: Point2::new(center.x, center.y),
            ccw: *ccw,
        },
    }
}

/// Convert an IR Sketch2D operation into a kernel SketchProfile.
pub fn ir_sketch_to_profile(
    origin: &IrVec3,
    x_dir: &IrVec3,
    y_dir: &IrVec3,
    segments: &[SketchSegment2D],
) -> Result<SketchProfile, SketchError> {
    let segments: Vec<SketchSegment> = segments.iter().map(convert_segment).collect();
    SketchProfile::new(to_point3(origin), to_vec3(x_dir), to_vec3(y_dir), segments)
}

/// Convert IR hole loops into kernel segment loops.
pub fn ir_holes_to_segments(holes: &[Vec<SketchSegment2D>]) -> Vec<Vec<SketchSegment>> {
    holes
        .iter()
        .map(|hole| hole.iter().map(convert_segment).collect())
        .collect()
}
