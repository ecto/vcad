//! Gradients through CAD: gradient descent on a named `.vcad` parameter.
//!
//! An L-bracket authored in loon (`gradient_bracket.loon`) declares its wall
//! thickness `t` with `defparam`. Each iteration asks the differentiable seam
//! for `dm/dt` via [`document_parameter_gradient`] — an analytic seam
//! evaluation, not a finite difference — and takes a gradient step on
//!
//! ```text
//! J(t) = ½ (m(t) − m*)²,   dJ/dt = (m − m*) · dm/dt
//! ```
//!
//! A rebuild central finite difference of the mass is computed alongside
//! **only as a check** and logged next to the seam value; it never feeds the
//! optimizer.
//!
//! ```text
//! cargo run --release -p vcad-eval --example gradient_clip -- <out_dir>
//! ```
//!
//! Writes `<out_dir>/frame_NNN.vcad` (one document per iteration, geometry
//! baked at that iteration's `t`, ready for `vcad-render`) and
//! `<out_dir>/log.csv`.

use std::fmt::Write as _;
use std::path::PathBuf;

use vcad_eval::diff::document_parameter_gradient;
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{Document, Expr};
use vcad_kernel_diff::{evaluate_with_sensitivity, mass_properties, ParamSeeding};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

const SOURCE: &str = include_str!("gradient_bracket.loon");
/// Aluminium, g/mm³ — masses come out in grams.
const DENSITY: f64 = 2.70e-3;
/// Target mass, grams.
const TARGET: f64 = 55.0;
/// Step size on dJ/dt (mm per g²·mm⁻¹).
const ALPHA: f64 = 0.0015;
const ITERS: usize = 30;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

fn with_t(doc: &Document, t: f64) -> Document {
    let mut d = doc.clone();
    d.parameters.get_mut("t").expect("defparam t").value = Expr::Number(t);
    d
}

/// Rebuild-and-measure mass (the FD check's oracle).
fn rebuilt_mass(doc: &Document, t: f64) -> f64 {
    let scene = evaluate_document(&with_t(doc, t), &EvalOptions::default()).expect("evaluate");
    let brep = scene.parts[0]
        .solid
        .as_ref()
        .and_then(|s| s.as_brep())
        .expect("solid part");
    let plan = capture_plan(brep, &tess()).expect("capture");
    let seam = evaluate_with_sensitivity(brep, &plan, &ParamSeeding::new()).expect("seam");
    mass_properties(&seam.positions, &seam.triangles, DENSITY).mass
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "gradient_clip".into()),
    );
    std::fs::create_dir_all(&out).expect("create out dir");

    let doc = vcad_loon::eval_vcad(SOURCE, None).expect("loon eval");
    let mut t = 9.0_f64;
    let mut csv = String::from("iter,t_mm,mass_g,objective,dm_dt_seam,dm_dt_fd,fd_rel_err,dJ_dt\n");
    let mut worst_rel = 0.0_f64;

    for k in 0..ITERS {
        let g = document_parameter_gradient(&with_t(&doc, t), "t", DENSITY, &tess(), 1e-4)
            .expect("seam gradient");
        let m = g[0].properties.mass;
        let dm_dt = g[0].derivative.mass;
        let j = 0.5 * (m - TARGET).powi(2);
        let dj_dt = (m - TARGET) * dm_dt;

        // Check only: rebuild central FD of the mass.
        let h = 1e-4;
        let fd = (rebuilt_mass(&doc, t + h) - rebuilt_mass(&doc, t - h)) / (2.0 * h);
        let rel = (dm_dt - fd).abs() / fd.abs();
        worst_rel = worst_rel.max(rel);

        // Geometry baked at this t, so any renderer shows exactly this frame.
        let baked = vcad_loon::eval_vcad(
            &SOURCE.replace("[defparam t 9.0]", &format!("[defparam t {t:.9}]")),
            None,
        )
        .expect("bake frame");
        let json = serde_json::to_string_pretty(&baked).expect("serialize");
        std::fs::write(out.join(format!("frame_{k:03}.vcad")), json).expect("write frame");

        writeln!(
            csv,
            "{k},{t:.6},{m:.6},{j:.6},{dm_dt:.9},{fd:.9},{rel:.3e},{dj_dt:.9}"
        )
        .unwrap();
        println!(
            "iter {k:2}  t={t:.4} mm  m={m:.3} g  J={j:.4}  dm/dt seam={dm_dt:.6}  fd={fd:.6}  rel={rel:.2e}"
        );

        t -= ALPHA * dj_dt;
    }

    std::fs::write(out.join("log.csv"), csv).expect("write log");
    println!("worst seam-vs-FD relative error: {worst_rel:.3e}");
}
