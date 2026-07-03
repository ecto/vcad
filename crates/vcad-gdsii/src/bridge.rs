//! GDS layers → vcad-ir document bridge.
//!
//! Converts a flattened GDSII layout into a [`vcad_ir::Document`]: one part
//! (scene root) per GDS layer, where every polygon on the layer becomes a
//! [`vcad_ir::CsgOp::Sketch2D`] profile extruded along +Z — the same
//! sketch + extrude representation the app's sketch pipeline produces, so
//! imported layouts are editable like any other extrude.
//!
//! Dies are tiny in CAD terms, so coordinates are scaled by a caller-chosen
//! view scale: `1 µm in the layout` becomes `view_scale / 1000` mm in the
//! document. With [`DEFAULT_VIEW_SCALE`] (1000), 1 µm renders as 1 mm.

use vcad_ir::{
    CsgOp, Document, MaterialDef, Node, NodeId, SceneEntry, SketchSegment2D, Vec2, Vec3,
};

use crate::error::{GdsError, Result};
use crate::flatten::flatten;
use crate::model::Library;

/// Default view scale: 1 µm of layout renders as 1 mm of model.
pub const DEFAULT_VIEW_SCALE: f64 = 1000.0;

/// One entry of the caller-supplied layer stack.
///
/// `(gds_layer_number, z_bottom_um, thickness_um, part_name)` — vertical
/// extent in microns of the physical film the GDS layer represents.
pub type LayerStackEntry<'a> = (i16, f64, f64, &'a str);

/// Distinct display colors cycled across layers (bottom of stack first).
const PALETTE: [[f64; 3]; 6] = [
    [0.85, 0.35, 0.25], // diffusion-ish red
    [0.30, 0.65, 0.35], // poly-ish green
    [0.30, 0.45, 0.85], // metal1-ish blue
    [0.80, 0.70, 0.25], // metal2-ish gold
    [0.60, 0.35, 0.75], // via-ish purple
    [0.35, 0.70, 0.75], // teal
];

/// Flatten `top_cell` of `lib` and build a vcad document from it.
///
/// One part per entry of `layer_stack` that has geometry: all polygons on
/// that GDS layer are extruded from `z_bottom_um` up by `thickness_um` and
/// unioned into a single named root. Stack entries whose layer has no
/// flattened polygons are skipped; flattened layers absent from the stack
/// are ignored.
///
/// `view_scale` maps layout microns to document millimeters
/// (`mm = µm · view_scale / 1000`); pass [`DEFAULT_VIEW_SCALE`] for the
/// 1 µm = 1 mm view scale.
pub fn to_vcad_document(
    lib: &Library,
    top_cell: &str,
    layer_stack: &[LayerStackEntry<'_>],
    view_scale: f64,
) -> Result<Document> {
    if !(view_scale.is_finite() && view_scale > 0.0) {
        return Err(GdsError::Unencodable(format!(
            "view_scale must be positive and finite, got {view_scale}"
        )));
    }

    let flat = flatten(lib, top_cell)?;
    // DB units → document mm.
    let db_to_mm = lib.db_unit_in_meters * 1e6 * view_scale / 1000.0;
    // Stack microns → document mm.
    let um_to_mm = view_scale / 1000.0;

    let mut doc = Document::new();
    let mut next_id: NodeId = 1;
    let mut alloc = |doc: &mut Document, name: Option<String>, op: CsgOp| -> NodeId {
        let id = next_id;
        next_id += 1;
        doc.nodes.insert(id, Node { id, name, op });
        id
    };

    for (stack_index, &(layer, z_bottom_um, thickness_um, name)) in layer_stack.iter().enumerate() {
        let Some(layer_polys) = flat.iter().find(|lp| lp.layer == layer) else {
            continue;
        };
        if layer_polys.polygons.is_empty() {
            continue;
        }

        let z_mm = z_bottom_um * um_to_mm;
        let thickness_mm = thickness_um * um_to_mm;
        let mut extrudes: Vec<NodeId> = Vec::with_capacity(layer_polys.polygons.len());

        for polygon in &layer_polys.polygons {
            let segments: Vec<SketchSegment2D> = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .map(|(a, b)| SketchSegment2D::Line {
                    start: Vec2::new(a[0] * db_to_mm, a[1] * db_to_mm),
                    end: Vec2::new(b[0] * db_to_mm, b[1] * db_to_mm),
                })
                .collect();
            let sketch = alloc(
                &mut doc,
                None,
                CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, z_mm),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments,
                },
            );
            let extrude = alloc(
                &mut doc,
                None,
                CsgOp::Extrude {
                    sketch,
                    direction: Vec3::new(0.0, 0.0, thickness_mm),
                    twist_angle: None,
                    scale_end: None,
                },
            );
            extrudes.push(extrude);
        }

        // Union the extrudes as a balanced tree, not a left chain — real
        // dies flatten to tens of thousands of polygons per layer, and a
        // chain that deep overflows the stack of any recursive consumer.
        let mut level = extrudes;
        while level.len() > 1 {
            let mut next = Vec::with_capacity(level.len().div_ceil(2));
            let mut iter = level.into_iter();
            while let Some(left) = iter.next() {
                match iter.next() {
                    Some(right) => next.push(alloc(&mut doc, None, CsgOp::Union { left, right })),
                    None => next.push(left),
                }
            }
            level = next;
        }
        let root = level
            .pop()
            .expect("layer with polygons always yields a root");
        // Name the root node after the layer so it reads well in the tree.
        if let Some(node) = doc.nodes.get_mut(&root) {
            node.name = Some(name.to_string());
        }

        let material_key = format!("gds_{name}");
        let color = PALETTE[stack_index % PALETTE.len()];
        doc.materials.insert(
            material_key.clone(),
            MaterialDef {
                name: material_key.clone(),
                color,
                metallic: 0.3,
                roughness: 0.6,
                ..Default::default()
            },
        );
        doc.roots.push(SceneEntry {
            root,
            material: material_key,
            visible: None,
        });
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Element, Strans};

    /// 1000×1000 nm square on layer 1 plus a 2000-long, 200-wide path on
    /// layer 2, instanced twice (direct + translated SREF).
    fn sample_library() -> Library {
        let mut unit = Cell::new("unit");
        unit.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)],
        });
        unit.elements.push(Element::Path {
            layer: 2,
            datatype: 0,
            pathtype: 0,
            width: 200,
            xy: vec![(500, -500), (500, 1500)],
        });

        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "unit".into(),
            strans: Strans::default(),
            origin: (0, 0),
        });
        top.elements.push(Element::Sref {
            sname: "unit".into(),
            strans: Strans::default(),
            origin: (3000, 0),
        });

        let mut lib = Library::new("bridge_test");
        lib.cells = vec![unit, top];
        lib
    }

    #[test]
    fn one_part_per_stack_layer() {
        let lib = sample_library();
        let stack = [
            (1, 0.0, 0.2, "diffusion"),
            (2, 0.3, 0.18, "poly"),
            (99, 1.0, 0.5, "unused"), // no geometry → skipped
        ];
        let doc = to_vcad_document(&lib, "top", &stack, DEFAULT_VIEW_SCALE).unwrap();

        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.materials.len(), 2);
        let names: Vec<_> = doc
            .roots
            .iter()
            .map(|r| doc.nodes[&r.root].name.clone().unwrap())
            .collect();
        assert_eq!(names, vec!["diffusion", "poly"]);
    }

    #[test]
    fn polygons_become_extruded_sketches() {
        let lib = sample_library();
        let stack = [(1, 0.0, 0.2, "diffusion")];
        let doc = to_vcad_document(&lib, "top", &stack, DEFAULT_VIEW_SCALE).unwrap();

        let sketches = doc
            .nodes
            .values()
            .filter(|n| matches!(n.op, CsgOp::Sketch2D { .. }))
            .count();
        let extrudes = doc
            .nodes
            .values()
            .filter(|n| matches!(n.op, CsgOp::Extrude { .. }))
            .count();
        let unions = doc
            .nodes
            .values()
            .filter(|n| matches!(n.op, CsgOp::Union { .. }))
            .count();
        // Two instances of the square → 2 sketches, 2 extrudes, 1 union.
        assert_eq!(sketches, 2);
        assert_eq!(extrudes, 2);
        assert_eq!(unions, 1);
        assert!(matches!(
            doc.nodes[&doc.roots[0].root].op,
            CsgOp::Union { .. }
        ));
    }

    #[test]
    fn default_scale_maps_one_um_to_one_mm() {
        let lib = sample_library(); // nm database grid
        let stack = [(1, 0.5, 0.2, "diffusion")];
        let doc = to_vcad_document(&lib, "top", &stack, DEFAULT_VIEW_SCALE).unwrap();

        let (origin, segments) = doc
            .nodes
            .values()
            .find_map(|n| match &n.op {
                CsgOp::Sketch2D {
                    origin, segments, ..
                } => Some((origin, segments)),
                _ => None,
            })
            .unwrap();
        // z_bottom 0.5 µm → 0.5 mm.
        assert_eq!(origin.z, 0.5);
        // The 1000 nm (= 1 µm) square edge → 1 mm.
        let xs: Vec<f64> = segments
            .iter()
            .map(|s| match s {
                SketchSegment2D::Line { start, .. } => start.x,
                SketchSegment2D::Arc { .. } => unreachable!(),
            })
            .collect();
        let edge =
            xs.iter().fold(f64::MIN, |a, &b| a.max(b)) - xs.iter().fold(f64::MAX, |a, &b| a.min(b));
        assert!((edge - 1.0).abs() < 1e-12);

        // Extrude thickness 0.2 µm → 0.2 mm.
        let direction = doc
            .nodes
            .values()
            .find_map(|n| match &n.op {
                CsgOp::Extrude { direction, .. } => Some(direction),
                _ => None,
            })
            .unwrap();
        assert!((direction.z - 0.2).abs() < 1e-12);
    }

    #[test]
    fn custom_scale_is_linear() {
        let lib = sample_library();
        let stack = [(1, 0.0, 0.2, "diffusion")];
        let doc = to_vcad_document(&lib, "top", &stack, 2000.0).unwrap();
        let direction = doc
            .nodes
            .values()
            .find_map(|n| match &n.op {
                CsgOp::Extrude { direction, .. } => Some(direction),
                _ => None,
            })
            .unwrap();
        assert!((direction.z - 0.4).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_scale() {
        let lib = sample_library();
        assert!(to_vcad_document(&lib, "top", &[], 0.0).is_err());
        assert!(to_vcad_document(&lib, "top", &[], f64::NAN).is_err());
    }

    #[test]
    fn document_serializes() {
        let lib = sample_library();
        let stack = [(1, 0.0, 0.2, "diffusion"), (2, 0.3, 0.18, "poly")];
        let doc = to_vcad_document(&lib, "top", &stack, DEFAULT_VIEW_SCALE).unwrap();
        let json = doc.to_json().unwrap();
        let parsed = Document::from_json(&json).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn union_tree_is_balanced_not_a_chain() {
        // 1000 instances on one layer: a left-fold chain would nest unions
        // 999 deep and overflow recursive consumers on real dies; a balanced
        // tree stays at ~log2(n) depth.
        let mut unit = Cell::new("unit");
        unit.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)],
        });
        let mut top = Cell::new("top");
        top.elements.push(Element::Aref {
            sname: "unit".into(),
            strans: Strans::default(),
            cols: 100,
            rows: 10,
            xy: [(0, 0), (20_000, 0), (0, 2_000)],
        });
        let mut lib = Library::new("depth_test");
        lib.cells = vec![unit, top];

        let doc =
            to_vcad_document(&lib, "top", &[(1, 0.0, 0.2, "l1")], DEFAULT_VIEW_SCALE).unwrap();

        fn depth(doc: &Document, id: NodeId) -> usize {
            match &doc.nodes[&id].op {
                CsgOp::Union { left, right } => 1 + depth(doc, *left).max(depth(doc, *right)),
                _ => 0,
            }
        }
        let d = depth(&doc, doc.roots[0].root);
        // ceil(log2(1000)) == 10; allow a little slack, forbid chains.
        assert!(d <= 12, "union depth {d} — expected a balanced tree");
    }
}
