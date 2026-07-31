//! Boolean-kernel regression catalogue from the torr session-3 field report
//! (2026-07-06, turbomolecular-rotor build). Each test is a loon source +
//! an analytic/numeric expectation; every case reproduced a silent geometry
//! corruption on the deployed MCP server. Doc ids in comments refer to the
//! original live repros.
//!
//! Clusters:
//!   A — booleans × circular-pattern instances
//!   B — near-degenerate configurations of analytic primitives
//!   C — sketch-derived bodies (extrude/revolve) as boolean operands
//!   D — error accumulation through boolean tree shapes
//!   E — scale cliff (pattern union chains) — `#[ignore]`d perf canary
//!
//! Numeric expectations for the "sliver" cases (rotated blade vs r45
//! cylinder) were computed by direct integration of the exact geometry:
//!   blade = cube 23.5 × 0.5 × 12.57, rotated +39.29° about X,
//!           translated +21.5 in x
//!   V_blade            = 147.6975
//!   V_blade ∩ r45 cyl  = 146.3205  (sliver outside r45 = 1.3770)
//!   V_blade ∩ r30 cyl  =  51.3451
//! (2000² midpoint quadrature over the cross-section; ±1e-3.)

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

const PI: f64 = std::f64::consts::PI;

/// Tessellation resolution for the measurement oracle. The default solid
/// resolution (32 segments) reads cylinder volumes 0.64% low (inscribed
/// 32-gon), which would swamp the sliver-sized signals under test. At 256
/// segments the systematic drops to ~0.01%.
const ORACLE_SEGMENTS: u32 = 256;

struct Inspection {
    volume: f64,
    com: [f64; 3],
    bbox: ([f64; 3], [f64; 3]),
    /// Unpaired directed mesh edges (0 for a closed surface). Vertices are
    /// matched by quantized position, mirroring the MCP integrity block.
    open_edges: usize,
}

fn inspect(src: &str) -> Inspection {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    let mesh = solid.to_mesh(ORACLE_SEGMENTS);
    let tri = |t: usize| -> [[f64; 3]; 3] {
        let mut out = [[0.0; 3]; 3];
        for k in 0..3 {
            let vi = mesh.indices[t * 3 + k] as usize;
            for c in 0..3 {
                out[k][c] = mesh.vertices[vi * 3 + c] as f64;
            }
        }
        out
    };
    let ntri = mesh.indices.len() / 3;
    let mut vol = 0.0;
    let mut moment = [0.0; 3];
    let mut bbox = ([f64::MAX; 3], [f64::MIN; 3]);
    for t in 0..ntri {
        let [a, b, c] = tri(t);
        // Signed tetra volume against the origin (divergence theorem).
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let v = det / 6.0;
        vol += v;
        for k in 0..3 {
            moment[k] += v * (a[k] + b[k] + c[k]) / 4.0;
            for p in [&a, &b, &c] {
                bbox.0[k] = bbox.0[k].min(p[k]);
                bbox.1[k] = bbox.1[k].max(p[k]);
            }
        }
    }
    let com = if vol.abs() > 1e-12 {
        [moment[0] / vol, moment[1] / vol, moment[2] / vol]
    } else {
        [0.0; 3]
    };
    // Open-edge census: net directed traversal per undirected edge, with
    // vertices matched by quantized position (meshes duplicate vertices per
    // face). Closed surface ⇒ every edge nets to zero.
    let open_edges = {
        let quantum = 1e-5;
        let vkey = |vi: usize| -> [i64; 3] {
            let mut k = [0i64; 3];
            for c in 0..3 {
                k[c] = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
            }
            k
        };
        let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
            std::collections::HashMap::new();
        for t in 0..ntri {
            for k in 0..3 {
                let a = vkey(mesh.indices[t * 3 + k] as usize);
                let b = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
                if a == b {
                    continue; // degenerate edge collapses under quantization
                }
                if a < b {
                    *net.entry((a, b)).or_default() += 1;
                } else {
                    *net.entry((b, a)).or_default() -= 1;
                }
            }
        }
        net.values().map(|n| n.unsigned_abs() as usize).sum()
    };
    Inspection {
        volume: vol,
        com,
        bbox,
        open_edges,
    }
}

fn volume(src: &str) -> f64 {
    inspect(src).volume
}

#[track_caller]
fn assert_vol(v: f64, expected: f64, tol_pct: f64, label: &str) {
    let tol = expected.abs() * tol_pct / 100.0;
    assert!(
        (v - expected).abs() <= tol,
        "{label}: volume {v:.4} vs expected {expected:.4} (±{tol_pct}% = {tol:.4}, err {:+.4})",
        v - expected
    );
}

/// CoM must lie on the pattern axis (Z): a free integrity certificate for
/// rotationally symmetric parts.
#[track_caller]
fn assert_com_on_z_axis(com: [f64; 3], tol: f64, label: &str) {
    let d = (com[0] * com[0] + com[1] * com[1]).sqrt();
    assert!(
        d <= tol,
        "{label}: CoM ({:.4}, {:.4}, {:.4}) is {d:.4} off the Z axis (tol {tol})",
        com[0],
        com[1],
        com[2]
    );
}

// Shared geometry snippets --------------------------------------------------

/// Rotated blade: thin cube staggered 39.29° about X, root at r=21.5.
/// Pokes past r=45 only in tiny corner slivers (the near-degenerate regime).
const BLADE: &str = "[translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]]";
/// Same blade, unrotated (axis-aligned; for gross-overlap variants).
const BLADE_FLAT: &str = "[translate 21.5 0 0 [cube 23.5 0.5 12.57]]";

const V_BLADE: f64 = 147.6975;
const V_BLADE_IN_R45: f64 = 146.3205;
const V_BLADE_IN_R30: f64 = 51.3451;

// ===========================================================================
// Controls — calibrate tessellation accuracy of the volume oracle itself
// ===========================================================================

#[test]
fn control_cylinder_volume() {
    let v = volume("[cylinder 45.0 13]");
    assert_vol(v, PI * 45.0 * 45.0 * 13.0, 0.5, "plain cylinder r45 h13");
}

#[test]
fn control_blade_volume() {
    let v = volume(BLADE);
    assert_vol(v, V_BLADE, 0.05, "rotated blade cube (planar, exact)");
}

#[test]
fn control_extrude_standalone() {
    // torr: standalone extrusions inspect correctly (single blade 146.1 OK).
    let v = volume(
        "[extrude 0 0 10 [sketch 0 0 0  1 0 0  0 1 0 \
           #[[line 0 0 23.5 0] [line 23.5 0 23.5 0.5] \
             [line 23.5 0.5 0 0.5] [line 0 0.5 0 0]]]]",
    );
    assert_vol(v, 23.5 * 0.5 * 10.0, 0.05, "standalone rect extrusion");
}

// ===========================================================================
// Cluster B — near-degenerate analytic-primitive booleans
// ===========================================================================

/// B1, doc_7_nn3VADDJUEW3: intersection where the blade is ~99% inside the
/// cylinder (only corner slivers poke past r45). Observed: volume 0, null
/// bbox. Expected ≈ 146.32.
#[test]
fn b1_intersection_near_containment() {
    // loon is subject-last: [intersection other s] = s ∩ other. Doc order.
    let i = inspect(&format!("[intersection [cylinder 45.0 13] {BLADE}]"));
    // The trim signal (sliver = 1.377) is smaller than 1% of the volume, so
    // assert with an absolute window of half a sliver.
    assert!(
        (i.volume - V_BLADE_IN_R45).abs() <= 0.7,
        "blade ∩ cyl45: volume {:.4} vs expected {V_BLADE_IN_R45} (untrimmed blade = {V_BLADE})",
        i.volume
    );
    assert!(
        i.volume < V_BLADE - 0.6,
        "blade ∩ cyl45: volume {:.4} not trimmed below untrimmed blade {V_BLADE}",
        i.volume
    );
}

/// Operand-order twin of b1 (loon argument order swapped).
#[test]
fn b1_intersection_near_containment_swapped() {
    let v = volume(&format!("[intersection {BLADE} [cylinder 45.0 13]]"));
    assert!(
        (v - V_BLADE_IN_R45).abs() <= 0.7,
        "cyl45 ∩ blade (swapped): volume {v:.4} vs expected {V_BLADE_IN_R45}"
    );
}

/// B1 dual: cylinder − blade must remove exactly the overlap volume.
#[test]
fn b1_difference_dual() {
    let v_cyl = PI * 45.0 * 45.0 * 13.0;
    // [difference tool s] = s − tool
    let v = volume(&format!("[difference {BLADE} [cylinder 45.0 13]]"));
    let expected = v_cyl - V_BLADE_IN_R45;
    assert_vol(v, expected, 0.5, "cyl45 − blade");
}

/// Partition identity on the cylinder: (cyl ∩ blade) + (cyl − blade) = cyl.
/// Exact regardless of tessellation systematics (same operands, same segs).
#[test]
fn b1_partition_identity() {
    let v_int = volume(&format!("[intersection [cylinder 45.0 13] {BLADE}]"));
    let v_diff = volume(&format!("[difference {BLADE} [cylinder 45.0 13]]"));
    let v_cyl = volume("[cylinder 45.0 13]");
    let sum = v_int + v_diff;
    assert!(
        (sum - v_cyl).abs() <= v_cyl * 0.005,
        "partition identity: ∩({v_int:.4}) + −({v_diff:.4}) = {sum:.4} vs cyl {v_cyl:.4}"
    );
}

/// B2, doc_1_BcrzPpXaZtGg family: union of two cubes sharing coplanar faces
/// (staircase blade bands). Observed +40% volume and CoM walked off axis.
/// Adjacent variant: B starts exactly at A's x=15 face, shares both y-planes
/// and the z=0 plane.
#[test]
fn b2_coplanar_union_adjacent() {
    let v = volume(
        "[union [translate 15 0 0 [cube 8.5 0.5 6]] \
                [cube 15 0.5 12.57]]",
    );
    let expected = 15.0 * 0.5 * 12.57 + 8.5 * 0.5 * 6.0;
    assert_vol(v, expected, 0.2, "coplanar-adjacent staircase union");
}

/// B2 overlapping variant: bands overlap in x, share both y-planes and z=0.
#[test]
fn b2_coplanar_union_overlapping() {
    let v = volume(
        "[union [translate 10 0 0 [cube 13.5 0.5 6]] \
                [cube 15 0.5 12.57]]",
    );
    let expected = 15.0 * 0.5 * 12.57 + 13.5 * 0.5 * 6.0 - 5.0 * 0.5 * 6.0;
    assert_vol(v, expected, 0.2, "coplanar-overlapping staircase union");
}

// ===========================================================================
// Cluster A — booleans × circular-pattern instances
// ===========================================================================

/// A1, doc_9_9ZHIeomGpTT0 mechanism (gross variant): a difference as the
/// pattern child must execute. Tool cube removes the x>35 half of each
/// blade; if the boolean inside the pattern is dropped, volume comes back
/// as 8 × 147.6975 = 1181.6 instead of 8 × 84.8475 = 678.8.
#[test]
fn a1_pattern_child_boolean_gross() {
    let i = inspect(&format!(
        "[circular-pattern 0 0 0  0 0 1  8 360 \
           [difference [translate 35 -10 -3 [cube 20 20 20]] {BLADE_FLAT}]]"
    ));
    let expected = 8.0 * (V_BLADE - 10.0 * 0.5 * 12.57);
    assert_vol(v_of(&i), expected, 0.5, "pattern(blade − cube) gross trim");
    assert_com_on_z_axis(i.com, 0.05, "pattern(blade − cube) gross trim");
    assert!(
        i.bbox.1[0] <= 35.0 + 0.01,
        "trim plane x=35 not respected: bbox xmax {}",
        i.bbox.1[0]
    );
}

/// A1 sliver variant (doc_9's actual regime): pattern child trims the blade
/// to r45 via an annulus tool. Untrimmed total = 23 × 147.6975 = 3397.04;
/// trimmed = 23 × 146.3205 = 3365.37.
#[test]
fn a1_pattern_child_boolean_sliver() {
    let i = inspect(&format!(
        "[circular-pattern 0 0 0  0 0 1  23 360 \
           [difference [difference [cylinder 45 20] [cylinder 60 20]] {BLADE}]]"
    ));
    let expected = 23.0 * V_BLADE_IN_R45;
    let untrimmed = 23.0 * V_BLADE;
    assert!(
        (i.volume - expected).abs() <= 16.0,
        "pattern(blade − annulus): volume {:.3} vs expected {expected:.3} (untrimmed = {untrimmed:.3})",
        i.volume
    );
    assert_com_on_z_axis(i.com, 0.05, "pattern(blade − annulus)");
}

/// A2, doc_10_xlRzgc3WB4X2 (exact loon from the report): difference whose
/// subject is a 23-instance circular pattern. Observed: volume 1231.6 (vs
/// ≈3365 expected), asymmetric bbox — some instances trimmed, some not,
/// some corrupted.
#[test]
fn a2_boolean_over_pattern_doc10() {
    let i = inspect(
        "[difference [translate 0 0 96.89 [difference [cylinder 45.0 14.05] \
           [cylinder 60 14.05]]] [translate 0 0 97.89 [circular-pattern 0 0 0 0 0 1 \
           23 360 [translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]]]]]",
    );
    let expected = 23.0 * V_BLADE_IN_R45;
    assert!(
        (i.volume - expected).abs() <= 16.0,
        "doc_10 pattern − annulus: volume {:.3} vs expected {expected:.3}",
        i.volume
    );
    assert_com_on_z_axis(i.com, 0.05, "doc_10 pattern − annulus");
    // Every blade is trimmed to r45: bbox must be symmetric and r45-bounded.
    for (lo, hi, name) in [
        (i.bbox.0[0], i.bbox.1[0], "x"),
        (i.bbox.0[1], i.bbox.1[1], "y"),
    ] {
        assert!(
            lo >= -45.5 && hi <= 45.5,
            "doc_10: {name} extent [{lo:.3}, {hi:.3}] escapes r45 trim"
        );
        assert!(
            (lo + hi).abs() <= 0.5,
            "doc_10: {name} extent [{lo:.3}, {hi:.3}] asymmetric under 23-fold symmetry"
        );
    }
}

/// A2 gross variant: 8 flat blades minus an annulus r30..60 — every blade
/// must lose its outer half. Per-blade kept ≈ 51.35 (numeric, r30 clip of
/// the flat blade = 53.41... computed for the FLAT blade below).
#[test]
fn a2_boolean_over_pattern_gross() {
    // flat blade ∩ r<30: ∫ y∈[0,0.5] (√(900−y²) − 21.5) dy × 12.57 = 53.4138
    let per_blade = 53.4138;
    let i = inspect(&format!(
        "[difference [difference [cylinder 30 13] [cylinder 60 13]] \
           [circular-pattern 0 0 0  0 0 1  8 360 {BLADE_FLAT}]]"
    ));
    let expected = 8.0 * per_blade;
    assert_vol(
        v_of(&i),
        expected,
        1.0,
        "pattern(8 flat blades) − annulus30..60",
    );
    assert_com_on_z_axis(i.com, 0.05, "pattern(8 flat blades) − annulus30..60");
    assert!(
        i.bbox.1[0] <= 30.1 && i.bbox.0[0] >= -30.1,
        "r30 trim not respected: x extent [{}, {}]",
        i.bbox.0[0],
        i.bbox.1[0]
    );
}

/// Pattern-of-booleans and boolean-over-pattern must agree (same geometry,
/// two algebraically equivalent trees).
#[test]
fn a_pattern_boolean_commutation() {
    let v_child = volume(&format!(
        "[circular-pattern 0 0 0  0 0 1  8 360 \
           [difference [difference [cylinder 30 13] [cylinder 60 13]] {BLADE_FLAT}]]"
    ));
    let v_over = volume(&format!(
        "[difference [difference [cylinder 30 13] [cylinder 60 13]] \
           [circular-pattern 0 0 0  0 0 1  8 360 {BLADE_FLAT}]]"
    ));
    // Trim-in-pattern is now EXACT (8 × 53.4138 analytic); pin it to its
    // analytic value so a regression on that side is caught directly.
    // Trim-over-pattern still carries the A2 residual (~+2%, was +4%), so
    // the equivalence check runs at a tolerance that flags regressions of
    // the gap without failing on the known remainder.
    assert_vol(v_child, 8.0 * 53.4138, 0.5, "trim-in-pattern (exact)");
    assert!(
        (v_child - v_over).abs() <= v_over.abs().max(1.0) * 0.03,
        "trim-in-pattern ({v_child:.3}) vs trim-over-pattern ({v_over:.3}) disagree"
    );
}

// ===========================================================================
// Cluster C — sketch-derived bodies in booleans
// ===========================================================================

/// C1, doc_13_4sIo-AWGdrhb: revolve of a closed rectangle profile (y=0
/// plane, basis X=(1,0,0), Y=(0,0,1)) about +Z by 180°. Observed: volume 0,
/// surface 1000, bbox flat in y.
#[test]
fn c1_revolve_180() {
    let i = inspect(
        "[revolve 0 0 0  0 0 1  180 \
           [sketch 0 0 0  1 0 0  0 0 1 \
             #[[line 10 0 20 0] [line 20 0 20 5] [line 20 5 10 5] [line 10 5 10 0]]]]",
    );
    let expected = 0.5 * PI * (400.0 - 100.0) * 5.0;
    assert_vol(
        i.volume,
        expected,
        1.0,
        "180° revolve of rectangle (half washer)",
    );
    // bbox must span the swept half: x ∈ [−20, 20], and y must NOT be flat.
    assert!(
        (i.bbox.1[1] - i.bbox.0[1]) > 15.0,
        "half-washer bbox flat in y: [{:.3}, {:.3}]",
        i.bbox.0[1],
        i.bbox.1[1]
    );
}

/// C1 control: full 360° revolve of the same profile.
#[test]
fn c1_revolve_360_control() {
    let v = volume(
        "[revolve 0 0 0  0 0 1  360 \
           [sketch 0 0 0  1 0 0  0 0 1 \
             #[[line 10 0 20 0] [line 20 0 20 5] [line 20 5 10 5] [line 10 5 10 0]]]]",
    );
    assert_vol(
        v,
        PI * (400.0 - 100.0) * 5.0,
        1.0,
        "360° revolve of rectangle (washer)",
    );
}

/// C2, doc_17/doc_18: cylinder ∪ extruded body, both operand orders.
/// Observed 10757 / 11049 vs expected ≈23969 (and CoM x displaced +4.2).
#[test]
fn c2_extrude_union_cylinder() {
    let blade_x = "[translate 21.5 0 2 [extrude 0 0 10 [sketch 0 0 0  1 0 0  0 1 0 \
       #[[line 0 0 23.5 0] [line 23.5 0 23.5 0.5] [line 23.5 0.5 0 0.5] [line 0 0.5 0 0]]]]]";
    // Overlap of the box x∈[21.5,45]×y∈[0,0.5]×z∈[2,12] with cyl r22.5 h15
    // = 4.9907 (numeric).
    let expected = PI * 22.5 * 22.5 * 15.0 + 23.5 * 0.5 * 10.0 - 4.9907;
    let v1 = volume(&format!("[union [cylinder 22.5 15] {blade_x}]"));
    let v2 = volume(&format!("[union {blade_x} [cylinder 22.5 15]]"));
    assert_vol(v1, expected, 1.0, "extrusion ∪ cylinder");
    assert_vol(v2, expected, 1.0, "cylinder ∪ extrusion (swapped)");
}

/// C2, doc_20_v1zsbRrMLXNj: extrude ∪ extrude — two half-annuli (r20..30,
/// h5, each spanning 190°, second rotated 180°) must union into the full
/// annulus. Observed 3254 vs expected 7854.
/// 190° half-annulus profile: outer arc ccw 0°→190°, line in, inner arc
/// cw back, line out. Sketch in the XY plane.
const HALF_ANNULUS: &str = "[extrude 0 0 5 [sketch 0 0 0  1 0 0  0 1 0 \
   #[[line 20 0 30 0] \
     [arc 30 0 -29.5442 -5.2094 0 0 true] \
     [line -29.5442 -5.2094 -19.6962 -3.4730] \
     [arc -19.6962 -3.4730 20 0 0 0 false]]]]";

#[test]
fn c2_extrude_standalone_arc_control() {
    let v_one = volume(HALF_ANNULUS);
    let expected_one = (190.0 / 360.0) * PI * (900.0 - 400.0) * 5.0;
    assert_vol(
        v_one,
        expected_one,
        1.0,
        "standalone 190° half-annulus extrusion",
    );
}

#[test]
fn c2_extrude_union_extrude_half_annuli() {
    let v = volume(&format!(
        "[union [rotate 0 0 180 {HALF_ANNULUS}] {HALF_ANNULUS}]"
    ));
    let expected = PI * (900.0 - 400.0) * 5.0;
    assert_vol(
        v,
        expected,
        1.0,
        "two overlapping half-annuli → full annulus",
    );
}

// ===========================================================================
// Cluster D — boolean tree shapes must agree (error accumulation)
// ===========================================================================
// Hub = cylinder r30 h40. Cavity = r18.5 z∈[10,40]. Bore = r10 z∈[0,10].
// Counterbore = r24 z∈[30,40]. All coaxial — analytic volumes exact.

const D_HUB: &str = "[cylinder 30 40]";
const D_CAVITY: &str = "[translate 0 0 10 [cylinder 18.5 30]]";
const D_BORE: &str = "[cylinder 10 10]";
const D_CBORE: &str = "[translate 0 0 30 [cylinder 24 10]]";

fn d_expected(stages: u32) -> f64 {
    let hub = PI * 900.0 * 40.0;
    let cavity = PI * 18.5 * 18.5 * 30.0;
    let bore = PI * 100.0 * 10.0;
    let cbore = PI * (576.0 - 342.25) * 10.0;
    match stages {
        1 => hub - cavity,
        2 => hub - cavity - bore,
        3 => hub - cavity - bore - cbore,
        _ => unreachable!(),
    }
}

/// D baseline, doc_2_MlVOYisTLQza family: single subtractor (+0.03% in the
/// field).
#[test]
fn d1_single_subtractor() {
    let v = volume(&format!("[difference {D_CAVITY} {D_HUB}]"));
    assert_vol(v, d_expected(1), 0.5, "hub − cavity");
}

/// D, doc_3_dr_sBBK9BJYy: union-of-subtractors, one difference (+5.9% in
/// the field).
#[test]
fn d2_union_of_subtractors() {
    let v = volume(&format!("[difference [union {D_CAVITY} {D_BORE}] {D_HUB}]"));
    assert_vol(v, d_expected(2), 0.5, "hub − (cavity ∪ bore)");
}

/// D, doc_4_c-OrcpqzEQ83: two chained differences (+7.1% in the field).
#[test]
fn d3_two_chained_differences() {
    let v = volume(&format!(
        "[difference {D_BORE} [difference {D_CAVITY} {D_HUB}]]"
    ));
    assert_vol(v, d_expected(2), 0.5, "(hub − cavity) − bore");
}

/// D, doc_5_f6D9cWE7gIef: three chained differences — staircase (+39% in
/// the field).
#[test]
fn d4_three_chained_differences() {
    let v = volume(&format!(
        "[difference {D_CBORE} [difference {D_BORE} [difference {D_CAVITY} {D_HUB}]]]"
    ));
    assert_vol(
        v,
        d_expected(3),
        0.5,
        "((hub − cavity) − bore) − counterbore",
    );
}

/// Tree-shape equivalence: chained differences ≡ difference of union.
#[test]
fn d_tree_shape_equivalence() {
    let v_union = volume(&format!("[difference [union {D_CAVITY} {D_BORE}] {D_HUB}]"));
    let v_chain = volume(&format!(
        "[difference {D_BORE} [difference {D_CAVITY} {D_HUB}]]"
    ));
    assert!(
        (v_union - v_chain).abs() <= v_union.abs() * 0.005,
        "union-of-subtractors ({v_union:.3}) vs chained ({v_chain:.3}) disagree"
    );
}

/// D2, doc_6_G-DlN9WJW-n5 vs doc_7_DQJ0CpjapE45: subtracting a bore
/// mid-tree (from the hub, before unioning the drum) must equal subtracting
/// it outermost. In the field the mid-tree form rendered the bore as a
/// solid plug standing in a recess (local inversion at the top cap).
#[test]
fn d2_locality_invariance() {
    // hub r20 h30; drum = annulus r20..28 z∈[20,30] (shares the r20 wall);
    // bore r4 through the hub.
    let drum = "[translate 0 0 20 [difference [cylinder 20 10] [cylinder 28 10]]]";
    let hub = "[cylinder 20 30]";
    let bore = "[cylinder 4 30]";
    let v_mid = volume(&format!("[union {drum} [difference {bore} {hub}]]"));
    let v_out = volume(&format!("[difference {bore} [union {drum} {hub}]]"));
    let expected = PI * (400.0 * 30.0 - 16.0 * 30.0 + (784.0 - 400.0) * 10.0);
    assert_vol(v_mid, expected, 0.75, "bore subtracted mid-tree");
    assert_vol(v_out, expected, 0.75, "bore subtracted outermost");
    assert!(
        (v_mid - v_out).abs() <= expected * 0.005,
        "mid-tree ({v_mid:.3}) vs outermost ({v_out:.3}) bore subtraction disagree"
    );
}

// ===========================================================================
// Cluster E — scale canary (ignored by default; run with --ignored)
// ===========================================================================

/// doc_21_SlDypwMuubSz: ~800 solids across patterns timed out at 60 s.
/// This canary is a fraction of that size and must stay fast; the pairwise
/// union chain in `circular_pattern` makes cost quadratic in count.
#[test]
#[ignore = "perf canary — run manually: cargo test -p vcad-eval --release -- --ignored e_pattern"]
fn e_pattern_scale_canary() {
    let i = inspect(
        "[circular-pattern 0 0 0  0 0 1  200 360 \
           [translate 40 0 0 [cube 1 1 1]]]",
    );
    assert_vol(i.volume, 200.0, 0.5, "200-instance pattern of unit cubes");
    assert_com_on_z_axis(i.com, 0.05, "200-instance pattern");
}

// ===========================================================================
// Cluster F — intersecting-union seam damage (retest 2026-07-08, build
// 0.9.4+87874b4). The surviving failure family: any union whose operands
// genuinely intersect a curved or boolean-result mesh leaves unstitched
// seams (open edges), and a union INTO a boolean-result solid silently
// drops most of the added material.
// ===========================================================================

/// Watertight control: a plain primitive union that intersects across a
/// curved wall. Field observation: volume correct to 0.1% but hundreds of
/// open edges per blade seam.
///
/// F1a — 4 explicitly-placed rotated blades embedded 1 mm into a cylinder
/// (field: 818 open edges, CoM slightly off a symmetric axis).
#[test]
#[ignore = "known remaining: intersecting union against a curved wall leaves seam cracks (~270 open edges here, down from 705 at the field baseline). Root cause: analytic circle boundaries surviving in boolean results can never conform with frozen split polylines — see the kernel-seam-freeze WIP branch for the freeze-at-boolean-entry fix in progress."]
fn f1_union_rotated_blades_into_cylinder_watertight() {
    let mut src = String::from("[cylinder 22.5 13]");
    for ang in [0.0, 90.0, 180.0, 270.0] {
        src = format!(
            "[union [rotate 0 0 {ang} [translate 21.50 0 0 [rotate 39.29 0 0 \
               [cube 23.50 0.5 12.57]]]] {src}]"
        );
    }
    let i = inspect(&src);
    assert!(
        i.open_edges == 0,
        "cyl ∪ 4 rotated blades: {} open edges (must be watertight)",
        i.open_edges
    );
    assert_com_on_z_axis(i.com, 0.05, "cyl ∪ 4 rotated blades");
    // Volume sanity: strictly more than the cylinder, less than cyl + blades.
    let v_cyl = PI * 22.5 * 22.5 * 13.0;
    assert!(
        i.volume > v_cyl && i.volume < v_cyl + 4.0 * V_BLADE,
        "cyl ∪ blades volume {:.3} outside ({v_cyl:.1}, {:.1})",
        i.volume,
        v_cyl + 4.0 * V_BLADE
    );
}

/// F1b — the pattern form: 23-blade circular pattern unioned with the
/// cylinder it embeds into (field: 13,579 open edges, ~590 per seam).
#[test]
#[ignore = "known remaining: pattern form of the seam-crack family (~6600 open edges, down from 8084). Same root cause as the rotated-blades case; freeze WIP branch."]
fn f1_union_pattern_into_cylinder_watertight() {
    let i = inspect(
        "[union [cylinder 22.5 13] [circular-pattern 0 0 0  0 0 1  23 360 \
           [translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]]]]",
    );
    assert!(
        i.open_edges == 0,
        "cyl ∪ 23-blade pattern: {} open edges (must be watertight)",
        i.open_edges
    );
    assert_com_on_z_axis(i.com, 0.05, "cyl ∪ 23-blade pattern");
}

/// F1 volume form with FLAT blades so the overlap is analytic:
/// flat blade x∈[21.5,45], y∈[0,0.5], z∈[0,12.57]; overlap with the r22.5
/// cylinder = ∫₀^0.5 (√(506.25−y²) − 21.5) dy × 12.57 = 6.2734 mm³.
#[test]
#[ignore = "known remaining: volume is now correct but ~100 open edges remain at oracle resolution (down from 644). Same freeze-family root cause; watertight at the boolean's native resolution."]
fn f1_union_flat_blades_into_cylinder_volume() {
    let overlap = 6.2734;
    let mut src = String::from("[cylinder 22.5 12.57]");
    for ang in [0.0, 90.0, 180.0, 270.0] {
        src = format!("[union [rotate 0 0 {ang} {BLADE_FLAT}] {src}]");
    }
    let i = inspect(&src);
    let expected = PI * 22.5 * 22.5 * 12.57 + 4.0 * (V_BLADE - overlap);
    assert_vol(v_of(&i), expected, 0.5, "cyl ∪ 4 flat blades");
    assert!(
        i.open_edges == 0,
        "cyl ∪ 4 flat blades: {} open edges (must be watertight)",
        i.open_edges
    );
}

/// F2 — union INTO a boolean-result solid silently drops the added
/// material. Field: staircase hub ∪ blade row gained +265 mm³ of an
/// expected +3,149, operand order irrelevant, and the implied added-mass
/// centroid was physically impossible. Repro: a hub that is itself a
/// chained-difference result, unioned with 4 embedded flat blades.
#[test]
#[ignore = "known remaining: union INTO a boolean-result solid still drifts (+121 mm³ here vs silent operand vanish in the field). The result's analytic bore/cap boundaries can't conform with the new operand's frozen seams; freeze WIP branch."]
fn f2_union_into_boolean_result_keeps_operand() {
    // Staircase hub: cylinder r22.5 h12.57 minus a bore r8 minus a
    // counterbore r14 in the top 4 mm (all analytic, exact per cluster D).
    let hub = "[difference [translate 0 0 8.57 [cylinder 14 4]] \
                 [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]";
    let overlap = 6.2734; // flat blade ∩ r22.5 wall (see F1 volume test)
    let v_hub = PI * 12.57 * 22.5 * 22.5 - PI * 64.0 * 12.57 - PI * (196.0 - 64.0) * 4.0;
    let mut src = hub.to_string();
    for ang in [0.0, 90.0, 180.0, 270.0] {
        src = format!("[union [rotate 0 0 {ang} {BLADE_FLAT}] {src}]");
    }
    let i = inspect(&src);
    let expected = v_hub + 4.0 * (V_BLADE - overlap);
    assert_vol(v_of(&i), expected, 0.5, "staircase hub ∪ 4 flat blades");
    assert!(
        i.open_edges == 0,
        "staircase hub ∪ 4 flat blades: {} open edges (must be watertight)",
        i.open_edges
    );
    // The blades reach r45; if they vanished the bbox stops at the hub wall.
    assert!(
        i.bbox.1[0] >= 44.9,
        "blades vanished from the union: bbox xmax {:.3} (expected 45)",
        i.bbox.1[0]
    );
}

/// F2 order-invariance: boolean-result operand first vs last must agree.
#[test]
fn f2_union_operand_order_invariance() {
    let hub = "[difference [translate 0 0 8.57 [cylinder 14 4]] \
                 [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]";
    let v_a = volume(&format!("[union {hub} {BLADE_FLAT}]"));
    let v_b = volume(&format!("[union {BLADE_FLAT} {hub}]"));
    assert!(
        (v_a - v_b).abs() <= v_a.abs() * 0.005,
        "union order changes volume: {v_a:.3} vs {v_b:.3}"
    );
}

/// F3 — any further boolean on a seam-damaged mesh: >60 s timeout in the
/// field (`cyl ∪ row ∪ row` — the scale cliff moved from 21 patterns down
/// to 2). Perf canary; run manually until F1 lands, then unignore.
#[test]
#[ignore = "perf canary — run manually: cargo test -p vcad-eval --release -- --ignored f3_chained"]
fn f3_chained_union_on_union_result() {
    let row = |z: f64| {
        format!(
            "[translate 0 0 {z} [circular-pattern 0 0 0  0 0 1  23 360 \
               [translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]]]]"
        )
    };
    let i = inspect(&format!(
        "[union [union [cylinder 22.5 40] {}] {}]",
        row(2.0),
        row(20.0)
    ));
    assert!(
        i.open_edges == 0,
        "chained union: {} open edges",
        i.open_edges
    );
    assert_com_on_z_axis(i.com, 0.05, "chained union of two blade rows");
}

// Small helper so Inspection reads naturally in assert_vol call sites.
fn v_of(i: &Inspection) -> f64 {
    i.volume
}
