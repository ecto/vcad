//! **M1 — frozen tessellation + lift-bridge.**
//!
//! Acceptance:
//! 1. interior-sample `dx/dθ` on a single generic face (plane and cylinder)
//!    matches the central-difference oracle to the gate (≤ 1e-6);
//! 2. the topology-signature assertion is in place and, against a deliberately
//!    topology-changing perturbation, **errors** rather than returning garbage.

use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, SurfaceSeed};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_tessellate::frozen::{
    audit, models::BlockWithHole, AuditError, FrozenTessellation, SampleAddr,
};

const GATE: f64 = 1e-6;
const H: f64 = 1e-6;

/// Node-level FD of a one-surface frozen patch under a scalar parameter.
/// Returns the max relative error between analytic (dual) and FD node dx/dθ.
fn interior_sample_max_rel_err(
    build: impl Fn(f64) -> GeometryStore,
    seed: SurfaceSeed,
    nodes: Vec<SampleAddr>,
    theta: f64,
) -> f64 {
    let tess = FrozenTessellation {
        nodes,
        tris: vec![],
        seeds: vec![seed],
    };
    let dual = tess.positions_dual(&build(theta)).unwrap();
    let xp = tess.positions(&build(theta + H));
    let xm = tess.positions(&build(theta - H));
    let mut max_rel = 0.0_f64;
    for (i, d) in dual.iter().enumerate() {
        let fd = [
            (xp[i].x - xm[i].x) / (2.0 * H),
            (xp[i].y - xm[i].y) / (2.0 * H),
            (xp[i].z - xm[i].z) / (2.0 * H),
        ];
        let an = [d.x.dual, d.y.dual, d.z.dual];
        for k in 0..3 {
            let denom = an[k].abs().max(fd[k].abs()).max(1e-9);
            max_rel = max_rel.max((an[k] - fd[k]).abs() / denom);
        }
    }
    max_rel
}

#[test]
fn m1_cylinder_interior_samples_match_fd() {
    // Grid of interior (u=φ, v=z) samples on a cylinder whose radius is θ.
    let mut nodes = Vec::new();
    for iu in 0..8 {
        for iv in 0..5 {
            let u = std::f64::consts::TAU * (iu as f64 + 0.37) / 8.0;
            let v = -2.0 + (iv as f64) * 1.3;
            nodes.push(SampleAddr {
                surface_index: 0,
                u,
                v,
            });
        }
    }
    let build = |r: f64| {
        let mut s = GeometryStore::new();
        s.add_surface(Box::new(CylinderSurface::new(r)));
        s
    };
    let err = interior_sample_max_rel_err(build, SurfaceSeed::CylinderRadius, nodes, 6.25);
    eprintln!("M1 cylinder interior max rel err = {err:e}");
    assert!(
        err <= GATE,
        "cylinder interior dx/dr max rel err {err} > {GATE}"
    );
}

#[test]
fn m1_plane_interior_samples_match_fd() {
    // Grid of interior (u, v) samples on a plane that slides along +z with θ.
    let mut nodes = Vec::new();
    for iu in 0..5 {
        for iv in 0..5 {
            nodes.push(SampleAddr {
                surface_index: 0,
                u: -3.0 + iu as f64 * 1.5,
                v: -3.0 + iv as f64 * 1.5,
            });
        }
    }
    let build = |t: f64| {
        let mut s = GeometryStore::new();
        s.add_surface(Box::new(Plane::new(
            Point3::new(0.0, 0.0, t),
            Vec3::x(),
            Vec3::y(),
        )));
        s
    };
    let err = interior_sample_max_rel_err(
        build,
        SurfaceSeed::PlaneTranslate { rate: Vec3::z() },
        nodes,
        4.0,
    );
    eprintln!("M1 plane interior max rel err = {err:e}");
    assert!(
        err <= GATE,
        "plane interior dx/dθ max rel err {err} > {GATE}"
    );
}

#[test]
fn m1_valid_step_has_invariant_signature() {
    // A hole comfortably inside the block: the derivative step keeps topology.
    let model = BlockWithHole::new(10.0, 2.0, 5.0, 64);
    let report = audit(&model, H).expect("valid step must not change topology");
    // The frozen structural hash is stable; the audit succeeded => signature invariant.
    assert_eq!(report.signature.n_vertices, 4 * 64);
}

#[test]
fn m1_topology_change_errors_not_lies() {
    // Nominal radius sits one h below the block half-width, so θ+h pushes the
    // hole *through* the wall — a genuine topology change. The frozen seam must
    // detect the signature flip and refuse, not return a plausible-wrong value.
    let half = 10.0;
    let model = BlockWithHole::new(half, 2.0, half - H, 64);
    match audit(&model, H) {
        Err(AuditError::TopologyChanged(tc)) => {
            // The plus step differs from center; the minus step does not.
            assert_ne!(tc.center.orientation_hash, tc.plus.orientation_hash);
            assert_eq!(tc.center.orientation_hash, tc.minus.orientation_hash);
        }
        other => panic!("expected TopologyChanged, got {other:?}"),
    }
}
