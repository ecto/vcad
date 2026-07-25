//! Diagnostic: how fast does the two-plane image series actually converge, for
//! a single loop versus a magnetically balanced alternating-pole set?

use std::f64::consts::PI;
use vcad_kernel_magnetostatic::{Filament, IronStack, Vec3};

fn ring(r: f64, z0: f64, i: f64, n: usize, center: Vec3) -> Filament {
    let pts = (0..n)
        .map(|k| {
            let t = 2.0 * PI * (k as f64) / (n as f64);
            Vec3::new(center.x + r * t.cos(), center.y + r * t.sin(), z0)
        })
        .collect();
    Filament::closed_loop(pts, i, 0.0)
}

fn b_sum(stack: &IronStack, srcs: &[Filament], p: Vec3) -> Vec3 {
    srcs.iter()
        .flat_map(|s| stack.expand(s))
        .map(|f| f.b_at(p))
        .sum()
}

fn main() {
    let z_lo = 0.0;
    let z_hi = 0.0056;
    let probe = Vec3::new(0.018, 0.0, 0.0045);

    // Case 1: one loop — net magnetic moment, nothing to cancel at distance.
    let single = vec![ring(0.0075, 0.003, 100.0, 256, Vec3::ZERO)];

    // Case 2: six alternating poles on a 22.5 mm pitch circle — a real rotor.
    let mut alternating = Vec::new();
    for k in 0..6 {
        let t = 2.0 * PI * (k as f64) / 6.0;
        let c = Vec3::new(0.0225 * t.cos(), 0.0225 * t.sin(), 0.0);
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        alternating.push(ring(0.0075, 0.003, 100.0 * sign, 256, c));
    }
    let probe_alt = Vec3::new(0.0225, 0.0, 0.0045);

    for (label, srcs, p) in [
        ("single loop (net moment)", &single, probe),
        ("6 alternating poles", &alternating, probe_alt),
    ] {
        println!("\n=== {label} ===");
        let mut prev: Option<Vec3> = None;
        for depth in [2usize, 4, 8, 12, 16, 24, 32] {
            let stack = IronStack::pair(z_lo, z_hi, depth);
            let n_images = stack.expand(&srcs[0]).len();
            let b = b_sum(&stack, srcs, p);
            let delta = prev
                .map(|q: Vec3| (b - q).norm() / b.norm().max(1e-18))
                .unwrap_or(f64::NAN);
            println!(
                "depth {depth:>3}  images/src {n_images:>4}  |B| {:>12.6e} T   Δ vs prev {:.3e}",
                b.norm(),
                delta
            );
            prev = Some(b);
        }
    }
}
