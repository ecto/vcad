//! GDS layout → plan-view masks in µm.
//!
//! Bridges [`vcad_gdsii::flatten()`] output (f64 database units) into the
//! [`Masks`] the simulator consumes: polygons are
//! scaled to µm, clipped to the region of interest, and unioned per layer
//! so downstream booleans and interval extraction see disjoint geometry.

use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon, Rect};
use vcad_gdsii::{flatten, LayerPolygons, Library};

use crate::error::Result;
use crate::sim::Masks;

/// Flatten `top_cell` and return per-layer polygons scaled to µm.
pub fn flatten_um(lib: &Library, top_cell: &str) -> Result<Vec<LayerPolygons>> {
    let db_to_um = lib.db_unit_in_meters * 1e6;
    let mut flat = flatten(lib, top_cell)?;
    for layer in &mut flat {
        for polygon in &mut layer.polygons {
            for p in polygon.iter_mut() {
                p[0] *= db_to_um;
                p[1] *= db_to_um;
            }
        }
    }
    Ok(flat)
}

/// Bounding box of all flattened geometry, `Rect` in µm. `None` when the
/// layout is empty.
pub fn layout_bounds(flat: &[LayerPolygons]) -> Option<Rect<f64>> {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for layer in flat {
        for polygon in &layer.polygons {
            for p in polygon {
                min[0] = min[0].min(p[0]);
                min[1] = min[1].min(p[1]);
                max[0] = max[0].max(p[0]);
                max[1] = max[1].max(p[1]);
            }
        }
    }
    (min[0] <= max[0]).then(|| {
        Rect::new(
            Coord {
                x: min[0],
                y: min[1],
            },
            Coord {
                x: max[0],
                y: max[1],
            },
        )
    })
}

fn ring(points: &[[f64; 2]]) -> LineString<f64> {
    LineString::from(
        points
            .iter()
            .map(|p| Coord { x: p[0], y: p[1] })
            .collect::<Vec<_>>(),
    )
}

/// Union a set of multipolygons as a balanced tree (a left fold makes the
/// intermediate result grow with every step; pairing keeps both operands
/// small).
fn union_all(mut level: Vec<MultiPolygon<f64>>) -> MultiPolygon<f64> {
    if level.is_empty() {
        return MultiPolygon::new(Vec::new());
    }
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut iter = level.into_iter();
        while let Some(a) = iter.next() {
            match iter.next() {
                Some(b) => next.push(a.union(&b)),
                None => next.push(a),
            }
        }
        level = next;
    }
    level.pop().expect("non-empty level")
}

/// Build the simulator masks: every flattened layer, clipped to `bounds`
/// and unioned into disjoint polygons.
///
/// Clipping first keeps the booleans small — a full die may carry tens of
/// thousands of polygons per layer, but a cross-section band or a 3D
/// window only intersects a handful.
pub fn masks_in_bounds(flat: &[LayerPolygons], bounds: Rect<f64>) -> Masks {
    let clip = MultiPolygon::new(vec![bounds.to_polygon()]);
    flat.iter()
        .map(|layer| {
            let clipped: Vec<MultiPolygon<f64>> = layer
                .polygons
                .iter()
                .filter(|polygon| polygon.len() >= 3)
                .filter(|polygon| {
                    // Cheap bbox rejection before the real boolean.
                    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                    for p in polygon.iter() {
                        lo[0] = lo[0].min(p[0]);
                        lo[1] = lo[1].min(p[1]);
                        hi[0] = hi[0].max(p[0]);
                        hi[1] = hi[1].max(p[1]);
                    }
                    lo[0] <= bounds.max().x
                        && hi[0] >= bounds.min().x
                        && lo[1] <= bounds.max().y
                        && hi[1] >= bounds.min().y
                })
                .map(|polygon| {
                    MultiPolygon::new(vec![Polygon::new(ring(polygon), Vec::new())])
                        .intersection(&clip)
                })
                .collect();
            (layer.layer, union_all(clipped))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{coord, Area};

    fn layer(layer: i16, polygons: Vec<Vec<[f64; 2]>>) -> LayerPolygons {
        LayerPolygons { layer, polygons }
    }

    #[test]
    fn masks_are_clipped_and_unioned() {
        // Two overlapping unit squares → union area 1.75; clipped to a
        // window covering only the left one → area 1.0 + sliver.
        let flat = vec![layer(
            1,
            vec![
                vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                vec![[0.5, 0.0], [1.5, 0.0], [1.5, 0.75], [0.5, 0.75]],
            ],
        )];
        let bounds = Rect::new(coord! { x: 0.0, y: 0.0 }, coord! { x: 2.0, y: 2.0 });
        let masks = masks_in_bounds(&flat, bounds);
        let union = &masks[&1];
        assert!((union.unsigned_area() - (1.0 + 0.5 * 0.75)).abs() < 1e-9);

        let window = Rect::new(coord! { x: 0.0, y: 0.0 }, coord! { x: 1.0, y: 2.0 });
        let masks = masks_in_bounds(&flat, window);
        assert!((masks[&1].unsigned_area() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn far_away_polygons_are_rejected() {
        let flat = vec![layer(
            2,
            vec![vec![
                [100.0, 100.0],
                [101.0, 100.0],
                [101.0, 101.0],
                [100.0, 101.0],
            ]],
        )];
        let bounds = Rect::new(coord! { x: 0.0, y: 0.0 }, coord! { x: 10.0, y: 10.0 });
        let masks = masks_in_bounds(&flat, bounds);
        assert!(masks[&2].0.is_empty());
    }

    #[test]
    fn layout_bounds_covers_all_layers() {
        let flat = vec![
            layer(1, vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]]),
            layer(2, vec![vec![[-2.0, 3.0], [4.0, 3.0], [4.0, 5.0]]]),
        ];
        let b = layout_bounds(&flat).unwrap();
        assert_eq!(b.min(), coord! { x: -2.0, y: 0.0 });
        assert_eq!(b.max(), coord! { x: 4.0, y: 5.0 });
        assert!(layout_bounds(&[]).is_none());
    }
}
