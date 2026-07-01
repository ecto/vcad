//! M3 — mass-property QoIs with exact θ-derivatives, and the contraction
//! hook a physics functional plugs into.

use vcad_kernel_diff::{
    contract_sensitivity, evaluate_with_sensitivity, mass_properties,
    mass_properties_with_derivative, volume_gradient, volume_with_derivative, ParamSeeding,
    SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_sketch::SketchProfile;
use vcad_kernel_tessellate::frozen::{capture_plan, evaluate_plan};
use vcad_kernel_tessellate::TessellationParams;

const GATE: f64 = 1e-6;
const H: f64 = 1e-6;
const RHO: f64 = 1.0;

#[test]
fn extrude_inertia_derivative_matches_closed_form_and_fd() {
    // Cuboid w × l × d with θ = extrude distance d:
    //   I_zz about the centroid = m (w² + l²) / 12,  m = ρ w l d
    //   → dI_zz/dd = ρ w l (w² + l²) / 12 (exact, since z-translation of the
    //     centroid does not affect I_zz).
    let (w, l, d0) = (4.0, 3.0, 2.0);
    let build = |d: f64| -> BRepSolid {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), w, l);
        vcad_kernel_sketch::extrude(&profile, Vec3::z() * d).expect("extrude")
    };
    let base = build(d0);
    let plan = capture_plan(&base, &TessellationParams::default()).expect("capture");

    let mut seeding = ParamSeeding::new();
    let n = seeding.seed_where(
        &base.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, d0)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1);

    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam");
    let (props, dprops) = mass_properties_with_derivative(&seam, RHO);

    // Values.
    let m = RHO * w * l * d0;
    assert!((props.mass - m).abs() / m < 1e-12);
    let izz = m * (w * w + l * l) / 12.0;
    assert!((props.inertia_centroid[2][2] - izz).abs() / izz < 1e-12);

    // Analytic derivative gate.
    let dizz_exact = RHO * w * l * (w * w + l * l) / 12.0;
    let rel = (dprops.inertia_centroid[2][2] - dizz_exact).abs() / dizz_exact;
    assert!(
        rel <= GATE,
        "dIzz/dd = {} vs closed form {dizz_exact} (rel {rel:.3e})",
        dprops.inertia_centroid[2][2]
    );
    assert!((dprops.mass - RHO * w * l).abs() / (RHO * w * l) <= GATE);
    // Centroid rises at half the extrude rate.
    assert!((dprops.centroid.z - 0.5).abs() <= GATE);

    // FD oracle gate on every reported derivative field.
    let plus = evaluate_plan(&build(d0 + H), &plan).expect("plus");
    let minus = evaluate_plan(&build(d0 - H), &plan).expect("minus");
    let fp = mass_properties(&plus.positions, &plus.triangles, RHO);
    let fm = mass_properties(&minus.positions, &minus.triangles, RHO);
    let fd_check = |dual: f64, plus: f64, minus: f64, what: &str| {
        let fd = (plus - minus) / (2.0 * H);
        let rel = (dual - fd).abs() / fd.abs().max(1.0);
        assert!(
            rel <= GATE,
            "{what}: dual {dual} vs fd {fd} (rel {rel:.3e})"
        );
    };
    fd_check(dprops.mass, fp.mass, fm.mass, "dm/dd");
    fd_check(dprops.centroid.z, fp.centroid.z, fm.centroid.z, "dcz/dd");
    for i in 0..3 {
        fd_check(
            dprops.inertia_centroid[i][i],
            fp.inertia_centroid[i][i],
            fm.inertia_centroid[i][i],
            "dI/dd",
        );
    }
}

#[test]
fn cylinder_radius_inertia_derivatives_match_fd() {
    // All mass-property derivatives for θ = cylinder radius, FD-gated.
    let r0 = 5.0;
    let build = |r: f64| make_cylinder(r, 8.0, 32);
    let params = TessellationParams {
        circle_segments: 32,
        height_segments: 3,
        ..Default::default()
    };
    let base = build(r0);
    let plan = capture_plan(&base, &params).expect("capture");
    let mut seeding = ParamSeeding::new();
    assert_eq!(
        seeding.seed_where(
            &base.geometry,
            |s| s.as_any().downcast_ref::<CylinderSurface>().is_some(),
            SurfaceSeed::CylinderRadius { rate: 1.0 },
        ),
        1
    );
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam");
    let (_, dprops) = mass_properties_with_derivative(&seam, RHO);

    let plus = evaluate_plan(&build(r0 + H), &plan).expect("plus");
    let minus = evaluate_plan(&build(r0 - H), &plan).expect("minus");
    let fp = mass_properties(&plus.positions, &plus.triangles, RHO);
    let fm = mass_properties(&minus.positions, &minus.triangles, RHO);

    let fd_check = |dual: f64, plus: f64, minus: f64, what: &str| {
        let fd = (plus - minus) / (2.0 * H);
        let rel = (dual - fd).abs() / fd.abs().max(1.0);
        assert!(
            rel <= GATE,
            "{what}: dual {dual} vs fd {fd} (rel {rel:.3e})"
        );
    };
    fd_check(dprops.mass, fp.mass, fm.mass, "dm/dr");
    for i in 0..3 {
        fd_check(
            dprops.inertia_centroid[i][i],
            fp.inertia_centroid[i][i],
            fm.inertia_centroid[i][i],
            "dI/dr",
        );
    }
}

#[test]
fn contraction_hook_reproduces_dual_volume_derivative() {
    // The physics hook: dJ/dθ = Σ (∂J/∂x_i)·(dx_i/dθ). With J = volume and
    // its analytic per-node gradient, the contraction must reproduce the
    // dual-number derivative to machine precision — this is the exact
    // interface a phyz functional's ∂J/∂x plugs into.
    let r0 = 5.0;
    let build = |r: f64| make_cylinder(r, 8.0, 32);
    let params = TessellationParams {
        circle_segments: 32,
        height_segments: 3,
        ..Default::default()
    };
    let base = build(r0);
    let plan = capture_plan(&base, &params).expect("capture");
    let mut seeding = ParamSeeding::new();
    seeding.seed_where(
        &base.geometry,
        |s| s.as_any().downcast_ref::<CylinderSurface>().is_some(),
        SurfaceSeed::CylinderRadius { rate: 1.0 },
    );
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam");

    let (_, dv_dual) = volume_with_derivative(&seam);
    let dj_dx = volume_gradient(&seam.positions, &seam.triangles);
    let dv_contracted = contract_sensitivity(&seam, &dj_dx);
    assert!(
        (dv_dual - dv_contracted).abs() / dv_dual.abs() < 1e-12,
        "dual {dv_dual} vs contracted {dv_contracted}"
    );
}
