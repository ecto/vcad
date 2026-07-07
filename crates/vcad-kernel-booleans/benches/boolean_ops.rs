//! Criterion benchmarks for vcad-kernel-booleans.
//!
//! Measures performance of:
//! - Micro-benchmarks: individual pipeline stages (AABB, SSI, trim, classify)
//! - Macro-benchmarks: full boolean operations on realistic geometry
//! - Scaling benchmarks: performance vs. tessellation resolution

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use vcad_kernel_booleans::{bbox, classify, point_in_mesh, ssi, trim, BooleanOp, BooleanResult};

/// Bench wrapper: unwrap the now-fallible `boolean_op` — bench geometry
/// never hits an SSI error.
fn boolean_op(a: &BRepSolid, b: &BRepSolid, op: BooleanOp, segments: u32) -> BooleanResult {
    vcad_kernel_booleans::boolean_op(a, b, op, segments).expect("boolean_op should succeed")
}
use vcad_kernel_geom::{CylinderSurface, Line3d, Plane};
use vcad_kernel_math::predicates::{incircle, insphere, orient2d, orient3d};
use vcad_kernel_math::{Point2, Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

// =============================================================================
// Test geometry helpers
// =============================================================================

/// Translate a BRepSolid by a given offset.
fn translate_brep(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
    let t = Transform::translation(dx, dy, dz);
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(&t))
        .collect();
}

/// Create a plate with a centered through-hole.
fn make_plate_with_hole(
    plate_x: f64,
    plate_y: f64,
    plate_z: f64,
    hole_diameter: f64,
    segments: u32,
) -> (BRepSolid, BRepSolid) {
    let plate = make_cube(plate_x, plate_y, plate_z);
    let mut hole = make_cylinder(hole_diameter / 2.0, plate_y + 10.0, segments);
    // Center hole in plate
    translate_brep(&mut hole, plate_x / 2.0, -5.0, plate_z / 2.0);
    (plate, hole)
}

/// Create two disjoint cubes (no overlap).
fn make_disjoint_cubes(size: f64) -> (BRepSolid, BRepSolid) {
    let a = make_cube(size, size, size);
    let mut b = make_cube(size, size, size);
    translate_brep(&mut b, size * 2.0, 0.0, 0.0);
    (a, b)
}

/// Create two overlapping cubes.
fn make_overlapping_cubes(size: f64) -> (BRepSolid, BRepSolid) {
    let a = make_cube(size, size, size);
    let mut b = make_cube(size, size, size);
    translate_brep(&mut b, size / 2.0, size / 2.0, size / 2.0);
    (a, b)
}

// =============================================================================
// Predicate micro-benchmarks
// =============================================================================

fn bench_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("predicates");

    // 2D predicates
    let a2 = Point2::new(0.0, 0.0);
    let b2 = Point2::new(1.0, 0.0);
    let c2 = Point2::new(0.5, 1.0);
    let d2 = Point2::new(0.5, 0.3);

    group.bench_function("orient2d", |bencher| {
        bencher.iter(|| orient2d(black_box(&a2), black_box(&b2), black_box(&c2)))
    });

    group.bench_function("incircle", |bencher| {
        bencher.iter(|| {
            incircle(
                black_box(&a2),
                black_box(&b2),
                black_box(&c2),
                black_box(&d2),
            )
        })
    });

    // 3D predicates
    let a3 = Point3::new(0.0, 0.0, 0.0);
    let b3 = Point3::new(1.0, 0.0, 0.0);
    let c3 = Point3::new(0.0, 1.0, 0.0);
    let d3 = Point3::new(0.0, 0.0, 1.0);
    let e3 = Point3::new(0.3, 0.3, 0.3);

    group.bench_function("orient3d", |bencher| {
        bencher.iter(|| {
            orient3d(
                black_box(&a3),
                black_box(&b3),
                black_box(&c3),
                black_box(&d3),
            )
        })
    });

    group.bench_function("insphere", |bencher| {
        bencher.iter(|| {
            insphere(
                black_box(&a3),
                black_box(&b3),
                black_box(&c3),
                black_box(&d3),
                black_box(&e3),
            )
        })
    });

    // Near-degenerate cases (where exact arithmetic matters)
    let near_collinear_c = Point2::new(0.5, 1e-15);
    group.bench_function("orient2d_near_degenerate", |bencher| {
        bencher.iter(|| orient2d(black_box(&a2), black_box(&b2), black_box(&near_collinear_c)))
    });

    let near_coplanar_d = Point3::new(0.5, 0.5, 1e-15);
    group.bench_function("orient3d_near_degenerate", |bencher| {
        bencher.iter(|| {
            orient3d(
                black_box(&a3),
                black_box(&b3),
                black_box(&c3),
                black_box(&near_coplanar_d),
            )
        })
    });

    group.finish();
}

// =============================================================================
// Micro-benchmarks: individual pipeline stages
// =============================================================================

fn bench_aabb_overlap(c: &mut Criterion) {
    let mut group = c.benchmark_group("aabb");

    // Simple overlap check
    let (a, b) = make_overlapping_cubes(20.0);
    let aabb_a = bbox::solid_aabb(&a);
    let aabb_b = bbox::solid_aabb(&b);

    group.bench_function("solid_aabb", |bencher| {
        bencher.iter(|| bbox::solid_aabb(black_box(&a)))
    });

    group.bench_function("overlaps", |bencher| {
        bencher.iter(|| black_box(&aabb_a).overlaps(black_box(&aabb_b)))
    });

    group.bench_function("find_candidate_pairs", |bencher| {
        bencher.iter(|| bbox::find_candidate_face_pairs(black_box(&a), black_box(&b)))
    });

    group.finish();
}

fn bench_ssi(c: &mut Criterion) {
    let mut group = c.benchmark_group("ssi");

    // Plane-plane intersection (XY plane vs XZ plane)
    let plane_a = Plane::xy();
    let plane_b = Plane::xz();

    group.bench_function("plane_plane", |bencher| {
        bencher.iter(|| ssi::intersect_surfaces(black_box(&plane_a), black_box(&plane_b)))
    });

    // Plane-cylinder intersection (produces a circle)
    // Horizontal plane at z=5, cylinder along Z axis with radius 5
    let plane_top = Plane::new(Point3::new(0.0, 0.0, 5.0), Vec3::x(), Vec3::y());
    let cylinder = CylinderSurface::new(5.0);

    group.bench_function("plane_cylinder", |bencher| {
        bencher.iter(|| ssi::intersect_surfaces(black_box(&plane_top), black_box(&cylinder)))
    });

    group.finish();
}

fn bench_trim_curve_to_face(c: &mut Criterion) {
    let mut group = c.benchmark_group("trim");

    // Create cube and get a face for trimming
    let cube = make_cube(20.0, 20.0, 20.0);
    let face_id = cube.topology.faces.keys().next().unwrap();

    // Create a line that crosses the face
    let line = ssi::IntersectionCurve::Line(Line3d {
        origin: Point3::new(10.0, -5.0, 10.0),
        direction: Vec3::new(0.0, 1.0, 0.0),
    });

    for samples in [16, 32, 64, 128] {
        group.bench_with_input(
            BenchmarkId::new("trim_line", samples),
            &samples,
            |bencher, &samples| {
                bencher.iter(|| {
                    trim::trim_curve_to_face(
                        black_box(&line),
                        black_box(face_id),
                        black_box(&cube),
                        samples,
                    )
                })
            },
        );
    }

    group.finish();
}

fn bench_point_in_mesh(c: &mut Criterion) {
    let mut group = c.benchmark_group("point_in_mesh");

    let cube = make_cube(20.0, 20.0, 20.0);
    let mesh = tessellate_brep(&cube, 32);

    let point_inside = Point3::new(10.0, 10.0, 10.0);
    let point_outside = Point3::new(30.0, 10.0, 10.0);

    group.bench_function("inside", |bencher| {
        bencher.iter(|| point_in_mesh(black_box(&point_inside), black_box(&mesh)))
    });

    group.bench_function("outside", |bencher| {
        bencher.iter(|| point_in_mesh(black_box(&point_outside), black_box(&mesh)))
    });

    group.finish();
}

fn bench_classify_faces(c: &mut Criterion) {
    let mut group = c.benchmark_group("classify");

    let (a, b) = make_overlapping_cubes(20.0);

    group.bench_function("classify_all_faces", |bencher| {
        bencher.iter(|| classify::classify_all_faces(black_box(&a), black_box(&b), 32))
    });

    group.finish();
}

// =============================================================================
// Macro-benchmarks: full boolean operations
// =============================================================================

fn bench_boolean_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("boolean_full");

    // Disjoint cubes (fast path)
    let (a_disj, b_disj) = make_disjoint_cubes(20.0);

    group.bench_function("union_disjoint", |bencher| {
        bencher.iter(|| boolean_op(black_box(&a_disj), black_box(&b_disj), BooleanOp::Union, 32))
    });

    // Overlapping cubes
    let (a_over, b_over) = make_overlapping_cubes(20.0);

    group.bench_function("union_overlapping", |bencher| {
        bencher.iter(|| boolean_op(black_box(&a_over), black_box(&b_over), BooleanOp::Union, 32))
    });

    group.bench_function("difference_overlapping", |bencher| {
        bencher.iter(|| {
            boolean_op(
                black_box(&a_over),
                black_box(&b_over),
                BooleanOp::Difference,
                32,
            )
        })
    });

    group.bench_function("intersection_overlapping", |bencher| {
        bencher.iter(|| {
            boolean_op(
                black_box(&a_over),
                black_box(&b_over),
                BooleanOp::Intersection,
                32,
            )
        })
    });

    // Cube minus cylinder (realistic: hole in plate)
    let (plate, hole) = make_plate_with_hole(80.0, 6.0, 60.0, 10.0, 32);

    group.bench_function("cube_minus_cylinder", |bencher| {
        bencher.iter(|| {
            boolean_op(
                black_box(&plate),
                black_box(&hole),
                BooleanOp::Difference,
                32,
            )
        })
    });

    group.finish();
}

// =============================================================================
// Scaling benchmarks: performance vs. complexity
// =============================================================================

fn bench_cylinder_segments(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling_cylinder_segments");

    let plate = make_cube(80.0, 6.0, 60.0);

    for segments in [8, 16, 32, 64] {
        let mut hole = make_cylinder(5.0, 16.0, segments);
        translate_brep(&mut hole, 40.0, -5.0, 30.0);

        group.bench_with_input(
            BenchmarkId::new("difference", segments),
            &segments,
            |bencher, &seg| {
                bencher.iter(|| {
                    boolean_op(
                        black_box(&plate),
                        black_box(&hole),
                        BooleanOp::Difference,
                        seg,
                    )
                })
            },
        );
    }

    group.finish();
}

fn bench_multi_hole_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling_hole_count");
    group.sample_size(20); // Fewer samples for expensive benchmarks

    let plate = make_cube(100.0, 6.0, 100.0);

    for hole_count in [1usize, 2, 4] {
        group.bench_with_input(
            BenchmarkId::new("difference", hole_count),
            &hole_count,
            |bencher, &count| {
                bencher.iter(|| {
                    let mut result = plate.clone();
                    let spacing = 80.0 / (count as f64).sqrt();
                    let rows = (count as f64).sqrt().ceil() as usize;
                    let cols = count.div_ceil(rows);

                    for i in 0..rows {
                        for j in 0..cols {
                            if i * cols + j >= count {
                                break;
                            }
                            let mut hole = make_cylinder(3.0, 16.0, 16);
                            translate_brep(
                                &mut hole,
                                10.0 + (j as f64) * spacing,
                                -5.0,
                                10.0 + (i as f64) * spacing,
                            );
                            let vcad_kernel_booleans::BooleanResult::BRep(brep) =
                                boolean_op(&result, &hole, BooleanOp::Difference, 32);
                            result = *brep;
                        }
                    }
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

// =============================================================================
// Criterion configuration
// =============================================================================

criterion_group!(
    benches,
    bench_predicates,
    bench_aabb_overlap,
    bench_ssi,
    bench_trim_curve_to_face,
    bench_point_in_mesh,
    bench_classify_faces,
    bench_boolean_ops,
    bench_cylinder_segments,
    bench_multi_hole_count,
);

criterion_main!(benches);
