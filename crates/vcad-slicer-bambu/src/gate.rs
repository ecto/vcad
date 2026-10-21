//! Fail-closed print gate.
//!
//! A print starts only through a [`PrintTicket`], and a ticket only comes out
//! of [`check`] when every condition holds:
//!
//! 1. a human passed an explicit confirmation (`--confirm-print`);
//! 2. printcheck ran on the part and passed;
//! 3. the file is a sliced `.gcode.3mf` whose `plate_N.gcode` matches its
//!    `.md5` sidecar;
//! 4. the file was sliced for the connected printer's model;
//! 5. the file's filament type matches what is loaded;
//! 6. the printer is idle (or finished), not mid-print or in error.
//!
//! The ticket binds the archive's md5; [`crate::link::print_gated`] refuses
//! to upload bytes that hash differently.

use std::io::{Cursor, Read};

use crate::error::{BambuError, Result};
use crate::status::{PrintState, PrinterStatus};

/// Everything the gate looks at besides the file itself.
#[derive(Debug, Clone)]
pub struct GateInputs<'a> {
    /// The human's explicit go (`--confirm-print`).
    pub confirmed: bool,
    /// printcheck verdict on the part; `None` = not run (refused).
    pub printcheck_ok: Option<bool>,
    /// Model id of the connected printer (`C12` for the P1S); `None` = unknown (refused).
    pub printer_model_id: Option<&'a str>,
    /// Filament type loaded in the printer (e.g. `PLA`); `None` = unknown (refused).
    pub loaded_filament: Option<&'a str>,
    /// Fresh status from the printer.
    pub status: &'a PrinterStatus,
}

/// Proof that the gate passed for one exact archive. Fields are private, so
/// the only way to get one is [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintTicket {
    archive_md5: String,
    plate_index: u32,
}

impl PrintTicket {
    /// Uppercase hex md5 of the whole archive this ticket authorises.
    pub fn archive_md5(&self) -> &str {
        &self.archive_md5
    }
    /// 0-based plate to print.
    pub fn plate_index(&self) -> u32 {
        self.plate_index
    }
}

/// What the gate read out of a sliced archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlicedInfo {
    /// `printer_model_id` from `slice_info.config`.
    pub printer_model_id: Option<String>,
    /// `filament_type` from `slice_info.config`.
    pub filament_type: Option<String>,
}

/// Uppercase hex md5.
pub fn md5_upper(data: &[u8]) -> String {
    format!("{:X}", md5::compute(data))
}

fn read_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<Vec<u8>>> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let mut v = Vec::new();
            f.read_to_end(&mut v)?;
            Ok(Some(v))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(BambuError::ThreeMfError(e.to_string())),
    }
}

fn metadata_value(xml: &str, key: &str) -> Option<String> {
    let needle = format!("key=\"{key}\" value=\"");
    let start = xml.find(&needle)? + needle.len();
    let end = xml[start..].find('"')?;
    Some(xml[start..start + end].to_string())
}

/// Verify the plate's G-code against its md5 sidecar and read the slice info.
pub fn inspect_sliced(archive: &[u8], plate_index: u32) -> Result<SlicedInfo> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|e| BambuError::ThreeMfError(format!("not a 3MF archive: {e}")))?;
    let n = plate_index + 1;
    let gcode = read_entry(&mut zip, &format!("Metadata/plate_{n}.gcode"))?.ok_or_else(|| {
        BambuError::ThreeMfError(format!(
            "no Metadata/plate_{n}.gcode: this is not a sliced .gcode.3mf"
        ))
    })?;
    let sidecar = read_entry(&mut zip, &format!("Metadata/plate_{n}.gcode.md5"))?
        .ok_or_else(|| BambuError::ThreeMfError(format!("no Metadata/plate_{n}.gcode.md5")))?;
    let want = String::from_utf8_lossy(&sidecar)
        .trim()
        .to_ascii_uppercase();
    let got = md5_upper(&gcode);
    if want != got {
        return Err(BambuError::ThreeMfError(format!(
            "plate_{n}.gcode md5 {got} does not match its sidecar {want}"
        )));
    }
    let info = read_entry(&mut zip, "Metadata/slice_info.config")?
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    Ok(SlicedInfo {
        printer_model_id: metadata_value(&info, "printer_model_id"),
        filament_type: metadata_value(&info, "filament_type"),
    })
}

/// Run every gate. Returns the ticket, or the full list of reasons it was refused.
pub fn check(archive: &[u8], plate_index: u32, inputs: &GateInputs<'_>) -> Result<PrintTicket> {
    let mut refusals: Vec<String> = Vec::new();

    if !inputs.confirmed {
        refusals.push("no explicit confirmation (pass --confirm-print)".into());
    }
    match inputs.printcheck_ok {
        Some(true) => {}
        Some(false) => refusals.push("printcheck failed".into()),
        None => refusals.push("printcheck was not run".into()),
    }
    match &inputs.status.state {
        PrintState::Idle | PrintState::Finished => {}
        other => refusals.push(format!("printer is not idle ({other:?})")),
    }

    match inspect_sliced(archive, plate_index) {
        Err(e) => refusals.push(e.to_string()),
        Ok(info) => {
            match (inputs.printer_model_id, info.printer_model_id.as_deref()) {
                (Some(want), Some(got)) if want == got => {}
                (None, _) => refusals.push("connected printer model is unknown".into()),
                (Some(want), got) => refusals.push(format!(
                    "file was sliced for printer model {got:?}, connected printer is {want}"
                )),
            }
            match (inputs.loaded_filament, info.filament_type.as_deref()) {
                (Some(l), Some(f)) if l.eq_ignore_ascii_case(f) => {}
                (None, _) => refusals.push("loaded filament is unknown".into()),
                (Some(l), f) => {
                    refusals.push(format!("file wants filament {f:?}, printer has {l}"))
                }
            }
        }
    }

    if !refusals.is_empty() {
        return Err(BambuError::PrintError(format!(
            "print refused: {}",
            refusals.join("; ")
        )));
    }
    Ok(PrintTicket {
        archive_md5: md5_upper(archive),
        plate_index,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::threemf::ThreeMfModel;

    pub(crate) fn sliced(model: Option<&str>, filament: &str) -> Vec<u8> {
        let mut m = ThreeMfModel::new(
            "cube".into(),
            vec![0., 0., 0., 1., 0., 0., 0., 1., 0.],
            vec![0, 1, 2],
        )
        .with_gcode(b"G28\nM104 S220\n".to_vec());
        m.settings.filament_type = filament.into();
        if let Some(id) = model {
            m = m.with_printer_model(id);
        }
        m.to_bytes().unwrap()
    }

    pub(crate) fn idle() -> PrinterStatus {
        PrinterStatus {
            state: PrintState::Idle,
            ..Default::default()
        }
    }

    fn ok_inputs(status: &PrinterStatus) -> GateInputs<'_> {
        GateInputs {
            confirmed: true,
            printcheck_ok: Some(true),
            printer_model_id: Some("C12"),
            loaded_filament: Some("PLA"),
            status,
        }
    }

    #[test]
    fn passes_when_everything_matches() {
        let st = idle();
        let f = sliced(Some("C12"), "PLA");
        let t = check(&f, 0, &ok_inputs(&st)).unwrap();
        assert_eq!(t.archive_md5(), md5_upper(&f));
        assert_eq!(t.plate_index(), 0);
    }

    #[test]
    fn each_condition_refuses_on_its_own() {
        let st = idle();
        let f = sliced(Some("C12"), "PLA");
        let cases: Vec<(GateInputs, &str)> = vec![
            (
                GateInputs {
                    confirmed: false,
                    ..ok_inputs(&st)
                },
                "confirm-print",
            ),
            (
                GateInputs {
                    printcheck_ok: None,
                    ..ok_inputs(&st)
                },
                "printcheck was not run",
            ),
            (
                GateInputs {
                    printcheck_ok: Some(false),
                    ..ok_inputs(&st)
                },
                "printcheck failed",
            ),
            (
                GateInputs {
                    printer_model_id: Some("BL-P001"),
                    ..ok_inputs(&st)
                },
                "sliced for printer model",
            ),
            (
                GateInputs {
                    printer_model_id: None,
                    ..ok_inputs(&st)
                },
                "model is unknown",
            ),
            (
                GateInputs {
                    loaded_filament: Some("PETG"),
                    ..ok_inputs(&st)
                },
                "printer has PETG",
            ),
            (
                GateInputs {
                    loaded_filament: None,
                    ..ok_inputs(&st)
                },
                "filament is unknown",
            ),
        ];
        for (inputs, why) in cases {
            let e = check(&f, 0, &inputs).unwrap_err().to_string();
            assert!(e.contains(why), "expected `{why}` in `{e}`");
        }
    }

    #[test]
    fn busy_printer_is_refused() {
        let st = PrinterStatus {
            state: PrintState::Printing,
            ..Default::default()
        };
        let f = sliced(Some("C12"), "PLA");
        let e = check(&f, 0, &ok_inputs(&st)).unwrap_err().to_string();
        assert!(e.contains("not idle"), "{e}");
    }

    #[test]
    fn untagged_or_unsliced_files_are_refused() {
        let st = idle();
        let e = check(&sliced(None, "PLA"), 0, &ok_inputs(&st))
            .unwrap_err()
            .to_string();
        assert!(e.contains("sliced for printer model None"), "{e}");

        let unsliced =
            ThreeMfModel::new("c".into(), vec![0.; 9], vec![0, 1, 2]).with_printer_model("C12");
        let e = check(&unsliced.to_bytes().unwrap(), 0, &ok_inputs(&st))
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a sliced"), "{e}");

        let e = check(b"not a zip", 0, &ok_inputs(&st))
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a 3MF"), "{e}");
    }

    #[test]
    fn tampered_gcode_fails_md5() {
        use std::io::Write;
        // Rebuild the archive with the gcode changed but the sidecar kept.
        let f = sliced(Some("C12"), "PLA");
        let mut src = zip::ZipArchive::new(Cursor::new(&f[..])).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..src.len() {
            let mut e = src.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).unwrap();
            if name == "Metadata/plate_1.gcode" {
                buf = b"G28\nM104 S300\n".to_vec();
            }
            out.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(&buf).unwrap();
        }
        let bad = out.finish().unwrap().into_inner();
        let st = idle();
        let e = check(&bad, 0, &ok_inputs(&st)).unwrap_err().to_string();
        assert!(e.contains("does not match its sidecar"), "{e}");
    }
}
