//! Regression: an unsupported/mismatched surface pair in SSI must surface
//! as a clean `Err` from `boolean_op`, never a panic.
//!
//! In the browser a panic poisons the WASM instance for the rest of the
//! session, so the SSI failure path is required to propagate a typed error
//! through the whole 4-stage pipeline (AABB filter → SSI → classification →
//! sewing) instead of unwinding.

use vcad_kernel_booleans::{boolean_op, BooleanError, BooleanOp, SsiError};
use vcad_kernel_geom::{SphereSurface, Surface, SurfaceKind};
use vcad_kernel_math::{Dir3, Point2, Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_cube, BRepSolid};

/// A surface that reports [`SurfaceKind::Plane`] but whose concrete type is
/// not `Plane` — the downcast-mismatch condition that used to be a hard
/// failure on the SSI path.
#[derive(Debug, Clone)]
struct LyingSurface(SphereSurface);

impl Surface for LyingSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        self.0.evaluate(uv)
    }
    fn normal(&self, uv: Point2) -> Dir3 {
        self.0.normal(uv)
    }
    fn d_du(&self, uv: Point2) -> Vec3 {
        self.0.d_du(uv)
    }
    fn d_dv(&self, uv: Point2) -> Vec3 {
        self.0.d_dv(uv)
    }
    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        self.0.domain()
    }
    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Plane // the lie
    }
    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        Box::new(LyingSurface(
            self.0
                .transform(t)
                .as_any()
                .downcast_ref::<SphereSurface>()
                .expect("sphere transform yields sphere")
                .clone(),
        ))
    }
}

fn translate(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
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

#[test]
fn mismatched_surface_pair_returns_clean_err() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 5.0, 5.0, 5.0);

    // Corrupt every surface of B so any candidate face pair hits the
    // mismatch: each reports Plane while the concrete type is a sphere.
    for s in &mut b.geometry.surfaces {
        *s = Box::new(LyingSurface(SphereSurface::new(5.0)));
    }

    for op in [
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersection,
    ] {
        let result = boolean_op(&a, &b, op, 16);
        assert!(
            matches!(
                result,
                Err(BooleanError::Ssi(SsiError::SurfaceKindMismatch { .. }))
            ),
            "{op:?} should return a clean SSI error, got {result:?}"
        );
    }
}

#[test]
fn healthy_pairs_still_succeed() {
    // Sanity: the fallible signature doesn't change behavior for good input.
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 5.0, 5.0, 5.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 16).expect("healthy union succeeds");
    assert!(result.as_brep().is_some());
}
