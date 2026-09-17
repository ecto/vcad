//! Section a binary STL at a Z height and, optionally, diff the result against
//! a DXF outline.
//!
//! ```text
//! cargo run -p vcad-kernel-cam --release --example section_stl -- part.stl 14.1 [outline.dxf]
//! ```
//!
//! An STL is triangle soup by construction, so this exercises the path the app
//! takes for a degraded solid. With a DXF it answers the question nothing in
//! the app asked on the first real job: is this outline the part on screen?

use vcad_kernel_cam::outline::{compare_outlines, is_prismatic, read_dxf, section_at_z};
use vcad_kernel_cam::outline::{CompareOptions, SectionOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: section_stl <part.stl> <z> [outline.dxf]".into());
    }
    let bytes = std::fs::read(&args[1])?;
    let z: f64 = args[2].parse()?;
    if bytes.len() < 84 {
        return Err("not a binary STL".into());
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into()?) as usize;
    if bytes.len() < 84 + count * 50 {
        return Err("binary STL is shorter than its triangle count says".into());
    }
    let mut positions = Vec::with_capacity(count * 3);
    for t in 0..count {
        let base = 84 + t * 50 + 12;
        for v in 0..3 {
            let at = base + v * 12;
            let f = |k: usize| f32::from_le_bytes(bytes[at + k..at + k + 4].try_into().unwrap());
            positions.push([f(0) as f64, f(4) as f64, f(8) as f64]);
        }
    }
    let indices: Vec<u32> = (0..positions.len() as u32).collect();
    let (lo, hi) = positions.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(p[2]), hi.max(p[2]))
    });
    println!("{count} triangles, z {lo:.3} .. {hi:.3}");

    // f32 STL coordinates disagree in the last bits between copies of a vertex.
    let options = SectionOptions {
        weld_tolerance: 1e-4,
        heal_tolerance: std::env::var("HEAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1e-3),
        ..SectionOptions::default()
    };
    let outline = section_at_z(&positions, &indices, z, &options)?;
    println!(
        "section at z {z}: {} region(s), {} hole(s), area {:.4} mm^2, plane nudge {:e}, {} healed gap(s)",
        outline.regions.len(),
        outline.holes().len(),
        outline.area(),
        outline.plane_nudge,
        outline.healed.len()
    );
    for hole in outline.circular_holes(1e-3) {
        println!(
            "  circle D{:.4} at ({:.4}, {:.4}), rms {:.2e}",
            hole.fit.diameter(),
            hole.fit.center.x,
            hole.fit.center.y,
            hole.fit.rms_error
        );
    }
    match is_prismatic(&positions, &indices, lo, hi, 0.01) {
        Ok(report) => println!(
            "prismatic: {} (worst {:.5} mm at z {:.3})",
            report.prismatic, report.max_boundary_distance, report.worst_z
        ),
        Err(e) => println!("prismatic check refused: {e}"),
    }
    if let Some(path) = args.get(3) {
        let dxf = read_dxf(&std::fs::read_to_string(path)?)?;
        let diff = compare_outlines(&dxf, &outline, &CompareOptions::default());
        println!(
            "vs {path}: symmetric difference {:.5} mm^2, max boundary distance {:.5} mm, holes {} vs {}",
            diff.symmetric_difference_area,
            diff.max_boundary_distance,
            diff.hole_count_a,
            diff.hole_count_b
        );
        println!(
            "  worst at {:?}; unmatched holes: dxf {:?} solid {:?}",
            diff.max_boundary_at, diff.unmatched_a, diff.unmatched_b
        );
        for h in &diff.holes {
            println!("  hole {h:?}");
        }
    }
    Ok(())
}
