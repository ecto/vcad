//! `vcad printer`: talk to a Bambu printer on the LAN.
//!
//! M0 surface: `discover`, `status`, `camera-snapshot`, `upload-test`. The
//! only command that can start a print is `print`, and it refuses unless
//! `--confirm-print` is passed AND printcheck passes on the part AND the
//! sliced file matches the connected printer's model, the loaded filament,
//! and its own md5 (see `vcad_slicer_bambu::gate`).
//!
//! Credentials come from `~/.config/vcad/printers.toml` (chmod 600; never
//! committed). The access code is never printed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use vcad_slicer_bambu::config::{self, PrinterEntry};
use vcad_slicer_bambu::gate::GateInputs;
use vcad_slicer_bambu::link::{self, PrinterLink};

#[derive(Subcommand)]
pub enum PrinterCommands {
    /// Listen for Bambu printers on the LAN (SSDP). Read-only.
    Discover {
        /// Seconds to listen.
        #[arg(long, default_value = "6")]
        timeout: u64,
    },
    /// Connect over MQTT, request a full status push, print it. Read-only.
    Status {
        /// Printer name from printers.toml (optional with one printer).
        #[arg(long)]
        printer: Option<String>,
        /// Print the raw parsed status as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Grab one JPEG from the camera (port 6000). Read-only.
    CameraSnapshot {
        #[arg(long)]
        printer: Option<String>,
        /// Output path.
        #[arg(short, long, default_value = "p1s-snapshot.jpg")]
        output: PathBuf,
    },
    /// Upload a harmless text file over FTPS, verify its size, delete it.
    /// Never starts a print.
    UploadTest {
        #[arg(long)]
        printer: Option<String>,
    },
    /// Upload a sliced .gcode.3mf and START A PRINT. Refused unless every
    /// gate passes; a human must pass --confirm-print each time.
    Print {
        /// Sliced .gcode.3mf.
        file: PathBuf,
        /// The part's STL, checked with printcheck before anything uploads.
        #[arg(long)]
        printcheck_mesh: PathBuf,
        #[arg(long)]
        printer: Option<String>,
        /// 0-based plate to print.
        #[arg(long, default_value = "0")]
        plate: u32,
        /// Explicit human go. Without it nothing is uploaded.
        #[arg(long)]
        confirm_print: bool,
    },
}

/// Path to printers.toml, with a helpful error.
fn printers_path() -> Result<PathBuf> {
    config::default_path().context("no $HOME; set VCAD_PRINTERS to the printers.toml path")
}

fn load_entry(name: Option<&str>) -> Result<PrinterEntry> {
    let path = printers_path()?;
    if !path.exists() {
        bail!(
            "{} not found. Create it (chmod 600) with:\n\n[[printer]]\nname = \"p1s\"\nmodel = \"P1S\"\nip = \"192.168.x.y\"\nserial = \"...\"\naccess_code = \"...\"\n",
            path.display()
        );
    }
    let printers = config::load(&path)?;
    Ok(config::select(&printers, name)?.clone())
}

pub fn run(cmd: PrinterCommands) -> Result<()> {
    match cmd {
        PrinterCommands::Discover { timeout } => {
            let found = vcad_slicer_bambu::discover_printers(Duration::from_secs(timeout))?;
            if found.is_empty() {
                println!("no printers answered in {timeout}s (is the Mac on the printer's LAN?)");
            }
            for p in found {
                println!(
                    "{}  serial={}  model={}  name={}  fw={}",
                    p.ip,
                    p.serial,
                    p.model,
                    p.name,
                    p.firmware.as_deref().unwrap_or("?")
                );
            }
            Ok(())
        }
        PrinterCommands::Status { printer, json } => {
            let mut lan = link::LanPrinter::new(load_entry(printer.as_deref())?)?;
            run_status(&mut lan, json)
        }
        PrinterCommands::CameraSnapshot { printer, output } => {
            let mut lan = link::LanPrinter::new(load_entry(printer.as_deref())?)?;
            run_camera(&mut lan, &output)
        }
        PrinterCommands::UploadTest { printer } => {
            let mut lan = link::LanPrinter::new(load_entry(printer.as_deref())?)?;
            run_upload_test(&mut lan, &uuid_nonce())
        }
        PrinterCommands::Print {
            file,
            printcheck_mesh,
            printer,
            plate,
            confirm_print,
        } => {
            // Refuse before touching the network or the credentials file.
            if !confirm_print {
                bail!("print refused: pass --confirm-print to start a print");
            }
            let entry = load_entry(printer.as_deref())?;
            let model_id = entry.model_id();
            let mut lan = link::LanPrinter::new(entry)?;
            run_print(
                &mut lan,
                &file,
                &printcheck_mesh,
                plate,
                confirm_print,
                model_id,
            )
        }
    }
}

fn uuid_nonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{t:x}")
}

pub fn run_status<L: PrinterLink>(link: &mut L, json: bool) -> Result<()> {
    let s = link.status()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!("state      {:?}", s.state);
    println!(
        "progress   {:.0}%  layer {}/{}",
        s.progress_percent, s.layer_current, s.layer_total
    );
    println!(
        "nozzle     {:.0} / {:.0} °C",
        s.nozzle_temp, s.nozzle_target
    );
    println!("bed        {:.0} / {:.0} °C", s.bed_temp, s.bed_target);
    println!(
        "filament   {}",
        s.loaded_filament.as_deref().unwrap_or("unknown")
    );
    println!(
        "firmware   {}",
        s.firmware_version.as_deref().unwrap_or("unknown")
    );
    Ok(())
}

pub fn run_camera<L: PrinterLink>(link: &mut L, output: &Path) -> Result<()> {
    let jpeg = link.camera_jpeg()?;
    std::fs::write(output, &jpeg).with_context(|| format!("writing {}", output.display()))?;
    println!("wrote {} ({} bytes)", output.display(), jpeg.len());
    Ok(())
}

pub fn run_upload_test<L: PrinterLink>(link: &mut L, nonce: &str) -> Result<()> {
    let r = link::upload_test(link, nonce)?;
    println!(
        "upload-test ok: {} ({} bytes) uploaded, size verified, deleted",
        r.name, r.bytes
    );
    Ok(())
}

pub fn run_print<L: PrinterLink>(
    link: &mut L,
    file: &Path,
    mesh: &Path,
    plate: u32,
    confirmed: bool,
    printer_model_id: Option<&str>,
) -> Result<()> {
    let archive = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let report = vcad_printcheck::check_file(mesh, &vcad_printcheck::Options::default())
        .map_err(|e| anyhow::anyhow!("printcheck could not read {}: {e}", mesh.display()))?;
    if !report.ok {
        eprintln!("{}", vcad_printcheck::render_text(&report));
    }
    let status = link.status()?;
    let inputs = GateInputs {
        confirmed,
        printcheck_ok: Some(report.ok),
        printer_model_id,
        loaded_filament: status.loaded_filament.as_deref(),
        status: &status,
    };
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .context("file name is not UTF-8")?
        .to_string();
    let ticket = link::print_gated(link, &name, &archive, plate, &inputs)?;
    println!(
        "print started: {name} plate {} (md5 {}). Stop is on the printer touchscreen.",
        ticket.plate_index() + 1,
        ticket.archive_md5()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_slicer_bambu::link::{FakeCall, FakePrinter};
    use vcad_slicer_bambu::status::{PrintState, PrinterStatus};
    use vcad_slicer_bambu::threemf::ThreeMfModel;

    fn idle_pla() -> FakePrinter {
        FakePrinter {
            status: PrinterStatus {
                state: PrintState::Idle,
                loaded_filament: Some("PLA".into()),
                ..Default::default()
            },
            jpeg: Some(vec![0xFF, 0xD8, 1, 0xFF, 0xD9]),
            ..Default::default()
        }
    }

    /// A closed 10 mm cube STL and a matching sliced .gcode.3mf.
    fn fixtures(dir: &Path) -> (PathBuf, PathBuf) {
        let v = |x: f32, y: f32, z: f32| [x * 10.0, y * 10.0, z * 10.0];
        let c = [
            v(0., 0., 0.),
            v(1., 0., 0.),
            v(1., 1., 0.),
            v(0., 1., 0.),
            v(0., 0., 1.),
            v(1., 0., 1.),
            v(1., 1., 1.),
            v(0., 1., 1.),
        ];
        let tris = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        let mut stl = String::from("solid cube\n");
        for t in tris {
            stl.push_str("facet normal 0 0 0\nouter loop\n");
            for i in t {
                let p = c[i];
                stl.push_str(&format!("vertex {} {} {}\n", p[0], p[1], p[2]));
            }
            stl.push_str("endloop\nendfacet\n");
        }
        stl.push_str("endsolid cube\n");
        let stl_path = dir.join("cube.stl");
        std::fs::write(&stl_path, stl).unwrap();

        let m = ThreeMfModel::new("cube".into(), vec![0.; 9], vec![0, 1, 2])
            .with_gcode(b"G28\n".to_vec())
            .with_printer_model("C12");
        let f = dir.join("cube.gcode.3mf");
        std::fs::write(&f, m.to_bytes().unwrap()).unwrap();
        (stl_path, f)
    }

    #[test]
    fn status_camera_upload_test_never_print() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = idle_pla();
        run_status(&mut p, false).unwrap();
        run_status(&mut p, true).unwrap();
        let out = dir.path().join("snap.jpg");
        run_camera(&mut p, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap()[..2], [0xFF, 0xD8]);
        run_upload_test(&mut p, "n1").unwrap();
        assert!(p.files.is_empty());
        assert!(!p.printed(), "{:?}", p.calls);
    }

    #[test]
    fn print_without_confirm_uploads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (stl, f) = fixtures(dir.path());
        let mut p = idle_pla();
        assert!(run_print(&mut p, &f, &stl, 0, false, Some("C12")).is_err());
        assert!(!p.uploaded() && !p.printed(), "{:?}", p.calls);
    }

    #[test]
    fn print_with_confirm_and_passing_gates_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (stl, f) = fixtures(dir.path());
        let mut p = idle_pla();
        run_print(&mut p, &f, &stl, 0, true, Some("C12")).unwrap();
        assert!(p
            .calls
            .contains(&FakeCall::StartPrint("cube.gcode.3mf".into(), 0)));
    }

    #[test]
    fn print_refused_on_wrong_printer_or_filament() {
        let dir = tempfile::tempdir().unwrap();
        let (stl, f) = fixtures(dir.path());
        let mut p = idle_pla();
        assert!(run_print(&mut p, &f, &stl, 0, true, Some("N2S")).is_err());
        let mut p = idle_pla();
        p.status.loaded_filament = Some("PETG".into());
        assert!(run_print(&mut p, &f, &stl, 0, true, Some("C12")).is_err());
        assert!(!p.uploaded());
    }

    #[test]
    fn print_refused_when_printcheck_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (_, f) = fixtures(dir.path());
        // An open mesh (single triangle) fails the manifold check.
        let bad = dir.path().join("open.stl");
        std::fs::write(
            &bad,
            "solid o\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid o\n",
        )
        .unwrap();
        let mut p = idle_pla();
        let e = run_print(&mut p, &f, &bad, 0, true, Some("C12"));
        assert!(e.is_err());
        assert!(!p.uploaded());
    }
}
