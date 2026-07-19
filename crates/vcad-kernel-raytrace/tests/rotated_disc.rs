//! Regression: a rotated disc's cylindrical wall must not trace as an
//! infinite tube.
//!
//! A full cylinder wall's outer loop projects to a zero-area UV polygon
//! (only seam vertices survive projection — the rim circles collapse), and
//! the degenerate-loop "untrimmed" fallback (added for full spheres) left v
//! unbounded. On screen this rendered e.g. the Stirling-engine flywheel
//! (cylinder r40 h8 rotated 90° about X) elongated ~10x along its axis.

use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_raytrace::{Bvh, Ray};

/// Cylinder r40 h8, rotated 90° about X: axis ends up along -Y, wall spans
/// y in [-8, 0].
fn rotated_disc() -> BRepSolid {
    let mut brep = make_cylinder(40.0, 8.0, 32);
    let t = Transform::rotation_x(90f64.to_radians());
    for (_id, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut brep.geometry.surfaces {
        *s = s.transform(&t);
    }
    brep
}

#[test]
fn cpu_wall_bounded_along_axis() {
    let bvh = Bvh::build(&rotated_disc());

    // Ray aimed at the wall surface but 40mm beyond the disc along its
    // axis — must miss.
    let beyond = Ray::new(Point3::new(0.0, 40.0, 100.0), Vec3::new(0.0, 0.0, -1.0));
    assert!(
        bvh.trace_closest(&beyond).is_none(),
        "wall must not extend past the disc height"
    );
    let far_side = Ray::new(Point3::new(0.0, -48.0, 100.0), Vec3::new(0.0, 0.0, -1.0));
    assert!(
        bvh.trace_closest(&far_side).is_none(),
        "wall must not extend past the disc on the -axis side"
    );

    // Ray through the disc itself — must hit the wall at z = 40 (t = 60).
    let at_disc = Ray::new(Point3::new(0.0, -4.0, 100.0), Vec3::new(0.0, 0.0, -1.0));
    let hit = bvh.trace_closest(&at_disc).expect("must hit the disc wall");
    assert!(
        (hit.t - 60.0).abs() < 1e-9,
        "wall front at t=60, got {}",
        hit.t
    );
}

#[test]
fn cpu_unrotated_wall_bounded_along_axis() {
    // Same defect exists unrotated (axis +Z, wall z in [0, 8]).
    let bvh = Bvh::build(&make_cylinder(40.0, 8.0, 32));
    let beyond = Ray::new(Point3::new(0.0, -100.0, 48.0), Vec3::new(0.0, 1.0, 0.0));
    assert!(bvh.trace_closest(&beyond).is_none());
    let at_disc = Ray::new(Point3::new(0.0, -100.0, 4.0), Vec3::new(0.0, 1.0, 0.0));
    assert!(bvh.trace_closest(&at_disc).is_some());
}

#[cfg(feature = "gpu")]
#[test]
fn gpu_scene_packs_wall_v_range() {
    use vcad_kernel_raytrace::gpu::GpuScene;
    // The GPU scene must not upload the wall as an untrimmed (zero-area)
    // loop — it collapses to the 2-vertex v-range form that the shader's
    // trim_count==2 path clamps against.
    let scene = GpuScene::from_brep(&rotated_disc()).expect("scene builds");

    let wall_faces: Vec<_> = scene
        .faces
        .iter()
        .filter(|f| scene.surfaces[f.surface_idx as usize].surface_type == 1)
        .collect();
    assert!(
        !wall_faces.is_empty(),
        "disc must have a cylindrical wall face"
    );

    for face in wall_faces {
        assert_eq!(
            face.trim_count, 2,
            "degenerate wall loop must collapse to a 2-vertex v-range"
        );
        let a = scene.trim_verts[face.trim_start as usize];
        let b = scene.trim_verts[face.trim_start as usize + 1];
        let (v_min, v_max) = (a.y.min(b.y), a.y.max(b.y));
        assert!(
            (v_max - v_min - 8.0).abs() < 1e-4,
            "wall v-range must equal the 8mm height, got [{v_min}, {v_max}]"
        );
    }
}
