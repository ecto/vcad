//! Boolean-kernel regression: phantom volume from intersecting two deep
//! union trees of coaxial cylinders/annuli (turbopump rotating group vs
//! static group, 2026-07 field repro).
//!
//! Observed on the deployed MCP server: `intersection(rotating, static)`
//! returned ~661 mm³ of phantom volume with only ~100 mm² of surface area —
//! isoperimetrically impossible (any solid satisfies A³ ≥ 36·π·V², so
//! 100 mm² can enclose at most ~94 mm³) — while every *pairwise*
//! intersection of the same leaf parts correctly returned 0. The corruption
//! only appears when both operands are many-solid union trees whose shells
//! nest coaxially (shaft floating inside bearing bores, annuli stacked with
//! coincident planar faces), i.e. the ray-cast face classification has to
//! thread through many coaxial cylindrical walls.
//!
//! Style follows `torr_boolean_catalogue.rs`.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

const PI: f64 = std::f64::consts::PI;

/// Tessellation resolution for the measurement oracle (see the catalogue:
/// 256 segments keeps the cylinder-volume systematic at ~0.01%).
const ORACLE_SEGMENTS: u32 = 256;

struct Inspection {
    volume: f64,
    area: f64,
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
    let mut volume = 0.0;
    let mut area = 0.0;
    for part in &scene.parts {
        let Some(solid) = part.solid.as_ref() else {
            continue;
        };
        let mesh = solid.to_mesh(ORACLE_SEGMENTS);
        let ntri = mesh.indices.len() / 3;
        for t in 0..ntri {
            let mut p = [[0.0f64; 3]; 3];
            for k in 0..3 {
                let vi = mesh.indices[t * 3 + k] as usize;
                for c in 0..3 {
                    p[k][c] = mesh.vertices[vi * 3 + c] as f64;
                }
            }
            let [a, b, c] = p;
            // Signed tetra volume against the origin (divergence theorem).
            let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
            volume += det / 6.0;
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cx = u[1] * v[2] - u[2] * v[1];
            let cy = u[2] * v[0] - u[0] * v[2];
            let cz = u[0] * v[1] - u[1] * v[0];
            area += 0.5 * (cx * cx + cy * cy + cz * cz).sqrt();
        }
    }
    Inspection { volume, area }
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

// Shared geometry --------------------------------------------------------
//
// All loon booleans are binary and subject-last ([union other s] = s ∪
// other), so the union trees below are left-nested folds — the same deep
// tree shape the live repro produced.

/// `[translate 0 0 z0 [cylinder r h]]` — cylinder r spanning z ∈ [z0, z0+h].
fn cyl(r: f64, z0: f64, z1: f64) -> String {
    format!("[translate 0 0 {z0} [cylinder {r} {h}]]", h = z1 - z0)
}

/// Annulus (washer): outer radius `r`, coaxial hole `hole`, z ∈ [z0, z1].
fn annulus(r: f64, hole: f64, z0: f64, z1: f64) -> String {
    let h = z1 - z0;
    format!("[translate 0 0 {z0} [difference [cylinder {hole} {h}] [cylinder {r} {h}]]]")
}

/// Off-axis pin: cylinder `r`, z ∈ [z0, z1], axis at polar (radius, deg).
fn pin(r: f64, z0: f64, z1: f64, radius: f64, deg: f64) -> String {
    let (s, c) = deg.to_radians().sin_cos();
    format!(
        "[translate {x} {y} {z0} [cylinder {r} {h}]]",
        x = radius * c,
        y = radius * s,
        h = z1 - z0
    )
}

/// Left-fold a slice of solids into a nested binary union tree.
fn union_tree(parts: &[String]) -> String {
    parts
        .iter()
        .skip(1)
        .fold(parts[0].clone(), |acc, p| format!("[union {p} {acc}]"))
}

/// Pin ring angles (deg) for both the lower r2.5 pins and upper r2.85 pins.
const PIN_ANGLES: [f64; 3] = [20.0, 140.0, 260.0];
const PIN_RING_RADIUS: f64 = 32.5;

/// Rotating group: two impeller-back annuli, the stepped hub (bored), and
/// the shaft. The shaft floats inside every hole it passes through — the
/// union has multiple disjoint shells with coaxial cylindrical walls.
fn rotating_group() -> String {
    let hub = format!(
        "[difference {bore} [union {top} {base}]]",
        bore = cyl(4.05, 36.1, 47.1),
        top = cyl(7.5, 39.1, 47.1),
        base = cyl(11.0, 36.1, 39.1)
    );
    union_tree(&[
        annulus(29.0, 4.2, 31.8, 33.4),
        annulus(29.0, 4.2, 33.4, 36.1),
        hub,
        cyl(4.0, 8.0, 47.0),
    ])
}

/// Static group: bored/pocketed base, two press-fit bearing annuli filling
/// the pocket (coincident r11 walls, coincident z=18 interface), a pin
/// ring, two stator annuli, and an upper pin ring.
fn static_group() -> String {
    // Base: disc r40 z0–6 + tower r12 z6–25, bore r5.5 through, pocket
    // r11 z11–25.2 (pokes 0.2 above the tower top — open pocket).
    let base = format!(
        "[difference {pocket} [difference {bore} [union {tower} {disc}]]]",
        disc = cyl(40.0, 0.0, 6.0),
        tower = cyl(12.0, 6.0, 25.0),
        bore = cyl(5.5, 0.0, 25.0),
        pocket = cyl(11.0, 11.0, 25.2)
    );
    let mut parts = vec![base];
    for deg in PIN_ANGLES {
        parts.push(pin(2.5, 6.0, 26.0, PIN_RING_RADIUS, deg));
    }
    parts.push(annulus(35.0, 8.0, 26.0, 28.7));
    parts.push(annulus(35.0, 5.0, 28.7, 30.3));
    for deg in PIN_ANGLES {
        parts.push(pin(2.85, 30.3, 31.95, PIN_RING_RADIUS, deg));
    }
    parts.push(annulus(11.0, 4.05, 11.0, 18.0));
    parts.push(annulus(11.0, 4.05, 18.0, 25.0));
    union_tree(&parts)
}

// Analytic volumes (all leaf solids are disjoint within each group, so the
// union volume is the plain sum; coincident-face contacts are measure-zero).

fn rotating_expected() -> f64 {
    let annuli = (29.0f64.powi(2) - 4.2f64.powi(2)) * (1.6 + 2.7);
    let hub = 11.0f64.powi(2) * 3.0 + 7.5f64.powi(2) * 8.0 - 4.05f64.powi(2) * 11.0;
    let shaft = 4.0f64.powi(2) * 39.0;
    PI * (annuli + hub + shaft)
}

fn static_expected() -> f64 {
    let disc = 40.0f64.powi(2) * 6.0;
    let tower = 12.0f64.powi(2) * 19.0;
    let pocket_cut = 11.0f64.powi(2) * 14.0; // pocket ∩ tower, z ∈ [11, 25]
    let bore_cut = 5.5f64.powi(2) * 11.0; // bore below the pocket, z ∈ [0, 11]
    let pins = 3.0 * 2.5f64.powi(2) * 20.0;
    let stator1 = (35.0f64.powi(2) - 8.0f64.powi(2)) * 2.7;
    let stator2 = (35.0f64.powi(2) - 5.0f64.powi(2)) * 1.6;
    let pins2 = 3.0 * 2.85f64.powi(2) * 1.65;
    let bearings = 2.0 * (11.0f64.powi(2) - 4.05f64.powi(2)) * 7.0;
    PI * (disc + tower - pocket_cut - bore_cut + pins + stator1 + stator2 + pins2 + bearings)
}

// ===========================================================================
// Controls — each union tree alone must measure at its analytic volume
// ===========================================================================

#[test]
fn control_rotating_group_volume() {
    let i = inspect(&rotating_group());
    assert_vol(i.volume, rotating_expected(), 0.5, "rotating group union");
}

#[test]
fn control_static_group_volume() {
    let i = inspect(&static_group());
    assert_vol(i.volume, static_expected(), 0.5, "static group union");
}

/// Pairwise control (matches the field observation that leaf-vs-leaf
/// intersections were all correct): shaft ∩ bearing annulus — the tightest
/// clearance in the assembly (r4 shaft inside an r4.05 bore, 0.05 mm gap).
#[test]
fn control_pairwise_shaft_vs_bearing_empty() {
    let i = inspect(&format!(
        "[intersection {bearing} {shaft}]",
        bearing = annulus(11.0, 4.05, 11.0, 18.0),
        shaft = cyl(4.0, 8.0, 47.0)
    ));
    assert!(
        i.volume.abs() < 1e-6,
        "shaft ∩ bearing: expected empty, got volume {:.4} (area {:.4})",
        i.volume,
        i.area
    );
}

// ===========================================================================
// The regression — intersection of the full trees must be empty
// ===========================================================================
// Every rotating solid clears every static solid (tightest gap: 0.05 mm
// radial, shaft inside the bearing bores), so rotating ∩ static = ∅. The
// field repro returned ~661 mm³ / ~100 mm² instead — a wrong-but-watertight
// phantom that violates the isoperimetric inequality A³ ≥ 36·π·V².

#[test]
fn phantom_intersection_of_union_trees_is_empty() {
    let i = inspect(&format!(
        "[intersection {static_g} {rotating}]",
        static_g = static_group(),
        rotating = rotating_group()
    ));
    assert!(
        i.volume.abs() < 1e-6,
        "rotating ∩ static: expected empty, got phantom volume {:.4} mm³ (area {:.4} mm²)",
        i.volume,
        i.area
    );
}

/// Operand-order twin — intersection must be symmetric.
#[test]
fn phantom_intersection_of_union_trees_is_empty_swapped() {
    let i = inspect(&format!(
        "[intersection {rotating} {static_g}]",
        static_g = static_group(),
        rotating = rotating_group()
    ));
    assert!(
        i.volume.abs() < 1e-6,
        "static ∩ rotating (swapped): expected empty, got phantom volume {:.4} mm³ (area {:.4} mm²)",
        i.volume,
        i.area
    );
}
