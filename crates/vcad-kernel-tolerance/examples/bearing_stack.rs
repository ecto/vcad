//! A realistic shaft/bearing/housing axial stackup, analyzed three ways.
//!
//! The assembly: a gearbox input shaft rides in two 6205-pattern ball
//! bearings inside a housing bore; a machined shoulder and a ground
//! spacer set the inner-race spacing, and a circlip closes the stack.
//! The gap of interest is the **axial play** left after assembly:
//!
//! ```text
//! play = housing_depth − bearing_A − shoulder − spacer − bearing_B − circlip
//! ```
//!
//! Requirement: 0.05 mm ≤ play ≤ 0.75 mm (never jammed, never rattling).
//!
//! Distributions are deliberately mixed, the way real BOMs are:
//! machined dimensions carry ISO 2768-m general tolerances as centered
//! normals (±tol = 3σ); purchased bearing widths are unilateral
//! (+0/−0.12, the normal-class width tolerance pattern for this size —
//! cf. ISO 492) and modeled uniform across the vendor band; the spacer
//! comes from two suppliers with distinct grinding setups — a two-point
//! mix inside the drawing band.
//!
//! Run: `cargo run -p vcad-kernel-tolerance --example bearing_stack`

use vcad_kernel_tolerance::analysis::{monte_carlo, rss, worst_case, McOptions};
use vcad_kernel_tolerance::dist::{Distribution, SigmaConvention};
use vcad_kernel_tolerance::sensitivity::sensitivities;
use vcad_kernel_tolerance::stackup::{iso2768, Contributor, Iso2768Class, Requirement, Stackup};

fn build() -> Stackup {
    let conv = SigmaConvention::ThreeSigma;
    let t_housing = iso2768(62.0, Iso2768Class::M).expect("in table"); // ±0.3
    let t_shoulder = iso2768(18.0, Iso2768Class::M).expect("in table"); // ±0.2
    Stackup {
        name: "input-shaft axial stack".into(),
        contributors: vec![
            Contributor::normal("housing bore depth", 1.0, 62.0, t_housing, conv),
            Contributor::uniform("bearing A width", -1.0, 15.0, 0.12, 0.0),
            Contributor::normal("shaft shoulder", -1.0, 18.0, t_shoulder, conv),
            Contributor::with_dist(
                "spacer (2 suppliers)",
                -1.0,
                12.0,
                0.10,
                0.10,
                Distribution::TwoPoint {
                    a: -0.03, // supplier A grinds low
                    b: 0.04,  // supplier B grinds high
                    p_b: 0.4, // 40% of stock is supplier B
                },
            ),
            Contributor::uniform("bearing B width", -1.0, 15.0, 0.12, 0.0),
            Contributor::normal("circlip thickness", -1.0, 1.6, 0.06, conv),
        ],
        requirement: Requirement::between("axial play", 0.05, 0.75),
    }
}

fn main() {
    let s = build();
    s.validate().expect("valid stackup");

    println!("== {} ==", s.name);
    println!(
        "requirement: {} ∈ [{:.2}, {:.2}] mm\n",
        s.requirement.name,
        s.requirement.lower_mm.unwrap(),
        s.requirement.upper_mm.unwrap()
    );

    println!("contributors (mm):");
    println!(
        "  {:<22} {:>6} {:>9} {:>8} {:>8}  distribution",
        "name", "coeff", "nominal", "tol−", "tol+"
    );
    for c in &s.contributors {
        println!(
            "  {:<22} {:>6.1} {:>9.3} {:>8.3} {:>8.3}  {:?}",
            c.name, c.coeff, c.nominal, c.tol_minus, c.tol_plus, c.dist
        );
    }

    // Worst case: every part at its worst drawing limit simultaneously.
    let wc = worst_case(&s).unwrap();
    println!("\n-- worst case (interval over drawing limits) --");
    println!("  gap ∈ [{:.3}, {:.3}] mm", wc.min_gap, wc.max_gap);
    println!(
        "  margin to lower = {:.3} mm, to upper = {:.3} mm → {}",
        wc.margin_lower.unwrap(),
        wc.margin_upper.unwrap(),
        if wc.passes { "PASSES" } else { "FAILS" }
    );

    // RSS: exact moments, Φ-based yield.
    let r = rss(&s).unwrap();
    println!("\n-- RSS (linear variance propagation) --");
    println!(
        "  gap: μ = {:.4} mm, σ = {:.4} mm (moments exact under independence)",
        r.mean_gap, r.sigma_gap
    );
    println!(
        "  Cp = {:.3}, Cpk = {:.3}, predicted yield = {:.4}%{}",
        r.cp.unwrap(),
        r.cpk.unwrap(),
        100.0 * r.yield_estimate,
        if r.all_normal {
            ""
        } else {
            "  (yield via CLT: chain has non-normal contributors)"
        }
    );

    // Monte Carlo: the check on the CLT step, with error bars.
    let mc = monte_carlo(
        &s,
        &McOptions {
            n: 200_000,
            seed: 0xBEA_121C,
            batches: 16,
        },
    )
    .unwrap();
    println!("\n-- Monte Carlo (n = {}, seed = {:#x}) --", mc.n, mc.seed);
    println!(
        "  gap: μ = {:.4} ± {:.4} mm, σ = {:.4} ± {:.4} mm",
        mc.mean_gap, mc.mean_gap_se, mc.sigma_gap, mc.sigma_gap_se
    );
    println!(
        "  fit probability = {:.4} ± {:.4}  ({} of {} fit; batch-SE {:.4})",
        mc.fit.p, mc.fit.standard_error, mc.fit.successes, mc.fit.n, mc.fit_se_batch
    );
    println!(
        "  sampled gap range: [{:.3}, {:.3}] mm (WC bounds [{:.3}, {:.3}])",
        mc.min_sample, mc.max_sample, wc.min_gap, wc.max_gap
    );

    // Sensitivities: which dimension is killing the yield.
    let rows = sensitivities(&s).unwrap();
    println!("\n-- exact sensitivities, ranked by variance share --");
    println!(
        "  {:<22} {:>7} {:>8} {:>10} {:>12} {:>12}",
        "name", "∂G/∂nom", "σᵢ", "var share", "∂Y/∂nom", "∂Y/∂σᵢ"
    );
    for row in &rows {
        println!(
            "  {:<22} {:>7.1} {:>8.4} {:>9.1}% {:>12.4} {:>12.4}",
            row.name,
            row.d_gap_d_nominal,
            row.sigma,
            100.0 * row.variance_share,
            row.d_yield_d_nominal,
            row.d_yield_d_sigma
        );
    }

    // The compass in action: the yield gradient says the mean sits too
    // high (unilateral bearing bands push the play up). Re-center by
    // lengthening the spacer to put μ at the middle of the requirement.
    let mid = 0.5 * (s.requirement.lower_mm.unwrap() + s.requirement.upper_mm.unwrap());
    let shift = r.mean_gap - mid; // spacer has coeff −1: grow it by this
    let mut recentered = s.clone();
    recentered.contributors[3].nominal += shift;
    let r2 = rss(&recentered).unwrap();
    println!(
        "\n-- re-centering move (from ∂Y/∂nominal signs) --\n  \
         spacer {:.3} → {:.3} mm puts μ at {:.3}; yield {:.2}% → {:.2}%",
        s.contributors[3].nominal,
        recentered.contributors[3].nominal,
        r2.mean_gap,
        100.0 * r.yield_estimate,
        100.0 * r2.yield_estimate
    );
    println!(
        "  (allocation — tightening the right σ's to hit a yield target \
         at minimum cost — is milestone M2)"
    );
}
