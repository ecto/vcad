//! Regression: the spherical corner blends of a filleted box must trim as
//! whole spherical triangles, with no hole where the three fillet cylinders
//! meet.
//!
//! `try_sphere_blend` builds each corner patch with `axis` set to one of the
//! three face normals, so one of the patch's three loop vertices lands
//! exactly on the sphere's pole — where longitude is indeterminate and
//! `project_to_sphere` reports `u = 0` for want of anything better. The
//! straight-line point-in-polygon trim test then saw the right triangle
//! `(0, π/2), (0, 0), (π/2, 0)` instead of the true square
//! `[0, π/2] × [0, π/2]`, and its hypotenuse sliced away half of every
//! octant. Rays through the missing half passed into the solid: the
//! crescent-shaped hole visible at each corner of a ray-traced filleted part.
//!
//! Note this defect is invisible to a watertightness check. The B-rep and its
//! tessellation are both closed manifolds before and after the fix — the hole
//! lived purely in the ray tracer's trim test — so the oracle has to probe
//! trimming directly.

use vcad_kernel_geom::SurfaceKind;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_raytrace::trim::{extract_face_uv_loop, point_in_face, project_face_uv};
use vcad_kernel_raytrace::{Bvh, Ray};
use vcad_kernel_topo::FaceId;

/// Sample directions strictly inside the spherical triangle spanned by three
/// unit corner directions, as normalized positive barycentric combinations.
fn interior_directions(corners: [Vec3; 3], steps: usize) -> Vec<Vec3> {
    let mut out = Vec::new();
    for i in 1..steps {
        for j in 1..(steps - i) {
            let k = steps - i - j;
            let w = [i as f64, j as f64, k as f64];
            let d = corners[0] * w[0] + corners[1] * w[1] + corners[2] * w[2];
            if d.norm() > 1e-12 {
                out.push(d.normalize());
            }
        }
    }
    out
}

/// Every spherical corner patch of the filleted plate, with its sphere and
/// the three unit directions of its loop vertices.
fn corner_patches(brep: &BRepSolid) -> Vec<(FaceId, vcad_kernel_geom::SphereSurface, [Vec3; 3])> {
    let mut out = Vec::new();
    for (face_id, face) in &brep.topology.faces {
        let surface = &brep.geometry.surfaces[face.surface_index];
        if surface.surface_type() != SurfaceKind::Sphere {
            continue;
        }
        let sph = surface
            .as_any()
            .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            .expect("sphere kind must downcast");
        let verts = brep.topology.loop_vertices(face.outer_loop);
        assert_eq!(
            verts.len(),
            3,
            "a convex corner blend is a spherical triangle"
        );
        let dirs: Vec<Vec3> = verts
            .iter()
            .map(|&v| (brep.topology.vertices[v].point - sph.center).normalize())
            .collect();
        out.push((face_id, sph.clone(), [dirs[0], dirs[1], dirs[2]]));
    }
    out
}

fn filleted_plate() -> BRepSolid {
    let cube = vcad_kernel_primitives::make_cube(40.0, 30.0, 6.0);
    vcad_kernel_fillet::fillet_all_edges(&cube, 2.0)
}

#[test]
fn corner_blends_are_eight_spherical_octants() {
    let brep = filleted_plate();
    let patches = corner_patches(&brep);
    assert_eq!(patches.len(), 8, "one corner blend per box vertex");

    // Each patch is a true octant: its three loop directions are mutually
    // orthogonal, and each vertex sits exactly on the blend sphere.
    for (face_id, sph, dirs) in &patches {
        assert!(
            (sph.radius.abs() - 2.0).abs() < 1e-9,
            "{face_id:?} blend radius must equal the fillet radius"
        );
        for a in 0..3 {
            let b = (a + 1) % 3;
            assert!(
                dirs[a].dot(&dirs[b]).abs() < 1e-9,
                "{face_id:?} corner directions must be orthogonal"
            );
        }
    }
}

#[test]
fn corner_blend_trim_polygon_covers_the_whole_octant() {
    let brep = filleted_plate();
    for (face_id, _sph, dirs) in corner_patches(&brep) {
        // An octant's boundary is three 90° great arcs, so in the sphere's
        // own (longitude, latitude) space the patch is a π/2 × π/2 square of
        // area (π/2)². The pole-collapsed polygon was the right triangle
        // with half that area.
        let uvs = extract_face_uv_loop(&brep, face_id);
        let area = {
            let mut a = 0.0;
            for i in 0..uvs.len() {
                let p = uvs[i];
                let q = uvs[(i + 1) % uvs.len()];
                a += p.x * q.y - q.x * p.y;
            }
            (a / 2.0).abs()
        };
        let expected = std::f64::consts::FRAC_PI_2 * std::f64::consts::FRAC_PI_2;
        assert!(
            (area - expected).abs() < 1e-9,
            "{face_id:?} trim polygon covers {area} of the octant's {expected} \
             in UV — the pole vertex collapsed its longitude"
        );
        let _ = dirs;
    }
}

#[test]
fn corner_blend_interior_is_never_trimmed_away() {
    let brep = filleted_plate();
    for (face_id, sph, dirs) in corner_patches(&brep) {
        for d in interior_directions(dirs, 12) {
            let p = Point3::from(sph.center.to_vec() + d * sph.radius.abs());
            let uv = project_face_uv(&brep, face_id, &p);
            assert!(
                point_in_face(&brep, face_id, uv),
                "{face_id:?} rejects its own surface point {p:?} at uv \
                 ({}, {}) — the corner blend has a hole",
                uv.x,
                uv.y
            );
        }
    }
}

#[test]
fn rays_into_every_corner_hit_the_blend() {
    let brep = filleted_plate();
    let patches = corner_patches(&brep);
    let bvh = Bvh::build(&brep);

    for (face_id, sph, dirs) in patches {
        for d in interior_directions(dirs, 8) {
            let surf = Point3::from(sph.center.to_vec() + d * sph.radius.abs());
            // Fire inward along the surface normal from well outside.
            let origin = Point3::from(surf.to_vec() + d * 100.0);
            let ray = Ray::new(origin, -d);
            let hit = bvh.trace_closest(&ray).unwrap_or_else(|| {
                panic!("{face_id:?}: ray into the corner blend missed entirely")
            });
            assert!(
                (hit.t - 100.0).abs() < 1e-6,
                "{face_id:?}: ray into the corner blend landed at t={} \
                 instead of the blend surface at t=100 — it passed through a \
                 hole and hit something behind it",
                hit.t
            );
        }
    }
}
