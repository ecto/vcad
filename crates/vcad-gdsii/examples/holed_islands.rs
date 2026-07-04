//! Throwaway benchmark generator for extrude-with-holes.
//!
//! Mimics what the 2D-union GDSII bridge emits for a power-grid layer:
//! per-island `Sketch2D` + `Extrude`, where each island has a large outer
//! profile and hundreds of interior holes. Writes two equivalent documents:
//!
//! - `<out>_difference.vcad` — holes as `Difference { outer, Union(holes) }`
//!   (the representation the bridge uses while the IR lacks native holes)
//! - `<out>_holes.vcad` — holes carried natively on `Sketch2D::holes`
//!
//! ```sh
//! cargo run --release -p vcad-gdsii --example holed_islands -- /tmp/holed
//! ```
//!
//! Render both with vcad-render to compare evaluation cost.

use vcad_ir::{
    CsgOp, Document, MaterialDef, Node, NodeId, SceneEntry, SketchSegment2D, Vec2, Vec3,
};

/// Islands per document.
const ISLANDS: usize = 4;
/// Hole grid per island (HOLES_X × HOLES_Y holes).
const HOLES_X: usize = 20;
const HOLES_Y: usize = 12;
/// Sawtooth teeth on the outer profile's top edge — drives the outer
/// vertex count to ~10k like a real li1/met1 island boundary.
const TEETH: usize = 2500;

fn line(a: (f64, f64), b: (f64, f64)) -> SketchSegment2D {
    SketchSegment2D::Line {
        start: Vec2::new(a.0, a.1),
        end: Vec2::new(b.0, b.1),
    }
}

/// Closed CCW loop through `pts`.
fn closed_loop(pts: &[(f64, f64)]) -> Vec<SketchSegment2D> {
    (0..pts.len())
        .map(|i| line(pts[i], pts[(i + 1) % pts.len()]))
        .collect()
}

/// Island outer profile: a 100×50 plate whose top edge is a fine sawtooth
/// (2 vertices per tooth → ~2·TEETH+4 outer vertices).
fn island_outline(ox: f64, oy: f64) -> Vec<(f64, f64)> {
    let w = 100.0;
    let h = 50.0;
    let mut pts = vec![(ox, oy), (ox + w, oy), (ox + w, oy + h)];
    let tooth_w = w / TEETH as f64;
    for i in 0..TEETH {
        let x1 = ox + w - (i as f64 + 0.5) * tooth_w;
        let x2 = ox + w - (i as f64 + 1.0) * tooth_w;
        pts.push((x1, oy + h - 0.5));
        pts.push((x2, oy + h));
    }
    pts
}

/// Hole loops for one island: a HOLES_X × HOLES_Y grid of small squares.
fn island_holes(ox: f64, oy: f64) -> Vec<Vec<(f64, f64)>> {
    let mut holes = Vec::new();
    for i in 0..HOLES_X {
        for j in 0..HOLES_Y {
            let cx = ox + 5.0 + i as f64 * 4.5;
            let cy = oy + 4.0 + j as f64 * 3.4;
            holes.push(vec![
                (cx, cy),
                (cx + 2.0, cy),
                (cx + 2.0, cy + 1.5),
                (cx, cy + 1.5),
            ]);
        }
    }
    holes
}

struct Builder {
    doc: Document,
    next: NodeId,
}

impl Builder {
    fn alloc(&mut self, op: CsgOp) -> NodeId {
        let id = self.next;
        self.next += 1;
        self.doc.nodes.insert(id, Node { id, name: None, op });
        id
    }

    fn sketch(&mut self, outline: &[(f64, f64)], holes: Option<Vec<Vec<(f64, f64)>>>) -> NodeId {
        self.alloc(CsgOp::Sketch2D {
            origin: Vec3::new(0.0, 0.0, 0.0),
            x_dir: Vec3::new(1.0, 0.0, 0.0),
            y_dir: Vec3::new(0.0, 1.0, 0.0),
            segments: closed_loop(outline),
            holes: holes.map(|hs| hs.iter().map(|h| closed_loop(h)).collect()),
        })
    }

    fn extrude(&mut self, sketch: NodeId) -> NodeId {
        self.alloc(CsgOp::Extrude {
            sketch,
            direction: Vec3::new(0.0, 0.0, 2.0),
            twist_angle: None,
            scale_end: None,
        })
    }

    /// Balanced union tree over `ids` (mirrors the bridge's emission).
    fn union_tree(&mut self, ids: &[NodeId]) -> NodeId {
        match ids.len() {
            1 => ids[0],
            n => {
                let (l, r) = ids.split_at(n / 2);
                let (l, r) = (self.union_tree(l), self.union_tree(r));
                self.alloc(CsgOp::Union { left: l, right: r })
            }
        }
    }

    fn finish(mut self, root: NodeId) -> Document {
        self.doc.materials.insert(
            "metal".into(),
            MaterialDef {
                name: "metal".into(),
                color: [0.8, 0.7, 0.25],
                metallic: 0.3,
                roughness: 0.6,
                ..Default::default()
            },
        );
        self.doc.roots.push(SceneEntry {
            root,
            material: "metal".into(),
            visible: None,
        });
        self.doc
    }
}

fn new_builder() -> Builder {
    Builder {
        doc: Document::new(),
        next: 1,
    }
}

/// Difference-based document: per island, Extrude(outer) − Union(hole extrudes).
fn difference_doc() -> Document {
    let mut b = new_builder();
    let mut islands = Vec::new();
    for k in 0..ISLANDS {
        let (ox, oy) = ((k % 2) as f64 * 110.0, (k / 2) as f64 * 60.0);
        let outer_sketch = b.sketch(&island_outline(ox, oy), None);
        let outer = b.extrude(outer_sketch);
        let hole_ids: Vec<NodeId> = island_holes(ox, oy)
            .iter()
            .map(|h| {
                let s = b.sketch(h, None);
                b.extrude(s)
            })
            .collect();
        let holes_union = b.union_tree(&hole_ids);
        islands.push(b.alloc(CsgOp::Difference {
            left: outer,
            right: holes_union,
        }));
    }
    let root = b.union_tree(&islands.clone());
    b.finish(root)
}

/// Native-holes document: per island, Extrude(Sketch2D with holes).
fn holes_doc() -> Document {
    let mut b = new_builder();
    let mut islands = Vec::new();
    for k in 0..ISLANDS {
        let (ox, oy) = ((k % 2) as f64 * 110.0, (k / 2) as f64 * 60.0);
        let sketch = b.sketch(&island_outline(ox, oy), Some(island_holes(ox, oy)));
        islands.push(b.extrude(sketch));
    }
    let root = b.union_tree(&islands.clone());
    b.finish(root)
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "holed".into());

    let diff = difference_doc();
    let holes = holes_doc();
    let n_holes = HOLES_X * HOLES_Y;
    eprintln!(
        "{} islands, {} holes each, ~{} outer vertices each",
        ISLANDS,
        n_holes,
        2 * TEETH + 4
    );

    std::fs::write(format!("{out}_difference.vcad"), diff.to_json().unwrap()).unwrap();
    std::fs::write(format!("{out}_holes.vcad"), holes.to_json().unwrap()).unwrap();
    eprintln!("wrote {out}_difference.vcad and {out}_holes.vcad");
}
