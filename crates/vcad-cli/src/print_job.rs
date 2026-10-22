//! `vcad print`: part → centred on the plate → sliced → safe start/end →
//! linted → printcheck → `.gcode.3mf`. Pure (no network); the send step
//! lives in `printer_cmd::run_print_job`.

use anyhow::{bail, Result};
use vcad_slicer_gcode::{GcodeSettings, LintLimits, LintReport, PrinterProfile};

/// Filament preset for the P1S (M1 values: conservative speeds come from
/// `GcodeSettings::default()`; full Bambu-derived profiles are M2).
#[derive(Debug, Clone, PartialEq)]
pub struct Filament {
    /// Type string written to slice_info and matched against the printer.
    pub kind: &'static str,
    /// Nozzle °C.
    pub nozzle: u32,
    /// Bed °C (smooth/engineering plate).
    pub bed: u32,
    /// Part fan 0–255.
    pub fan: u8,
}

/// Look up a filament preset by CLI name.
pub fn filament(name: &str) -> Result<Filament> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "pla" => Filament {
            kind: "PLA",
            nozzle: 220,
            bed: 55,
            fan: 255,
        },
        "petg" => Filament {
            kind: "PETG",
            nozzle: 255,
            bed: 70,
            fan: 77,
        },
        other => bail!("unknown --profile `{other}` (use pla or petg)"),
    })
}

/// Translate in place so the XY bbox centre sits on the plate centre and
/// the lowest point is at Z = 0. Refuses parts that don't fit.
pub fn center_on_plate(vertices: &mut [f32], printer: &PrinterProfile) -> Result<()> {
    if vertices.len() < 9 {
        bail!("empty mesh");
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in vertices.as_chunks::<3>().0 {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    // Keep a 10 mm margin clear of the purge line at the front edge.
    let (bx, by, bz) = (
        printer.bed_x as f32,
        printer.bed_y as f32,
        printer.bed_z as f32,
    );
    if size[0] > bx - 20.0 || size[1] > by - 20.0 || size[2] > bz {
        bail!(
            "part is {:.1} × {:.1} × {:.1} mm; the {} fits {:.0} × {:.0} × {:.0}",
            size[0],
            size[1],
            size[2],
            printer.name,
            bx - 20.0,
            by - 20.0,
            bz
        );
    }
    let d = [
        bx / 2.0 - (lo[0] + hi[0]) / 2.0,
        by / 2.0 - (lo[1] + hi[1]) / 2.0,
        -lo[2],
    ];
    for v in vertices.as_chunks_mut::<3>().0 {
        for k in 0..3 {
            v[k] += d[k];
        }
    }
    Ok(())
}

/// A prepared, checked job.
pub struct Job {
    /// The `.gcode.3mf` bytes.
    pub archive: Vec<u8>,
    /// Linter verdict on the embedded G-code (always clean in a returned Job).
    pub lint: LintReport,
    /// printcheck verdict on the centred part.
    pub printcheck: vcad_printcheck::Report,
    /// Layer count.
    pub layers: usize,
    /// Filament used.
    pub filament: Filament,
}

/// Centre, slice, generate, lint, printcheck and package one part.
pub fn prepare(
    name: &str,
    mut vertices: Vec<f32>,
    indices: Vec<u32>,
    filament: Filament,
) -> Result<Job> {
    let printer = PrinterProfile::bambu_p1s();
    center_on_plate(&mut vertices, &printer)?;

    let pc_mesh = vcad_printcheck::Mesh {
        tris: indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let p = |i: u32| {
                    let i = i as usize * 3;
                    [
                        vertices[i] as f64,
                        vertices[i + 1] as f64,
                        vertices[i + 2] as f64,
                    ]
                };
                [p(t[0]), p(t[1]), p(t[2])]
            })
            .collect(),
    };
    let printcheck = vcad_printcheck::check_mesh(
        &pc_mesh,
        name.to_string(),
        &vcad_printcheck::Options::default(),
    );

    let mesh = vcad_kernel_tessellate::TriangleMesh {
        vertices: vertices.clone(),
        indices: indices.clone(),
        normals: Vec::new(),
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    };
    let settings = vcad_slicer::SliceSettings {
        layer_height: 0.2,
        first_layer_height: 0.2,
        wall_count: 2,
        infill_density: 0.15,
        ..Default::default()
    };
    let result = vcad_slicer::slice(&mesh, &settings)?;
    if result.layers.is_empty() {
        bail!("slicer produced no layers");
    }
    let limits = LintLimits::for_printer(&printer);
    let gcode = vcad_slicer_gcode::generate_gcode(
        &result,
        GcodeSettings {
            printer,
            print_temp: filament.nozzle,
            bed_temp: filament.bed,
            fan_speed: filament.fan,
            ..Default::default()
        },
    );
    let lint = vcad_slicer_gcode::lint(&gcode, &limits);
    if !lint.ok() {
        let shown: Vec<String> = lint
            .findings
            .iter()
            .take(10)
            .map(|f| format!("  line {}: {}", f.line, f.message))
            .collect();
        bail!(
            "G-code linter refused ({} findings):\n{}",
            lint.findings.len(),
            shown.join("\n")
        );
    }

    let mut model = vcad_slicer_bambu::ThreeMfModel::new(name.to_string(), vertices, indices)
        .with_gcode(gcode.into_bytes())
        .with_printer_model("C12");
    model.settings = vcad_slicer_bambu::PrintSettings {
        layer_height: settings.layer_height,
        first_layer_height: settings.first_layer_height,
        wall_count: settings.wall_count,
        infill_density: settings.infill_density,
        print_temp: filament.nozzle,
        bed_temp: filament.bed,
        filament_type: filament.kind.into(),
    };
    Ok(Job {
        archive: model.to_bytes()?,
        lint,
        printcheck,
        layers: result.layers.len(),
        filament,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A closed axis-aligned box, corner at `o`.
    pub(crate) fn cube(o: [f32; 3], s: f32) -> (Vec<f32>, Vec<u32>) {
        let c = [
            [0., 0., 0.],
            [s, 0., 0.],
            [s, s, 0.],
            [0., s, 0.],
            [0., 0., s],
            [s, 0., s],
            [s, s, s],
            [0., s, s],
        ];
        let v = c
            .iter()
            .flat_map(|p: &[f32; 3]| [p[0] + o[0], p[1] + o[1], p[2] + o[2]])
            .collect();
        let i = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6, 0, 4, 7, 0, 7,
            3, 1, 2, 6, 1, 6, 5,
        ];
        (v, i)
    }

    #[test]
    fn centers_on_plate_and_drops_to_z0() {
        let (mut v, _) = cube([-50.0, 400.0, 7.0], 20.0);
        center_on_plate(&mut v, &PrinterProfile::bambu_p1s()).unwrap();
        let xs: Vec<f32> = v.iter().step_by(3).copied().collect();
        let zs: Vec<f32> = v.iter().skip(2).step_by(3).copied().collect();
        assert_eq!(xs.iter().cloned().fold(f32::INFINITY, f32::min), 118.0);
        assert_eq!(zs.iter().cloned().fold(f32::INFINITY, f32::min), 0.0);
    }

    #[test]
    fn refuses_oversized_parts() {
        let (mut v, _) = cube([0.0; 3], 250.0);
        assert!(center_on_plate(&mut v, &PrinterProfile::bambu_p1s()).is_err());
        assert!(filament("abs").is_err());
    }

    #[test]
    fn cube_job_is_lint_clean_and_gate_ready() {
        let (v, i) = cube([0.0; 3], 20.0);
        let job = prepare("cube", v, i, filament("pla").unwrap()).unwrap();
        assert!(job.lint.ok());
        assert!(
            job.printcheck.ok,
            "{}",
            vcad_printcheck::render_text(&job.printcheck)
        );
        assert_eq!(job.layers, 100);
        let info = vcad_slicer_bambu::gate::inspect_sliced(&job.archive, 0).unwrap();
        assert_eq!(info.printer_model_id.as_deref(), Some("C12"));
        assert_eq!(info.filament_type.as_deref(), Some("PLA"));

        // First layer prints at its full height above the plate, not at the
        // mid-layer slicing plane.
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&job.archive)).unwrap();
        let mut g = String::new();
        zip.by_name("Metadata/plate_1.gcode")
            .unwrap()
            .read_to_string(&mut g)
            .unwrap();
        let after = &g[g.find("; Layer 0").unwrap()..];
        let first_z = after.lines().find(|l| l.starts_with("G1 Z")).unwrap();
        assert!(first_z.starts_with("G1 Z0.200"), "{first_z}");
    }
}
