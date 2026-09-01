//! KNOWN OPEN DEFECT (2026-08-27), captured with its diagnosis.
//!
//! A boss whose top and bottom faces are FLUSH with the faces of the body
//! it is unioned onto goes non-manifold when its footprint straddles a
//! line an earlier boolean already split into that face. Gear teeth are
//! the canonical case: `planet-72t` scores 2133 bad edges, all of them on
//! the z = 0 and z = h caps at the tooth roots.
//!
//! Bisection (`gear_teeth_are_manifold` below):
//!  - one tooth on a fresh blank: manifold at every angle;
//!  - a second tooth is manifold everywhere EXCEPT a ~3° band around 180°
//!    — the diameter opposite the first tooth. The first union splits the
//!    cap with the tooth's side-plane LINES, which run the full diameter,
//!    so the cap becomes two half-faces meeting on a chord that passes
//!    through the 180° point. A coplanar footprint landing on that seam
//!    contributes fragments to both half-faces and one of them is kept
//!    twice: 14 over-used edges, zero net volume (a zero-thickness flap,
//!    which is why the volume oracle passes it);
//!  - with the teeth counts a real gear uses, every tooth at 180° from an
//!    earlier one repeats it — the count grows linearly with tooth count.
//!
//! WORKAROUND until this is fixed: give the boss a small overhang past the
//! body's faces (model the tooth taller and let the union trim it). The
//! `overhang` test below is the same geometry with 0.5 mm of overhang and
//! it is manifold, so nothing about the arrangement is intrinsically hard.
//!
//! The root fix is in the pipeline, not in the splitters this file's
//! sibling test covers: either stop the line splitter from cutting a face
//! clear across where the other solid's face does not reach, or classify
//! coplanar contact per-fragment across sub-faces.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

const SEGMENTS: u32 = 44;

fn apply(brep: &mut BRepSolid, t: &Transform) {
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

fn bad_edges(mesh: &TriangleMesh) -> (usize, usize) {
    let quantum = 1e-5;
    let key = |vi: usize| -> [i64; 3] {
        [
            (mesh.vertices[vi * 3] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 1] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 2] as f64 / quantum).round() as i64,
        ]
    };
    let mut uses: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();
    for tri in mesh.indices.chunks(3) {
        let v = [
            key(tri[0] as usize),
            key(tri[1] as usize),
            key(tri[2] as usize),
        ];
        for i in 0..3 {
            let (a, b) = (v[i], v[(i + 1) % 3]);
            if a == b {
                continue;
            }
            let e = if a < b { (a, b) } else { (b, a) };
            *uses.entry(e).or_default() += 1;
        }
    }
    (
        uses.values().filter(|&&n| n == 1).count(),
        uses.values().filter(|&&n| n > 2).count(),
    )
}

/// `teeth` bosses unioned one at a time onto a cylindrical blank, each
/// spanning the blank's full height (flush caps) — `overhang` extends them
/// past both caps instead.
fn gear(teeth: usize, overhang: f64) -> BRepSolid {
    let mut blank = make_cylinder(17.35, 8.0, SEGMENTS);
    for i in 0..teeth {
        let mut tooth = make_cube(1.2, 0.70, 8.0 + 2.0 * overhang);
        apply(&mut tooth, &Transform::translation(17.3, -0.42, -overhang));
        let ang = 2.0 * std::f64::consts::PI * (i as f64) / (teeth as f64);
        apply(&mut tooth, &Transform::rotation_z(ang));
        blank = boolean_op(&blank, &tooth, BooleanOp::Union, SEGMENTS)
            .unwrap()
            .into_brep()
            .expect("union returned no B-rep");
    }
    blank
}

/// The workaround, and the proof that the arrangement is representable:
/// with 0.5 mm of overhang the same teeth carry NO doubled surface. (A
/// handful of hairline seams remain — the advisory population documented
/// in `crate::mesh_report`; slicers close those, they do not fill bores
/// over them.)
#[test]
fn overhanging_teeth_have_no_doubled_surface() {
    let (open, over) = bad_edges(&tessellate_brep(&gear(16, 0.5), SEGMENTS));
    assert_eq!(over, 0, "over-used edges with overhang (open = {open})");
    assert!(open <= 8, "unexpectedly many hairline seams: {open}");
}

#[test]
#[ignore = "known open defect: coplanar boss footprint straddling an earlier line split"]
fn gear_teeth_are_manifold() {
    let (open, over) = bad_edges(&tessellate_brep(&gear(16, 0.0), SEGMENTS));
    assert_eq!(
        (open, over),
        (0, 0),
        "flush-capped teeth: {open} unpaired + {over} over-used edges"
    );
}
