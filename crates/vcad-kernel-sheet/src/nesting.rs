//! Multi-part nesting on stock sheets.
//!
//! Foundation-tier: rectangular bounding-box packing via **Bottom-Left
//! Fill Decreasing** (BLFD). Each part is reduced to its flat-pattern
//! `(width, height)` and `quantity`; the algorithm tries each instance in
//! both orientations (0° / 90°), places it at the lowest-then-leftmost
//! free position on the current sheet, and starts a new sheet when no
//! position fits. Spacing between parts and a margin from the sheet edge
//! are configurable.
//!
//! Returns per-instance placements + per-sheet utilization. Good enough
//! to drive a layered multi-part DXF and produce a real quote-able sheet
//! count from a parts list. True-shape NFP nesting lands in a later tier.

use serde::{Deserialize, Serialize};

/// One distinct part to nest, with a bounding-box footprint and a
/// requested copy count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartFootprint {
    /// Optional label for the part (surfaced in placements / DXF).
    #[serde(default)]
    pub name: String,
    /// Flat-pattern width (mm).
    pub width_mm: f64,
    /// Flat-pattern height (mm).
    pub height_mm: f64,
    /// Number of copies to place. Clamped to >= 1.
    pub quantity: u32,
}

/// Stock + spacing inputs to [`nest_rectangles`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NestingParams {
    /// Stock sheet width (mm) — the longer dimension is conventionally x.
    pub stock_width_mm: f64,
    /// Stock sheet height (mm).
    pub stock_height_mm: f64,
    /// Minimum spacing between parts (mm).
    pub spacing_mm: f64,
    /// Margin from the sheet edge (mm).
    pub edge_margin_mm: f64,
    /// If false, parts are only placed in their original orientation.
    pub allow_rotation: bool,
}

impl Default for NestingParams {
    fn default() -> Self {
        Self::generic()
    }
}

impl NestingParams {
    /// Reasonable defaults — a 4'×8' sheet (≈1219×2438 mm), 3 mm
    /// spacing, 5 mm edge margin, rotation allowed.
    pub fn generic() -> Self {
        Self {
            stock_width_mm: 2438.0,
            stock_height_mm: 1219.0,
            spacing_mm: 3.0,
            edge_margin_mm: 5.0,
            allow_rotation: true,
        }
    }
}

/// A single placed instance on a particular sheet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// Index into the original `parts` array.
    pub part_index: usize,
    /// Instance number for this part (0-based).
    pub copy: u32,
    /// Stock sheet (0-based).
    pub sheet: usize,
    /// Lower-left x (mm) on the sheet.
    pub x_mm: f64,
    /// Lower-left y (mm) on the sheet.
    pub y_mm: f64,
    /// Effective footprint width (mm) after any rotation.
    pub width_mm: f64,
    /// Effective footprint height (mm) after any rotation.
    pub height_mm: f64,
    /// True when the part is rotated 90° from its original orientation.
    pub rotated: bool,
    /// Echoed name (helps the agent / DXF).
    pub name: String,
}

/// Result of [`nest_rectangles`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NestingResult {
    /// Every placed instance.
    pub placements: Vec<Placement>,
    /// How many stock sheets were used.
    pub sheets_used: usize,
    /// Overall utilization, parts area ÷ (sheets × stock area), as a
    /// percentage.
    pub utilization_pct: f64,
    /// Sum of placed part areas (mm²).
    pub used_area_mm2: f64,
    /// Sum of stock areas used (mm²).
    pub stock_area_mm2: f64,
    /// Per-sheet utilization (last entry may be partial).
    pub per_sheet_pct: Vec<f64>,
    /// Instances that didn't fit even on an empty sheet (oversized).
    pub unplaceable: Vec<usize>,
}

/// Nest a list of part footprints on stock sheets using BLFD.
pub fn nest_rectangles(parts: &[PartFootprint], params: &NestingParams) -> NestingResult {
    let stock_w = (params.stock_width_mm - 2.0 * params.edge_margin_mm).max(0.0);
    let stock_h = (params.stock_height_mm - 2.0 * params.edge_margin_mm).max(0.0);
    let spacing = params.spacing_mm.max(0.0);

    // Expand parts into instances; sort by max dim descending.
    #[derive(Clone)]
    struct Instance {
        part_index: usize,
        copy: u32,
        w: f64,
        h: f64,
        name: String,
    }
    let mut instances: Vec<Instance> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let q = p.quantity.max(1);
        for c in 0..q {
            instances.push(Instance {
                part_index: i,
                copy: c,
                w: p.width_mm,
                h: p.height_mm,
                name: p.name.clone(),
            });
        }
    }
    instances.sort_by(|a, b| {
        a.w.max(a.h)
            .partial_cmp(&b.w.max(b.h))
            .unwrap_or(std::cmp::Ordering::Equal)
            .reverse()
    });

    let mut sheets: Vec<Vec<(f64, f64, f64, f64)>> = Vec::new(); // (x,y,w,h) per placement
    let mut placements: Vec<Placement> = Vec::new();
    let mut unplaceable: Vec<usize> = Vec::new();
    let mut used_area = 0.0;

    'instance: for inst in &instances {
        // Try the original instance in this loop iteration.
        // The orientation loop chooses (w, h) per candidate.
        let area = inst.w * inst.h;
        let orientations: Vec<(f64, f64, bool)> = if params.allow_rotation {
            vec![(inst.w, inst.h, false), (inst.h, inst.w, true)]
        } else {
            vec![(inst.w, inst.h, false)]
        };

        // Try every existing sheet in order, then a new sheet.
        for sheet_idx in 0..=sheets.len() {
            // Ensure the sheet exists.
            if sheet_idx == sheets.len() {
                sheets.push(Vec::new());
            }
            for (w, h, rotated) in &orientations {
                let w = *w;
                let h = *h;
                if w > stock_w || h > stock_h {
                    continue;
                }
                if let Some((x, y)) =
                    bottom_left(&sheets[sheet_idx], w, h, stock_w, stock_h, spacing)
                {
                    sheets[sheet_idx].push((x, y, w, h));
                    placements.push(Placement {
                        part_index: inst.part_index,
                        copy: inst.copy,
                        sheet: sheet_idx,
                        x_mm: x + params.edge_margin_mm,
                        y_mm: y + params.edge_margin_mm,
                        width_mm: w,
                        height_mm: h,
                        rotated: *rotated,
                        name: inst.name.clone(),
                    });
                    used_area += area;
                    continue 'instance;
                }
            }
        }

        // No fit anywhere — the part is too big for the stock.
        unplaceable.push(inst.part_index);
        // Drop the speculative new sheet we may have added.
        if let Some(last) = sheets.last() {
            if last.is_empty() {
                sheets.pop();
            }
        }
    }

    let sheets_used = sheets.len();
    let stock_area =
        sheets_used as f64 * params.stock_width_mm.max(0.0) * params.stock_height_mm.max(0.0);
    let utilization_pct = if stock_area > 0.0 {
        used_area / stock_area * 100.0
    } else {
        0.0
    };
    let per_sheet_pct: Vec<f64> = sheets
        .iter()
        .map(|placed| {
            let area: f64 = placed.iter().map(|r| r.2 * r.3).sum();
            let denom = params.stock_width_mm * params.stock_height_mm;
            if denom > 0.0 {
                area / denom * 100.0
            } else {
                0.0
            }
        })
        .collect();

    NestingResult {
        placements,
        sheets_used,
        utilization_pct,
        used_area_mm2: used_area,
        stock_area_mm2: stock_area,
        per_sheet_pct,
        unplaceable,
    }
}

/// Find the bottom-left-most position on a sheet where a `w × h`
/// rectangle fits without overlapping any existing placement.
///
/// Candidate positions are generated from the corners of existing
/// rectangles + the sheet origin. Returns the (x, y) with lowest y,
/// breaking ties by lowest x.
fn bottom_left(
    placed: &[(f64, f64, f64, f64)],
    w: f64,
    h: f64,
    stock_w: f64,
    stock_h: f64,
    spacing: f64,
) -> Option<(f64, f64)> {
    // Build candidate positions: (0,0) plus the right/top edge of every
    // placed rect (offset by spacing).
    let mut candidates: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    for r in placed {
        candidates.push((r.0 + r.2 + spacing, r.1));
        candidates.push((r.0, r.1 + r.3 + spacing));
    }
    // Sort by y, then x — bottom-left preference.
    candidates.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
    });
    for (x, y) in candidates {
        if x < 0.0 || y < 0.0 {
            continue;
        }
        if x + w > stock_w + 1e-9 || y + h > stock_h + 1e-9 {
            continue;
        }
        let mut clash = false;
        for r in placed {
            // r = (rx, ry, rw, rh). Add spacing as an inflation.
            let rx = r.0 - spacing;
            let ry = r.1 - spacing;
            let rw = r.2 + 2.0 * spacing;
            let rh = r.3 + 2.0 * spacing;
            // Treat touching as non-overlapping (the spacing inflation
            // gives us the buffer).
            if x + w <= rx + 1e-9 || y + h <= ry + 1e-9 {
                continue;
            }
            if x >= rx + rw - 1e-9 || y >= ry + rh - 1e-9 {
                continue;
            }
            clash = true;
            break;
        }
        if !clash {
            return Some((x, y));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, w: f64, h: f64, qty: u32) -> PartFootprint {
        PartFootprint {
            name: name.into(),
            width_mm: w,
            height_mm: h,
            quantity: qty,
        }
    }

    #[test]
    fn empty_input_yields_empty_result() {
        let r = nest_rectangles(&[], &NestingParams::generic());
        assert_eq!(r.placements.len(), 0);
        assert_eq!(r.sheets_used, 0);
        assert_eq!(r.utilization_pct, 0.0);
    }

    #[test]
    fn single_part_fits_on_one_sheet() {
        let parts = vec![p("base", 200.0, 100.0, 1)];
        let r = nest_rectangles(&parts, &NestingParams::generic());
        assert_eq!(r.placements.len(), 1);
        assert_eq!(r.sheets_used, 1);
        assert_eq!(r.unplaceable.len(), 0);
        assert!(r.utilization_pct > 0.0 && r.utilization_pct < 100.0);
    }

    #[test]
    fn many_small_parts_share_a_sheet() {
        // 50 small parts on a generic 4'×8' sheet → one sheet plenty.
        let parts = vec![p("tab", 30.0, 30.0, 50)];
        let r = nest_rectangles(&parts, &NestingParams::generic());
        assert_eq!(r.placements.len(), 50);
        assert_eq!(r.sheets_used, 1);
    }

    #[test]
    fn parts_too_big_for_stock_are_flagged() {
        let parts = vec![p("huge", 9000.0, 4000.0, 1), p("small", 100.0, 50.0, 1)];
        let r = nest_rectangles(&parts, &NestingParams::generic());
        // The small one places; the huge one is unplaceable.
        assert_eq!(r.placements.len(), 1);
        assert_eq!(r.unplaceable, vec![0]);
        assert_eq!(r.sheets_used, 1);
    }

    #[test]
    fn rotation_finds_a_fit_when_orientation_fails() {
        // 1200×100 part on a 1219×500 sheet only fits along the wide axis.
        let parts = vec![p("strip", 100.0, 1200.0, 1)];
        let mut params = NestingParams::generic();
        params.stock_width_mm = 1219.0;
        params.stock_height_mm = 500.0;
        params.edge_margin_mm = 0.0;
        params.spacing_mm = 0.0;
        let r = nest_rectangles(&parts, &params);
        assert_eq!(r.placements.len(), 1);
        assert!(r.placements[0].rotated, "should have rotated to fit");
    }

    #[test]
    fn spacing_is_honoured_between_neighbours() {
        let parts = vec![p("a", 100.0, 100.0, 4)];
        let mut params = NestingParams::generic();
        params.stock_width_mm = 250.0;
        params.stock_height_mm = 250.0;
        params.edge_margin_mm = 0.0;
        params.spacing_mm = 10.0;
        let r = nest_rectangles(&parts, &params);
        // 100 + 10 + 100 = 210 fits in 250; 4 parts arranged 2×2 → 1 sheet.
        assert_eq!(r.placements.len(), 4);
        assert_eq!(r.sheets_used, 1);
    }
}
