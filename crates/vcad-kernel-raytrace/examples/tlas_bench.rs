//! Benchmark: two-level acceleration (TLAS over per-solid BLAS) versus the
//! previous strategy — bake each instance's transform into a cloned
//! `BRepSolid`, build one BVH per instance, and scan them linearly per ray.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p vcad-kernel-raytrace --example tlas_bench
//! ```

use std::sync::Arc;
use std::time::Instant;

use vcad_kernel_math::{Dir3, Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_raytrace::tlas::transform_from_column_major;
use vcad_kernel_raytrace::{Bvh, Ray, Tlas};
use vcad_kernel_topo::Orientation;

const RES: u32 = 256;
const FOV_DEG: f64 = 60.0;

/// World-space bounds as (min, max).
type Bounds = ([f64; 3], [f64; 3]);

/// The pre-TLAS baking path, kept here as the benchmark's baseline.
fn transform_brep(solid: &BRepSolid, matrix: &[f64]) -> BRepSolid {
    let t = transform_from_column_major(matrix).expect("16 elements");
    let mut new_solid = solid.clone();
    for (_id, vertex) in &mut new_solid.topology.vertices {
        vertex.point = t.apply_point(&vertex.point);
    }
    for surface in &mut new_solid.geometry.surfaces {
        *surface = surface.transform(&t);
    }
    let det = matrix[0] * (matrix[5] * matrix[10] - matrix[6] * matrix[9])
        - matrix[1] * (matrix[4] * matrix[10] - matrix[6] * matrix[8])
        + matrix[2] * (matrix[4] * matrix[9] - matrix[5] * matrix[8]);
    if det < 0.0 {
        for (_id, face) in &mut new_solid.topology.faces {
            face.orientation = match face.orientation {
                Orientation::Forward => Orientation::Reversed,
                Orientation::Reversed => Orientation::Forward,
            };
        }
    }
    new_solid
}

/// A `count`-instance cubic grid of one part: the "pattern of N bolts" case.
///
/// Returns the shared part, the flat row-major transforms, and the assembly's
/// world bounds (min, max) so the camera can frame it exactly.
fn pattern(count: usize) -> (Arc<BRepSolid>, Vec<f64>, Bounds) {
    // A *cubic* grid with pitch barely wider than the part, so the assembly
    // stays a dense block: as `count` grows it keeps roughly constant screen
    // coverage rather than receding into background. That matters — a scene
    // that shrinks away would flatter the TLAS by turning most rays into
    // cheap misses.
    const R: f64 = 3.0;
    const H: f64 = 10.0;
    let part = Arc::new(make_cylinder(R, H, 24));
    let side = (count as f64).cbrt().ceil() as usize;
    let (pitch_xy, pitch_z) = (6.5, 11.0);
    let mut transforms = Vec::with_capacity(count * 16);
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for i in 0..count {
        let x = (i % side) as f64 * pitch_xy;
        let y = ((i / side) % side) as f64 * pitch_xy;
        let z = (i / (side * side)) as f64 * pitch_z;
        for (k, (c, half)) in [(x, R), (y, R), (z + H / 2.0, H / 2.0)].iter().enumerate() {
            lo[k] = lo[k].min(c - half);
            hi[k] = hi[k].max(c + half);
        }
        // Column-major, matching the wire format `render_scene` consumes:
        // the translation lives in the last *column*, i.e. indices 12..15.
        transforms.extend_from_slice(&[
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            x, y, z, 1.0,
        ]);
    }
    (part, transforms, (lo, hi))
}

/// Frame the assembly's bounding sphere: aim at its true centre and back off
/// far enough that it fills the frame. Without this the block drifts out of
/// view as `count` grows and the benchmark silently degenerates into measuring
/// background misses.
fn camera_rays(bounds: Bounds) -> (Point3, Dir3, Dir3, Dir3) {
    let (lo, hi) = bounds;
    let center = Point3::new(
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    );
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) / 2.0)
        .fold(0.0f64, |a, b| a + b * b)
        .sqrt()
        .max(1.0);
    // Distance that makes the bounding sphere subtend ~the full frame.
    let dist = radius / (FOV_DEG / 2.0).to_radians().sin() * 1.05;
    let dir = Vec3::new(0.62, -0.66, 0.42).normalize();
    let camera = Point3::new(
        center.x + dir.x * dist,
        center.y + dir.y * dist,
        center.z + dir.z * dist,
    );
    let fwd = center - camera;
    let forward = Dir3::new_normalize(Vec3::new(fwd.x, fwd.y, fwd.z));
    let up = Dir3::new_normalize(Vec3::new(0.0, 0.0, 1.0));
    let right = Dir3::new_normalize(forward.cross(up));
    let cam_up = Dir3::new_normalize(right.cross(forward));
    (camera, forward, right, cam_up)
}

/// Fire one primary ray per pixel; return (checksum, elapsed_ms). The checksum
/// is the summed hit distance, so the two paths can be compared for agreement
/// as well as speed.
fn sweep(bounds: Bounds, mut trace: impl FnMut(&Ray) -> Option<f64>) -> (f64, f64) {
    let (camera, forward, right, cam_up) = camera_rays(bounds);
    let half = (FOV_DEG.to_radians() / 2.0).tan();
    let start = Instant::now();
    let mut checksum = 0.0;
    let mut hits = 0u32;
    for py in 0..RES {
        for px in 0..RES {
            let sx = (2.0 * (px as f64 + 0.5) / RES as f64 - 1.0) * half;
            let sy = (1.0 - 2.0 * (py as f64 + 0.5) / RES as f64) * half;
            let dir = Vec3::new(
                forward.x + sx * right.x + sy * cam_up.x,
                forward.y + sx * right.y + sy * cam_up.y,
                forward.z + sx * right.z + sy * cam_up.z,
            );
            if let Some(t) = trace(&Ray::new(camera, dir)) {
                checksum += t;
                hits += 1;
            }
        }
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let pct = 100.0 * hits as f64 / (RES * RES) as f64;
    println!("   [{hits} hits, {pct:.0}% coverage]");
    (checksum, ms)
}

fn bench(count: usize) {
    let (part, transforms, bounds) = pattern(count);
    println!("\n=== {count} instances, {RES}x{RES} primary rays ===");

    // Baseline: bake the transform into cloned geometry, one BVH each.
    let t0 = Instant::now();
    let baked: Vec<Bvh> = (0..count)
        .map(|i| Bvh::build_brep(&transform_brep(&part, &transforms[i * 16..(i + 1) * 16])))
        .collect();
    let build_linear = t0.elapsed().as_secs_f64() * 1000.0;

    // TLAS: one shared BLAS, N instances.
    let t0 = Instant::now();
    let placed: Vec<(Arc<BRepSolid>, Transform, usize)> = (0..count)
        .map(|i| {
            (
                Arc::clone(&part),
                transform_from_column_major(&transforms[i * 16..(i + 1) * 16]).unwrap(),
                i,
            )
        })
        .collect();
    let tlas = Tlas::from_placed(&placed);
    let build_tlas = t0.elapsed().as_secs_f64() * 1000.0;

    println!(
        "  build: linear {build_linear:8.1} ms   tlas {build_tlas:8.1} ms   ({:.1}x)",
        build_linear / build_tlas
    );

    print!("  closest-hit (linear scan)");
    let (sum_a, ms_a) = sweep(bounds, |ray| {
        let mut best: Option<f64> = None;
        for bvh in &baked {
            if let Some(h) = bvh.trace_closest(ray) {
                if best.is_none_or(|t| h.t < t) {
                    best = Some(h.t);
                }
            }
        }
        best
    });
    print!("  closest-hit (tlas)       ");
    let (sum_b, ms_b) = sweep(bounds, |ray| tlas.trace_closest(ray).map(|h| h.hit.t));

    assert!(
        (sum_a - sum_b).abs() < 1e-6 * sum_a.abs().max(1.0),
        "paths disagree: {sum_a} vs {sum_b}"
    );

    println!(
        "  trace: linear {ms_a:8.1} ms   tlas {ms_b:8.1} ms   ({:.1}x faster)",
        ms_a / ms_b
    );

    // Shadow rays. These are the case any-hit traversal is for, so they must
    // start where real ones do — on hit surfaces, not in mid-air, where every
    // ray would escape and the test would only measure miss cost.
    let origins = shadow_origins(&tlas, bounds);
    let key = Vec3::new(0.45, -0.35, 0.82);
    let shoot = |o: &Point3| Ray::new(*o + Vec3::new(0.0, 0.0, 0.0), key);

    let t0 = Instant::now();
    let blocked_a = origins
        .iter()
        .filter(|o| {
            let r = shoot(o);
            baked.iter().any(|b| b.trace_closest(&r).is_some())
        })
        .count();
    let ms_c = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let blocked_b = origins
        .iter()
        .filter(|o| tlas.occluded(&shoot(o), f64::INFINITY))
        .count();
    let ms_d = t0.elapsed().as_secs_f64() * 1000.0;

    assert_eq!(blocked_a, blocked_b, "shadow paths disagree");
    println!(
        "  shadow ({} rays, {blocked_a} blocked): linear {ms_c:8.1} ms   tlas {ms_d:8.1} ms   ({:.1}x faster)",
        origins.len(),
        ms_c / ms_d
    );
}

/// Primary-hit points, nudged off the surface — the origins a shading pass
/// would fire shadow rays from.
use vcad_kernel_raytrace::{BrepBvh, BrepTlas};
fn shadow_origins(tlas: &Tlas, bounds: Bounds) -> Vec<Point3> {
    let (camera, forward, right, cam_up) = camera_rays(bounds);
    let half = (FOV_DEG.to_radians() / 2.0).tan();
    let mut out = Vec::new();
    // Every 4th pixel: enough origins to time, without dominating the run.
    for py in (0..RES).step_by(4) {
        for px in (0..RES).step_by(4) {
            let sx = (2.0 * (px as f64 + 0.5) / RES as f64 - 1.0) * half;
            let sy = (1.0 - 2.0 * (py as f64 + 0.5) / RES as f64) * half;
            let dir = Vec3::new(
                forward.x + sx * right.x + sy * cam_up.x,
                forward.y + sx * right.y + sy * cam_up.y,
                forward.z + sx * right.z + sy * cam_up.z,
            );
            if let Some(h) = tlas.trace_closest(&Ray::new(camera, dir)) {
                out.push(h.hit.point + h.hit.normal.into_inner() * 1e-4);
            }
        }
    }
    out
}

fn main() {
    for count in [1, 8, 27, 64, 125, 216] {
        bench(count);
    }
}
