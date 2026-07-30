//! Temporary probe: dump open-edge locations for the seam-crack family.
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

fn probe(src: &str, segments: u32) {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    if let Some(brep) = solid.as_brep() {
        let mut unpaired = 0usize;
        for (he_id, he) in &brep.topology.half_edges {
            if he.loop_id.is_some() && he.twin.is_none() {
                unpaired += 1;
                let a = brep.topology.vertices[he.origin].point;
                let b = brep.topology.vertices[brep.topology.half_edge_dest(he_id)].point;
                let face = he
                    .loop_id
                    .and_then(|l| brep.topology.loops[l].face)
                    .map(|f| {
                        let fc = &brep.topology.faces[f];
                        format!(
                            "{:?} {:?} nv={}",
                            f,
                            brep.geometry.surfaces[fc.surface_index].surface_type(),
                            brep.topology.loop_len(fc.outer_loop)
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "  TOPO unpaired ({:8.4},{:8.4},{:8.4}) -> ({:8.4},{:8.4},{:8.4}) face {}",
                    a.x, a.y, a.z, b.x, b.y, b.z, face
                );
            }
        }
        println!("TOPO: {} unpaired half-edges", unpaired);
        // Dump full loops of faces owning unpaired half-edges.
        let mut bad_faces = std::collections::HashSet::new();
        for (_, he) in &brep.topology.half_edges {
            if he.loop_id.is_some() && he.twin.is_none() {
                if let Some(f) = he.loop_id.and_then(|l| brep.topology.loops[l].face) {
                    bad_faces.insert(f);
                }
            }
        }
        for f in &bad_faces {
            let fc = &brep.topology.faces[*f];
            let surf = &brep.geometry.surfaces[fc.surface_index];
            println!("BADFACE {:?} {:?}", f, surf.surface_type());
            for he in brep.topology.loop_half_edges(fc.outer_loop) {
                let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
                let tw = brep.topology.half_edges[he].twin.is_some();
                println!("    ({:8.4},{:8.4},{:8.4}) twin={}", p.x, p.y, p.z, tw);
            }
        }
        // Dump loops of faces having vertices near the z=13 rim crack.
        for (face_id, face) in &brep.topology.faces {
            let surf = &brep.geometry.surfaces[face.surface_index];
            let mut hit = false;
            let mut pts = Vec::new();
            for he in brep.topology.loop_half_edges(face.outer_loop) {
                let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
                if (p.z - 13.0).abs() < 1e-6 && (p.x * p.x + p.y * p.y).sqrt() > 22.4
                    && p.y < -6.0 && p.y > -8.0
                {
                    hit = true;
                }
                pts.push(p);
            }
            if hit {
                println!(
                    "FACE {:?} surf_kind={} nverts={}",
                    face_id,
                    format!("{:?}", surf.surface_type()),
                    pts.len()
                );
                for p in pts.iter().filter(|p| {
                    (p.z - 13.0).abs() < 1e-6 && p.y < -6.0 && p.y > -8.0 && p.x > 20.0
                }) {
                    println!("    ({:8.4},{:8.4},{:8.4})", p.x, p.y, p.z);
                }
            }
        }
    }
    let mesh = solid.to_mesh(segments);
    let quantum = 1e-5;
    let vkey = |vi: usize| -> [i64; 3] {
        let mut k = [0i64; 3];
        for c in 0..3 {
            k[c] = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
        }
        k
    };
    let ntri = mesh.indices.len() / 3;
    let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
        std::collections::HashMap::new();
    for t in 0..ntri {
        for k in 0..3 {
            let a = vkey(mesh.indices[t * 3 + k] as usize);
            let b = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
            if a == b {
                continue;
            }
            if a < b {
                *net.entry((a, b)).or_default() += 1;
            } else {
                *net.entry((b, a)).or_default() -= 1;
            }
        }
    }
    let mut open: Vec<_> = net
        .iter()
        .filter(|(_, &n)| n != 0)
        .map(|((a, b), &n)| {
            let p = |k: &[i64; 3]| [k[0] as f64 * quantum, k[1] as f64 * quantum, k[2] as f64 * quantum];
            (p(a), p(b), n)
        })
        .collect();
    open.sort_by(|x, y| x.partial_cmp(y).unwrap());
    println!("segments={segments}: {} open edges", open.len());
    for (a, b, n) in &open {
        let r_a = (a[0] * a[0] + a[1] * a[1]).sqrt();
        let r_b = (b[0] * b[0] + b[1] * b[1]).sqrt();
        println!(
            "  ({:8.4},{:8.4},{:8.4}) -> ({:8.4},{:8.4},{:8.4}) net={n} r=({r_a:.4},{r_b:.4})",
            a[0], a[1], a[2], b[0], b[1], b[2]
        );
    }
}

#[test]
fn zz_probe_rotated_blades() {
    let mut src = String::from("[cylinder 22.5 13]");
    for ang in [0.0] {
        src = format!(
            "[union [rotate 0 0 {ang} [translate 21.50 0 0 [rotate 39.29 0 0 \
               [cube 23.50 0.5 12.57]]]] {src}]"
        );
    }
    probe(&src, 256);
}
