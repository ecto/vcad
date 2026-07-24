//! Ribbon-quad mesh generation from stitch data.
//!
//! Turns consecutive stitch pairs into thin quads (two triangles each) at
//! Z=0, with per-vertex thread colors. This is the render-side counterpart
//! of the format readers: parse (DST/PES) → stitches → `ribbon_mesh` →
//! triangles. The web engine previously did this in TypeScript
//! (`embroideryPatternToMesh`); the kernel is now the single source of truth
//! and the TS implementation is a fallback for older WASM builds.

/// Half of the rendered ribbon width in mm (0.3 mm total width).
pub const RIBBON_HALF_WIDTH: f64 = 0.15;

/// One stitch group ready for meshing: a resolved thread color plus stitch
/// positions in mm (embroidery Y-down convention, as parsed from DST/PES).
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonGroup {
    /// Linear RGB thread color in `[0, 1]`.
    pub color: [f32; 3],
    /// Stitch positions as `[x, y]` pairs in mm, Y-down.
    pub stitches: Vec<[f64; 2]>,
}

/// A flat triangle mesh with per-vertex colors, ready for the renderer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RibbonMesh {
    /// Flat vertex positions `[x0, y0, z0, x1, ...]` (Z always 0).
    pub positions: Vec<f32>,
    /// Flat triangle indices.
    pub indices: Vec<u32>,
    /// Flat per-vertex RGB colors `[r0, g0, b0, r1, ...]`.
    pub colors: Vec<f32>,
}

/// Generate a ribbon-quad mesh from stitch groups.
///
/// Each consecutive stitch pair becomes a quad of `2 * RIBBON_HALF_WIDTH`
/// total width; zero-length segments (< 1e-6 mm) are skipped. Stitch Y is
/// negated on the way in: embroidery formats are Y-down, CAD is Y-up.
pub fn ribbon_mesh(groups: &[RibbonGroup]) -> RibbonMesh {
    let mut mesh = RibbonMesh::default();

    for group in groups {
        let [r, g, b] = group.color;
        for pair in group.stitches.windows(2) {
            let [x0, raw_y0] = pair[0];
            let [x1, raw_y1] = pair[1];
            // Flip Y: embroidery uses Y-down, CAD uses Y-up
            let y0 = -raw_y0;
            let y1 = -raw_y1;

            let dx = x1 - x0;
            let dy = y1 - y0;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }

            // Perpendicular direction
            let px = (-dy / len) * RIBBON_HALF_WIDTH;
            let py = (dx / len) * RIBBON_HALF_WIDTH;

            let base = (mesh.positions.len() / 3) as u32;
            // 4 vertices: left-start, right-start, right-end, left-end
            #[rustfmt::skip]
            mesh.positions.extend_from_slice(&[
                (x0 + px) as f32, (y0 + py) as f32, 0.0,
                (x0 - px) as f32, (y0 - py) as f32, 0.0,
                (x1 - px) as f32, (y1 - py) as f32, 0.0,
                (x1 + px) as f32, (y1 + py) as f32, 0.0,
            ]);

            // 4 vertex colors (same thread color per quad)
            for _ in 0..4 {
                mesh.colors.extend_from_slice(&[r, g, b]);
            }

            // 2 triangles
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(color: [f32; 3], stitches: &[[f64; 2]]) -> RibbonGroup {
        RibbonGroup {
            color,
            stitches: stitches.to_vec(),
        }
    }

    #[test]
    fn single_horizontal_stitch_makes_one_quad() {
        let m = ribbon_mesh(&[group([1.0, 0.0, 0.0], &[[0.0, 0.0], [10.0, 0.0]])]);
        assert_eq!(m.positions.len(), 12);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(m.colors.len(), 12);
        // Segment along +X, perpendicular is ±Y with half-width 0.15.
        let expect = [
            [0.0, 0.15, 0.0],
            [0.0, -0.15, 0.0],
            [10.0, -0.15, 0.0],
            [10.0, 0.15, 0.0],
        ];
        for (i, e) in expect.iter().enumerate() {
            for (j, v) in e.iter().enumerate() {
                assert!((m.positions[i * 3 + j] - *v as f32).abs() < 1e-6);
            }
        }
        assert_eq!(&m.colors[0..3], &[1.0, 0.0, 0.0]);
    }

    #[test]
    fn y_is_flipped() {
        let m = ribbon_mesh(&[group([0.5; 3], &[[0.0, 1.0], [0.0, 5.0]])]);
        // Y-down input 1..5 → Y-up -1..-5; midpoints of quad ends.
        let y_start = (m.positions[1] + m.positions[4]) / 2.0;
        let y_end = (m.positions[7] + m.positions[10]) / 2.0;
        assert!((y_start - -1.0).abs() < 1e-6);
        assert!((y_end - -5.0).abs() < 1e-6);
    }

    #[test]
    fn zero_length_segments_are_skipped() {
        let m = ribbon_mesh(&[group([0.5; 3], &[[0.0, 0.0], [0.0, 0.0], [1.0, 0.0]])]);
        // Only the second pair produces a quad.
        assert_eq!(m.indices.len(), 6);
    }

    #[test]
    fn empty_and_single_stitch_groups_produce_nothing() {
        let m = ribbon_mesh(&[group([0.5; 3], &[]), group([0.5; 3], &[[1.0, 2.0]])]);
        assert_eq!(m, RibbonMesh::default());
    }
}
