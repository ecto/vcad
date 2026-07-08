//! Merged cut silhouette of a flat pattern.
//!
//! Fab services (SendCutSend et al.) reject flat patterns made of disjoint
//! per-panel outlines: the bend-allowance strips between panels leave
//! "open entities that cannot be manufactured". A laser needs **one closed
//! exterior ring per part** plus its holes.
//!
//! [`silhouette`] computes that ring: union of every panel polygon plus one
//! allowance quad per bend (the quad sweeps the crease along the
//! child-pointing normal by the bend allowance, oversized by 5 µm on both
//! sides to defeat float fuzz), then snap-rounds to a 1 µm grid and drops
//! collinear vertices. Bend lines are recentered on the allowance midline
//! (the true bend center) while they're at it.
//!
//! If the union does not produce exactly one region the part is pathological
//! (a panel not actually connected through any bend chain) and we **fail
//! loudly** with the disconnected islands rather than silently emitting the
//! old per-panel format.

use crate::model::BendDirection;
use crate::poly2d::{self, Poly};
use crate::unfold::{FlatCrease, FlatPattern};
use vcad_kernel_math::{Point2, Vec2};

/// Oversize applied to each allowance quad on both sides along the bend
/// normal (mm). 5 µm — bigger than any accumulated float fuzz, far below
/// any manufacturing tolerance, and absorbed by the 1 µm snap grid.
pub const QUAD_OVERSIZE_MM: f64 = 0.005;

/// A bend line recentered on the allowance midline, ready for DXF export.
#[derive(Debug, Clone, PartialEq)]
pub struct BendLine {
    /// Line on the allowance midline in global flat 2D coords.
    pub line: (Point2, Point2),
    /// Up or Down — drives layer selection.
    pub direction: BendDirection,
    /// Backreference to the producing bend.
    pub bend_id: usize,
}

/// Merged single-part cut silhouette.
#[derive(Debug, Clone, PartialEq)]
pub struct Silhouette {
    /// The single closed exterior ring (CCW).
    pub exterior: Vec<Point2>,
    /// Hole rings (CW).
    pub holes: Vec<Vec<Point2>>,
    /// One recentered line per bend.
    pub bend_lines: Vec<BendLine>,
}

/// Why a silhouette could not be built.
#[derive(Debug, Clone, PartialEq)]
pub enum SilhouetteError {
    /// The flat pattern has no panels with usable outlines.
    Empty,
    /// The union produced multiple disconnected regions. Carries the
    /// bounding box `((min_x, min_y), (max_x, max_y))` and area of every
    /// island so the caller can name them in a diagnostic.
    DisconnectedIslands(Vec<Island>),
}

/// One disconnected region of a failed union.
#[derive(Debug, Clone, PartialEq)]
pub struct Island {
    /// Bounding box `((min_x, min_y), (max_x, max_y))` in mm.
    pub bbox: ((f64, f64), (f64, f64)),
    /// Area (mm²).
    pub area_mm2: f64,
}

impl std::fmt::Display for SilhouetteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SilhouetteError::Empty => write!(f, "flat pattern has no panel outlines"),
            SilhouetteError::DisconnectedIslands(islands) => {
                write!(
                    f,
                    "flat pattern union produced {} disconnected islands (expected one \
                     closed exterior per part); islands: ",
                    islands.len()
                )?;
                for (i, isl) in islands.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(
                        f,
                        "#{i} bbox ({:.2}, {:.2})–({:.2}, {:.2}) area {:.2} mm²",
                        isl.bbox.0 .0, isl.bbox.0 .1, isl.bbox.1 .0, isl.bbox.1 .1, isl.area_mm2
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SilhouetteError {}

/// Unit normal of a crease pointing from the parent edge toward the child
/// panel (the direction the allowance strip extends).
///
/// Read from [`FlatCrease::outward`], which `FlatPattern::from_model`
/// derives from the parent panel's frame — chained flat frames can be
/// orientation-reversing in global 2D, so the crease line's own winding
/// cannot be trusted.
pub fn crease_child_normal(crease: &FlatCrease) -> Vec2 {
    let n = crease.outward;
    let len = n.norm();
    if len < 1e-12 {
        return Vec2::new(0.0, 0.0);
    }
    n / len
}

/// Bend allowance of a crease (mm): `θ · (R + K · t)`.
pub fn crease_allowance(crease: &FlatCrease, thickness: f64) -> f64 {
    crease.angle.abs() * (crease.radius + crease.k_factor * thickness)
}

/// Build the merged silhouette of a flat pattern.
pub fn silhouette(flat: &FlatPattern) -> Result<Silhouette, SilhouetteError> {
    let mut inputs: Vec<Poly> =
        Vec::with_capacity(flat.panel_outlines_2d.len() + flat.creases.len());
    for (outline, holes) in flat.panel_outlines_2d.iter().zip(&flat.panel_holes_2d) {
        if outline.len() < 3 {
            continue;
        }
        inputs.push(Poly {
            outer: outline.clone(),
            holes: holes.clone(),
        });
    }
    if inputs.is_empty() {
        return Err(SilhouetteError::Empty);
    }
    for crease in &flat.creases {
        let n = crease_child_normal(crease);
        let a = crease_allowance(crease, flat.thickness);
        let (p0, p1) = crease.line;
        let lo = -QUAD_OVERSIZE_MM;
        let hi = a + QUAD_OVERSIZE_MM;
        inputs.push(Poly::new(vec![
            Point2::new(p0.x + n.x * lo, p0.y + n.y * lo),
            Point2::new(p1.x + n.x * lo, p1.y + n.y * lo),
            Point2::new(p1.x + n.x * hi, p1.y + n.y * hi),
            Point2::new(p0.x + n.x * hi, p0.y + n.y * hi),
        ]));
    }

    let mut merged = poly2d::union_all(&inputs);
    if merged.is_empty() {
        return Err(SilhouetteError::Empty);
    }
    if merged.len() > 1 {
        let islands = merged
            .iter()
            .map(|p| Island {
                bbox: p.bbox(),
                area_mm2: p.area(),
            })
            .collect();
        return Err(SilhouetteError::DisconnectedIslands(islands));
    }
    let part = merged.swap_remove(0);

    let bend_lines = flat
        .creases
        .iter()
        .map(|crease| {
            let n = crease_child_normal(crease);
            let half = 0.5 * crease_allowance(crease, flat.thickness);
            let (p0, p1) = crease.line;
            BendLine {
                line: (
                    Point2::new(p0.x + n.x * half, p0.y + n.y * half),
                    Point2::new(p1.x + n.x * half, p1.y + n.y * half),
                ),
                direction: crease.direction,
                bend_id: crease.bend_id,
            }
        })
        .collect();

    Ok(Silhouette {
        exterior: part.outer,
        holes: part.holes,
        bend_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::unfold::unfold;
    use std::f64::consts::FRAC_PI_2;

    fn flange(panel: usize, edge: usize) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length: 25.0,
            angle: FRAC_PI_2,
            radius: 1.0,
            direction: crate::model::BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".into(),
            manual_k: None,
        }
    }

    fn l_bracket() -> FlatPattern {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0)).unwrap();
        unfold(&mut m).unwrap();
        FlatPattern::from_model(&m)
    }

    #[test]
    fn l_bracket_merges_to_one_exterior() {
        let flat = l_bracket();
        let s = silhouette(&flat).expect("silhouette");
        assert!(s.holes.is_empty());
        // Single rectangle: 100 wide, 50 + BA + 25 tall → 4 corners after
        // collinear simplification.
        assert_eq!(s.exterior.len(), 4, "got {:?}", s.exterior);
        let ba = crease_allowance(&flat.creases[0], flat.thickness);
        let area = crate::poly2d::signed_area_f(&s.exterior);
        let expected = 100.0 * (50.0 + 25.0 + ba);
        assert!(
            (area - expected).abs() < 0.05,
            "area {area} vs expected {expected}"
        );
    }

    #[test]
    fn bend_line_sits_on_allowance_midline() {
        let flat = l_bracket();
        let s = silhouette(&flat).expect("silhouette");
        assert_eq!(s.bend_lines.len(), 1);
        let ba = crease_allowance(&flat.creases[0], flat.thickness);
        // Crease is along y=0, child extends toward -y → midline at -BA/2.
        let (a, b) = s.bend_lines[0].line;
        assert!((a.y - (-ba / 2.0)).abs() < 1e-9, "a.y = {}", a.y);
        assert!((b.y - (-ba / 2.0)).abs() < 1e-9, "b.y = {}", b.y);
    }

    #[test]
    fn u_channel_merges_with_two_bends() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0)).unwrap();
        add_edge_flange(&mut m, &table, flange(0, 2)).unwrap();
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        let s = silhouette(&flat).expect("silhouette");
        assert_eq!(s.exterior.len(), 4);
        assert_eq!(s.bend_lines.len(), 2);
    }

    #[test]
    fn holes_survive_the_union() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.panels[0].holes.push(vec![
            Point2::new(40.0, 20.0),
            Point2::new(40.0, 30.0),
            Point2::new(60.0, 30.0),
            Point2::new(60.0, 20.0),
        ]);
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0)).unwrap();
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        let s = silhouette(&flat).expect("silhouette");
        assert_eq!(s.holes.len(), 1);
    }

    #[test]
    fn disconnected_panels_fail_loudly() {
        // Hand-build a flat pattern whose second panel is nowhere near the
        // first and has no crease bridging it.
        let flat = FlatPattern {
            thickness: 1.0,
            panel_outlines_2d: vec![
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 0.0),
                    Point2::new(10.0, 10.0),
                    Point2::new(0.0, 10.0),
                ],
                vec![
                    Point2::new(100.0, 0.0),
                    Point2::new(110.0, 0.0),
                    Point2::new(110.0, 10.0),
                    Point2::new(100.0, 10.0),
                ],
            ],
            panel_holes_2d: vec![vec![], vec![]],
            creases: vec![],
            engravings_2d: vec![],
            area_mm2: 200.0,
        };
        match silhouette(&flat) {
            Err(SilhouetteError::DisconnectedIslands(islands)) => {
                assert_eq!(islands.len(), 2);
            }
            other => panic!("expected DisconnectedIslands, got {other:?}"),
        }
    }
}
