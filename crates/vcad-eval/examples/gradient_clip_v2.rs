//! Gradients through CAD, v2: a nonlinear, two-parameter design problem.
//!
//! A cantilevered aluminum L-section beam (`gradient_cantilever.loon`, 150 mm
//! along +y, fixed at y = 0, point load P at the free end) declares wall
//! thickness `t` and flange height `h` with `defparam`. The optimizer minimizes
//! mass subject to a tip-deflection limit, via a quadratic exterior penalty:
//!
//! ```text
//! J(t, h) = m + λ · max(0, δ/δmax − 1)²
//! δ       = P L³ / (3 E I),     I = ∫ (z − z̄)² dA
//! ```
//!
//! δ is **not** linear in either parameter: `I` is cubic in `h` and couples
//! `t` and `h` through the moving neutral axis, and δ goes as `1/I`.
//!
//! Where the numbers come from:
//!
//! - `m`, `I`, and their derivatives `dm/dp`, `dI/dp` for `p ∈ {t, h}` come
//!   from [`document_parameter_gradient`]: exact seam derivatives of the
//!   mass-property integrals of the tessellated BRep, one call per parameter.
//!   `I` is read from the centroidal inertia tensor — for a prism along y,
//!   `∫(z − z̄)² dV = (I_xx + I_yy − I_zz) / 2ρ`, and dividing by `L` gives the
//!   area moment exactly.
//! - `dδ/dp = −(δ/I) · dI/dp` and `dJ/dp` follow by the chain rule.
//! - δ itself is Euler–Bernoulli beam theory on the seam's section moment
//!   (bending about the centroidal x-axis; the L-section's product of inertia
//!   is ignored, i.e. the beam is taken as laterally restrained). It is not a
//!   3D FEA solve.
//!
//! A central finite difference of `J` and of `δ`, each from two full document
//! rebuilds at `p ± ε`, is computed alongside **only as a check** and logged
//! next to the seam value. It never feeds the optimizer.
//!
//! The step is projected gradient descent with box bounds on `t` and `h`.
//! The penalty leaves δ slightly above δmax, so the run ends with a
//! feasibility projection: `t` held, Newton steps on `δ(h) = δmax` driven by
//! the seam's `dδ/dh` (logged with `phase = projection`, FD-checked like the
//! descent rows).
//!
//! ```text
//! cargo run --release -p vcad-eval --example gradient_clip_v2 -- <out_dir>
//! ```
//!
//! Writes `<out_dir>/frame_NNN.vcad` (one document per iteration, geometry
//! baked at that iteration's `t`, `h`, ready for `vcad-render`) and
//! `<out_dir>/log.csv`, plus `<out_dir>/landscape.csv` (J and δ on a (t, h)
//! grid, each point a document rebuild) for plotting the trajectory.

use std::fmt::Write as _;
use std::path::PathBuf;

use vcad_eval::diff::document_parameter_gradient;
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{Document, Expr};
use vcad_kernel_diff::{evaluate_with_sensitivity, mass_properties, ParamSeeding};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

const SOURCE: &str = include_str!("gradient_cantilever.loon");
/// Aluminium, g/mm³ — masses come out in grams.
const DENSITY: f64 = 2.70e-3;
/// Young's modulus, MPa (N/mm²).
const E: f64 = 69_000.0;
/// Tip load, N.
const P: f64 = 400.0;
/// Beam length along y, mm (must match the loon source).
const L: f64 = 150.0;
/// Tip deflection limit, mm.
const DELTA_MAX: f64 = 0.5;
/// Penalty weight, g.
const LAMBDA: f64 = 300.0;
/// Step sizes (mm per unit dJ/dp) for t and h.
const ALPHA_T: f64 = 0.004;
const ALPHA_H: f64 = 0.16;
const T_BOUNDS: (f64, f64) = (2.0, 12.0);
const H_MAX: f64 = 60.0;
const ITERS: usize = 100;
/// Newton feasibility-projection steps after the penalty descent.
const PROJ_STEPS: usize = 4;
/// Projection target, just inside the limit so the final design is
/// strictly feasible (the penalty alone leaves δ slightly above δmax).
const DELTA_TARGET: f64 = DELTA_MAX * 0.9995;
/// FD check step, mm.
const EPS: f64 = 1e-4;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

fn with_params(doc: &Document, t: f64, h: f64) -> Document {
    let mut d = doc.clone();
    d.parameters.get_mut("t").expect("defparam t").value = Expr::Number(t);
    d.parameters.get_mut("h").expect("defparam h").value = Expr::Number(h);
    d
}

/// Area second moment about the centroidal x-axis from a centroidal mass
/// inertia tensor: `(I_xx + I_yy − I_zz) / (2 ρ L)`.
fn area_moment(i: &[[f64; 3]; 3]) -> f64 {
    (i[0][0] + i[1][1] - i[2][2]) / (2.0 * DENSITY * L)
}

fn deflection(i_area: f64) -> f64 {
    P * L.powi(3) / (3.0 * E * i_area)
}

fn objective(m: f64, delta: f64) -> f64 {
    m + LAMBDA * (delta / DELTA_MAX - 1.0).max(0.0).powi(2)
}

/// Rebuild-and-measure `(J, δ)` — the FD check's oracle.
fn rebuilt(doc: &Document, t: f64, h: f64) -> (f64, f64) {
    let scene =
        evaluate_document(&with_params(doc, t, h), &EvalOptions::default()).expect("evaluate");
    let brep = scene.parts[0]
        .solid
        .as_ref()
        .and_then(|s| s.as_brep())
        .expect("solid part");
    let plan = capture_plan(brep, &tess()).expect("capture");
    let seam = evaluate_with_sensitivity(brep, &plan, &ParamSeeding::new()).expect("seam");
    let mp = mass_properties(&seam.positions, &seam.triangles, DENSITY);
    let delta = deflection(area_moment(&mp.inertia_centroid));
    (objective(mp.mass, delta), delta)
}

fn rel_err(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "gradient_clip_v2".into()),
    );
    std::fs::create_dir_all(&out).expect("create out dir");

    let doc = vcad_loon::eval_vcad(SOURCE, None).expect("loon eval");
    let (mut t, mut h) = (9.0_f64, 30.0_f64);
    let mut csv = String::from(
        "iter,t_mm,h_mm,mass_g,I_mm4,delta_mm,delta_max_mm,objective,\
         dJ_dt_seam,dJ_dt_fd,dJ_dt_rel_err,dJ_dh_seam,dJ_dh_fd,dJ_dh_rel_err,\
         ddelta_dt_seam,ddelta_dt_fd,ddelta_dt_rel_err,\
         ddelta_dh_seam,ddelta_dh_fd,ddelta_dh_rel_err,phase\n",
    );
    let mut worst = [0.0_f64; 4];

    for k in 0..ITERS + PROJ_STEPS {
        let projecting = k >= ITERS;
        let at = with_params(&doc, t, h);
        let gt = document_parameter_gradient(&at, "t", DENSITY, &tess(), 1e-4).expect("seam t");
        let gh = document_parameter_gradient(&at, "h", DENSITY, &tess(), 1e-4).expect("seam h");
        let props = &gt[0].properties;
        let m = props.mass;
        let i_area = area_moment(&props.inertia_centroid);
        let delta = deflection(i_area);
        let j = objective(m, delta);

        // Chain rule on seam outputs.
        let viol = (delta / DELTA_MAX - 1.0).max(0.0);
        let dj_ddelta = 2.0 * LAMBDA * viol / DELTA_MAX;
        let grad = |d: &vcad_kernel_diff::MassProperties<f64>| {
            let di = area_moment(&d.inertia_centroid);
            let ddelta = -delta / i_area * di;
            (d.mass + dj_ddelta * ddelta, ddelta)
        };
        let (dj_dt, dd_dt) = grad(&gt[0].derivative);
        let (dj_dh, dd_dh) = grad(&gh[0].derivative);

        // Check only: rebuild central FDs.
        let (jtp, dtp) = rebuilt(&doc, t + EPS, h);
        let (jtm, dtm) = rebuilt(&doc, t - EPS, h);
        let (jhp, dhp) = rebuilt(&doc, t, h + EPS);
        let (jhm, dhm) = rebuilt(&doc, t, h - EPS);
        let fd = [
            (jtp - jtm) / (2.0 * EPS),
            (jhp - jhm) / (2.0 * EPS),
            (dtp - dtm) / (2.0 * EPS),
            (dhp - dhm) / (2.0 * EPS),
        ];
        let seam = [dj_dt, dj_dh, dd_dt, dd_dh];
        let rel: Vec<f64> = seam.iter().zip(&fd).map(|(s, f)| rel_err(*s, *f)).collect();
        for (w, r) in worst.iter_mut().zip(&rel) {
            *w = w.max(*r);
        }

        // Geometry baked at this (t, h), so any renderer shows exactly this frame.
        let baked = vcad_loon::eval_vcad(
            &SOURCE
                .replace("[defparam t 9.0]", &format!("[defparam t {t:.9}]"))
                .replace("[defparam h 30.0]", &format!("[defparam h {h:.9}]")),
            None,
        )
        .expect("bake frame");
        let json = serde_json::to_string_pretty(&baked).expect("serialize");
        std::fs::write(out.join(format!("frame_{k:03}.vcad")), json).expect("write frame");

        writeln!(
            csv,
            "{k},{t:.6},{h:.6},{m:.6},{i_area:.6},{delta:.6},{DELTA_MAX},{j:.6},\
             {:.9},{:.9},{:.3e},{:.9},{:.9},{:.3e},{:.9e},{:.9e},{:.3e},{:.9e},{:.9e},{:.3e},{}",
            seam[0],
            fd[0],
            rel[0],
            seam[1],
            fd[1],
            rel[1],
            seam[2],
            fd[2],
            rel[2],
            seam[3],
            fd[3],
            rel[3],
            if projecting { "projection" } else { "descent" }
        )
        .unwrap();
        println!(
            "iter {k:2}  t={t:.4} h={h:.4}  m={m:.3} g  δ={delta:.4} mm  J={j:.4}  \
             dJ/dt={dj_dt:.5} (fd rel {:.1e})  dJ/dh={dj_dh:.5} (fd rel {:.1e})",
            rel[0], rel[1]
        );

        if projecting {
            // Feasibility projection: hold t, one Newton step on
            // δ(h) = δtarget using the seam's dδ/dh. δ is convex and
            // decreasing in h, so the iterates approach the root from the
            // infeasible side and converge quadratically.
            h = (h - (delta - DELTA_TARGET) / dd_dh).clamp(t + 5.0, H_MAX);
        } else {
            t = (t - ALPHA_T * dj_dt).clamp(T_BOUNDS.0, T_BOUNDS.1);
            h = (h - ALPHA_H * dj_dh).clamp(t + 5.0, H_MAX);
        }
    }

    std::fs::write(out.join("log.csv"), csv).expect("write log");

    // Landscape for the trajectory inset: J and δ on a (t, h) grid, each
    // point a full document rebuild (no optimizer involvement).
    let mut grid = String::from("t_mm,h_mm,objective,delta_mm\n");
    for i in 0..=32 {
        for j in 0..=32 {
            let gt = 1.5 + 8.5 * i as f64 / 32.0;
            let gh = 15.0 + 25.0 * j as f64 / 32.0;
            let (jv, dv) = rebuilt(&doc, gt, gh);
            writeln!(grid, "{gt:.6},{gh:.6},{jv:.6},{dv:.6}").unwrap();
        }
    }
    std::fs::write(out.join("landscape.csv"), grid).expect("write landscape");
    println!(
        "worst seam-vs-FD relative error: dJ/dt {:.3e}  dJ/dh {:.3e}  dδ/dt {:.3e}  dδ/dh {:.3e}",
        worst[0], worst[1], worst[2], worst[3]
    );
}
