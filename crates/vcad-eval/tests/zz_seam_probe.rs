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
            root_cache: None,
            mesh_segments: 0,
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
        // Dump z=0 faces near x=-22 (the 180-blade footprint).
        for (face_id, face) in &brep.topology.faces {
            let pts: Vec<_> = brep
                .topology
                .loop_half_edges(face.outer_loop)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if pts.iter().all(|p| p.z.abs() < 1e-6)
                && pts
                    .iter()
                    .any(|p| p.x.abs() > 21.0 && p.x.abs() < 23.0 && p.y.abs() < 1.0)
            {
                let near: Vec<_> = pts
                    .iter()
                    .filter(|p| p.x.abs() > 21.0 && p.x.abs() < 23.5 && p.y.abs() < 1.2)
                    .map(|p| ((p.x * 1e4).round() / 1e4, (p.y * 1e4).round() / 1e4))
                    .collect();
                println!("Z0FACE {face_id:?} nv={} near: {near:?}", pts.len());
            }
        }
        // Dump loops of faces having vertices near the z=13 rim crack.
        for (face_id, face) in &brep.topology.faces {
            let surf = &brep.geometry.surfaces[face.surface_index];
            let mut hit = false;
            let mut pts = Vec::new();
            for he in brep.topology.loop_half_edges(face.outer_loop) {
                let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
                if (p.z - 13.0).abs() < 1e-6
                    && (p.x * p.x + p.y * p.y).sqrt() > 22.4
                    && p.y < -6.0
                    && p.y > -8.0
                {
                    hit = true;
                }
                pts.push(p);
            }
            if hit {
                println!(
                    "FACE {:?} surf_kind={:?} nverts={}",
                    face_id,
                    surf.surface_type(),
                    pts.len()
                );
                for p in pts
                    .iter()
                    .filter(|p| (p.z - 13.0).abs() < 1e-6 && p.y < -6.0 && p.y > -8.0 && p.x > 20.0)
                {
                    println!("    ({:8.4},{:8.4},{:8.4})", p.x, p.y, p.z);
                }
            }
        }
    }
    if let Some(brep) = solid.as_brep() {
        for (fid, face) in &brep.topology.faces {
            let pts: Vec<_> = brep
                .topology
                .loop_half_edges(face.outer_loop)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if pts.iter().any(|p| (p.z - 10.8146).abs() < 1e-3) {
                let zmin = pts.iter().map(|p| p.z).fold(f64::MAX, f64::min);
                let zmax = pts.iter().map(|p| p.z).fold(f64::MIN, f64::max);
                println!(
                    "Z1081 {fid:?} {:?} nv={} z[{zmin:.3},{zmax:.3}]",
                    brep.geometry.surfaces[face.surface_index].surface_type(),
                    pts.len()
                );
            }
        }
        let params = vcad_kernel_tessellate::TessellationParams::from_segments(segments);
        for (fid, kind, fmesh) in vcad_kernel_tessellate::tessellate_brep_by_face(brep, &params) {
            let zmin = fmesh
                .vertices
                .chunks(3)
                .map(|c| c[2] as f64)
                .fold(f64::MAX, f64::min);
            let zmax = fmesh
                .vertices
                .chunks(3)
                .map(|c| c[2] as f64)
                .fold(f64::MIN, f64::max);
            if zmin < -0.01 || zmax > 13.01 {
                let nv = brep.topology.loop_len(brep.topology.faces[fid].outer_loop);
                println!(
                    "PHANTOM {fid:?} {kind:?} nv={nv} tess z[{zmin:.3},{zmax:.3}] tris {}",
                    fmesh.indices.len() / 3
                );
            }
        }
    }
    let mesh = solid.to_mesh(segments);
    let quantum = 1e-5;
    let vkey = |vi: usize| -> [i64; 3] {
        let mut k = [0i64; 3];
        for (c, slot) in k.iter_mut().enumerate() {
            *slot = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
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
    {
        let mut vol = 0.0;
        for t in 0..ntri {
            let p = |i: usize| {
                let b = mesh.indices[t * 3 + i] as usize * 3;
                (
                    mesh.vertices[b] as f64,
                    mesh.vertices[b + 1] as f64,
                    mesh.vertices[b + 2] as f64,
                )
            };
            let (a, b, c) = (p(0), p(1), p(2));
            vol += (a.0 * (b.1 * c.2 - b.2 * c.1) - a.1 * (b.0 * c.2 - b.2 * c.0)
                + a.2 * (b.0 * c.1 - b.1 * c.0))
                / 6.0;
        }
        println!("volume = {vol:.4}");
    }
    let mut open: Vec<_> = net
        .iter()
        .filter(|(_, &n)| n != 0)
        .map(|((a, b), &n)| {
            let p = |k: &[i64; 3]| {
                [
                    k[0] as f64 * quantum,
                    k[1] as f64 * quantum,
                    k[2] as f64 * quantum,
                ]
            };
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
    for ang in [0.0, 90.0, 180.0, 270.0] {
        src = format!(
            "[union [rotate 0 0 {ang} [translate 21.50 0 0 [rotate 39.29 0 0 \
               [cube 23.50 0.5 12.57]]]] {src}]"
        );
    }
    probe(&src, 256);
}

#[test]
fn zz_probe_flat_blade() {
    probe(
        "[union [translate 21.5 0 0 [cube 23.5 0.5 12.57]] [cylinder 22.5 12.57]]",
        256,
    );
}

#[test]
fn zz_probe_flat_two() {
    let mut src = String::from("[cylinder 22.5 12.57]");
    for ang in [0.0, 180.0] {
        src =
            format!("[union [rotate 0 0 {ang} [translate 21.5 0 0 [cube 23.5 0.5 12.57]]] {src}]");
    }
    probe(&src, 256);
}

#[test]
fn zz_probe_f2() {
    // Staircase hub from the f2 test: cylinder minus bore minus counterbore,
    // then union a single flat blade.
    let hub = "[difference [translate 0 0 8.57 [cylinder 14 4]] \
                 [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]";
    let src = format!("[union [translate 21.5 0 0 [cube 23.5 0.5 12.57]] {hub}]");
    probe(&src, 256);
}

#[test]
fn zz_probe_hub_only() {
    probe(
        "[difference [translate 0 0 8.57 [cylinder 14 4]] \
          [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]",
        256,
    );
}

#[test]
fn zz_probe_ring() {
    probe("[difference [cylinder 8 12.57] [cylinder 22.5 12.57]]", 256);
}
