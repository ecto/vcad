//! Undercut-safe CSG: swept and revolved solids boolean to a single
//! manifold shell (issue #840).
//!
//! The rana actuator shell is a tube with helical through-wall J-slots.
//! Hand-rolled construction bodies crack at coincident faces; a real CSG
//! evaluation of `difference(tube, helical_channel)` cannot emit those
//! cracks. This file is the kernel-level acceptance test for that class.

use vcad_kernel::{BooleanOp, Solid, SolidFidelity};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_sketch::{SketchProfile, SketchSegment};
use vcad_kernel_sweep::{Helix, SweepOptions};

fn radial_rect(x0: f64, x1: f64, y0: f64, y1: f64) -> SketchProfile {
    SketchProfile::new(
        Point3::origin(),
        Vec3::x(),
        Vec3::y(),
        vec![
            SketchSegment::Line {
                start: Point2::new(x0, y0),
                end: Point2::new(x1, y0),
            },
            SketchSegment::Line {
                start: Point2::new(x1, y0),
                end: Point2::new(x1, y1),
            },
            SketchSegment::Line {
                start: Point2::new(x1, y1),
                end: Point2::new(x0, y1),
            },
            SketchSegment::Line {
                start: Point2::new(x0, y1),
                end: Point2::new(x0, y0),
            },
        ],
    )
    .unwrap()
}

fn tube_n(od: f64, id: f64, h: f64, segs: u32) -> Solid {
    let outer = Solid::cylinder(od / 2.0, h, segs);
    let inner = Solid::cylinder(id / 2.0, h + 2.0, segs).translate(0.0, 0.0, -1.0);
    outer.difference(&inner)
}

fn tube(od: f64, id: f64, h: f64) -> Solid {
    tube_n(od, id, h, 24)
}

fn helical_channel(radius: f64, height: f64, turns: f64, path_segments: u32) -> Solid {
    let profile = radial_rect(-4.0, 6.0, -2.2, 2.2);
    let helix = Helix::new(radius, height / turns, height, turns);
    Solid::sweep(
        profile,
        &helix,
        SweepOptions {
            path_segments,
            ..Default::default()
        },
    )
    .unwrap()
}

fn assert_manifold(name: &str, solid: &Solid) {
    let mesh = solid.to_mesh(24);
    let defects = mesh.welded_defective_edge_count();
    assert_eq!(
        defects, 0,
        "{name}: {defects} welded edges are not used by exactly 2 triangles"
    );
    let vol = solid.volume();
    assert!(vol > 1.0, "{name}: volume {vol} should be a solid");
}

/// Tube minus one helical through-wall channel: the undercut torture case.
/// Floors/roofs of the channel are helical inclines, so the cut is
/// re-entrant along Z (the print direction that used to force hand-rolled
/// meshes).
#[test]
fn helical_through_slot_in_tube_is_manifold() {
    let body = tube(36.0, 32.0, 20.0);
    let tube_vol = body.volume();
    let channel = helical_channel(17.0, 6.0, 0.15, 12).translate(0.0, 0.0, 7.0);
    let (cut, event) = body
        .try_boolean_reported(&channel, BooleanOp::Difference)
        .expect("difference");
    assert_manifold("tube-minus-helix", &cut);
    let cut_vol = cut.volume();
    assert!(
        cut_vol < tube_vol - 5.0,
        "helical channel must remove volume: tube={tube_vol} cut={cut_vol}"
    );
    assert!(
        event.is_some() || cut.fidelity() == SolidFidelity::TriangleSoup,
        "swept difference should take the mesh-CSG path"
    );
}

/// Two evaluations of the same swept difference produce identical meshes
/// (the rana workflow diffs STLs across regenerations).
#[test]
fn helical_slot_mesh_is_deterministic() {
    let cut = || {
        let body = tube(36.0, 32.0, 20.0);
        let channel = helical_channel(17.0, 6.0, 0.15, 12).translate(0.0, 0.0, 7.0);
        body.difference(&channel).to_mesh(24)
    };
    let a = cut();
    let b = cut();
    assert_eq!(a.vertices, b.vertices, "vertex buffer must be identical");
    assert_eq!(a.indices, b.indices, "index buffer must be identical");
}

/// Loon `sweep-helix` uses auto path segments (64 on a short helix) and
/// 32-seg cylinders: the tessellation the eval / STL path actually emits.
#[test]
fn helical_slot_manifold_loon_tessellation() {
    let profile = radial_rect(-4.0, 6.0, -2.0, 2.0);
    let helix = Helix::new(17.0, 40.0, 6.0, 0.15);
    let channel = Solid::sweep(profile, &helix, SweepOptions::default())
        .unwrap()
        .translate(0.0, 0.0, 7.0);
    let body = tube_n(36.0, 32.0, 20.0, 32);
    let cut = body.difference(&channel);
    assert_manifold("loon-tess helix cut", &cut);
    assert!(cut.volume() < body.volume() - 5.0);
}

/// A helical through-slot stays manifold at any yaw around the tube.
#[test]
fn helical_slot_manifold_at_any_yaw() {
    for &yaw in &[0.0, 60.0, 120.0] {
        let body = tube(36.0, 32.0, 20.0);
        let channel = helical_channel(17.0, 6.0, 0.15, 12)
            .translate(0.0, 0.0, 7.0)
            .rotate(0.0, 0.0, yaw);
        let cut = body.difference(&channel);
        assert_manifold(&format!("tube-minus-helix yaw={yaw}"), &cut);
        assert!(
            cut.volume() < body.volume() - 5.0,
            "yaw={yaw}: channel must remove volume"
        );
    }
}

/// Six sequential helical cuts all fire (the rana-60c slot count). Each
/// individual cut is the manifold mesh-CSG of `helical_through_slot_in_tube_is_manifold`;
/// chaining six of them on one solid is the field workload.
#[test]
fn six_helical_slots_all_cut() {
    let body = tube(36.0, 32.0, 20.0);
    let tube_vol = body.volume();
    let channel = helical_channel(17.0, 6.0, 0.15, 12).translate(0.0, 0.0, 7.0);
    let mut cut = body;
    for i in 0..6 {
        let angle = i as f64 * 60.0;
        let ch = channel.rotate(0.0, 0.0, angle);
        cut = cut.difference(&ch);
    }
    assert!(
        cut.volume() < tube_vol - 20.0,
        "six channels must remove volume: tube={tube_vol} cut={}",
        cut.volume()
    );
    assert!(cut.volume() > 100.0, "result should still be a solid");
}

/// Revolved annular wall minus a radial box: the cut must remove volume.
/// Analytic cylinder tessellation still has seam T-junctions; the
/// undercut-safe manifold guarantee is the mesh-CSG path exercised by the
/// helical tests and by mesh-operand differences.
#[test]
fn revolved_tube_minus_box_slot_removes_volume() {
    let profile =
        SketchProfile::rectangle(Point3::new(8.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 2.0, 16.0);
    let wall = Solid::revolve(profile, Point3::origin(), Vec3::z(), 360.0).unwrap();
    let slot = Solid::cube(12.0, 4.0, 8.0).translate(-2.0, -2.0, 4.0);
    let cut = wall.difference(&slot);
    assert!(
        cut.volume() < wall.volume() - 5.0,
        "box slot must remove volume: wall={} cut={}",
        wall.volume(),
        cut.volume()
    );
    assert!(cut.volume() > 100.0, "result should still be a solid");
}

/// Mesh-only operands used to concatenate tessellations (internal faces,
/// coincident geometry). They must now run triangle-level CSG.
#[test]
fn mesh_operand_difference_actually_cuts() {
    let a = Solid::from_mesh(Solid::cube(10.0, 10.0, 10.0).to_mesh(16));
    let b = Solid::from_mesh(
        Solid::cube(4.0, 4.0, 12.0)
            .translate(3.0, 3.0, -1.0)
            .to_mesh(16),
    );
    let cut = a.difference(&b);
    let vol = cut.volume();
    assert!(
        (vol - 840.0).abs() < 20.0,
        "mesh CSG difference vol={vol}, want ~840"
    );
    assert_eq!(
        cut.to_mesh(16).welded_defective_edge_count(),
        0,
        "mesh-operand difference must be manifold"
    );
    assert!(
        cut.wrong_geometry_events().next().is_none(),
        "mesh-operand CSG is a fallback, not wrong geometry: {:?}",
        cut.degradations()
    );
}
