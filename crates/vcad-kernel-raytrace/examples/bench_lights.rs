//! Timing harness for next-event estimation cost vs light count.
use std::sync::Arc;
use std::time::Instant;
use vcad_kernel_primitives::make_cube;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::pathtrace::render;
use vcad_kernel_raytrace::*;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let cube = make_cube(10.0, 10.0, 10.0);
    let center = Point3::new(5.0, 5.0, 5.0);
    let mut lights = Vec::new();
    for i in 0..n {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        // A dome of downward-facing panels: every one of them is above every
        // shading point, so none is culled by the cos-at-light test and the
        // old estimator really does fire one shadow ray per panel per bounce.
        let c = Point3::new(center.x + a.cos() * 26.0, center.y + a.sin() * 26.0, 40.0);
        lights.push(AreaLight {
            center: c,
            u: Vec3::new(6.0, 0.0, 0.0),
            v: Vec3::new(0.0, -6.0, 0.0),
            emission: [4.0 + i as f32 * 0.3; 3],
        });
    }
    let scene = Scene {
        objects: {
            // Enough occluders that a shadow ray is a real traversal, which
            // is the cost this benchmark is about.
            let blas = Arc::new(Bvh::build(&cube));
            let mut v = Vec::new();
            for i in 0..6 {
                for j in 0..6 {
                    let mut o = Object::new(Arc::clone(&blas), Pbr::plastic([0.8, 0.3, 0.2], 0.35, 0.0));
                    o.transform = vcad_kernel_math::Transform::translation(
                        i as f64 * 14.0 - 35.0,
                        j as f64 * 14.0 - 35.0,
                        0.0,
                    );
                    v.push(o);
                }
            }
            v
        },
        lights,
        env: Environment::default(),
        ground: Some(Ground { z: 0.0, material: Pbr::plastic([0.5, 0.5, 0.55], 0.5, 0.0), shadow_catcher: false }),
    };
    let cam = Camera::look_at(Point3::new(30.0, -34.0, 24.0), center, Vec3::new(0.0, 0.0, 1.0), 32.0);
    let opts = PathTraceOptions { spp: 128, max_depth: 2, rr_start: 99, firefly_clamp: None, denoise: false, ..Default::default() };
    let _ = render(&scene, &cam, 64, 64, &opts);
    let mut best = f64::INFINITY;
    let mut sum = 0.0f32;
    for _ in 0..5 {
        let t = Instant::now();
        let f = render(&scene, &cam, 256, 256, &opts);
        best = best.min(t.elapsed().as_secs_f64());
        sum = f.rgb.iter().sum();
    }
    println!("{n} lights: best {best:.3}s (checksum {sum:.1})");
}
