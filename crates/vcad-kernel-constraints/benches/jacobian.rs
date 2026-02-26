//! Criterion benchmarks for vcad-kernel-constraints.
//!
//! Compares finite-difference vs symbolic Jacobian computation,
//! measures build time, evaluation throughput, full solver, and
//! reports sparsity ratios at different sketch sizes.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use vcad_kernel_constraints::jacobian::{compute_all_residuals, compute_jacobian};
use vcad_kernel_constraints::symbolic::CompiledSystem;
use vcad_kernel_constraints::{Constraint, EntityRef, Sketch2D};

// =============================================================================
// Sketch builders
// =============================================================================

/// Small sketch: 4 points, 4 lines forming a rectangle with constraints.
/// 8 params, ~8 constraint equations.
fn make_small_sketch() -> Sketch2D {
    let mut s = Sketch2D::new();

    let p0 = s.add_point(0.1, -0.2);
    let p1 = s.add_point(10.3, 0.5);
    let p2 = s.add_point(9.8, 5.3);
    let p3 = s.add_point(0.4, 4.8);

    let l0 = s.add_line(p0, p1);
    let l1 = s.add_line(p1, p2);
    let l2 = s.add_line(p2, p3);
    let l3 = s.add_line(p3, p0);

    s.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
    s.constrain_horizontal(l0);
    s.constrain_horizontal(l2);
    s.constrain_vertical(l1);
    s.constrain_vertical(l3);
    s.constrain_length(l0, 10.0);
    s.constrain_length(l1, 5.0);

    s
}

/// Medium sketch: ~10 points, ~15 constraints, ~20 params.
/// Two connected rectangles sharing an edge (L-shape).
fn make_medium_sketch() -> Sketch2D {
    let mut s = Sketch2D::new();

    // First rectangle: p0-p1-p2-p3 (10x5)
    let p0 = s.add_point(0.1, -0.1);
    let p1 = s.add_point(10.2, 0.3);
    let p2 = s.add_point(9.9, 5.2);
    let p3 = s.add_point(0.3, 4.9);

    let l0 = s.add_line(p0, p1);
    let l1 = s.add_line(p1, p2);
    let l2 = s.add_line(p2, p3);
    let l3 = s.add_line(p3, p0);

    // Second rectangle: p4-p5-p6-p7 (8x5) sharing edge with first
    let p4 = s.add_point(10.1, 0.2);
    let p5 = s.add_point(18.3, -0.1);
    let p6 = s.add_point(17.9, 5.1);
    let p7 = s.add_point(10.2, 4.8);

    let l4 = s.add_line(p4, p5);
    let l5 = s.add_line(p5, p6);
    let l6 = s.add_line(p6, p7);
    let l7 = s.add_line(p7, p4);

    // Extra diagonal brace
    let p8 = s.add_point(5.1, 2.4);
    let p9 = s.add_point(14.2, 2.6);
    let _ld = s.add_line(p8, p9);

    // Constraints for first rectangle
    s.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
    s.constrain_horizontal(l0);
    s.constrain_horizontal(l2);
    s.constrain_vertical(l1);
    s.constrain_vertical(l3);
    s.constrain_length(l0, 10.0);
    s.constrain_length(l1, 5.0);

    // Constraints for second rectangle
    s.constrain_horizontal(l4);
    s.constrain_horizontal(l6);
    s.constrain_vertical(l5);
    s.constrain_vertical(l7);
    s.constrain_length(l4, 8.0);
    s.constrain_length(l5, 5.0);

    // Shared edge: p1==p4, p2==p7
    s.constrain_coincident(EntityRef::Point(p1), EntityRef::Point(p4));
    s.constrain_coincident(EntityRef::Point(p2), EntityRef::Point(p7));

    s
}

/// Large sketch: 25+ points, ~40 constraints, ~50+ params.
/// A grid of connected rectangles with distance and angle constraints.
fn make_large_sketch() -> Sketch2D {
    let mut s = Sketch2D::new();

    // 5x5 grid of points = 25 points = 50 params
    let rows = 5;
    let cols = 5;
    let mut pts = Vec::new();
    for r in 0..rows {
        let mut row = Vec::new();
        for c in 0..cols {
            // Perturb slightly from ideal grid
            let x = c as f64 * 10.0 + (r as f64 * 0.3) + 0.1;
            let y = r as f64 * 8.0 + (c as f64 * 0.2) - 0.1;
            row.push(s.add_point(x, y));
        }
        pts.push(row);
    }

    // Create horizontal lines and constrain them
    let mut h_lines = Vec::new();
    for r in 0..rows {
        for c in 0..(cols - 1) {
            let l = s.add_line(pts[r][c], pts[r][c + 1]);
            h_lines.push(l);
            s.constrain_horizontal(l);
            s.constrain_length(l, 10.0);
        }
    }

    // Create vertical lines and constrain them
    let mut v_lines = Vec::new();
    for r in 0..(rows - 1) {
        for c in 0..cols {
            let l = s.add_line(pts[r][c], pts[r + 1][c]);
            v_lines.push(l);
            s.constrain_vertical(l);
            s.constrain_length(l, 8.0);
        }
    }

    // Fix corner
    s.constrain_fixed(EntityRef::Point(pts[0][0]), 0.0, 0.0);

    // Add some equal-length constraints on diagonals
    for r in 0..(rows - 1) {
        let d = s.add_line(pts[r][0], pts[r + 1][1]);
        if r > 0 {
            let prev_d = s.add_line(pts[r - 1][0], pts[r][1]);
            s.constrain_equal_length(d, prev_d);
        }
    }

    s
}

// =============================================================================
// Benchmark: Jacobian computation — FD vs symbolic (single evaluation)
// =============================================================================

fn bench_jacobian_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("jacobian_computation");

    let sketches: Vec<(&str, Sketch2D)> = vec![
        ("small_8p", make_small_sketch()),
        ("medium_20p", make_medium_sketch()),
        ("large_50p", make_large_sketch()),
    ];

    for (name, sketch) in &sketches {
        let params = &sketch.parameters;
        let constraints = &sketch.constraints;
        let entities = &sketch.entities;
        let num_params = params.len();

        // Finite-difference Jacobian
        group.bench_with_input(BenchmarkId::new("fd", name), &(), |bencher, _| {
            bencher.iter(|| {
                compute_jacobian(
                    black_box(constraints),
                    black_box(params),
                    black_box(entities),
                )
            })
        });

        // Symbolic Jacobian (pre-built, evaluation only)
        let system = CompiledSystem::build(constraints, entities, num_params);
        group.bench_with_input(
            BenchmarkId::new("symbolic_dense", name),
            &(),
            |bencher, _| bencher.iter(|| system.eval_jacobian(black_box(params))),
        );

        // Symbolic sparse Jacobian (raw sparse values)
        group.bench_with_input(
            BenchmarkId::new("symbolic_sparse", name),
            &(),
            |bencher, _| bencher.iter(|| system.eval_jacobian_sparse(black_box(params))),
        );
    }

    group.finish();
}

// =============================================================================
// Benchmark: CompiledSystem::build() time (one-shot cost)
// =============================================================================

fn bench_build_time(c: &mut Criterion) {
    let mut group = c.benchmark_group("compiled_system_build");

    let sketches: Vec<(&str, Sketch2D)> = vec![
        ("small_8p", make_small_sketch()),
        ("medium_20p", make_medium_sketch()),
        ("large_50p", make_large_sketch()),
    ];

    for (name, sketch) in &sketches {
        let constraints = &sketch.constraints;
        let entities = &sketch.entities;
        let num_params = sketch.parameters.len();

        group.bench_with_input(BenchmarkId::new("build", name), &(), |bencher, _| {
            bencher.iter(|| {
                CompiledSystem::build(
                    black_box(constraints),
                    black_box(entities),
                    black_box(num_params),
                )
            })
        });
    }

    group.finish();
}

// =============================================================================
// Benchmark: Evaluation — residuals, dense Jacobian, sparse JtJ+Jtr
// =============================================================================

fn bench_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("evaluation");

    let sketches: Vec<(&str, Sketch2D)> = vec![
        ("small_8p", make_small_sketch()),
        ("medium_20p", make_medium_sketch()),
        ("large_50p", make_large_sketch()),
    ];

    for (name, sketch) in &sketches {
        let params = &sketch.parameters;
        let constraints = &sketch.constraints;
        let entities = &sketch.entities;
        let num_params = params.len();
        let system = CompiledSystem::build(constraints, entities, num_params);

        // Residual evaluation — FD reference
        group.bench_with_input(BenchmarkId::new("residuals_fd", name), &(), |bencher, _| {
            bencher.iter(|| {
                compute_all_residuals(
                    black_box(constraints),
                    black_box(params),
                    black_box(entities),
                )
            })
        });

        // Residual evaluation — symbolic
        group.bench_with_input(
            BenchmarkId::new("residuals_symbolic", name),
            &(),
            |bencher, _| bencher.iter(|| system.eval_residuals(black_box(params))),
        );

        // Dense Jacobian evaluation (symbolic)
        group.bench_with_input(
            BenchmarkId::new("jacobian_dense", name),
            &(),
            |bencher, _| bencher.iter(|| system.eval_jacobian(black_box(params))),
        );

        // Sparse JtJ + Jtr evaluation (used by the solver's inner loop)
        group.bench_with_input(
            BenchmarkId::new("jtj_jtr_sparse", name),
            &(),
            |bencher, _| bencher.iter(|| system.eval_jtj_jtr(black_box(params))),
        );
    }

    group.finish();
}

// =============================================================================
// Benchmark: Full solver — solve_default() end-to-end
// =============================================================================

fn bench_full_solver(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_solver");

    // Rectangle (small)
    group.bench_function("rectangle_8p", |bencher| {
        bencher.iter_batched(
            make_small_sketch,
            |mut sketch| {
                let result = sketch.solve_default();
                black_box(result)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // L-shape (medium)
    group.bench_function("l_shape_20p", |bencher| {
        bencher.iter_batched(
            make_medium_sketch,
            |mut sketch| {
                let result = sketch.solve_default();
                black_box(result)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Grid (large)
    group.bench_function("grid_50p", |bencher| {
        bencher.iter_batched(
            make_large_sketch,
            |mut sketch| {
                let result = sketch.solve_default();
                black_box(result)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

// =============================================================================
// Benchmark: Sparsity ratio report (prints stats, not timed)
// =============================================================================

fn bench_sparsity_report(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparsity_info");

    let sketches: Vec<(&str, Sketch2D)> = vec![
        ("small_8p", make_small_sketch()),
        ("medium_20p", make_medium_sketch()),
        ("large_50p", make_large_sketch()),
    ];

    for (name, sketch) in &sketches {
        let constraints = &sketch.constraints;
        let entities = &sketch.entities;
        let num_params = sketch.parameters.len();

        // We use a benchmark that runs once just to capture the numbers;
        // the real value is in the printed output.
        group.bench_with_input(
            BenchmarkId::new("build_and_report", name),
            &(),
            |bencher, _| {
                bencher.iter(|| {
                    let sys = CompiledSystem::build(constraints, entities, num_params);
                    black_box((sys.sparsity_ratio(), sys.num_nonzero, sys.dense_size))
                })
            },
        );

        // Print once for the report
        let sys = CompiledSystem::build(constraints, entities, num_params);
        eprintln!(
            "[sparsity] {}: nonzero={}/{} ratio={:.2}% (params={}, residuals={})",
            name,
            sys.num_nonzero,
            sys.dense_size,
            sys.sparsity_ratio() * 100.0,
            sys.num_params,
            sys.num_residuals,
        );
    }

    group.finish();
}

// =============================================================================
// Criterion configuration
// =============================================================================

criterion_group!(
    benches,
    bench_jacobian_computation,
    bench_build_time,
    bench_evaluation,
    bench_full_solver,
    bench_sparsity_report,
);

criterion_main!(benches);
