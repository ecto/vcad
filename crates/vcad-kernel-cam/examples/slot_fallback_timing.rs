//! Time the thin-slot centre-line fallback on the stator.
//!
//! ```text
//! cargo run -p vcad-kernel-cam --release --example slot_fallback_timing -- \
//!     docs/cam-fixtures/stator-outline.dxf 3.9
//! ```

use vcad_kernel_cam::outline::read_dxf;
use vcad_kernel_cam::{CamSettings, Contour, Contour2D, Point2D, Tool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "docs/cam-fixtures/stator-outline.dxf".into());
    let diameter: f64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(3.9);

    let outline = read_dxf(&std::fs::read_to_string(&path)?)?;
    let tool = Tool::FlatEndMill {
        diameter,
        flute_length: 20.0,
        flutes: 2,
    };
    let settings = CamSettings {
        stepdown: 0.5,
        ..CamSettings::default()
    };

    // Every loop in the file, outer and holes: the bore-and-12-slots loop is
    // the one with the 3.87 mm slot mouths, and it is a hole.
    let mut loops: Vec<Vec<[f64; 2]>> = Vec::new();
    for region in &outline.regions {
        loops.push(region.outer.points.iter().map(|p| [p.x, p.y]).collect());
        for hole in &region.holes {
            loops.push(hole.points.iter().map(|p| [p.x, p.y]).collect());
        }
    }

    for (i, points) in loops.iter().enumerate() {
        let mut contour = Contour::new(Point2D::new(points[0][0], points[0][1]));
        for p in points.iter().skip(1) {
            contour.line_to(Point2D::new(p[0], p[1]));
        }
        contour.line_to(Point2D::new(points[0][0], points[0][1]));

        let op = Contour2D::inside(contour, 1.0).with_centre_line_fallback(1.5);
        let started = std::time::Instant::now();
        let result = op.generate_reported(&tool, &settings);
        let ms = started.elapsed().as_secs_f64() * 1e3;
        match result {
            Ok((tp, report)) => println!(
                "loop {i}: {} points -> {} segments, {} centre-line stretch(es), \
                 worst wall error {:.4} mm, {ms:.0} ms",
                points.len(),
                tp.segments.len(),
                report.centre_line.len(),
                report.max_wall_error
            ),
            Err(e) => println!("loop {i}: {} points -> {e} ({ms:.0} ms)", points.len()),
        }
    }
    Ok(())
}
