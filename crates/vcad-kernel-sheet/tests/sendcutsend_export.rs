//! SendCutSend-compatibility tests for the flat-pattern DXF exporter.
//!
//! - Golden topology: the 16-bend "origami crane" (16-gon base, 4 chained
//!   flange sequences, 4 single flanges on angled edges, 2 interior holes)
//!   must export as ONE exterior polyline + hole polylines + dashed bend
//!   lines on the allowance midlines.
//! - Property: random valid flange trees always union to a single polygon
//!   whose area is Σ(panel areas) + Σ(allowance × hinge length).
//! - Regression: export is byte-for-byte deterministic, and the L-bracket
//!   matches its checked-in golden file exactly.

use std::f64::consts::{FRAC_PI_2, PI};
use vcad_kernel_math::Point2;
use vcad_kernel_sheet::{
    add_edge_flange, base_flange_polygon_with_holes, base_flange_rect,
    edge_flange::EdgeFlangeParams, flat_pattern_to_dxf, silhouette, unfold, BendDirection,
    BendTable, FlangePosition, FlatPattern, SheetMetalModel,
};

fn flange(panel: usize, edge: usize, length: f64) -> EdgeFlangeParams {
    EdgeFlangeParams {
        panel,
        edge_index: edge,
        length,
        angle: FRAC_PI_2,
        radius: 1.0,
        direction: BendDirection::Up,
        position: FlangePosition::MaterialInside,
        material: "Al-soft".into(),
        manual_k: Some(0.42),
    }
}

/// 16-gon base with 2 interior holes; 4 chained flange sequences of depth 3
/// (12 bends) off edges 0/4/8/12 and 4 single flanges on the angled edges
/// 2/6/10/14 — 16 bends, 17 panels.
fn origami_crane() -> SheetMetalModel {
    let n = 16usize;
    let r = 60.0;
    let outline: Vec<Point2> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            Point2::new(r * a.cos(), r * a.sin())
        })
        .collect();
    let hole = |cx: f64, cy: f64, hr: f64| -> Vec<Point2> {
        // CW circle approximation (holes are CW).
        (0..12)
            .map(|i| {
                let a = -2.0 * PI * (i as f64) / 12.0;
                Point2::new(cx + hr * a.cos(), cy + hr * a.sin())
            })
            .collect()
    };
    let mut m = base_flange_polygon_with_holes(
        outline,
        vec![hole(-20.0, 0.0, 6.0), hole(20.0, 0.0, 6.0)],
        1.0,
    )
    .unwrap();
    m.material = "al-soft".into();
    let table = BendTable::builtin();

    // 4 chained sequences of depth 3.
    for &edge in &[0usize, 4, 8, 12] {
        let (mut child, _) = add_edge_flange(&mut m, &table, flange(0, edge, 18.0)).unwrap();
        for _ in 0..2 {
            // Edge 2 of a freshly created flange rectangle is its far edge.
            let (next, _) = add_edge_flange(&mut m, &table, flange(child, 2, 14.0)).unwrap();
            child = next;
        }
    }
    // 4 single flanges on angled edges.
    for &edge in &[2usize, 6, 10, 14] {
        add_edge_flange(&mut m, &table, flange(0, edge, 12.0)).unwrap();
    }
    unfold(&mut m).unwrap();
    m
}

fn extract_lines(dxf: &str) -> Vec<(f64, f64, f64, f64, String)> {
    // Parse LINE entities: layer, linetype and the two endpoints.
    let mut out = Vec::new();
    let tokens: Vec<&str> = dxf.lines().collect();
    let mut i = 0;
    while i + 1 < tokens.len() {
        if tokens[i] == "0" && tokens[i + 1] == "LINE" {
            let mut ltype = String::new();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
            let mut j = i + 2;
            while j + 1 < tokens.len() && tokens[j] != "0" {
                match tokens[j] {
                    "6" => ltype = tokens[j + 1].to_string(),
                    "10" => x0 = tokens[j + 1].parse().unwrap(),
                    "20" => y0 = tokens[j + 1].parse().unwrap(),
                    "11" => x1 = tokens[j + 1].parse().unwrap(),
                    "21" => y1 = tokens[j + 1].parse().unwrap(),
                    _ => {}
                }
                j += 2;
            }
            out.push((x0, y0, x1, y1, ltype));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn origami_crane_golden_topology() {
    let m = origami_crane();
    assert_eq!(m.panels.len(), 17);
    assert_eq!(m.bends.len(), 16);

    let flat = FlatPattern::from_model(&m);
    let dxf = flat_pattern_to_dxf(&flat).expect("16-bend crane must merge to one region");

    // Exactly 1 exterior + 2 hole polylines on CUT.
    let polylines = dxf.matches("0\nLWPOLYLINE\n8\nCUT").count();
    assert_eq!(polylines, 3, "1 exterior + 2 holes, got {polylines}");

    // Exactly B = 16 LINE entities, every one DASHED.
    let lines = extract_lines(&dxf);
    assert_eq!(lines.len(), 16);
    for (x0, y0, x1, y1, ltype) in &lines {
        assert_eq!(ltype, "DASHED", "bend line not dashed");
        assert!(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite());
    }

    // Every bend line lies on its allowance midline to 1 µm.
    let sil = silhouette(&flat).unwrap();
    assert_eq!(sil.bend_lines.len(), 16);
    for bl in &sil.bend_lines {
        let found = lines.iter().any(|(x0, y0, x1, y1, _)| {
            let fwd = (x0 - bl.line.0.x).abs() < 1e-3
                && (y0 - bl.line.0.y).abs() < 1e-3
                && (x1 - bl.line.1.x).abs() < 1e-3
                && (y1 - bl.line.1.y).abs() < 1e-3;
            let rev = (x0 - bl.line.1.x).abs() < 1e-3
                && (y0 - bl.line.1.y).abs() < 1e-3
                && (x1 - bl.line.0.x).abs() < 1e-3
                && (y1 - bl.line.0.y).abs() < 1e-3;
            fwd || rev
        });
        assert!(found, "bend {} not on its allowance midline", bl.bend_id);
    }

    // Verify midlines analytically: midline = crease + n̂ · BA/2.
    for (crease, bl) in flat.creases.iter().zip(&sil.bend_lines) {
        let ba = crease.angle * (crease.radius + crease.k_factor * flat.thickness);
        let mid = Point2::new(
            (bl.line.0.x + bl.line.1.x) / 2.0,
            (bl.line.0.y + bl.line.1.y) / 2.0,
        );
        let crease_mid = Point2::new(
            (crease.line.0.x + crease.line.1.x) / 2.0,
            (crease.line.0.y + crease.line.1.y) / 2.0,
        );
        let dist = (mid - crease_mid).norm();
        assert!(
            (dist - ba / 2.0).abs() < 1e-6,
            "bend {}: midline offset {dist} != BA/2 {}",
            bl.bend_id,
            ba / 2.0
        );
    }

    // Sanity: no annotations of any kind.
    for forbidden in ["\nTEXT", "\nMTEXT", "\nDIMENSION", "\nATTRIB"] {
        assert!(!dxf.contains(forbidden));
    }
}

/// Deterministic LCG so the property test is reproducible without a
/// proptest dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() % 10_000) as f64 / 10_000.0)
    }
    fn usize(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

#[test]
fn random_flange_trees_union_to_single_polygon_with_exact_area() {
    for seed in 0..24u64 {
        let mut rng = Lcg(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1));
        // Random convex base polygon: 5–10 sorted angles on a circle.
        let sides = 5 + rng.usize(6);
        let mut angles: Vec<f64> = (0..sides).map(|_| rng.range(0.0, 2.0 * PI)).collect();
        angles.sort_by(f64::total_cmp);
        // Reject near-duplicate angles (degenerate edges).
        angles.dedup_by(|a, b| (*a - *b).abs() < 0.15);
        if angles.len() < 4 {
            continue;
        }
        let r = rng.range(40.0, 80.0);
        let outline: Vec<Point2> = angles
            .iter()
            .map(|&a| Point2::new(r * a.cos(), r * a.sin()))
            .collect();
        let mut m = base_flange_rect(1.0, 1.0, 1.0).unwrap();
        m.panels[0].outline = outline;
        m.material = "al-soft".into();
        let table = BendTable::builtin();

        // Flange a random subset of base edges (convex ⇒ outward strips
        // never overlap), occasionally chaining outward.
        let n_edges = m.panels[0].outline.len();
        let mut bends = 0;
        for edge in 0..n_edges {
            if rng.usize(2) == 0 {
                continue;
            }
            let len = rng.range(5.0, 25.0);
            let Ok((mut child, _)) = add_edge_flange(&mut m, &table, flange(0, edge, len)) else {
                continue;
            };
            bends += 1;
            let depth = rng.usize(3);
            for _ in 0..depth {
                let len = rng.range(5.0, 20.0);
                let Ok((next, _)) = add_edge_flange(&mut m, &table, flange(child, 2, len)) else {
                    break;
                };
                child = next;
                bends += 1;
            }
        }
        if bends == 0 {
            continue;
        }
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        let sil = silhouette(&flat).unwrap_or_else(|e| panic!("seed {seed}: {e}"));

        // Area = Σ panel areas + Σ allowance × hinge length.
        let panel_area: f64 = flat
            .panel_outlines_2d
            .iter()
            .map(|o| vcad_kernel_sheet::poly2d::signed_area_f(o).abs())
            .sum();
        let strip_area: f64 = m
            .bends
            .iter()
            .map(|b| {
                let (p0, p1) = b.edge_parent;
                (p1 - p0).norm() * b.allowance(m.thickness)
            })
            .sum();
        let expected = panel_area + strip_area;
        let got = vcad_kernel_sheet::Poly {
            outer: sil.exterior.clone(),
            holes: sil.holes.clone(),
        }
        .area();
        // Tolerance: 5 µm oversize per quad edge + 1 µm snapping.
        let tol = (expected * 1e-4).max(0.5);
        assert!(
            (got - expected).abs() < tol,
            "seed {seed}: area {got} vs expected {expected} (bends={bends})"
        );
    }
}

#[test]
fn export_is_deterministic_byte_for_byte() {
    let a = flat_pattern_to_dxf(&FlatPattern::from_model(&origami_crane())).unwrap();
    let b = flat_pattern_to_dxf(&FlatPattern::from_model(&origami_crane())).unwrap();
    assert_eq!(
        a, b,
        "re-unfolding the same part must reproduce the DXF exactly"
    );
}

#[test]
fn l_bracket_matches_checked_in_golden_file() {
    let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
    m.material = "al-soft".into();
    let table = BendTable::builtin();
    add_edge_flange(&mut m, &table, flange(0, 0, 25.0)).unwrap();
    unfold(&mut m).unwrap();
    let dxf = flat_pattern_to_dxf(&FlatPattern::from_model(&m)).unwrap();
    if std::env::var("REGEN_GOLDEN").is_ok() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/l_bracket.dxf"),
            &dxf,
        )
        .unwrap();
        return;
    }
    let golden = include_str!("golden/l_bracket.dxf");
    assert_eq!(
        dxf, golden,
        "L-bracket DXF deviates from tests/golden/l_bracket.dxf — if the \
         change is intentional, regenerate the golden file"
    );
}
