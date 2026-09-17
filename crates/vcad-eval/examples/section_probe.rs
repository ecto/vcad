//! Section ONE solve two ways — the raw kernel tessellation and the mesh that
//! export produces from it — and diff both against a 2D ground truth.
//!
//! ```text
//! cargo run -p vcad-eval --release --example section_probe -- \
//!     part.loon 11.4,14.1,16.8 [truth.dxf]
//! ```
//!
//! The point is to settle which reported defects belong to the B-rep and
//! which the export repair introduces or hides. `Solid::to_mesh` runs
//! `repair_export_mesh` (watertightness repair + re-projection) over the
//! tessellation; nothing else differs between the two meshes here, so any
//! disagreement is that pass.
//!
//! Every number it prints is a geometric assertion candidate: region and hole
//! counts, section area, healed-gap widths, and — with a DXF — symmetric
//! difference and max boundary distance against the 2D CSG evaluated exactly.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel::vcad_kernel_cam::outline::{
    compare_outlines, read_dxf, section_at_z, CompareOptions, SectionOptions,
};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};
use vcad_loon::eval_vcad;

const SEGMENTS: u32 = 256;

fn positions(mesh: &TriangleMesh) -> (Vec<[f64; 3]>, Vec<u32>) {
    let pts = (0..mesh.vertices.len() / 3)
        .map(|i| {
            [
                mesh.vertices[i * 3] as f64,
                mesh.vertices[i * 3 + 1] as f64,
                mesh.vertices[i * 3 + 2] as f64,
            ]
        })
        .collect();
    (pts, mesh.indices.clone())
}

fn report(label: &str, mesh: &TriangleMesh, heights: &[f64], truth: Option<&str>) {
    let (pts, idx) = positions(mesh);
    let rep = vcad_kernel::vcad_kernel_booleans::mesh_report(mesh);
    println!(
        "\n[{label}] {} triangles, volume {:.3} mm^3, open {} over-used {}",
        idx.len() / 3,
        rep.signed_volume,
        rep.open_edges,
        rep.overused_edges
    );
    // The default heal is the tessellation-scale tolerance: a section that
    // needs more than that is reporting a real hole, not round-off.
    let heal: f64 = std::env::var("HEAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-3);
    let opts = SectionOptions {
        weld_tolerance: 1e-4,
        heal_tolerance: heal,
        ..SectionOptions::default()
    };
    for &z in heights {
        match section_at_z(&pts, &idx, z, &opts) {
            Err(e) => println!("  z {z:6.3}: REFUSED — {e}"),
            Ok(outline) => {
                print!(
                    "  z {z:6.3}: {} region(s) {} hole(s) area {:9.4} healed {}",
                    outline.regions.len(),
                    outline.holes().len(),
                    outline.area(),
                    outline.healed.len()
                );
                if let Some(w) = outline
                    .healed
                    .iter()
                    .map(|h| h.distance)
                    .fold(None, |m: Option<f64>, w| Some(m.map_or(w, |m| m.max(w))))
                {
                    print!(" (worst {w:.5} mm)");
                }
                let circles = outline.circular_holes(1e-3);
                print!(" circles {}", circles.len());
                if let Some(path) = truth {
                    match std::fs::read_to_string(path)
                        .ok()
                        .and_then(|t| read_dxf(&t).ok())
                    {
                        None => print!("  [truth unreadable]"),
                        Some(dxf) => {
                            let d = compare_outlines(&dxf, &outline, &CompareOptions::default());
                            print!(
                                "  vs truth: symdiff {:.5} mm^2, max boundary {:.5} mm at {:?}, holes {} vs {}",
                                d.symmetric_difference_area,
                                d.max_boundary_distance,
                                d.max_boundary_at,
                                d.hole_count_a,
                                d.hole_count_b
                            );
                        }
                    }
                }
                println!();
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: section_probe <part.loon> <z,z,z> [truth.dxf]".into());
    }
    let src = std::fs::read_to_string(&args[1])?;
    let dir = std::path::Path::new(&args[1])
        .parent()
        .map(|p| p.to_path_buf());
    let heights: Vec<f64> = args[2]
        .split(',')
        .map(|s| s.trim().parse::<f64>())
        .collect::<Result<_, _>>()?;

    let doc = eval_vcad(&src, dir.as_deref())?;
    let t0 = std::time::Instant::now();
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            ..Default::default()
        },
    )?;
    let solve = t0.elapsed();
    let solid = scene.parts[0].solid.as_ref().ok_or("no root solid")?;
    let brep = solid.as_brep().ok_or("root is not a B-rep")?;
    println!(
        "solved in {:.1} s: {} faces, fidelity {:?}, soup {}",
        solve.as_secs_f64(),
        brep.topology.faces.len(),
        solid.fidelity(),
        vcad_kernel::vcad_kernel_booleans::is_triangle_soup(brep)
    );

    let truth = args.get(3).map(|s| s.as_str());
    let raw = tessellate_brep(brep, SEGMENTS);
    report("raw tessellation", &raw, &heights, truth);
    let exported = solid.to_mesh(SEGMENTS);
    report(
        "export mesh (repair_export_mesh)",
        &exported,
        &heights,
        truth,
    );

    if let Some(path) = std::env::var_os("DUMP_STL") {
        let path = std::path::PathBuf::from(path);
        write_stl(&path.with_extension("raw.stl"), &raw)?;
        write_stl(&path.with_extension("export.stl"), &exported)?;
        println!("\nwrote {} .raw.stl / .export.stl", path.display());
    }
    Ok(())
}

/// Binary STL, so `section_stl` can be pointed at the same two meshes.
fn write_stl(path: &std::path::Path, mesh: &TriangleMesh) -> std::io::Result<()> {
    let tris = mesh.indices.len() / 3;
    let mut out = Vec::with_capacity(84 + tris * 50);
    out.extend_from_slice(&[0u8; 80]);
    out.extend_from_slice(&(tris as u32).to_le_bytes());
    for t in 0..tris {
        out.extend_from_slice(&[0u8; 12]); // normal: the reader ignores it
        for k in 0..3 {
            let i = mesh.indices[t * 3 + k] as usize;
            for c in 0..3 {
                out.extend_from_slice(&mesh.vertices[i * 3 + c].to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    std::fs::write(path, out)
}
