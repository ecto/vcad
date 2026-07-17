//! The WC-vs-RSS-vs-MC comparison study (M5), plus the allocation
//! finale on the bearing stack. The numbers printed here feed
//! `docs/tolerance-paper-draft.md`.
//!
//! Run: `cargo run -p vcad-kernel-tolerance --example comparison`

use vcad_kernel_tolerance::allocate::{allocate, AllocationVar, CostModel};
use vcad_kernel_tolerance::analysis::{monte_carlo, rss, worst_case, McOptions};
use vcad_kernel_tolerance::dist::{Distribution, SigmaConvention};
use vcad_kernel_tolerance::stackup::{Contributor, Requirement, Stackup};

/// n equal ±0.1 normal contributors consuming from an opening sized so
/// the nominal gap is 1.0; requirement 1.0 ± 0.3.
fn equal_chain(n: usize) -> Stackup {
    let t = 0.1;
    let mut contributors = vec![Contributor::with_dist(
        "opening",
        1.0,
        n as f64 + 1.0,
        0.0,
        0.0,
        Distribution::Normal {
            mean: 0.0,
            sigma: 0.0,
        },
    )];
    for i in 0..n {
        contributors.push(Contributor::normal(
            &format!("dim{i}"),
            -1.0,
            1.0,
            t,
            SigmaConvention::ThreeSigma,
        ));
    }
    Stackup {
        name: format!("equal-{n}"),
        contributors,
        requirement: Requirement::between("gap", 0.69, 1.31),
    }
}

fn main() {
    println!("== study 1: WC vs RSS vs MC over chain length ==");
    println!("(n equal ±0.1 contributors, requirement gap = 1.0 ± 0.31)\n");
    println!(
        "{:>3} {:>12} {:>9} {:>8} {:>10} {:>20} {:>7}",
        "n", "WC interval", "WC pass", "σ_G", "RSS yield", "MC fit ± SE", "√n"
    );
    for n in 2..=10 {
        let s = equal_chain(n);
        let wc = worst_case(&s).unwrap();
        let r = rss(&s).unwrap();
        let mc = monte_carlo(
            &s,
            &McOptions {
                n: 100_000,
                seed: 1000 + n as u64,
                batches: 16,
            },
        )
        .unwrap();
        let ratio = 0.5 * (wc.max_gap - wc.min_gap) / (3.0 * r.sigma_gap);
        println!(
            "{:>3} [{:>4.2},{:>4.2}] {:>9} {:>8.4} {:>10.6} {:>13.5} ± {:.5} {:>7.3}",
            n,
            wc.min_gap,
            wc.max_gap,
            if wc.passes { "yes" } else { "NO" },
            r.sigma_gap,
            r.yield_estimate,
            mc.fit.p,
            mc.fit.standard_error,
            ratio
        );
    }
    println!(
        "\nThe WC/RSS width ratio IS √n (asserted to 1e-12 in \
         tests/benchmarks.rs): worst-case\nanalysis prices the ±0.31 \
         requirement as unbuildable from n = 4 on, while the actual\n\
         yield never drops below 99.5%. That gap is the entire economic \
         argument for\nstatistical tolerancing."
    );

    // ── Study 2: the bearing stack, re-centered, then allocated. ──
    println!("\n== study 2: allocation finale on the bearing stack ==");
    let conv = SigmaConvention::ThreeSigma;
    let s = Stackup {
        name: "input-shaft axial stack (re-centered)".into(),
        contributors: vec![
            Contributor::normal("housing bore depth", 1.0, 62.0, 0.3, conv),
            Contributor::uniform("bearing A width", -1.0, 15.0, 0.12, 0.0),
            Contributor::normal("shaft shoulder", -1.0, 18.0, 0.2, conv),
            Contributor::with_dist(
                "spacer (2 suppliers)",
                -1.0,
                12.122, // the M0 re-centering move
                0.10,
                0.10,
                Distribution::TwoPoint {
                    a: -0.03,
                    b: 0.04,
                    p_b: 0.4,
                },
            ),
            Contributor::uniform("bearing B width", -1.0, 15.0, 0.12, 0.0),
            Contributor::normal("circlip thickness", -1.0, 1.6, 0.06, conv),
        ],
        requirement: Requirement::between("axial play", 0.05, 0.75),
    };
    let r0 = rss(&s).unwrap();
    println!(
        "re-centered baseline: μ = {:.4}, σ = {:.4}, yield = {:.4}%",
        r0.mean_gap,
        r0.sigma_gap,
        100.0 * r0.yield_estimate
    );

    // Allocate the two machined dims (the circlip is purchased; the
    // bearings and spacer are vendor parts — not ours to re-spec).
    let vars = vec![
        AllocationVar {
            contributor: "housing bore depth".into(),
            cost: CostModel::Reciprocal { a: 4.0, b: 2.0 },
            t_min: 0.05,
            t_max: 0.5,
        },
        AllocationVar {
            contributor: "shaft shoulder".into(),
            cost: CostModel::Reciprocal { a: 2.0, b: 0.8 },
            t_min: 0.03,
            t_max: 0.4,
        },
    ];
    let target = 0.9973;
    let a = allocate(&s, &vars, target).unwrap();
    println!(
        "\nallocated to yield ≥ {:.2}% (σ_max = {:.4} mm):",
        100.0 * target,
        a.sigma_max
    );
    println!(
        "  {:<22} {:>8} {:>8} {:>9}",
        "contributor", "t before", "t after", "cost/part"
    );
    let before = [0.3, 0.2];
    for ((name, t, cost), b) in a.tolerances.iter().zip(before) {
        println!("  {name:<22} {b:>8.3} {t:>8.3} {cost:>9.3}");
    }
    println!(
        "  total cost {:.3} vs proportional-scaling baseline {:.3} \
         (saving {:.1}%)",
        a.cost,
        a.cost_proportional_baseline,
        100.0 * (1.0 - a.cost / a.cost_proportional_baseline)
    );
    println!(
        "  allocated chain: σ = {:.4}, RSS yield = {:.4}%",
        a.sigma_gap,
        100.0 * a.predicted_yield
    );
    let mc = monte_carlo(
        &a.stackup,
        &McOptions {
            n: 200_000,
            seed: 0xA110C,
            batches: 16,
        },
    )
    .unwrap();
    println!(
        "  Monte Carlo check: fit = {:.4} ± {:.4}",
        mc.fit.p, mc.fit.standard_error
    );

    // The same target with strongly unequal cost scales: allocation's
    // edge over proportional scaling grows with cost asymmetry.
    let vars2 = vec![
        AllocationVar {
            contributor: "housing bore depth".into(),
            cost: CostModel::Reciprocal { a: 4.0, b: 6.0 },
            t_min: 0.05,
            t_max: 0.5,
        },
        AllocationVar {
            contributor: "shaft shoulder".into(),
            cost: CostModel::Reciprocal { a: 2.0, b: 0.2 },
            t_min: 0.03,
            t_max: 0.4,
        },
    ];
    let a2 = allocate(&s, &vars2, target).unwrap();
    println!(
        "\nwith 30× cost asymmetry (boring is dear, turning is cheap): \
         total {:.3} vs proportional {:.3} (saving {:.1}%)",
        a2.cost,
        a2.cost_proportional_baseline,
        100.0 * (1.0 - a2.cost / a2.cost_proportional_baseline)
    );
    for (name, t, _) in &a2.tolerances {
        println!("  {name:<22} t = {t:.3}");
    }
}
