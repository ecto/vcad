//! Standalone fillet-iteration harness for the pork-chop kidney profile.
//!
//! Runs the full pipeline (arc extrude → fillet → tessellate), then writes
//! a set of diagnostic artifacts to the given output directory:
//!
//!   porkchop.obj           — per-face OBJ groups, easy to load in MeshLab
//!   porkchop_faces.csv     — face_id, surface_kind, triangle_count
//!   porkchop_junctions.csv — per-junction trace (ball/tan points, outcome)
//!   porkchop_boundary.obj  — the mesh boundary edges as line segments
//!
//! Run with:
//!
//!   cargo run --example porkchop_diag --release -- /tmp/diag
//!
//! The output directory is created if missing. Overwrites existing files.
//! Iteration loop is tight: edit, `cargo run --example porkchop_diag`,
//! re-open `/tmp/diag/porkchop.obj` in MeshLab — ≈3s per cycle vs. ≈45s
//! for cargo + wasm-pack + browser reload.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use vcad_kernel::vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel::vcad_kernel_sketch::{SketchProfile, SketchSegment};
use vcad_kernel::Solid;
use vcad_kernel_fillet::{fillet_edges_detailed_with_trace, JunctionOutcome};
use vcad_kernel_tessellate::{tessellate_brep_by_face, TessellationParams};

fn porkchop_segments() -> Vec<SketchSegment> {
    vec![
        SketchSegment::Arc {
            start: Point2::new(45.0, 0.0),
            end: Point2::new(20.0, 40.0),
            center: Point2::new(10.0, 15.0),
            ccw: true,
        },
        SketchSegment::Arc {
            start: Point2::new(20.0, 40.0),
            end: Point2::new(-30.0, 35.0),
            center: Point2::new(-5.0, 25.0),
            ccw: true,
        },
        SketchSegment::Arc {
            start: Point2::new(-30.0, 35.0),
            end: Point2::new(-50.0, 5.0),
            center: Point2::new(-25.0, 15.0),
            ccw: true,
        },
        SketchSegment::Arc {
            start: Point2::new(-50.0, 5.0),
            end: Point2::new(-35.0, -25.0),
            center: Point2::new(-30.0, -5.0),
            ccw: true,
        },
        SketchSegment::Arc {
            start: Point2::new(-35.0, -25.0),
            end: Point2::new(10.0, -30.0),
            center: Point2::new(-10.0, -10.0),
            ccw: true,
        },
        SketchSegment::Arc {
            start: Point2::new(10.0, -30.0),
            end: Point2::new(45.0, 0.0),
            center: Point2::new(20.0, -10.0),
            ccw: true,
        },
    ]
}

fn main() {
    let out_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/porkchop_diag"));
    fs::create_dir_all(&out_dir).expect("create out dir");

    let profile = SketchProfile::new(
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        porkchop_segments(),
    )
    .expect("valid profile");
    let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 18.0)).expect("extrude ok");

    let brep_in = extruded.as_brep().expect("extrude produced brep").clone();
    let target_edges = vcad_kernel::collect_fillet_target_edges(&brep_in);
    let (brep_out, _results, trace) =
        fillet_edges_detailed_with_trace(&brep_in, &target_edges, 4.0, true);

    let params = TessellationParams::from_segments(32);
    let per_face = tessellate_brep_by_face(&brep_out, &params);

    write_obj(&out_dir.join("porkchop.obj"), &per_face);
    write_faces_csv(&out_dir.join("porkchop_faces.csv"), &per_face);
    write_junctions_csv(&out_dir.join("porkchop_junctions.csv"), &trace);

    // Weld + boundary detection on the unified mesh.
    let full = extruded.fillet(4.0).to_mesh(32);
    write_boundary_obj(&out_dir.join("porkchop_boundary.obj"), &full);

    let n_built = trace
        .junctions
        .iter()
        .filter(|j| matches!(j.outcome, JunctionOutcome::BuiltPatch { .. }))
        .count();
    println!("wrote to {}", out_dir.display());
    println!("  faces:           {}", per_face.len());
    println!(
        "  total triangles: {}",
        per_face.iter().map(|(_, _, m)| m.num_triangles()).sum::<usize>()
    );
    println!(
        "  junctions: {} considered, {} patches built",
        trace.junctions.len(),
        n_built
    );
    println!(
        "  mesh: {} tris, {} boundary edges",
        full.num_triangles(),
        full.boundary_edges().len()
    );

    // For each boundary loop centroid, print the nearest face(s) and
    // their surface kinds. This immediately tells us which faces should
    // be meeting at a given gap — e.g. "this triangular loop is between
    // a Sphere patch, a Torus blend, and a Cylinder seam that don't
    // share vertices".
    println!();
    println!("boundary-loop → nearest-face reconciliation:");
    for (i, chain) in full.boundary_loops().iter().enumerate() {
        let mut cx = 0.0_f32;
        let mut cy = 0.0_f32;
        let mut cz = 0.0_f32;
        for &v in chain {
            cx += full.vertices[v as usize * 3];
            cy += full.vertices[v as usize * 3 + 1];
            cz += full.vertices[v as usize * 3 + 2];
        }
        let n = chain.len() as f32;
        let center = Point3::new((cx / n) as f64, (cy / n) as f64, (cz / n) as f64);

        // Rank faces by squared distance from their nearest vertex.
        let mut ranked: Vec<(vcad_kernel::vcad_kernel_topo::FaceId, vcad_kernel_geom::SurfaceKind, f64)> =
            per_face
                .iter()
                .map(|(fid, kind, mesh)| {
                    let mut best = f64::INFINITY;
                    for j in 0..mesh.num_vertices() {
                        let dx = mesh.vertices[j * 3] as f64 - center.x;
                        let dy = mesh.vertices[j * 3 + 1] as f64 - center.y;
                        let dz = mesh.vertices[j * 3 + 2] as f64 - center.z;
                        let d2 = dx * dx + dy * dy + dz * dz;
                        if d2 < best {
                            best = d2;
                        }
                    }
                    (*fid, *kind, best)
                })
                .collect();
        ranked.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
        print!(
            "  loop {:2} [{} verts, center ({:.2},{:.2},{:.2})]  near:",
            i,
            chain.len(),
            center.x,
            center.y,
            center.z
        );
        for (fid, kind, d2) in ranked.iter().take(4) {
            print!(" {:?}/{:?} d={:.3}", fid, kind, d2.sqrt());
        }
        println!();
    }
}

fn write_obj(
    path: &Path,
    per_face: &[(
        vcad_kernel::vcad_kernel_topo::FaceId,
        vcad_kernel_geom::SurfaceKind,
        vcad_kernel_tessellate::TriangleMesh,
    )],
) {
    let f = File::create(path).expect("create obj");
    let mut w = BufWriter::new(f);
    writeln!(w, "# pork-chop per-face OBJ — load in MeshLab").unwrap();
    let mut vert_offset: u32 = 0;
    for (face_id, kind, mesh) in per_face {
        let group_name = format!("face_{:?}_{:?}", face_id, kind);
        writeln!(w, "g {}", group_name).unwrap();
        writeln!(w, "usemtl {:?}", kind).unwrap();
        let n_verts = mesh.num_vertices();
        for i in 0..n_verts {
            let x = mesh.vertices[3 * i];
            let y = mesh.vertices[3 * i + 1];
            let z = mesh.vertices[3 * i + 2];
            writeln!(w, "v {} {} {}", x, y, z).unwrap();
        }
        let tri_count = mesh.indices.len() / 3;
        for t in 0..tri_count {
            let a = mesh.indices[3 * t] + vert_offset + 1;
            let b = mesh.indices[3 * t + 1] + vert_offset + 1;
            let c = mesh.indices[3 * t + 2] + vert_offset + 1;
            writeln!(w, "f {} {} {}", a, b, c).unwrap();
        }
        vert_offset += n_verts as u32;
    }
}

fn write_faces_csv(
    path: &Path,
    per_face: &[(
        vcad_kernel::vcad_kernel_topo::FaceId,
        vcad_kernel_geom::SurfaceKind,
        vcad_kernel_tessellate::TriangleMesh,
    )],
) {
    let f = File::create(path).expect("create csv");
    let mut w = BufWriter::new(f);
    writeln!(w, "face_id,surface_kind,triangles,vertices").unwrap();
    for (face_id, kind, mesh) in per_face {
        writeln!(
            w,
            "{:?},{:?},{},{}",
            face_id,
            kind,
            mesh.num_triangles(),
            mesh.num_vertices()
        )
        .unwrap();
    }
}

fn write_junctions_csv(path: &Path, trace: &vcad_kernel_fillet::FilletTrace) {
    let f = File::create(path).expect("create csv");
    let mut w = BufWriter::new(f);
    writeln!(
        w,
        "vertex,vx,vy,vz,outcome,ball_x,ball_y,ball_z,tc_x,tc_y,tc_z,t0_x,t0_y,t0_z,t1_x,t1_y,t1_z"
    )
    .unwrap();
    for j in &trace.junctions {
        let (outcome, ball, tc, t0, t1) = match &j.outcome {
            JunctionOutcome::BuiltPatch {
                ball_center,
                tan_cap,
                tan_cyls,
            } => (
                "BuiltPatch",
                Some(*ball_center),
                Some(*tan_cap),
                Some(tan_cyls[0]),
                Some(tan_cyls[1]),
            ),
            JunctionOutcome::SkippedWrongEdgeCount => ("SkippedWrongEdgeCount", None, None, None, None),
            JunctionOutcome::SkippedNotPlaneCylinder => {
                ("SkippedNotPlaneCylinder", None, None, None, None)
            }
            JunctionOutcome::SkippedMixedCapNormal => {
                ("SkippedMixedCapNormal", None, None, None, None)
            }
            JunctionOutcome::SkippedNoCylCylSeam => ("SkippedNoCylCylSeam", None, None, None, None),
            JunctionOutcome::SkippedCylRadiusTooSmall { .. } => {
                ("SkippedCylRadiusTooSmall", None, None, None, None)
            }
            JunctionOutcome::SkippedCirclesDisjoint => {
                ("SkippedCirclesDisjoint", None, None, None, None)
            }
            JunctionOutcome::SkippedRadialDegenerate => {
                ("SkippedRadialDegenerate", None, None, None, None)
            }
            JunctionOutcome::SkippedDegenerateTriangle => {
                ("SkippedDegenerateTriangle", None, None, None, None)
            }
        };
        fn p(opt: Option<Point3>) -> String {
            match opt {
                Some(q) => format!("{},{},{}", q.x, q.y, q.z),
                None => ",,".to_string(),
            }
        }
        writeln!(
            w,
            "{:?},{},{},{},{},{},{},{},{}",
            j.vertex,
            j.vertex_pos.x,
            j.vertex_pos.y,
            j.vertex_pos.z,
            outcome,
            p(ball),
            p(tc),
            p(t0),
            p(t1),
        )
        .unwrap();
    }
}

fn write_boundary_obj(path: &Path, mesh: &vcad_kernel_tessellate::TriangleMesh) {
    let f = File::create(path).expect("create boundary obj");
    let mut w = BufWriter::new(f);
    writeln!(w, "# mesh boundary edges as OBJ lines").unwrap();
    let positions = mesh.boundary_edge_positions();
    for (i, pair) in positions.iter().enumerate() {
        writeln!(w, "v {} {} {}", pair[0][0], pair[0][1], pair[0][2]).unwrap();
        writeln!(w, "v {} {} {}", pair[1][0], pair[1][1], pair[1][2]).unwrap();
        writeln!(w, "l {} {}", 2 * i + 1, 2 * i + 2).unwrap();
    }
}
