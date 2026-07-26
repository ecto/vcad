//! End-to-end tests for the loon fastener forms.
//!
//! The headline case is the failure that shipped twice in real work: a bolt
//! circle placed on a flange, the assembly mirrored, and half the heads
//! ending up on the far side of the flange with the shafts pointing into
//! free space. It happened because the head and the shaft were two separate
//! `rotate`s, so mirroring had to be undone by hand in three places.
//!
//! `[bolt-circle]` builds head and shaft as one solid and orients it from
//! its own axis, so the invariant below holds in both copies without the
//! author doing anything.

use vcad_ir::{CsgOp, Document, NodeId, Vec3};
use vcad_loon::eval_vcad;

/// A leaf primitive resolved into world space.
struct Leaf {
    radius: f64,
    /// Center of the primitive's base, in world space.
    origin: Vec3,
    /// The primitive's local +Z, in world space.
    axis: Vec3,
}

#[derive(Clone, Copy)]
struct Xf {
    m: [[f64; 3]; 3],
    t: Vec3,
}

impl Xf {
    fn identity() -> Self {
        Xf {
            m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            t: Vec3::new(0.0, 0.0, 0.0),
        }
    }
    fn point(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0][0] * p.x + self.m[0][1] * p.y + self.m[0][2] * p.z + self.t.x,
            self.m[1][0] * p.x + self.m[1][1] * p.y + self.m[1][2] * p.z + self.t.y,
            self.m[2][0] * p.x + self.m[2][1] * p.y + self.m[2][2] * p.z + self.t.z,
        )
    }
    fn dir(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0][0] * p.x + self.m[0][1] * p.y + self.m[0][2] * p.z,
            self.m[1][0] * p.x + self.m[1][1] * p.y + self.m[1][2] * p.z,
            self.m[2][0] * p.x + self.m[2][1] * p.y + self.m[2][2] * p.z,
        )
    }
    fn compose(&self, inner: &Xf) -> Xf {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.m[i][k] * inner.m[k][j]).sum();
            }
        }
        Xf {
            m,
            t: self.point(inner.t),
        }
    }
    fn translation(o: Vec3) -> Xf {
        Xf {
            m: Xf::identity().m,
            t: o,
        }
    }
    /// Euler XYZ in degrees, applied X then Y then Z (the `CsgOp::Rotate`
    /// convention).
    fn rotation(a: Vec3) -> Xf {
        let (x, y, z) = (a.x.to_radians(), a.y.to_radians(), a.z.to_radians());
        let rx = [
            [1.0, 0.0, 0.0],
            [0.0, x.cos(), -x.sin()],
            [0.0, x.sin(), x.cos()],
        ];
        let ry = [
            [y.cos(), 0.0, y.sin()],
            [0.0, 1.0, 0.0],
            [-y.sin(), 0.0, y.cos()],
        ];
        let rz = [
            [z.cos(), -z.sin(), 0.0],
            [z.sin(), z.cos(), 0.0],
            [0.0, 0.0, 1.0],
        ];
        let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
            let mut m = [[0.0; 3]; 3];
            for (i, row) in m.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
                }
            }
            m
        };
        Xf {
            m: mul(rz, mul(ry, rx)),
            t: Vec3::new(0.0, 0.0, 0.0),
        }
    }
    /// Reflection across a plane through `origin` with unit normal `n`.
    fn reflection(origin: Vec3, n: Vec3) -> Xf {
        let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
        let n = Vec3::new(n.x / len, n.y / len, n.z / len);
        let mut m = [[0.0; 3]; 3];
        let nv = [n.x, n.y, n.z];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = if i == j { 1.0 } else { 0.0 } - 2.0 * nv[i] * nv[j];
            }
        }
        let refl = Xf {
            m,
            t: Vec3::new(0.0, 0.0, 0.0),
        };
        // Reflect about a plane offset from the world origin.
        Xf::translation(origin)
            .compose(&refl)
            .compose(&Xf::translation(Vec3::new(-origin.x, -origin.y, -origin.z)))
    }
}

/// Collect every cylinder/cone/prism leaf under `root`, in world space.
fn leaves(doc: &Document, root: NodeId, xf: Xf, out: &mut Vec<Leaf>) {
    let Some(node) = doc.nodes.get(&root) else {
        return;
    };
    let up = Vec3::new(0.0, 0.0, 1.0);
    match &node.op {
        CsgOp::Cylinder { radius, .. } | CsgOp::Prism { radius, .. } => out.push(Leaf {
            radius: *radius,
            origin: xf.point(Vec3::new(0.0, 0.0, 0.0)),
            axis: xf.dir(up),
        }),
        CsgOp::Cone { radius_bottom, .. } => out.push(Leaf {
            radius: *radius_bottom,
            origin: xf.point(Vec3::new(0.0, 0.0, 0.0)),
            axis: xf.dir(up),
        }),
        CsgOp::Translate { child, offset } => {
            leaves(doc, *child, xf.compose(&Xf::translation(*offset)), out)
        }
        CsgOp::Rotate { child, angles } => {
            leaves(doc, *child, xf.compose(&Xf::rotation(*angles)), out)
        }
        CsgOp::Mirror {
            child,
            plane_origin,
            plane_normal,
        } => leaves(
            doc,
            *child,
            xf.compose(&Xf::reflection(*plane_origin, *plane_normal)),
            out,
        ),
        CsgOp::Union { left, right }
        | CsgOp::Difference { left, right }
        | CsgOp::Intersection { left, right } => {
            leaves(doc, *left, xf, out);
            leaves(doc, *right, xf, out);
        }
        _ => {}
    }
}

fn collect(doc: &Document) -> Vec<Leaf> {
    let mut out = Vec::new();
    for entry in &doc.roots {
        leaves(doc, entry.root, Xf::identity(), &mut out);
    }
    out
}

/// The failure that shipped twice: after mirroring, every head must still sit
/// on the outboard side of its own flange.
#[test]
fn mirrored_bolt_circle_keeps_every_head_on_the_same_side() {
    // A flange at z = 10 with six M4x12 SHCS driven 8 mm downward into it,
    // and the same thing mirrored across the XY plane.
    let src = r#"
[let ring [bolt-circle "M4x12" "shcs" 60 6  0 0 10  0 0 -1  8]]
[root [union [mirror 0 0 0  0 0 1 ring] ring] "steel"]
"#;
    let doc = eval_vcad(src, None).expect("evaluates");
    let leaves = collect(&doc);

    // M4 SHCS: shaft r = 2.0, head r = 3.5. Heads are the wide cylinders.
    let heads: Vec<&Leaf> = leaves
        .iter()
        .filter(|l| (l.radius - 3.5).abs() < 1e-9)
        .collect();
    assert_eq!(heads.len(), 12, "6 bolts in each of the two copies");

    let mut upper = 0;
    let mut lower = 0;
    for head in &heads {
        // Each head's own axis points from head toward tip. The head's base
        // must sit *behind* the flange face along that axis — i.e. the head
        // is outboard, never buried on the far side.
        let face_z = if head.axis.z < 0.0 { 10.0 } else { -10.0 };
        let along = (head.origin.z - face_z) * -head.axis.z.signum();
        assert!(
            along > 0.0,
            "head at z={} with axis z={} is on the wrong side of its flange",
            head.origin.z,
            head.axis.z
        );
        if head.axis.z < 0.0 {
            upper += 1;
        } else {
            lower += 1;
        }
    }
    assert_eq!((upper, lower), (6, 6), "one full ring per copy");
}

#[test]
fn bolt_circle_rolls_up_one_bom_line_with_the_right_count() {
    let src = r#"
[root [bolt-circle "M4x12" "shcs" 60 6  0 0 10  0 0 -1  8] "steel"]
"#;
    let doc = eval_vcad(src, None).expect("evaluates");
    assert_eq!(doc.hardware.len(), 1);
    assert_eq!(doc.hardware[0].catalog_id.as_deref(), Some("screw.m4-shcs"));
    assert_eq!(doc.hardware[0].spec, "M4x12 SHCS");
    assert_eq!(doc.hardware[0].qty, 6);
}

#[test]
fn a_pattern_multiplies_the_fastener_count() {
    let src = r#"
[root [linear-pattern 20 0 0 4 20 [bolt "M4x12" "shcs" 0 0 0  0 0 8]] "steel"]
"#;
    let doc = eval_vcad(src, None).expect("evaluates");
    assert_eq!(doc.hardware.len(), 1);
    assert_eq!(doc.hardware[0].qty, 4);
}

#[test]
fn stacked_bolt_emits_its_washer_and_nut() {
    let src = r#"
[root [bolt-stacked "M4x20" "shcs" 0 0 0  0 0 12 #["washer" "nut"]] "steel"]
"#;
    let doc = eval_vcad(src, None).expect("evaluates");
    let specs: Vec<&str> = doc.hardware.iter().map(|h| h.spec.as_str()).collect();
    assert!(specs.contains(&"M4x20 SHCS"), "{specs:?}");
    assert!(specs.iter().any(|s| s.contains("washer")), "{specs:?}");
    assert!(specs.iter().any(|s| s.contains("hex nut")), "{specs:?}");
}

#[test]
fn a_bolt_too_short_for_its_stack_is_an_error_not_a_render() {
    let src = r#"
[root [bolt-stacked "M4x12" "shcs" 0 0 0  0 0 10 #["washer" "nut"]] "steel"]
"#;
    let err = eval_vcad(src, None).expect_err("must not model an impossible stack");
    assert!(err.to_string().contains("too short"), "{err}");
}

#[test]
fn an_unstocked_length_is_rejected() {
    let src = r#"
[root [bolt "M4x11" "shcs" 0 0 0  0 0 8] "steel"]
"#;
    let err = eval_vcad(src, None).expect_err("M4x11 is not a thing");
    assert!(err.to_string().contains("not a stocked length"), "{err}");
}

#[test]
fn hole_and_fastener_share_one_dimension() {
    // A clearance hole is wider than the thread; a tapped hole is narrower.
    let src = r#"
[root [pipe [cube 40 20 6]
            [difference [clearance-hole "M4" 6  10 10 0  0 0 1]]
            [difference [tapped-hole "M4" 6  30 10 0  0 0 1]]] "aluminum"]
"#;
    let doc = eval_vcad(src, None).expect("evaluates");
    let radii: Vec<f64> = collect(&doc).iter().map(|l| l.radius).collect();
    assert!(
        radii.iter().any(|r| (r - 2.25).abs() < 1e-9),
        "M4 clearance hole is ⌀4.5: {radii:?}"
    );
    assert!(
        radii.iter().any(|r| (r - 1.65).abs() < 1e-9),
        "M4 tap drill is ⌀3.3: {radii:?}"
    );
}
