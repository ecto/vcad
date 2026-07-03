//! Film stack → [`vcad_ir::Document`] emission.
//!
//! Both outputs use the same sketch + extrude representation the vcad-gdsii
//! bridge produces, at the same view scale: **1 µm of layout = 1 mm of
//! model** (dies are invisible at true scale).

use geo::{Coord, MultiPolygon, Polygon, Rect};
use vcad_gdsii::Library;
use vcad_ir::{
    CsgOp, Document, MaterialDef, Node, NodeId, SceneEntry, SketchSegment2D, Vec2, Vec3,
};

use crate::error::{ProcessError, Result};
use crate::masks::{flatten_um, layout_bounds, masks_in_bounds};
use crate::recipe::{Axis, CutLine, Recipe};
use crate::section::footprint_intervals;
use crate::sim::{simulate, Film, FilmKind};

/// View scale: 1 µm of layout renders as 1 mm of model.
const UM_TO_MM: f64 = 1.0;

/// Half-thickness (µm) of the slab a cross-section is drawn as.
const SECTION_HALF_UM: f64 = 0.025;

/// Extra half-thickness (µm) added per film in a cross-section so
/// stacked films never share a coplanar front face (z-fighting).
const SECTION_BIAS_UM: f64 = 0.002;

/// How far (µm) an implant slab pokes above the substrate top in the 3D
/// view so its face wins the depth test against the substrate.
const IMPLANT_PROUD_UM: f64 = 0.002;

/// Cross-section band half-width (µm): masks are clipped to
/// `position ± this` before simulation to keep the 2D booleans tiny.
const SECTION_BAND_UM: f64 = 1.0;

/// Polygons smaller than this (µm²) are dropped before emission.
const MIN_AREA_UM2: f64 = 1e-9;

/// Per-material display colors. Anything unknown falls back to a neutral
/// teal so a recipe with exotic materials still renders.
///
/// The palette is deliberately more saturated than the physical
/// materials: renderers in the vcad family key their tint strength off
/// HSV saturation (achromatic parts fall back to a house monochrome
/// ramp), and a cross-section is only useful if films are tellable
/// apart.
fn material_def(material: &str) -> MaterialDef {
    let (color, metallic, roughness) = match material {
        "silicon" => ([0.42, 0.50, 0.80], 0.1, 0.7), // gray-blue wafer
        "sio2" => ([0.88, 0.85, 0.55], 0.0, 0.9),    // pale sandy oxide
        "poly" => ([0.88, 0.24, 0.18], 0.1, 0.7),    // classic poly red
        "aluminum" => ([0.35, 0.82, 0.78], 0.9, 0.35), // bright metal teal
        "li" | "tungsten" => ([0.65, 0.32, 0.88], 0.6, 0.5), // local interconnect purple
        "ndiff" | "n+" => ([0.22, 0.78, 0.30], 0.1, 0.7), // n-type green
        "pdiff" | "p+" => ([0.95, 0.60, 0.15], 0.1, 0.7), // p-type orange
        _ => ([0.30, 0.72, 0.74], 0.2, 0.6),
    };
    MaterialDef {
        name: material.to_string(),
        color,
        metallic,
        roughness,
        ..Default::default()
    }
}

/// Incremental document builder mirroring the vcad-gdsii bridge.
struct Builder {
    doc: Document,
    next_id: NodeId,
}

impl Builder {
    fn new() -> Self {
        Self {
            doc: Document::new(),
            next_id: 1,
        }
    }

    fn alloc(&mut self, name: Option<String>, op: CsgOp) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.doc.nodes.insert(id, Node { id, name, op });
        id
    }

    /// Sketch a closed ring at `z` (mm) and extrude it up by `height` mm.
    fn ring_extrude(&mut self, ring: &geo::LineString<f64>, z_mm: f64, height_mm: f64) -> NodeId {
        let segments: Vec<SketchSegment2D> = ring
            .lines()
            .filter(|l| l.start != l.end)
            .map(|l| SketchSegment2D::Line {
                start: Vec2::new(l.start.x * UM_TO_MM, l.start.y * UM_TO_MM),
                end: Vec2::new(l.end.x * UM_TO_MM, l.end.y * UM_TO_MM),
            })
            .collect();
        let sketch = self.alloc(
            None,
            CsgOp::Sketch2D {
                origin: Vec3::new(0.0, 0.0, z_mm),
                x_dir: Vec3::new(1.0, 0.0, 0.0),
                y_dir: Vec3::new(0.0, 1.0, 0.0),
                segments,
            },
        );
        self.alloc(
            None,
            CsgOp::Extrude {
                sketch,
                direction: Vec3::new(0.0, 0.0, height_mm),
                twist_angle: None,
                scale_end: None,
            },
        )
    }

    /// One polygon (with holes) as a prism from `z0` to `z1` (µm).
    fn prism(&mut self, polygon: &Polygon<f64>, z0_um: f64, z1_um: f64) -> NodeId {
        let z_mm = z0_um * UM_TO_MM;
        let h_mm = (z1_um - z0_um) * UM_TO_MM;
        let body = self.ring_extrude(polygon.exterior(), z_mm, h_mm);
        if polygon.interiors().is_empty() {
            return body;
        }
        // Subtract holes; overshoot them vertically so the boolean never
        // has to resolve coplanar top/bottom faces.
        let overshoot = h_mm * 0.05;
        let holes: Vec<NodeId> = polygon
            .interiors()
            .iter()
            .map(|ring| self.ring_extrude(ring, z_mm - overshoot, h_mm + 2.0 * overshoot))
            .collect();
        let holes = self
            .union_tree(holes)
            .expect("non-empty interiors yield a node");
        self.alloc(
            None,
            CsgOp::Difference {
                left: body,
                right: holes,
            },
        )
    }

    /// Balanced union so deep chains never overflow recursive consumers.
    fn union_tree(&mut self, mut level: Vec<NodeId>) -> Option<NodeId> {
        while level.len() > 1 {
            let mut next = Vec::with_capacity(level.len().div_ceil(2));
            let mut iter = level.into_iter();
            while let Some(left) = iter.next() {
                match iter.next() {
                    Some(right) => next.push(self.alloc(None, CsgOp::Union { left, right })),
                    None => next.push(left),
                }
            }
            level = next;
        }
        level.pop()
    }

    /// Register `nodes` as one named part with the material for `film`.
    fn push_part(&mut self, film: &Film, nodes: Vec<NodeId>) {
        let Some(root) = self.union_tree(nodes) else {
            return;
        };
        if let Some(node) = self.doc.nodes.get_mut(&root) {
            node.name = Some(film.name.clone());
        }
        self.doc
            .materials
            .entry(film.material.clone())
            .or_insert_with(|| material_def(&film.material));
        self.doc.roots.push(SceneEntry {
            root,
            material: film.material.clone(),
            visible: None,
        });
    }
}

fn window_rect(window: [f64; 4]) -> Result<Rect<f64>> {
    let [x0, y0, x1, y1] = window;
    if !(window.iter().all(|v| v.is_finite()) && x1 > x0 && y1 > y0) {
        return Err(ProcessError::BadRecipe(format!(
            "window must be finite [x0, y0, x1, y1] with positive extent, got {window:?}"
        )));
    }
    Ok(Rect::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 }))
}

fn nonempty_polygons(footprint: &MultiPolygon<f64>) -> impl Iterator<Item = &Polygon<f64>> {
    use geo::Area;
    footprint
        .iter()
        .filter(|p| p.unsigned_area() > MIN_AREA_UM2)
}

/// Simulate `recipe` over the masks of `top_cell` and emit the 3D film
/// stack as a document — one part per surviving film, bottom-up.
///
/// `window` optionally restricts the emitted die region to
/// `[x0, y0, x1, y1]` in µm; without it the full layout bounding box is
/// used (fine for test structures, slow for real dies).
pub fn simulate_3d(
    lib: &Library,
    top_cell: &str,
    recipe: &Recipe,
    window: Option<[f64; 4]>,
) -> Result<Document> {
    let flat = flatten_um(lib, top_cell)?;
    let bounds = match window {
        Some(w) => window_rect(w)?,
        None => layout_bounds(&flat)
            .ok_or_else(|| ProcessError::BadRecipe("layout has no geometry".into()))?,
    };
    let masks = masks_in_bounds(&flat, bounds);
    let films = simulate(recipe, &masks, bounds)?;

    let mut b = Builder::new();
    for film in &films {
        // Nudge implant tops proud of the substrate so the doped slab is
        // visible instead of z-fighting inside the wafer.
        let z_top = match film.kind {
            FilmKind::Implant => film.z_top_um + IMPLANT_PROUD_UM,
            _ => film.z_top_um,
        };
        let nodes: Vec<NodeId> = nonempty_polygons(&film.footprint)
            .map(|p| b.prism(p, film.z_bottom_um, z_top))
            .collect();
        b.push_part(film, nodes);
    }
    Ok(b.doc)
}

/// Simulate `recipe` and emit the classic textbook cross-section along
/// `cut` — a thin slab per (film × surviving interval).
///
/// For an [`Axis::X`] cut render with `--view front`; for [`Axis::Y`],
/// `--view side`. Each film's slab is drawn a hair thicker than the one
/// below so coplanar slab faces never z-fight in the orthographic view.
pub fn cross_section(
    lib: &Library,
    top_cell: &str,
    recipe: &Recipe,
    cut: &CutLine,
) -> Result<Document> {
    if !(cut.position_um.is_finite()
        && cut.span.iter().all(|v| v.is_finite())
        && cut.span[0] != cut.span[1])
    {
        return Err(ProcessError::BadRecipe(format!(
            "cut line must have a finite position and a non-empty span, got {cut:?}"
        )));
    }
    let flat = flatten_um(lib, top_cell)?;
    // Only geometry within a thin band around the cut can affect it.
    let (lo, hi) = (cut.span[0].min(cut.span[1]), cut.span[0].max(cut.span[1]));
    let band = match cut.axis {
        Axis::X => Rect::new(
            Coord {
                x: lo,
                y: cut.position_um - SECTION_BAND_UM,
            },
            Coord {
                x: hi,
                y: cut.position_um + SECTION_BAND_UM,
            },
        ),
        Axis::Y => Rect::new(
            Coord {
                x: cut.position_um - SECTION_BAND_UM,
                y: lo,
            },
            Coord {
                x: cut.position_um + SECTION_BAND_UM,
                y: hi,
            },
        ),
    };
    let masks = masks_in_bounds(&flat, band);
    let films = simulate(recipe, &masks, band)?;

    let mut b = Builder::new();
    for (index, film) in films.iter().enumerate() {
        let half = SECTION_HALF_UM + index as f64 * SECTION_BIAS_UM;
        let nodes: Vec<NodeId> = footprint_intervals(&film.footprint, cut)
            .into_iter()
            .map(|[a0, a1]| {
                let rect = match cut.axis {
                    Axis::X => Rect::new(
                        Coord {
                            x: a0,
                            y: cut.position_um - half,
                        },
                        Coord {
                            x: a1,
                            y: cut.position_um + half,
                        },
                    ),
                    Axis::Y => Rect::new(
                        Coord {
                            x: cut.position_um - half,
                            y: a0,
                        },
                        Coord {
                            x: cut.position_um + half,
                            y: a1,
                        },
                    ),
                };
                b.prism(&rect.to_polygon(), film.z_bottom_um, film.z_top_um)
            })
            .collect();
        b.push_part(film, nodes);
    }
    Ok(b.doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Polarity, ProcessStep};
    use vcad_gdsii::{Cell, Element, Library};

    /// Layout: layer 1 square x 2..6, y 2..6 on a 10×10 die outline
    /// (layer 0) so bounds are stable.
    fn sample_library() -> Library {
        let mut top = Cell::new("top");
        top.elements.push(Element::Boundary {
            layer: 0,
            datatype: 0,
            xy: vec![(0, 0), (10_000, 0), (10_000, 10_000), (0, 10_000), (0, 0)],
        });
        top.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![
                (2_000, 2_000),
                (6_000, 2_000),
                (6_000, 6_000),
                (2_000, 6_000),
                (2_000, 2_000),
            ],
        });
        let mut lib = Library::new("process_test");
        lib.cells = vec![top];
        lib
    }

    fn patterned_recipe() -> Recipe {
        Recipe {
            substrate_material: "silicon".into(),
            substrate_thickness_um: 1.0,
            steps: vec![
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.2,
                },
                ProcessStep::PatternEtch {
                    mask_layer: 1,
                    polarity: Polarity::KeepMasked,
                    depth_um: 0.2,
                },
            ],
        }
    }

    #[test]
    fn simulate_3d_emits_one_part_per_film() {
        let doc = simulate_3d(&sample_library(), "top", &patterned_recipe(), None).unwrap();
        // substrate + patterned poly.
        assert_eq!(doc.roots.len(), 2);
        let names: Vec<_> = doc
            .roots
            .iter()
            .map(|r| doc.nodes[&r.root].name.clone().unwrap())
            .collect();
        assert!(names[0].contains("silicon"));
        assert!(names[1].contains("poly"));
        doc.to_json().unwrap();
    }

    #[test]
    fn window_crops_the_stack() {
        // Window that misses the poly square: only the substrate remains.
        let doc = simulate_3d(
            &sample_library(),
            "top",
            &patterned_recipe(),
            Some([7.0, 7.0, 9.0, 9.0]),
        )
        .unwrap();
        assert_eq!(doc.roots.len(), 1);
    }

    #[test]
    fn cross_section_has_gaps_where_etched() {
        let cut = CutLine {
            axis: Axis::X,
            position_um: 4.0,
            span: [0.0, 10.0],
        };
        let doc = cross_section(&sample_library(), "top", &patterned_recipe(), &cut).unwrap();
        assert_eq!(doc.roots.len(), 2);
        // The poly part is a single 4 µm slab (one interval), not blanket:
        // its sketch spans x = 2..6 mm at the 1 µm = 1 mm view scale.
        let poly_root = doc.roots[1].root;
        let mut xs: Vec<f64> = Vec::new();
        fn collect_xs(doc: &Document, id: vcad_ir::NodeId, xs: &mut Vec<f64>) {
            match &doc.nodes[&id].op {
                CsgOp::Union { left, right } | CsgOp::Difference { left, right } => {
                    collect_xs(doc, *left, xs);
                    collect_xs(doc, *right, xs);
                }
                CsgOp::Extrude { sketch, .. } => collect_xs(doc, *sketch, xs),
                CsgOp::Sketch2D { segments, .. } => {
                    for s in segments {
                        if let SketchSegment2D::Line { start, end } = s {
                            xs.push(start.x);
                            xs.push(end.x);
                        }
                    }
                }
                _ => {}
            }
        }
        collect_xs(&doc, poly_root, &mut xs);
        let min = xs.iter().fold(f64::MAX, |a, &b| a.min(b));
        let max = xs.iter().fold(f64::MIN, |a, &b| a.max(b));
        assert!((min - 2.0).abs() < 1e-9, "poly slab starts at 2 µm");
        assert!((max - 6.0).abs() < 1e-9, "poly slab ends at 6 µm");
    }

    #[test]
    fn cross_section_missing_the_mask_has_no_poly() {
        let cut = CutLine {
            axis: Axis::X,
            position_um: 8.0, // above the square
            span: [0.0, 10.0],
        };
        let doc = cross_section(&sample_library(), "top", &patterned_recipe(), &cut).unwrap();
        assert_eq!(doc.roots.len(), 1); // substrate only
    }

    #[test]
    fn rejects_bad_window_and_cut() {
        let lib = sample_library();
        let recipe = patterned_recipe();
        assert!(simulate_3d(&lib, "top", &recipe, Some([0.0, 0.0, -1.0, 1.0])).is_err());
        let cut = CutLine {
            axis: Axis::X,
            position_um: f64::NAN,
            span: [0.0, 1.0],
        };
        assert!(cross_section(&lib, "top", &recipe, &cut).is_err());
    }
}
