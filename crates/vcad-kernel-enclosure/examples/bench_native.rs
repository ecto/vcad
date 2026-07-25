//! Native timing probe for the GWN voxel pass (not a correctness test).
//!
//! Run with `cargo run --release -p vcad-kernel-enclosure --example bench_native`.
//! Compare against the same fixture through the WASM bridge to see how much of
//! the cost is `atan2` (WASM has no hardware transcendentals).

use std::time::Instant;
use vcad_kernel_enclosure::extract_enclosure_features;

fn boxm(min: [f64; 3], max: [f64; 3]) -> (Vec<f64>, Vec<u32>) {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    let v = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    let f: [u32; 36] = [
        0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6, 0, 4, 5, 0, 5, 1, 3, 2, 6, 3, 6, 7, 0, 3, 7, 0, 7, 4,
        1, 5, 6, 1, 6, 2,
    ];
    (v.concat(), f.to_vec())
}

fn main() {
    for detail in [0usize, 20, 60, 140] {
        let (w, d, h, t, fz) = (40.0, 40.0, 12.0, 2.0, 2.0);
        let post = |cx: f64, cy: f64| boxm([cx - 1.5, cy - 1.5, fz], [cx + 1.5, cy + 1.5, 5.0]);
        let c = 20.0;
        let half = 30.5 / 2.0;
        let mut parts = vec![
            boxm([0.0, 0.0, 0.0], [w, d, fz]),
            boxm([0.0, 0.0, fz], [t, d, h]),
            boxm([w - t, 0.0, fz], [w, 15.0, h]),
            boxm([w - t, 25.0, fz], [w, d, h]),
            boxm([t, 0.0, fz], [w - t, t, h]),
            boxm([t, d - t, fz], [w - t, d, h]),
            post(c - half, c - half),
            post(c + half, c - half),
            post(c - half, c + half),
            post(c + half, c + half),
        ];
        for i in 0..detail {
            let a = (i as f64 / detail as f64) * std::f64::consts::TAU;
            parts.push(post(20.0 + 8.0 * a.cos(), 20.0 + 8.0 * a.sin()));
        }
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        let mut off = 0u32;
        for (p, idx) in &parts {
            positions.extend_from_slice(p);
            indices.extend(idx.iter().map(|i| i + off));
            off += (p.len() / 3) as u32;
        }
        let tris = indices.len() / 3;
        let reps = if tris > 1000 { 3 } else { 10 };
        let t0 = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(extract_enclosure_features(&positions, &indices));
        }
        println!(
            "{:5} tris  native Rust {:8.1} ms",
            tris,
            t0.elapsed().as_secs_f64() * 1000.0 / reps as f64
        );
    }
}
