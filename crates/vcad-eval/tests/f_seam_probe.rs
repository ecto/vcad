//! Scratch probe for the F-cluster seam investigation (not a regression
//! test). Separates BRep-level closure from tessellation-level closure.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

fn probe(label: &str, src: &str) {
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
    for segs in [32u32, 256] {
        let mesh = solid.to_mesh(segs);
        // exact index-based boundary edges
        let idx_boundary = {
            let mut net = std::collections::HashMap::new();
            let tris = mesh.indices.len() / 3;
            for t in 0..tris {
                for k in 0..3 {
                    let a = mesh.indices[t * 3 + k];
                    let b = mesh.indices[t * 3 + (k + 1) % 3];
                    let key = (a.min(b), a.max(b));
                    *net.entry(key).or_insert(0i64) += if a < b { 1 } else { -1 };
                }
            }
            net.values().filter(|n| **n != 0).count()
        };
        // quantized-position boundary edges (what MCP integrity sees)
        let pos_boundary = {
            let q = 1e-5;
            let vkey = |vi: usize| -> [i64; 3] {
                let mut k = [0i64; 3];
                for c in 0..3 {
                    k[c] = (mesh.vertices[vi * 3 + c] as f64 / q).round() as i64;
                }
                k
            };
            let mut net = std::collections::HashMap::new();
            let tris = mesh.indices.len() / 3;
            for t in 0..tris {
                for k in 0..3 {
                    let a = vkey(mesh.indices[t * 3 + k] as usize);
                    let b = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
                    if a == b {
                        continue;
                    }
                    if a < b {
                        *net.entry((a, b)).or_insert(0i64) += 1;
                    } else {
                        *net.entry((b, a)).or_insert(0i64) -= 1;
                    }
                }
            }
            net.values().map(|n| n.unsigned_abs()).sum::<u64>()
        };
        let mut vol = 0.0f64;
        for t in 0..mesh.indices.len() / 3 {
            let p = |i: usize| {
                let b = mesh.indices[t * 3 + i] as usize * 3;
                [
                    mesh.vertices[b] as f64,
                    mesh.vertices[b + 1] as f64,
                    mesh.vertices[b + 2] as f64,
                ]
            };
            let (a, b, c) = (p(0), p(1), p(2));
            vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0;
        }
        println!(
            "{label} segs={segs}: tris={} idx_open={} pos_open={} vol={vol:.2}",
            mesh.indices.len() / 3,
            idx_boundary,
            pos_boundary
        );
    }
}

#[test]
fn seam_probe() {
    probe(
        "one flat blade ∪ cyl",
        "[union [translate 21.5 0 0 [cube 23.5 0.5 12.57]] [cylinder 22.5 12.57]]",
    );
    probe(
        "one rotated blade ∪ cyl",
        "[union [translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]] [cylinder 22.5 13]]",
    );
    probe(
        "cube ∪ cube control",
        "[union [translate 5 5 5 [cube 10 10 10]] [cube 10 10 10]]",
    );
    probe("plain cube", "[cube 10 10 10]");
    probe("plain cylinder", "[cylinder 22.5 13]");
    probe(
        "cube − cube",
        "[difference [translate 5 5 5 [cube 10 10 10]] [cube 10 10 10]]",
    );
    probe(
        "cyl − cube (staircase-ish)",
        "[difference [translate 21.5 0 0 [cube 23.5 0.5 12.57]] [cylinder 22.5 13]]",
    );
    probe(
        "cyl − cyl (D-style)",
        "[difference [cylinder 8 12.57] [cylinder 22.5 12.57]]",
    );
    let hub = "[difference [translate 0 0 8.57 [cylinder 14 4]] \
                 [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]";
    probe("staircase hub alone", hub);
    probe(
        "hub ∪ 1 flat blade",
        &format!("[union [translate 21.5 0 0 [cube 23.5 0.5 12.57]] {hub}]"),
    );
    probe(
        "simple annulus ∪ 1 flat blade",
        "[union [translate 21.5 0 0 [cube 23.5 0.5 12.57]] \
           [difference [cylinder 8 12.57] [cylinder 22.5 12.57]]]",
    );
}
