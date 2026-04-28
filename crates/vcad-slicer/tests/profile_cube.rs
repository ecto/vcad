//! Per-stage timing for the slicer on a 20mm cube. Acts as both a
//! regression guard (asserts the full pipeline finishes promptly) and a
//! profiling reference. Run with:
//!
//!   cargo test -p vcad-slicer --release --test profile_cube -- --nocapture

use std::time::Instant;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_slicer::{
    generate_infill, generate_layer_heights, generate_perimeters, mesh_bounds, slice_mesh,
    InfillPattern, InfillSettings, PerimeterSettings, SliceSettings,
};

fn cube_mesh(size: f32) -> TriangleMesh {
    let vertices = vec![
        0.0, 0.0, 0.0, size, 0.0, 0.0, size, size, 0.0, 0.0, size, 0.0, 0.0, 0.0, size, size, 0.0,
        size, size, size, size, 0.0, size, size,
    ];
    let indices = vec![
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6, 0, 4, 7, 0, 7, 3,
        1, 2, 6, 1, 6, 5,
    ];
    TriangleMesh {
        vertices,
        indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
    }
}

#[test]
fn profile_20mm_cube() {
    let mesh = cube_mesh(20.0);
    let s = SliceSettings::default();

    let t = Instant::now();
    let (zmin, zmax) = mesh_bounds(&mesh).unwrap();
    let bounds_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let heights = generate_layer_heights(zmin[2], zmax[2], s.first_layer_height, s.layer_height);
    let heights_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let layers = slice_mesh(&mesh, &heights).unwrap();
    let slice_ms = t.elapsed().as_secs_f64() * 1000.0;

    let perim_settings = PerimeterSettings {
        wall_count: s.wall_count,
        line_width: s.line_width,
        ..Default::default()
    };

    let t = Instant::now();
    let perims: Vec<_> = layers
        .iter()
        .map(|l| generate_perimeters(l, &perim_settings))
        .collect();
    let perim_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let infill_paths: usize = perims
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let cfg = InfillSettings {
                pattern: InfillPattern::Grid,
                density: s.infill_density,
                line_width: s.line_width,
                layer_index: idx,
            };
            generate_infill(&p.infill_boundary, &cfg).paths.len()
        })
        .sum();
    let infill_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let result = vcad_slicer::slice(&mesh, &s).unwrap();
    let total_ms = t.elapsed().as_secs_f64() * 1000.0;

    eprintln!("vcad-slicer profile, 20mm cube, default settings:");
    eprintln!("  mesh_bounds:           {bounds_ms:>7.2} ms");
    eprintln!(
        "  generate_layer_heights:{heights_ms:>7.2} ms  ({} layers)",
        heights.len()
    );
    eprintln!(
        "  slice_mesh:            {slice_ms:>7.2} ms  (avg {:.0} verts/layer)",
        layers
            .iter()
            .flat_map(|l| l.contours.iter())
            .map(|c| c.len())
            .sum::<usize>() as f64
            / layers.len().max(1) as f64
    );
    eprintln!("  generate_perimeters:   {perim_ms:>7.2} ms");
    eprintln!("  generate_infill:       {infill_ms:>7.2} ms  ({infill_paths} infill paths)");
    eprintln!("  full vcad_slicer::slice:{total_ms:>6.2} ms  (release)");

    // Regression guard: a 20mm cube used to hang forever (NaN polygon →
    // infinite loop in compute_bounds → infill scan). Make sure we never
    // ship that bug again. 1s is a generous bound for any environment.
    assert!(
        total_ms < 1000.0,
        "slicing 20mm cube took {total_ms:.0} ms (regression?)"
    );
    assert_eq!(result.layers.len(), heights.len());
    assert!(infill_paths > 0, "infill should produce paths, not zero");
}
