//! Hierarchy flattening: resolve SREF/AREF into flat per-layer polygons.
//!
//! Output coordinates are **f64 database units** — the same grid as the
//! stored `i32` coordinates, promoted to `f64` because rotation and
//! magnification produce non-integer positions. Multiply by
//! [`Library::db_unit_in_meters`] to convert to physical units.

use std::collections::{BTreeMap, HashMap};

use crate::error::{GdsError, Result};
use crate::model::{Cell, Element, Library, Strans};

/// All flattened polygons on one GDS layer, in f64 database units.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPolygons {
    /// GDS layer number.
    pub layer: i16,
    /// Closed polygons as vertex lists (the closing edge back to the first
    /// vertex is implicit — no duplicated last point).
    pub polygons: Vec<Vec<[f64; 2]>>,
}

/// A 2D affine transform: `x' = a·x + b·y + tx`, `y' = c·x + d·y + ty`.
#[derive(Debug, Clone, Copy)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl Affine {
    const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Build the GDSII reference transform: mirror about X (`y → −y`),
    /// scale by `mag`, rotate CCW by `angle_deg`, translate by `origin`.
    fn from_strans(strans: &Strans, origin: (f64, f64)) -> Affine {
        let (cos, sin) = cos_sin_deg(strans.angle_deg);
        let m = if strans.mirror_x { -1.0 } else { 1.0 };
        let mag = strans.mag;
        Affine {
            a: mag * cos,
            b: -mag * m * sin,
            c: mag * sin,
            d: mag * m * cos,
            tx: origin.0,
            ty: origin.1,
        }
    }

    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.a * p[0] + self.b * p[1] + self.tx,
            self.c * p[0] + self.d * p[1] + self.ty,
        ]
    }

    /// `self ∘ inner`: apply `inner` first, then `self`.
    fn compose(&self, inner: &Affine) -> Affine {
        Affine {
            a: self.a * inner.a + self.b * inner.c,
            b: self.a * inner.b + self.b * inner.d,
            c: self.c * inner.a + self.d * inner.c,
            d: self.c * inner.b + self.d * inner.d,
            tx: self.a * inner.tx + self.b * inner.ty + self.tx,
            ty: self.c * inner.tx + self.d * inner.ty + self.ty,
        }
    }
}

/// Cosine/sine of an angle in degrees, exact for multiples of 90°.
fn cos_sin_deg(angle_deg: f64) -> (f64, f64) {
    let a = angle_deg.rem_euclid(360.0);
    if a == 0.0 {
        (1.0, 0.0)
    } else if a == 90.0 {
        (0.0, 1.0)
    } else if a == 180.0 {
        (-1.0, 0.0)
    } else if a == 270.0 {
        (0.0, -1.0)
    } else {
        (a.to_radians().cos(), a.to_radians().sin())
    }
}

/// Flatten `top_cell` and everything it references into per-layer polygons.
///
/// - BOUNDARY elements become polygons directly (duplicate closing point
///   dropped).
/// - PATH elements are expanded to boundary polygons by offsetting the
///   centerline by `width / 2` on each side with mitered corners. Only
///   `pathtype 0` (flush ends) is implemented; other pathtypes error.
/// - TEXT elements are annotations and are ignored.
/// - SREF/AREF are resolved recursively; cyclic references error.
///
/// Results are sorted by layer number. Datatype is not part of the output
/// key — polygons of all datatypes on a layer are merged into one bucket.
pub fn flatten(lib: &Library, top_cell: &str) -> Result<Vec<LayerPolygons>> {
    let by_name: HashMap<&str, &Cell> = lib.cells.iter().map(|c| (c.name.as_str(), c)).collect();
    let top = by_name
        .get(top_cell)
        .ok_or_else(|| GdsError::UnknownCell(top_cell.to_string()))?;

    let mut layers: BTreeMap<i16, Vec<Vec<[f64; 2]>>> = BTreeMap::new();
    let mut stack: Vec<&str> = vec![top_cell];
    flatten_cell(top, &by_name, &Affine::IDENTITY, &mut layers, &mut stack)?;

    Ok(layers
        .into_iter()
        .map(|(layer, polygons)| LayerPolygons { layer, polygons })
        .collect())
}

fn flatten_cell<'a>(
    cell: &'a Cell,
    by_name: &HashMap<&'a str, &'a Cell>,
    xform: &Affine,
    layers: &mut BTreeMap<i16, Vec<Vec<[f64; 2]>>>,
    stack: &mut Vec<&'a str>,
) -> Result<()> {
    for element in &cell.elements {
        match element {
            Element::Boundary { layer, xy, .. } => {
                let mut pts: Vec<[f64; 2]> = xy
                    .iter()
                    .map(|&(x, y)| xform.apply([f64::from(x), f64::from(y)]))
                    .collect();
                if pts.len() > 1 && pts.first() == pts.last() {
                    pts.pop();
                }
                if pts.len() >= 3 {
                    layers.entry(*layer).or_default().push(pts);
                }
            }
            Element::Path {
                layer,
                pathtype,
                width,
                xy,
                ..
            } => {
                if *pathtype != 0 {
                    return Err(GdsError::UnsupportedPathType(*pathtype));
                }
                let centerline: Vec<[f64; 2]> = xy
                    .iter()
                    .map(|&(x, y)| [f64::from(x), f64::from(y)])
                    .collect();
                let polygon = expand_path(&centerline, f64::from(width.abs()) / 2.0)?;
                let pts: Vec<[f64; 2]> = polygon.into_iter().map(|p| xform.apply(p)).collect();
                layers.entry(*layer).or_default().push(pts);
            }
            Element::Text { .. } => {}
            Element::Sref {
                sname,
                strans,
                origin,
            } => {
                let local = Affine::from_strans(strans, (f64::from(origin.0), f64::from(origin.1)));
                descend(sname, by_name, &xform.compose(&local), layers, stack)?;
            }
            Element::Aref {
                sname,
                strans,
                cols,
                rows,
                xy,
            } => {
                let cols_n = i32::from(*cols);
                let rows_n = i32::from(*rows);
                if cols_n <= 0 || rows_n <= 0 {
                    return Err(GdsError::InvalidRecord {
                        rectype: crate::record::rectype::COLROW,
                        reason: format!("AREF cols/rows must be positive, got {cols}×{rows}"),
                    });
                }
                let origin = [f64::from(xy[0].0), f64::from(xy[0].1)];
                let col_step = [
                    (f64::from(xy[1].0) - origin[0]) / f64::from(cols_n),
                    (f64::from(xy[1].1) - origin[1]) / f64::from(cols_n),
                ];
                let row_step = [
                    (f64::from(xy[2].0) - origin[0]) / f64::from(rows_n),
                    (f64::from(xy[2].1) - origin[1]) / f64::from(rows_n),
                ];
                for r in 0..rows_n {
                    for c in 0..cols_n {
                        let place = (
                            origin[0] + f64::from(c) * col_step[0] + f64::from(r) * row_step[0],
                            origin[1] + f64::from(c) * col_step[1] + f64::from(r) * row_step[1],
                        );
                        let local = Affine::from_strans(strans, place);
                        descend(sname, by_name, &xform.compose(&local), layers, stack)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn descend<'a>(
    sname: &'a str,
    by_name: &HashMap<&'a str, &'a Cell>,
    xform: &Affine,
    layers: &mut BTreeMap<i16, Vec<Vec<[f64; 2]>>>,
    stack: &mut Vec<&'a str>,
) -> Result<()> {
    if stack.contains(&sname) {
        return Err(GdsError::CircularReference(sname.to_string()));
    }
    let child = by_name
        .get(sname)
        .ok_or_else(|| GdsError::UnknownCell(sname.to_string()))?;
    stack.push(sname);
    let result = flatten_cell(child, by_name, xform, layers, stack);
    stack.pop();
    result
}

/// Expand a path centerline into a boundary polygon by offsetting `half`
/// database units to each side, with mitered interior corners and flush
/// (pathtype 0) ends.
fn expand_path(pts: &[[f64; 2]], half: f64) -> Result<Vec<[f64; 2]>> {
    if pts.len() < 2 {
        return Err(GdsError::InvalidPath(format!(
            "path needs at least 2 points, got {}",
            pts.len()
        )));
    }
    if half <= 0.0 {
        return Err(GdsError::InvalidPath("path width must be positive".into()));
    }

    // Left unit normals of each segment.
    let mut normals = Vec::with_capacity(pts.len() - 1);
    for w in pts.windows(2) {
        let dx = w[1][0] - w[0][0];
        let dy = w[1][1] - w[0][1];
        let len = (dx * dx + dy * dy).sqrt();
        if len == 0.0 {
            return Err(GdsError::InvalidPath(
                "path has coincident consecutive points".into(),
            ));
        }
        normals.push([-dy / len, dx / len]);
    }

    // One side of the offset outline (`side` = +1 for left, −1 for right).
    let offset_side = |side: f64| -> Result<Vec<[f64; 2]>> {
        let h = half * side;
        let mut out = Vec::with_capacity(pts.len());
        let first_n = normals[0];
        out.push([pts[0][0] + first_n[0] * h, pts[0][1] + first_n[1] * h]);
        for i in 1..pts.len() - 1 {
            let n0 = normals[i - 1];
            let n1 = normals[i];
            let denom = 1.0 + (n0[0] * n1[0] + n0[1] * n1[1]);
            if denom.abs() < 1e-9 {
                return Err(GdsError::InvalidPath(
                    "path doubles back on itself (180° turn)".into(),
                ));
            }
            // Standard miter: p + (n0 + n1) · h / (1 + n0·n1).
            let mx = (n0[0] + n1[0]) * h / denom;
            let my = (n0[1] + n1[1]) * h / denom;
            out.push([pts[i][0] + mx, pts[i][1] + my]);
        }
        let last = pts[pts.len() - 1];
        let last_n = normals[normals.len() - 1];
        out.push([last[0] + last_n[0] * h, last[1] + last_n[1] * h]);
        Ok(out)
    };

    let left = offset_side(1.0)?;
    let mut right = offset_side(-1.0)?;
    right.reverse();

    let mut polygon = left;
    polygon.extend(right);
    Ok(polygon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Element, Library, Strans};

    /// Unit square cell: 10×10 DB units on `layer`, corner at origin.
    fn square_cell(name: &str, layer: i16) -> Cell {
        let mut cell = Cell::new(name);
        cell.elements.push(Element::Boundary {
            layer,
            datatype: 0,
            xy: vec![(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)],
        });
        cell
    }

    fn lib_with(cells: Vec<Cell>) -> Library {
        let mut lib = Library::new("test");
        lib.cells = cells;
        lib
    }

    #[test]
    fn boundary_passthrough_drops_closing_point() {
        let lib = lib_with(vec![square_cell("sq", 1)]);
        let flat = flatten(&lib, "sq").unwrap();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].layer, 1);
        assert_eq!(
            flat[0].polygons,
            vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]
        );
    }

    #[test]
    fn sref_rotation_90_is_exact() {
        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "sq".into(),
            strans: Strans {
                mirror_x: false,
                mag: 1.0,
                angle_deg: 90.0,
            },
            origin: (100, 0),
        });
        let lib = lib_with(vec![square_cell("sq", 1), top]);
        let flat = flatten(&lib, "top").unwrap();
        // (x, y) → (100 − y, x), exactly.
        assert_eq!(
            flat[0].polygons,
            vec![vec![[100.0, 0.0], [100.0, 10.0], [90.0, 10.0], [90.0, 0.0]]]
        );
    }

    #[test]
    fn sref_mirror_negates_y() {
        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "sq".into(),
            strans: Strans {
                mirror_x: true,
                mag: 1.0,
                angle_deg: 0.0,
            },
            origin: (0, 0),
        });
        let lib = lib_with(vec![square_cell("sq", 1), top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(
            flat[0].polygons,
            vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, -10.0], [0.0, -10.0]]]
        );
    }

    #[test]
    fn sref_mirror_then_rotate_90() {
        // Mirror first (y → −y), then rotate 90° CCW: (x, y) → (y, x).
        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "sq".into(),
            strans: Strans {
                mirror_x: true,
                mag: 1.0,
                angle_deg: 90.0,
            },
            origin: (0, 0),
        });
        let lib = lib_with(vec![square_cell("sq", 1), top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(
            flat[0].polygons,
            vec![vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]]]
        );
    }

    #[test]
    fn sref_magnification_scales() {
        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "sq".into(),
            strans: Strans {
                mirror_x: false,
                mag: 2.5,
                angle_deg: 0.0,
            },
            origin: (1, 1),
        });
        let lib = lib_with(vec![square_cell("sq", 1), top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(
            flat[0].polygons,
            vec![vec![[1.0, 1.0], [26.0, 1.0], [26.0, 26.0], [1.0, 26.0]]]
        );
    }

    #[test]
    fn aref_2x3_places_six_instances() {
        let mut top = Cell::new("top");
        top.elements.push(Element::Aref {
            sname: "sq".into(),
            strans: Strans::default(),
            cols: 2,
            rows: 3,
            xy: [(0, 0), (40, 0), (0, 90)],
        });
        let lib = lib_with(vec![square_cell("sq", 1), top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(flat[0].polygons.len(), 6);
        // Column pitch 20, row pitch 30: instance (col 1, row 2) at (20, 60).
        let corners: Vec<[f64; 2]> = flat[0].polygons.iter().map(|p| p[0]).collect();
        assert!(corners.contains(&[0.0, 0.0]));
        assert!(corners.contains(&[20.0, 0.0]));
        assert!(corners.contains(&[0.0, 30.0]));
        assert!(corners.contains(&[20.0, 60.0]));
    }

    #[test]
    fn nested_srefs_compose() {
        // mid places sq at (100, 0) rotated 90°; top places mid at (0, 50).
        let mut mid = Cell::new("mid");
        mid.elements.push(Element::Sref {
            sname: "sq".into(),
            strans: Strans {
                mirror_x: false,
                mag: 1.0,
                angle_deg: 90.0,
            },
            origin: (100, 0),
        });
        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "mid".into(),
            strans: Strans::default(),
            origin: (0, 50),
        });
        let lib = lib_with(vec![square_cell("sq", 1), mid, top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(flat[0].polygons[0][0], [100.0, 50.0]);
    }

    #[test]
    fn horizontal_path_expands_to_rectangle() {
        let mut cell = Cell::new("wire");
        cell.elements.push(Element::Path {
            layer: 2,
            datatype: 0,
            pathtype: 0,
            width: 10,
            xy: vec![(0, 0), (100, 0)],
        });
        let lib = lib_with(vec![cell]);
        let flat = flatten(&lib, "wire").unwrap();
        assert_eq!(flat[0].layer, 2);
        assert_eq!(
            flat[0].polygons,
            vec![vec![[0.0, 5.0], [100.0, 5.0], [100.0, -5.0], [0.0, -5.0]]]
        );
    }

    #[test]
    fn l_path_miters_the_corner() {
        let mut cell = Cell::new("wire");
        cell.elements.push(Element::Path {
            layer: 2,
            datatype: 0,
            pathtype: 0,
            width: 10,
            xy: vec![(0, 0), (100, 0), (100, 100)],
        });
        let lib = lib_with(vec![cell]);
        let flat = flatten(&lib, "wire").unwrap();
        let polygon = &flat[0].polygons[0];
        assert_eq!(polygon.len(), 6);
        // Outer and inner miter corners of the 90° elbow.
        assert_eq!(polygon[1], [95.0, 5.0]);
        assert_eq!(polygon[4], [105.0, -5.0]);
    }

    #[test]
    fn pathtype_nonzero_is_rejected() {
        let mut cell = Cell::new("wire");
        cell.elements.push(Element::Path {
            layer: 2,
            datatype: 0,
            pathtype: 2,
            width: 10,
            xy: vec![(0, 0), (100, 0)],
        });
        let lib = lib_with(vec![cell]);
        assert!(matches!(
            flatten(&lib, "wire"),
            Err(GdsError::UnsupportedPathType(2))
        ));
    }

    #[test]
    fn cycles_are_detected() {
        let mut a = Cell::new("a");
        a.elements.push(Element::Sref {
            sname: "b".into(),
            strans: Strans::default(),
            origin: (0, 0),
        });
        let mut b = Cell::new("b");
        b.elements.push(Element::Sref {
            sname: "a".into(),
            strans: Strans::default(),
            origin: (0, 0),
        });
        let lib = lib_with(vec![a, b]);
        assert!(matches!(
            flatten(&lib, "a"),
            Err(GdsError::CircularReference(_))
        ));
    }

    #[test]
    fn unknown_top_cell_errors() {
        let lib = lib_with(vec![square_cell("sq", 1)]);
        assert!(matches!(
            flatten(&lib, "nope"),
            Err(GdsError::UnknownCell(_))
        ));
    }

    #[test]
    fn text_is_ignored() {
        let mut cell = Cell::new("t");
        cell.elements.push(Element::Text {
            layer: 63,
            texttype: 0,
            origin: (0, 0),
            strans: Strans::default(),
            string: "label".into(),
        });
        let lib = lib_with(vec![cell]);
        assert!(flatten(&lib, "t").unwrap().is_empty());
    }

    /// Golden test: a tiny "inverter-ish" two-layer layout.
    ///
    /// Cell `inv` has two diffusion boundaries (layer 1) and a poly gate
    /// path (layer 2) crossing them. The top cell places one plain copy and
    /// a 2×2 array — 3 instances total.
    #[test]
    fn golden_inverterish_layout() {
        let mut inv = Cell::new("inv");
        inv.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![(0, 0), (60, 0), (60, 40), (0, 40), (0, 0)],
        });
        inv.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![(0, 80), (60, 80), (60, 120), (0, 120), (0, 80)],
        });
        inv.elements.push(Element::Path {
            layer: 2,
            datatype: 0,
            pathtype: 0,
            width: 10,
            xy: vec![(30, -10), (30, 130)],
        });

        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "inv".into(),
            strans: Strans::default(),
            origin: (0, 0),
        });
        top.elements.push(Element::Aref {
            sname: "inv".into(),
            strans: Strans::default(),
            cols: 2,
            rows: 1,
            xy: [(200, 0), (400, 0), (200, 200)],
        });

        let lib = lib_with(vec![inv, top]);
        let flat = flatten(&lib, "top").unwrap();
        assert_eq!(flat.len(), 2);
        // 3 instances × 2 diffusion rectangles.
        assert_eq!(flat[0].layer, 1);
        assert_eq!(flat[0].polygons.len(), 6);
        // 3 instances × 1 gate path.
        assert_eq!(flat[1].layer, 2);
        assert_eq!(flat[1].polygons.len(), 3);
    }
}
