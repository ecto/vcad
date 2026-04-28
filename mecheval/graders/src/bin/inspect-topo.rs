//! Diagnostic: dump the BRep topology of a .vcad's evaluated solids.
//! Useful for chasing kernel bugs surfaced by mecheval grading.

use mecheval_grader::eval::evaluate_vcad;
use vcad_kernel_geom::SurfaceKind;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: inspect-topo <file.vcad>");
    let raw = std::fs::read_to_string(&path).expect("read");
    let snap = evaluate_vcad(&raw);
    if let Some(err) = &snap.fatal {
        eprintln!("eval error: {}", err);
        std::process::exit(1);
    }
    println!("solids: {}", snap.solids.len());
    for (i, solid) in snap.solids.iter().enumerate() {
        let Some(brep) = solid.as_brep() else {
            println!("solid {} mesh-only", i);
            continue;
        };
        let topo = &brep.topology;
        let geom = &brep.geometry;
        println!(
            "\n=== solid {}  vol={:.2}  V={} E={} F={} S={} ===",
            i,
            solid.volume(),
            topo.vertices.len(),
            topo.edges.len(),
            topo.faces.len(),
            topo.shells.len(),
        );
        for (face_id, face) in &topo.faces {
            let surf = geom
                .surfaces
                .get(face.surface_index)
                .map(|s| s.surface_type())
                .unwrap_or(SurfaceKind::Plane);
            let outer_len = topo.loop_len(face.outer_loop);
            let inner_lens: Vec<usize> =
                face.inner_loops.iter().map(|&l| topo.loop_len(l)).collect();
            println!(
                "  {:?}: surf={:?} outer={}vts inner={:?} orient={:?}",
                face_id, surf, outer_len, inner_lens, face.orientation
            );
        }
    }
}
