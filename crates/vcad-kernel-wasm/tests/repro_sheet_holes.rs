//! Regression tests for sheet-metal panel tessellation: hole loops must be
//! cut into the top/bottom caps and walled, and every face must wind
//! outward so the mesh is watertight and its signed-volume integral is
//! trustworthy. Field repro: a 64-gon D58 disc flange (t = 2.7) with a
//! 16-gon bore and four 12-gon BCD holes measured ~2374 mm³ (≈⅓ of truth)
//! with 256 unpaired directed edges — the caps ignored the holes and the
//! side quads were wound inside-out.

use std::collections::HashMap;
use std::f64::consts::PI;

fn ring(cx: f64, cy: f64, r: f64, n: usize, ccw: bool) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect();
    if !ccw {
        pts.reverse();
    }
    pts
}

/// Area of a regular n-gon inscribed in radius r.
fn ngon_area(r: f64, n: usize) -> f64 {
    0.5 * (n as f64) * r * r * (2.0 * PI / (n as f64)).sin()
}

struct MeshCheck {
    signed_volume_mm3: f64,
    open_directed_edges: i64,
}

/// Evaluate a base-flange-polygon chain and integrate its mesh.
fn evaluate_flange(outline: Vec<[f64; 2]>, holes: Vec<Vec<[f64; 2]>>, thickness: f64) -> MeshCheck {
    let chain = serde_json::json!([{
        "type": "BaseFlangePolygon",
        "outline": outline,
        "holes": holes,
        "thickness": thickness,
        "material": "steel"
    }]);
    let out = vcad_kernel_wasm::sheet_metal::evaluate_sheet_metal_chain(&chain.to_string());
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["error"].is_null(), "kernel error: {}", v["error"]);
    let positions: Vec<f64> = v["mesh"]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let indices: Vec<usize> = v["mesh"]["indices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as usize)
        .collect();

    let p = |i: usize| [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]];
    let mut vol = 0.0;
    for t in indices.chunks(3) {
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        vol += (a[0] * (b[1] * c[2] - c[1] * b[2]) - b[0] * (a[1] * c[2] - c[1] * a[2])
            + c[0] * (a[1] * b[2] - b[1] * a[2]))
            / 6.0;
    }

    // Unpaired directed edges, vertices matched by quantized position (the
    // walls duplicate cap vertices) — mirrors the MCP integrity check.
    let key = |i: usize| {
        let q = 1e-5;
        (
            (positions[i * 3] / q).round() as i64,
            (positions[i * 3 + 1] / q).round() as i64,
            (positions[i * 3 + 2] / q).round() as i64,
        )
    };
    let mut net: HashMap<((i64, i64, i64), (i64, i64, i64)), i64> = HashMap::new();
    for t in indices.chunks(3) {
        let ks = [key(t[0]), key(t[1]), key(t[2])];
        for e in 0..3 {
            let (a, b) = (ks[e], ks[(e + 1) % 3]);
            if a == b {
                continue;
            }
            if a < b {
                *net.entry((a, b)).or_default() += 1;
            } else {
                *net.entry((b, a)).or_default() -= 1;
            }
        }
    }
    let open: i64 = net.values().map(|c| c.abs()).sum();

    MeshCheck {
        signed_volume_mm3: vol,
        open_directed_edges: open,
    }
}

fn assert_solid(check: &MeshCheck, analytic_mm3: f64) {
    assert_eq!(
        check.open_directed_edges, 0,
        "mesh is not watertight ({} unpaired directed edges)",
        check.open_directed_edges
    );
    assert!(
        check.signed_volume_mm3 > 0.0,
        "signed volume is not positive: {} mm³ (inverted winding)",
        check.signed_volume_mm3
    );
    let err = (check.signed_volume_mm3 - analytic_mm3).abs();
    assert!(
        err <= analytic_mm3 * 0.01,
        "volume {} mm³ deviates from analytic {} mm³ by more than 1%",
        check.signed_volume_mm3,
        analytic_mm3
    );
}

/// Field repro: 64-gon circular flange D58, t = 2.7, one 16-gon bore D8.4
/// and four 12-gon D3.3 holes on a 22 mm BCD.
#[test]
fn disc_flange_with_bore_and_bcd_holes_is_watertight() {
    let outline = ring(0.0, 0.0, 29.0, 64, true);
    let mut holes = vec![ring(0.0, 0.0, 4.2, 16, false)];
    for k in 0..4 {
        let a = 2.0 * PI * (k as f64) / 4.0;
        holes.push(ring(11.0 * a.cos(), 11.0 * a.sin(), 1.65, 12, false));
    }
    let analytic = (ngon_area(29.0, 64) - ngon_area(4.2, 16) - 4.0 * ngon_area(1.65, 12)) * 2.7;
    let check = evaluate_flange(outline, holes, 2.7);
    assert_solid(&check, analytic);
}

/// Minimal case: a rectangular flange with one rectangular hole.
#[test]
fn square_flange_with_single_hole_is_watertight() {
    let outline = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]];
    // CW hole, matching the documented hole-winding convention.
    let hole = vec![[5.0, 3.0], [5.0, 7.0], [8.0, 7.0], [8.0, 3.0]];
    let analytic = (20.0 * 10.0 - 3.0 * 4.0) * 1.0;
    let check = evaluate_flange(outline, vec![hole], 1.0);
    assert_solid(&check, analytic);
}

/// Concave outline (L-bracket): the old fan triangulation filled the
/// notch; earcut must not.
#[test]
fn concave_outline_without_holes_is_watertight() {
    let outline = vec![
        [0.0, 0.0],
        [40.0, 0.0],
        [40.0, 10.0],
        [10.0, 10.0],
        [10.0, 40.0],
        [0.0, 40.0],
    ];
    let analytic = (40.0 * 10.0 + 10.0 * 30.0) * 1.0;
    let check = evaluate_flange(outline, Vec::new(), 1.0);
    assert_solid(&check, analytic);
}

/// Mis-wound input (CW outline, CCW hole) must tessellate identically —
/// the mesher normalizes ring windings rather than trusting the caller.
#[test]
fn miswound_rings_are_normalized() {
    let outline = ring(0.0, 0.0, 29.0, 64, false);
    let holes = vec![ring(0.0, 0.0, 4.2, 16, true)];
    let analytic = (ngon_area(29.0, 64) - ngon_area(4.2, 16)) * 2.7;
    let check = evaluate_flange(outline, holes, 2.7);
    assert_solid(&check, analytic);
}
