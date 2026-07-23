//! Bridge between the routing session and the GPU-resident board state
//! (GPU-router charter M0, integration half; `--features gpu`).
//!
//! Owns the CPU→GPU raster contract:
//! - builds **net-agnostic** class rasters (every copper element blocks at
//!   the class's `half_width + clearance` reach — own-net exceptions are
//!   per-search overlays, applied by the M1 kernels, which is what makes one
//!   resident raster shareable across every search of the class), and
//! - converts session dirty-grid regions into word-aligned delta rectangles
//!   so a commit ships bytes, not boards.
//!
//! The raster cell semantics are exactly the router's (BLOCKED/TIGHT/WIDE,
//! same reach arithmetic as the CPU `Raster`); a parity test pins them.

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;

use super::maze::{cell_states_for_class, class_grid_dims};

/// Cell states for one rule class over the full board, plus the geometry the
/// GPU side needs. This is the payload `vcad-kernel-gpu::router_state`
/// uploads; producing it here keeps every legality rule in the router crate.
pub struct ClassRasterSlice {
    /// Cells along X.
    pub nx: usize,
    /// Cells along Y.
    pub ny: usize,
    /// Copper layers, in stackup order.
    pub layers: Vec<PcbLayer>,
    /// Cell pitch (mm).
    pub pitch: f64,
    /// World origin of cell (0, 0).
    pub origin: [f64; 2],
    /// Layer-major cell states (`layers * ny * nx`), router CELL_* values.
    pub states: Vec<u8>,
    /// Session epoch the states reflect.
    pub epoch: u64,
}

/// Build the full-board raster for a `(half_width, clearance)` class.
///
/// Net-agnostic: passes an empty net name so every copper element blocks.
pub fn build_class_raster(
    session: &RouteSession,
    pcb: &Pcb,
    half_width: f64,
    clearance: f64,
) -> ClassRasterSlice {
    let layers: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let pitch = 2.0 * half_width + clearance;
    let (nx, ny, origin) = class_grid_dims(&pcb.outline.vertices, pitch);
    let states = cell_states_for_class(
        session,
        &pcb.outline.vertices,
        &layers,
        half_width,
        clearance,
        nx,
        ny,
        origin,
        pitch,
    );
    ClassRasterSlice {
        nx,
        ny,
        layers,
        pitch,
        origin,
        states,
        epoch: session.epoch(),
    }
}

/// A word-aligned dirty rectangle in cell space with fresh states.
pub struct ClassDelta {
    /// Inclusive min cell (x, y); `x` is 4-aligned.
    pub min: (usize, usize),
    /// Inclusive max cell (x, y); `x + 1` is 4-aligned.
    pub max: (usize, usize),
    /// Layer-major replacement states (`layers * h * w`).
    pub states: Vec<u8>,
}

/// Recompute the cells covering `world_rects` (each expanded by the class
/// reach so edge effects land inside the rect) and emit word-aligned deltas.
///
/// `world_rects` should be the session dirty regions since the resident
/// epoch — typically one small rect per committed route.
#[allow(clippy::too_many_arguments)]
pub fn class_deltas(
    session: &RouteSession,
    pcb: &Pcb,
    slice_geom: &ClassRasterSlice,
    half_width: f64,
    clearance: f64,
    world_rects: &[([f64; 2], [f64; 2])],
) -> Vec<ClassDelta> {
    let reach = half_width + clearance + slice_geom.pitch;
    let mut out = Vec::new();
    for &(lo, hi) in world_rects {
        // Cell range covering the expanded world rect, x word-aligned.
        let cx0 = (((lo[0] - reach - slice_geom.origin[0]) / slice_geom.pitch).floor()).max(0.0)
            as usize;
        let cy0 = (((lo[1] - reach - slice_geom.origin[1]) / slice_geom.pitch).floor()).max(0.0)
            as usize;
        let cx1 = ((((hi[0] + reach - slice_geom.origin[0]) / slice_geom.pitch).ceil()) as usize)
            .min(slice_geom.nx.saturating_sub(1));
        let cy1 = ((((hi[1] + reach - slice_geom.origin[1]) / slice_geom.pitch).ceil()) as usize)
            .min(slice_geom.ny.saturating_sub(1));
        if cx0 > cx1 || cy0 > cy1 {
            continue;
        }
        // Word-align the x span (4-cell granularity, clamped to the row).
        let ax0 = cx0 & !3;
        let ax1 = (((cx1 + 1).div_ceil(4) * 4) - 1).min(slice_geom.nx - 1);
        // A row whose end is not the grid edge must still be word-aligned;
        // when nx itself is not a multiple of 4 the final word is padded on
        // the GPU side, so clamp to the last full word boundary if needed.
        let ax1 = if (ax1 + 1) % 4 == 0 {
            ax1
        } else {
            // nx not divisible by 4 and rect touches the edge: shrink to the
            // last aligned column; edge cells beyond it are static border
            // (outline-blocked) so skipping them is sound.
            match (ax1 + 1) & !3 {
                0 => continue,
                b => b - 1,
            }
        };
        if ax0 > ax1 {
            continue;
        }
        let w = ax1 - ax0 + 1;
        let h = cy1 - cy0 + 1;
        let sub = cell_states_for_class_window(
            session,
            &pcb.outline.vertices,
            &slice_geom.layers,
            half_width,
            clearance,
            slice_geom.origin,
            slice_geom.pitch,
            (ax0, cy0),
            (w, h),
        );
        out.push(ClassDelta {
            min: (ax0, cy0),
            max: (ax1, cy1),
            states: sub,
        });
    }
    out
}

/// Window variant of the class raster build (same math, sub-rect output).
#[allow(clippy::too_many_arguments)]
fn cell_states_for_class_window(
    session: &RouteSession,
    outline: &[Vec2],
    layers: &[PcbLayer],
    half_width: f64,
    clearance: f64,
    origin: [f64; 2],
    pitch: f64,
    min_cell: (usize, usize),
    dims: (usize, usize),
) -> Vec<u8> {
    super::maze::cell_states_for_class_window(
        session, outline, layers, half_width, clearance, origin, pitch, min_cell, dims,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board() -> Pcb {
        serde_json::from_value(serde_json::json!({
            "outline": {"vertices": [{"x":0.0,"y":0.0},{"x":20.0,"y":0.0},
                                      {"x":20.0,"y":10.0},{"x":0.0,"y":10.0}],
                         "cutouts": [], "thickness": 1.6},
            "stackup": {"layers": [
                {"layer": "FCu", "copperThickness": 0.035},
                {"layer": "BCu", "copperThickness": 0.035}
            ]},
            "nets": [],
            "rules": {"defaultRules": {"name": "Default", "traceWidth": 0.2,
                       "clearance": 0.2, "viaDiameter": 0.6, "viaDrill": 0.3},
                      "edgeClearance": 0.2, "holeToHole": 0.2,
                      "minAnnularRing": 0.05, "minDrill": 0.1},
            "footprints": [], "traces": [
                {"start": {"x": 5.0, "y": 5.0}, "end": {"x": 15.0, "y": 5.0},
                 "width": 0.3, "layer": "FCu", "net": "A"}
            ],
            "traceArcs": [], "vias": [], "zones": []
        }))
        .expect("board")
    }

    /// Full build and a delta window agree cell-for-cell (parity contract).
    #[test]
    fn window_matches_full_build() {
        let pcb = board();
        let session = RouteSession::from_pcb(&pcb);
        let full = build_class_raster(&session, &pcb, 0.1, 0.2);
        let (w, h) = (8.min(full.nx), 4.min(full.ny));
        let sub = cell_states_for_class_window(
            &session,
            &pcb.outline.vertices,
            &full.layers,
            0.1,
            0.2,
            full.origin,
            full.pitch,
            (0, 0),
            (w, h),
        );
        for li in 0..full.layers.len() {
            for y in 0..h {
                for x in 0..w {
                    let a = sub[(li * h + y) * w + x];
                    let b = full.states[(li * full.ny + y) * full.nx + x];
                    assert_eq!(a, b, "cell ({x},{y},{li})");
                }
            }
        }
    }

    /// Deltas for the dirty rect around new copper change exactly the cells
    /// near it and reproduce the from-scratch raster (incremental = full).
    #[test]
    fn deltas_reproduce_full_rebuild() {
        let mut pcb = board();
        let session0 = RouteSession::from_pcb(&pcb);
        let before = build_class_raster(&session0, &pcb, 0.1, 0.2);

        // New copper appears.
        pcb.traces.push(Trace {
            start: Vec2::new(10.0, 2.0),
            end: Vec2::new(10.0, 8.0),
            width: 0.3,
            layer: PcbLayer::FCu,
            net: "B".into(),
            source: None,
        });
        let session1 = RouteSession::from_pcb(&pcb);
        let after_full = build_class_raster(&session1, &pcb, 0.1, 0.2);

        // Incremental: apply deltas for the new trace's world rect onto the
        // old raster.
        let deltas = class_deltas(
            &session1,
            &pcb,
            &before,
            0.1,
            0.2,
            &[([9.7, 1.7], [10.3, 8.3])],
        );
        assert!(!deltas.is_empty());
        let mut patched = before.states.clone();
        for d in &deltas {
            let (x0, y0) = d.min;
            let w = d.max.0 - x0 + 1;
            let h = d.max.1 - y0 + 1;
            for li in 0..before.layers.len() {
                for row in 0..h {
                    for col in 0..w {
                        patched[(li * before.ny + y0 + row) * before.nx + x0 + col] =
                            d.states[(li * h + row) * w + col];
                    }
                }
            }
        }
        assert_eq!(
            patched, after_full.states,
            "delta-patched raster must equal a from-scratch rebuild"
        );
    }
}
