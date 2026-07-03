//! THROWAWAY: like sky130_import but emits one scene root per polygon
//! (no unions at all) — trades document tidiness for render speed.
//! Usage: flat_import <in.gds> <out.vcad> [top]

use vcad_gdsii::{flatten, read_library};
use vcad_ir::{
    CsgOp, Document, MaterialDef, Node, NodeId, SceneEntry, SketchSegment2D, Vec2, Vec3,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let gds_path = args.next().expect("arg 1: in.gds");
    let vcad_path = args.next().expect("arg 2: out.vcad");
    let top = args.next().expect("arg 3: top cell");

    // Optional µm-space crop window: WINDOW="x0,y0,x1,y1" (µm)
    let window: Option<[f64; 4]> = std::env::var("WINDOW").ok().map(|w| {
        let v: Vec<f64> = w.split(',').map(|s| s.parse().expect("WINDOW")).collect();
        [v[0], v[1], v[2], v[3]]
    });

    let bytes = std::fs::read(&gds_path).expect("read gds");
    let lib = read_library(&bytes).expect("parse gds");
    let flat = flatten(&lib, &top).expect("flatten");
    let db_to_mm = lib.db_unit_in_meters * 1e6; // 1 µm = 1 mm view scale

    // (gds layer, z_bottom_mm, thickness_mm, name, rgb)
    let stack: [(i16, f64, f64, &str, [f64; 3]); 5] = [
        (65, 0.00, 0.12, "diff", [0.85, 0.35, 0.25]),
        (66, 0.30, 0.18, "poly", [0.30, 0.65, 0.35]),
        (67, 0.94, 0.10, "li1", [0.60, 0.35, 0.75]),
        (68, 1.38, 0.36, "met1", [0.30, 0.45, 0.85]),
        (69, 2.00, 0.36, "met2", [0.80, 0.70, 0.25]),
    ];

    let mut doc = Document::new();
    let mut next_id: NodeId = 1;
    for &(layer, z, t, name, color) in &stack {
        let Some(lp) = flat.iter().find(|lp| lp.layer == layer) else {
            continue;
        };
        let key = format!("gds_{name}");
        doc.materials.insert(
            key.clone(),
            MaterialDef {
                name: key.clone(),
                color,
                metallic: 0.3,
                roughness: 0.6,
                ..Default::default()
            },
        );
        for polygon in &lp.polygons {
            let mut polygon = polygon.clone();
            if let Some([x0, y0, x1, y1]) = window {
                let inside = polygon.iter().any(|p| {
                    let (x, y) = (p[0] * db_to_mm, p[1] * db_to_mm);
                    x >= x0 && x <= x1 && y >= y0 && y <= y1
                });
                if !inside {
                    continue;
                }
                // Clamp crossing polygons to the window (cleaved-die edge);
                // drop anything that degenerates to zero area.
                for p in &mut polygon {
                    p[0] = p[0].clamp(x0 / db_to_mm, x1 / db_to_mm);
                    p[1] = p[1].clamp(y0 / db_to_mm, y1 / db_to_mm);
                }
                let area2: f64 = polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                    .sum();
                if area2.abs() < 1e-6 {
                    continue;
                }
            }
            let polygon = &polygon;
            let segments: Vec<SketchSegment2D> = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .map(|(a, b)| SketchSegment2D::Line {
                    start: Vec2::new(a[0] * db_to_mm, a[1] * db_to_mm),
                    end: Vec2::new(b[0] * db_to_mm, b[1] * db_to_mm),
                })
                .collect();
            let sketch = next_id;
            doc.nodes.insert(
                sketch,
                Node {
                    id: sketch,
                    name: None,
                    op: CsgOp::Sketch2D {
                        origin: Vec3::new(0.0, 0.0, z),
                        x_dir: Vec3::new(1.0, 0.0, 0.0),
                        y_dir: Vec3::new(0.0, 1.0, 0.0),
                        segments,
                    },
                },
            );
            let extrude = sketch + 1;
            next_id += 2;
            doc.nodes.insert(
                extrude,
                Node {
                    id: extrude,
                    name: None,
                    op: CsgOp::Extrude {
                        sketch,
                        direction: Vec3::new(0.0, 0.0, t),
                        twist_angle: None,
                        scale_end: None,
                    },
                },
            );
            doc.roots.push(SceneEntry {
                root: extrude,
                material: key.clone(),
                visible: None,
            });
        }
    }
    println!("roots: {}", doc.roots.len());
    std::fs::write(&vcad_path, doc.to_json().expect("json")).expect("write");
    println!("wrote {vcad_path}");
}
