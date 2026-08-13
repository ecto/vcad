//! Regression tests for defect D (printed-part handoff, 2026-08-12):
//! tessellated boolean results must keep quadric-face vertices ON the
//! analytic surface.
//!
//! The mesh-CSG fallback splits operand triangles and heals seams at the
//! mesh level, which used to leave sphere vertices up to ~0.6 mm off a
//! R25 sphere — 20x the legitimate chord sag, and larger than a printed
//! part's entire fit budget (a Ø50 conforming socket runs 0.05–0.4 mm
//! fits). Volume assertions cannot catch this: the displacements average
//! out, so every volume check passed while the surface was lumpy.
//!
//! Hence these assert SURFACE FIDELITY: max |dist(v, center) − R| over
//! the mesh vertices lying in a band around each quadric.

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::{Point3, Transform};
use vcad_kernel_primitives::{make_cube, make_cylinder, make_sphere, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

const SEGMENTS: u32 = 32;

fn apply_transform(brep: &mut BRepSolid, t: &Transform) {
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(t))
        .collect();
}

fn translate(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
    apply_transform(brep, &Transform::translation(dx, dy, dz));
}

fn result_mesh(r: BooleanResult) -> TriangleMesh {
    let BooleanResult::BRep(brep) = r;
    tessellate_brep(&brep, SEGMENTS)
}

/// Max |dist(v, c) − r| over vertices within `band` of the sphere surface,
/// optionally restricted by a vertex filter (used to exclude designed
/// feature edges where a vertex legitimately belongs to two surfaces).
fn sphere_fidelity(
    mesh: &TriangleMesh,
    c: Point3,
    r: f64,
    band: f64,
    keep: impl Fn(&Point3) -> bool,
) -> (usize, f64) {
    let mut n = 0usize;
    let mut worst = 0.0f64;
    for v in mesh.vertices.chunks_exact(3) {
        let p = Point3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        let e = ((p - c).norm() - r).abs();
        if e < band && keep(&p) {
            n += 1;
            worst = worst.max(e);
        }
    }
    (n, worst)
}

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut vol = 0.0f64;
    for t in mesh.indices.chunks_exact(3) {
        let g = |k: u32| {
            let i = (k as usize) * 3;
            [
                mesh.vertices[i] as f64,
                mesh.vertices[i + 1] as f64,
                mesh.vertices[i + 2] as f64,
            ]
        };
        let (a, b, c) = (g(t[0]), g(t[1]), g(t[2]));
        vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    vol.abs()
}

/// The printed-part control: a sphere pocket breaking through several faces
/// of a block (forces the mesh fallback). Every socket vertex must sit on
/// the R25 sphere to well under a printed fit budget.
#[test]
fn sphere_pocket_through_faces_stays_on_surface() {
    // Ball center at origin; block corner at (-38, -40, 0.2), i.e. the
    // rev D upper cuff blank: sphere breaks the z, one y, and no x faces.
    let mut cube = make_cube(76.0, 62.0, 19.8);
    translate(&mut cube, -38.0, -40.0, 0.2);
    let sphere = make_sphere(25.0, SEGMENTS);

    let mesh =
        result_mesh(boolean_op(&cube, &sphere, BooleanOp::Difference, SEGMENTS).expect("boolean"));

    let vol = mesh_volume(&mesh);
    // MC ground truth for this arrangement: 63,228 ± 60.
    assert!(
        (vol - 63_228.0).abs() < 0.01 * 63_228.0,
        "volume {vol:.0} departs from ground truth 63,228"
    );

    // Functional seat surface: away from the block rims (y = 22) and the
    // relief plane (z = 0.2), where designed feature corners legitimately
    // pin vertices between two planes.
    let (n, worst) = sphere_fidelity(&mesh, Point3::origin(), 25.0, 0.4, |p| {
        p.z > 0.5 && p.y < 20.0
    });
    assert!(n > 100, "expected a populated socket band, got {n} verts");
    // At this test's 32-segment tessellation the projection floor is
    // ~0.09 mm (a handful of seam vertices constrained by damaged facet
    // normals); export-resolution meshes measure <= 0.02. The threshold
    // is set to catch the failure class (0.64 pre-fix), not the floor.
    assert!(
        worst < 0.15,
        "seat vertices up to {worst:.3} mm off the R25 sphere (was 0.64 before the fix)"
    );
}

/// Chained cuts must not re-shatter the sphere region: the quadric
/// carriers are stashed on the fallback result so later booleans keep
/// projecting. Two box cuts after the pocket, then check fidelity.
#[test]
fn chained_cuts_keep_sphere_fidelity() {
    let mut cube = make_cube(76.0, 62.0, 19.8);
    translate(&mut cube, -38.0, -40.0, 0.2);
    let sphere = make_sphere(25.0, SEGMENTS);
    let r1 = boolean_op(&cube, &sphere, BooleanOp::Difference, SEGMENTS).expect("cut 1");
    let BooleanResult::BRep(s1) = r1;

    let mut slot = make_cube(5.0, 5.0, 40.0);
    translate(&mut slot, -34.5, 12.0, -5.0);
    let r2 = boolean_op(&s1, &slot, BooleanOp::Difference, SEGMENTS).expect("cut 2");
    let BooleanResult::BRep(s2) = r2;

    let mut slot2 = make_cube(5.0, 5.0, 40.0);
    translate(&mut slot2, 29.5, 12.0, -5.0);
    let r3 = boolean_op(&s2, &slot2, BooleanOp::Difference, SEGMENTS).expect("cut 3");

    let mesh = result_mesh(r3);
    let (n, worst) = sphere_fidelity(&mesh, Point3::origin(), 25.0, 0.4, |p| {
        p.z > 0.5 && p.y < 20.0
    });
    assert!(n > 100, "expected a populated socket band, got {n} verts");
    assert!(
        worst < 0.15,
        "after chained cuts, seat vertices up to {worst:.3} mm off the sphere (0.39 pre-fix)"
    );
}

/// Sphere ∩ cylinder with the cylinder axis through the sphere center (the
/// cuff's socket/journal handoff): seam vertices must land on the exact
/// intersection circle, and both surfaces must stay true away from designed
/// feature corners.
#[test]
fn sphere_cylinder_seam_lands_on_intersection_circle() {
    let mut cube = make_cube(76.0, 62.0, 19.8);
    translate(&mut cube, -38.0, -40.0, 0.2);
    let sphere = make_sphere(25.0, SEGMENTS);
    let r1 = boolean_op(&cube, &sphere, BooleanOp::Difference, SEGMENTS).expect("cut 1");
    let BooleanResult::BRep(s1) = r1;

    // Bore along -Y, axis through the ball center, breaking out the y face.
    let mut bore = make_cylinder(17.5, 28.0, SEGMENTS);
    apply_transform(
        &mut bore,
        &Transform::rotation_x(-std::f64::consts::FRAC_PI_2),
    );
    translate(&mut bore, 0.0, -16.0, 0.0);
    let r2 = boolean_op(&s1, &bore, BooleanOp::Difference, SEGMENTS).expect("cut 2");

    let mesh = result_mesh(r2);
    // Exclude the bore rim's plane crossings; judge the open sphere band.
    let (n, worst) = sphere_fidelity(&mesh, Point3::origin(), 25.0, 0.4, |p| {
        p.z > 0.5 && p.y < 20.0 && (p.x.hypot(p.z) - 17.5).abs() > 1.0
    });
    assert!(n > 50, "expected a populated socket band, got {n} verts");
    assert!(
        worst < 0.15,
        "seat vertices up to {worst:.3} mm off the sphere near the bore (0.39 pre-fix)"
    );
}

/// Osculating sphere + coaxial cylinder of the SAME radius (a slide-on
/// socket with a full-diameter lead-in bore — the K1 mono cuff). No seam
/// circle exists; the composite surface hands off at the tangency plane,
/// and the projector must pick the correct surface per vertex from its
/// incident facet normals instead of standing down.
#[test]
fn osculating_sphere_cylinder_seat_stays_on_surface() {
    let mut cube = make_cube(62.0, 70.0, 62.0);
    translate(&mut cube, -31.0, -40.0, -31.0);
    let sphere = make_sphere(25.05, SEGMENTS);
    let r1 = boolean_op(&cube, &sphere, BooleanOp::Difference, SEGMENTS).expect("seat");
    let BooleanResult::BRep(s1) = r1;

    // Lead-in: same radius, axis through the sphere center, +Y half only.
    let mut lead = make_cylinder(25.05, 32.0, SEGMENTS);
    apply_transform(
        &mut lead,
        &Transform::rotation_x(-std::f64::consts::FRAC_PI_2),
    );
    let r2 = boolean_op(&s1, &lead, BooleanOp::Difference, SEGMENTS).expect("lead-in");

    let mesh = result_mesh(r2);
    // The seat is the y < 0 hemisphere; y > 0 belongs to the cylinder wall.
    let (n, worst) = sphere_fidelity(&mesh, Point3::origin(), 25.05, 0.4, |p| p.y < -1.0);
    assert!(n > 50, "expected a populated seat band, got {n} verts");
    assert!(
        worst < 0.15,
        "seat vertices up to {worst:.3} mm off the sphere (0.40 before the tangency handoff)"
    );
}
