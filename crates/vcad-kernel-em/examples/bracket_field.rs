//! Multiphysics demo, EM layer: the aluminum L-bracket's base plate doubles
//! as the controller's 200 A DC busbar. Planar magnetostatics on its YZ
//! cross-section (48×12 mm, current along +x) → |B| in mT on a 2D grid
//! with 12 mm of air around it; A = 0 on the far boundary.
//!
//! Honesty box: planar = infinitely long bar (per-meter field); the
//! flange, bolt end and return conductor are not modeled. The field is
//! drawn on the plate's mid-length cross-section (x = 48 mm).
//!
//! Writes `OUT.f32` (row-major (z, y) = 1×nz×ny, mT) and `OUT.json` with
//! `"plane":"yz"` (image columns → world y, rows → world z).
//! Run: `cargo run --release -p vcad-kernel-em --example bracket_field -- OUT`

use vcad_kernel_em::grid::SolveOptions;
use vcad_kernel_em::planar::{Conductor, PlanarMagnetostatics, Rect};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bracket_busbar".into());
    // Planar (u, v) = world (y, z). Domain y −12..60, z −12..24 mm.
    let (u0, u1, v0, v1) = (-12.0, 60.0, -12.0, 24.0);
    let mut dev = PlanarMagnetostatics::new(u0, u1, v0, v1);
    dev.conductors.push(Conductor {
        region: Rect {
            x_min_mm: 0.0,
            x_max_mm: 48.0,
            y_min_mm: 0.0,
            y_max_mm: 12.0,
        },
        total_current_a: 200.0,
    });
    // 0.25 mm node pitch.
    let (nu, nv) = (289usize, 145usize);
    let s = dev.solve(nu, nv, &SolveOptions::default()).expect("solve");
    // Sample |B| at cell centers of a 288×144 image.
    let (cu, cv) = (nu - 1, nv - 1);
    let (du, dv) = ((u1 - u0) / cu as f64, (v1 - v0) / cv as f64);
    let mut img = Vec::with_capacity(cu * cv);
    for j in 0..cv {
        for i in 0..cu {
            let (u, v) = (u0 + (i as f64 + 0.5) * du, v0 + (j as f64 + 0.5) * dv);
            let (bx, by) = s.b_at(u * 1e-3, v * 1e-3);
            img.push(((bx * bx + by * by).sqrt() * 1e3) as f32);
        }
    }
    let (lo, hi) = img
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    std::fs::write(
        format!("{out}.f32"),
        img.iter()
            .flat_map(|x| x.to_le_bytes())
            .collect::<Vec<u8>>(),
    )
    .unwrap();
    let meta = format!(
        "{{\"field\":\"flux_density\",\"units\":\"mT\",\"source\":\"vcad-kernel-em planar busbar 200 A (per-meter, ends ignored)\",\
         \"plane\":\"yz\",\"dims_zyx\":[1,{cv},{cu}],\"origin_mm\":[48,{u0},{v0}],\"voxel_mm\":[0.75,{du},{dv}],\
         \"min\":{lo},\"max\":{hi},\"sor_residual\":{:e}}}\n",
        s.residual
    );
    std::fs::write(format!("{out}.json"), &meta).unwrap();
    print!("{meta}");
}
