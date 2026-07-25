//! Writing the run's output to a directory.
//!
//! Two entry points, and the split between them is the fail-closed gate:
//!
//! * [`write_report`] always writes — the receipt, the annotated DRC dump, and
//!   the board itself. A run that failed is still worth reading, and the board
//!   is where the next run picks up.
//! * [`write_fab_package`] writes the fabrication files, and **refuses** when
//!   the report did not converge. `export_gerber`'s clean-DRC gate exists for
//!   the same reason; fab-prep is the supported way to *get* clean, never a way
//!   around it.

use std::path::Path;

use vcad_ir::ecad::Pcb;

use crate::{render, FabPrepOutcome};

/// Why a package could not be written.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    /// The fab-prep run did not converge; fabrication files are withheld.
    #[error("refusing to write a fab package: {0}")]
    NotConverged(String),
    /// A file could not be written.
    #[error("writing {path}: {source}")]
    Io {
        /// The file being written.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Gerber, Excellon, BOM or pick-and-place serialization failed.
    #[error("serializing {what}: {message}")]
    Serialize {
        /// Which artifact failed.
        what: String,
        /// Underlying message.
        message: String,
    },
}

fn write(dir: &Path, name: &str, bytes: impl AsRef<[u8]>) -> Result<String, PackageError> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|source| PackageError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(name.to_string())
}

/// Write the receipt, the annotated DRC dump, and the board. Always safe to
/// call — a non-converging run needs these more than a converging one does.
///
/// Returns the file names written, in write order.
pub fn write_report(
    dir: &Path,
    pcb: &Pcb,
    outcome: &FabPrepOutcome,
) -> Result<Vec<String>, PackageError> {
    std::fs::create_dir_all(dir).map_err(|source| PackageError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let report = &outcome.report;
    let written = vec![
        write(
            dir,
            "drc_report.txt",
            render::drc_report(report, &outcome.violations),
        )?,
        write(dir, "FAB_NOTES.md", render::fab_notes(report))?,
        write(
            dir,
            "fab_report.json",
            serde_json::to_string_pretty(report).map_err(|e| PackageError::Serialize {
                what: "fab_report.json".into(),
                message: e.to_string(),
            })?,
        )?,
        write(
            dir,
            "board.pcb.json",
            serde_json::to_string(pcb).map_err(|e| PackageError::Serialize {
                what: "board.pcb.json".into(),
                message: e.to_string(),
            })?,
        )?,
    ];
    Ok(written)
}

/// Write the complete fabrication package: everything [`write_report`] writes,
/// plus Gerbers, the Excellon drill file, a KiCad board, the BOM, the
/// pick-and-place CSV, and (when supplied) a board SVG.
///
/// Refuses with [`PackageError::NotConverged`] when the run did not converge.
/// The board and the receipt are still written first, so a refused call leaves
/// a directory a human can act on.
///
/// `svg` is passed in rather than rendered here: the PCB renderer lives above
/// this crate in the dependency graph, and pulling the CAD kernel in for one
/// optional file would be a poor trade.
pub fn write_fab_package(
    dir: &Path,
    pcb: &Pcb,
    outcome: &FabPrepOutcome,
    svg: Option<&str>,
) -> Result<Vec<String>, PackageError> {
    let mut written = write_report(dir, pcb, outcome)?;
    if !outcome.report.converged {
        return Err(PackageError::NotConverged(format!(
            "{} — the report and board were written to {} but no fabrication files were; \
             resolve the offenders in drc_report.txt and re-run",
            outcome.report.headline(),
            dir.display()
        )));
    }

    let gerbers =
        vcad_ecad_export::gerber::generate_gerbers(pcb).map_err(|e| PackageError::Serialize {
            what: "gerbers".into(),
            message: e.to_string(),
        })?;
    for (name, content) in &gerbers {
        written.push(write(dir, name, content)?);
    }

    // One drill file per via span — blind and buried vias are drilled
    // separately from the through-holes.
    let drills = vcad_ecad_export::excellon::generate_drill_files(pcb).map_err(|e| {
        PackageError::Serialize {
            what: "drill files".into(),
            message: e.to_string(),
        }
    })?;
    for (name, content) in &drills {
        written.push(write(dir, name, content.as_bytes())?);
    }

    written.push(write(
        dir,
        "board.kicad_pcb",
        vcad_ecad_symbols::write_kicad_pcb(pcb),
    )?);

    let mut bom = Vec::new();
    vcad_ecad_export::bom::write_bom(&mut bom, pcb).map_err(|e| PackageError::Serialize {
        what: "bom.csv".into(),
        message: e.to_string(),
    })?;
    written.push(write(dir, "bom.csv", &bom)?);

    let mut pnp = Vec::new();
    vcad_ecad_export::pick_place::write_pick_place(&mut pnp, pcb).map_err(|e| {
        PackageError::Serialize {
            what: "pick_place.csv".into(),
            message: e.to_string(),
        }
    })?;
    written.push(write(dir, "pick_place.csv", &pnp)?);

    if let Some(svg) = svg {
        written.push(write(dir, "board.svg", svg)?);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_board, with_smd_pad, with_trace};
    use crate::{run_fab_prep, FabPrepOptions};

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vcad-fabprep-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_converged_run_writes_the_whole_package() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        let outcome = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        );
        assert!(outcome.report.converged);

        let dir = tmpdir("converged");
        let written = write_fab_package(&dir, &pcb, &outcome, Some("<svg/>")).expect("package");
        for expected in [
            "drc_report.txt",
            "FAB_NOTES.md",
            "fab_report.json",
            "board.pcb.json",
            "drill.drl",
            "board.kicad_pcb",
            "bom.csv",
            "pick_place.csv",
            "board.svg",
        ] {
            assert!(written.iter().any(|w| w == expected), "missing {expected}");
            assert!(dir.join(expected).exists(), "{expected} not on disk");
        }
        assert!(
            written.iter().any(|w| w.ends_with(".gbr")),
            "no gerbers written: {written:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Blind and buried vias are drilled in separate operations from the
    /// through-holes, so the package carries one drill file per span. Merged
    /// into a single `drill.drl`, a blind via is fabricated as a through-hole
    /// and shorts every layer it crosses.
    #[test]
    fn blind_and_buried_vias_get_their_own_drill_files() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        pcb.vias.push(vcad_ir::ecad::Via {
            position: vcad_ir::Vec2::new(5.0, 2.0),
            diameter: 0.4,
            drill: 0.2,
            start_layer: vcad_ir::ecad::PcbLayer::FCu,
            end_layer: vcad_ir::ecad::PcbLayer::In1Cu,
            net: "A".into(),
            source: None,
        });
        let outcome = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        );

        let dir = tmpdir("spans");
        let written = write_fab_package(&dir, &pcb, &outcome, None).expect("package");
        assert!(
            written.iter().any(|w| w == "drill.drl"),
            "through-hole drill file missing: {written:?}"
        );
        assert!(
            written.iter().any(|w| w == "drill-F_Cu-In1_Cu.drl"),
            "blind via span has no drill file of its own: {written:?}"
        );
        let through = std::fs::read_to_string(dir.join("drill.drl")).unwrap();
        assert!(
            !through.contains("X5.0000Y2.0000"),
            "blind via drilled as a through-hole:\n{through}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_run_is_refused_but_still_leaves_the_evidence() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
        with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);
        let outcome = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                max_rounds: 1,
                ..Default::default()
            },
        );
        assert!(!outcome.report.converged);

        let dir = tmpdir("refused");
        let err = write_fab_package(&dir, &pcb, &outcome, None)
            .expect_err("a dirty board must never produce fab files");
        assert!(matches!(err, PackageError::NotConverged(_)), "{err}");

        assert!(dir.join("drc_report.txt").exists(), "evidence must survive");
        assert!(dir.join("FAB_NOTES.md").exists());
        assert!(dir.join("board.pcb.json").exists());
        assert!(
            !dir.join("drill.drl").exists(),
            "no fabrication file may be written for a non-converged board"
        );
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .all(|e| !e.file_name().to_string_lossy().ends_with(".gbr")),
            "no gerbers may be written for a non-converged board"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
