//! Fab-ready GDS export of binarized topology designs via `vcad-gdsii`.
//!
//! The FDTD design is **pixel geometry** — ε samples on Δ-squares — and
//! the GDS says so honestly: the binarized density (ρ̂ ≥ ½) is exported
//! as its exact pixel footprint, decomposed into **maximal horizontal
//! rectangles** (greedy row-run merging). Rectangle decomposition never
//! produces hole polygons (GDS BOUNDARY cannot express holes; abutting
//! same-layer rectangles union in mask prep), every shape is convex and
//! DRC-trivial, and what the fab receives is exactly what the solver
//! simulated — no smoothing that would un-validate the prediction.
//!
//! Units: 1 crate length unit = 1 µm by the examples' convention, so the
//! library uses the conventional GDS µm/nm system (database unit 1 nm).
//! A Δ = λ/40 = 38.75 nm pixel rounds to the nm grid with ≤ 0.5 nm
//! placement error per edge — recorded in the tape-out notes (M6).

use crate::design::TopologyParam;
use vcad_gdsii::{Cell, Element, Library};

/// One axis-aligned rectangle in length units (µm by convention):
/// `(x0, y0, x1, y1)`.
pub type Rect = (f64, f64, f64, f64);

/// Decompose a binary pixel grid (`nx × ny`, row-major, `y` fastest —
/// [`crate::grid::Field2`] layout) into maximal horizontal-run rectangles
/// `(i0, j0, i1, j1)` in **exclusive** pixel coordinates.
pub fn decompose_rects(solid: &[bool], nx: usize, ny: usize) -> Vec<(usize, usize, usize, usize)> {
    assert_eq!(solid.len(), nx * ny);
    // Runs along x for each y row: a run is a maximal [i0, i1) of solid.
    // Merge runs with identical spans across consecutive rows.
    let mut out = Vec::new();
    // Active rects from the previous row: (i0, i1, j_start).
    let mut active: Vec<(usize, usize, usize)> = Vec::new();
    for j in 0..=ny {
        // Runs of this row (empty row past the end flushes everything).
        let mut runs: Vec<(usize, usize)> = Vec::new();
        if j < ny {
            let mut i = 0;
            while i < nx {
                if solid[i * ny + j] {
                    let s = i;
                    while i < nx && solid[i * ny + j] {
                        i += 1;
                    }
                    runs.push((s, i));
                } else {
                    i += 1;
                }
            }
        }
        let mut next: Vec<(usize, usize, usize)> = Vec::new();
        for &(r0, r1) in &runs {
            if let Some(&(a0, a1, js)) = active.iter().find(|&&(a0, a1, _)| a0 == r0 && a1 == r1) {
                next.push((a0, a1, js)); // continues
            } else {
                next.push((r0, r1, j)); // new rect starts here
            }
        }
        for &(a0, a1, js) in &active {
            let continues = runs.iter().any(|&(r0, r1)| r0 == a0 && r1 == a1);
            if !continues {
                out.push((a0, js, a1, j));
            }
        }
        active = next;
    }
    out
}

/// Export a binarized topology design plus explicit waveguide rectangles
/// to a GDS library (single cell, one layer).
///
/// `guides` are physical rectangles (µm) — the access waveguides the
/// design connects to. The design pixels are the Δ-squares centered on
/// the region's Ez samples.
pub fn design_to_gds(
    topo: &TopologyParam,
    delta: f64,
    guides: &[Rect],
    layer: i16,
    cell_name: &str,
) -> Library {
    let (rx, ry) = (topo.region.ns_x(), topo.region.ns_y());
    let solid: Vec<bool> = topo.projected().iter().map(|&p| p >= 0.5).collect();
    let mut cell = Cell::new(cell_name);
    let nm = |v: f64| (v * 1000.0).round() as i32;
    // Design pixels: sample (di, dj) is centered at ((i0+di)Δ, (j0+dj)Δ).
    let ox = topo.region.i0 as f64 * delta - 0.5 * delta;
    let oy = topo.region.j0 as f64 * delta - 0.5 * delta;
    for (i0, j0, i1, j1) in decompose_rects(&solid, rx, ry) {
        let (x0, y0) = (ox + i0 as f64 * delta, oy + j0 as f64 * delta);
        let (x1, y1) = (ox + i1 as f64 * delta, oy + j1 as f64 * delta);
        cell.elements
            .push(rect_boundary(layer, nm(x0), nm(y0), nm(x1), nm(y1)));
    }
    for &(x0, y0, x1, y1) in guides {
        cell.elements
            .push(rect_boundary(layer, nm(x0), nm(y0), nm(x1), nm(y1)));
    }
    let mut lib = Library::new(cell_name);
    lib.cells.push(cell);
    lib
}

fn rect_boundary(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary {
        layer,
        datatype: 0,
        xy: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjoint::DesignRegion;

    #[test]
    fn solid_block_is_one_rectangle() {
        // 5×4 grid with a 3×2 solid block.
        let (nx, ny) = (5, 4);
        let mut solid = vec![false; nx * ny];
        for i in 1..4 {
            for j in 1..3 {
                solid[i * ny + j] = true;
            }
        }
        let rects = decompose_rects(&solid, nx, ny);
        assert_eq!(rects, vec![(1, 1, 4, 3)]);
    }

    #[test]
    fn donut_decomposes_without_holes_and_conserves_area() {
        // 6×6 ring: solid frame, empty 2×2 center.
        let (nx, ny) = (6, 6);
        let mut solid = vec![true; nx * ny];
        for i in 2..4 {
            for j in 2..4 {
                solid[i * ny + j] = false;
            }
        }
        let rects = decompose_rects(&solid, nx, ny);
        let area: usize = rects
            .iter()
            .map(|&(i0, j0, i1, j1)| (i1 - i0) * (j1 - j0))
            .sum();
        assert_eq!(area, 36 - 4);
        // No rect covers the hole.
        for &(i0, j0, i1, j1) in &rects {
            assert!(!(i0 <= 2 && i1 >= 4 && j0 <= 2 && j1 >= 4), "hole covered");
        }
    }

    #[test]
    fn gds_round_trips_and_conserves_pixel_area() {
        use vcad_gdsii::{flatten, read_library, write_library};
        let region = DesignRegion {
            i0: 20,
            i1: 39,
            j0: 10,
            j1: 24,
        };
        let mut topo = TopologyParam::uniform(region, 0.0, 2.0736, 12.1104);
        topo.beta = 64.0; // hard binarization for a clean pixel count
                          // A blob: densities above threshold inside a disc.
        let (rx, ry) = (region.ns_x(), region.ns_y());
        for di in 0..rx {
            for dj in 0..ry {
                let (x, y) = (di as f64 - 9.5, dj as f64 - 7.0);
                if x * x + y * y < 36.0 {
                    topo.rho[di * ry + dj] = 1.0;
                }
            }
        }
        topo.filter_radius_cells = 0.0; // exact pixel control for the test
        let delta = 1.55 / 40.0;
        let lib = design_to_gds(&topo, delta, &[(0.0, 0.5, 0.7, 0.72)], 1, "splitter_test");
        let bytes = write_library(&lib).unwrap();
        let back = read_library(&bytes).unwrap();
        assert_eq!(back, lib);
        let flat = flatten(&back, "splitter_test").unwrap();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].layer, 1);
        // Shoelace area over all polygons (db units = nm).
        let mut area_nm2 = 0.0f64;
        for poly in &flat[0].polygons {
            let mut a = 0.0f64;
            for k in 0..poly.len() {
                let [x0, y0] = poly[k];
                let [x1, y1] = poly[(k + 1) % poly.len()];
                a += x0 * y1 - x1 * y0;
            }
            area_nm2 += a.abs() / 2.0;
        }
        let solid_pixels = topo.projected().iter().filter(|&&p| p >= 0.5).count();
        let expected_design = solid_pixels as f64 * (delta * 1000.0) * (delta * 1000.0);
        let guide_area = 700.0 * 220.0;
        let rel = (area_nm2 - expected_design - guide_area).abs() / (expected_design + guide_area);
        // nm-grid rounding of pixel corners is the only error source.
        assert!(
            rel < 2e-2,
            "GDS area {area_nm2} vs pixels {expected_design} + guide {guide_area} (rel {rel})"
        );
        assert!(solid_pixels > 50, "blob did not binarize");
    }
}
