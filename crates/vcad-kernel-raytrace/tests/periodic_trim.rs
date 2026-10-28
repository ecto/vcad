//! Regression: a seamless cylinder wall whose circles are subdivided and
//! start away from the surface's `u = 0` (how Onshape writes STEP).
//!
//! Each ring crosses the projection's wrap mid-loop and winds a whole turn
//! without closing in UV; before the periodic trim, every hit on such a wall
//! was rejected and `--exact` renders lost the RS00's curved faces.

use vcad_kernel_geom::{CylinderSurface, GeometryStore};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_raytrace::{BrepBvh, Bvh, Ray};
use vcad_kernel_topo::{Orientation, ShellType, Topology};

const R: f64 = 5.0;
const H: f64 = 8.0;

fn seamless_wall() -> BRepSolid {
    let n = 24;
    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();
    let cyl = geom.add_surface(Box::new(CylinderSurface::new(R)));
    let mut ring = |z: f64, start: f64, dir: f64| {
        let hes: Vec<_> = (0..n)
            .map(|i| {
                let t = start + dir * std::f64::consts::TAU * i as f64 / n as f64;
                let v = topo.add_vertex(Point3::new(R * t.cos(), R * t.sin(), z));
                topo.add_half_edge(v)
            })
            .collect();
        topo.add_loop(&hes)
    };
    let outer = ring(0.0, 1.0, 1.0);
    let inner = ring(H, 2.5, -1.0);
    let face = topo.add_face(outer, cyl, Orientation::Forward);
    topo.add_inner_loop(face, inner);
    let shell = topo.add_shell(vec![face], ShellType::Outer);
    let solid_id = topo.add_solid(shell);
    BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    }
}

#[test]
fn cpu_hits_wall_all_the_way_round() {
    let bvh = Bvh::build_brep(&seamless_wall());
    for k in 0..12 {
        let a = std::f64::consts::TAU * k as f64 / 12.0;
        let (c, s) = (a.cos(), a.sin());
        let origin = Point3::new(30.0 * c, 30.0 * s, H / 2.0);
        let hit = bvh
            .trace_closest(&Ray::new(origin, Vec3::new(-c, -s, 0.0)))
            .unwrap_or_else(|| panic!("wall missed at {a:.2} rad"));
        assert!((hit.t - (30.0 - R)).abs() < 1e-6, "t={}", hit.t);
        let above = Point3::new(30.0 * c, 30.0 * s, H + 1.0);
        assert!(bvh
            .trace_closest(&Ray::new(above, Vec3::new(-c, -s, 0.0)))
            .is_none());
    }
}

#[cfg(feature = "gpu")]
#[test]
fn gpu_scene_uploads_unwrapped_strip() {
    use vcad_kernel_raytrace::gpu::{GpuScene, TRIM_PERIODIC_U, TRIM_WHOLE};
    let scene = GpuScene::from_brep(&seamless_wall()).expect("scene builds");
    let face = scene.faces[0];
    assert_eq!(
        face.trim_flags & (TRIM_PERIODIC_U | TRIM_WHOLE),
        TRIM_PERIODIC_U
    );
    let verts =
        &scene.trim_verts[face.trim_start as usize..(face.trim_start + face.trim_count) as usize];
    let (lo, hi) = verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
        (lo.min(v.x), hi.max(v.x))
    });
    assert!(
        hi - lo >= std::f32::consts::TAU - 1e-3,
        "strip spans a full turn"
    );
}
