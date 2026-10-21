//! Synchronous printer link: the seam between the M0 operations and the wire.
//!
//! [`PrinterLink`] is what `vcad printer` drives. [`LanPrinter`] speaks to a
//! real printer (MQTT/FTPS/camera); [`FakePrinter`] is an in-memory printer
//! for tests, in the style of the Carvera `FakeCarvera`: it records every
//! call so a test can assert what did, and did not, reach the machine.

use std::collections::BTreeMap;

use crate::error::{BambuError, Result};
use crate::gate::{self, GateInputs, PrintTicket};
use crate::status::PrinterStatus;

/// Operations vcad performs against one printer.
pub trait PrinterLink {
    /// Request a full status push (`pushall`) and return the parsed report.
    fn status(&mut self) -> Result<PrinterStatus>;
    /// One JPEG frame from the camera.
    fn camera_jpeg(&mut self) -> Result<Vec<u8>>;
    /// Upload a file to the SD card root.
    fn upload(&mut self, name: &str, data: &[u8]) -> Result<()>;
    /// Size of a file on the SD card, `None` when absent.
    fn file_size(&mut self, name: &str) -> Result<Option<u64>>;
    /// Delete a file from the SD card.
    fn delete(&mut self, name: &str) -> Result<()>;
    /// Start printing an already-uploaded file. Requires a gate ticket.
    fn start_print(&mut self, name: &str, ticket: &PrintTicket) -> Result<()>;
}

/// What `upload_test` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTestReport {
    /// File name used on the SD card.
    pub name: String,
    /// Bytes uploaded.
    pub bytes: u64,
    /// Size the printer reported back.
    pub reported: u64,
}

/// Upload a harmless text file, confirm its size, delete it, confirm it's gone.
/// The file is plain text with a non-printable name pattern
/// (`vcad-m0-test-*.txt`); the printer never treats it as a job.
pub fn upload_test<L: PrinterLink>(link: &mut L, nonce: &str) -> Result<UploadTestReport> {
    let name = format!("vcad-m0-test-{nonce}.txt");
    let body = format!("vcad M0 upload test {nonce}\nsafe to delete; never printed\n");
    link.upload(&name, body.as_bytes())?;
    let reported = link.file_size(&name)?;
    // Always try to clean up, even when the size check fails.
    let deleted = link.delete(&name);
    let reported = reported.ok_or_else(|| {
        BambuError::PrintError(format!("{name} not found on the printer after upload"))
    })?;
    if reported != body.len() as u64 {
        return Err(BambuError::PrintError(format!(
            "{name}: printer reports {reported} bytes, uploaded {}",
            body.len()
        )));
    }
    deleted?;
    if link.file_size(&name)?.is_some() {
        return Err(BambuError::PrintError(format!(
            "{name} still on the printer after delete"
        )));
    }
    Ok(UploadTestReport {
        name,
        bytes: body.len() as u64,
        reported,
    })
}

/// Gate, upload, verify, start. Nothing touches the printer until the gate
/// passes; the upload is skipped if the bytes don't hash to the ticket; the
/// start is skipped if the printer reports a different size.
pub fn print_gated<L: PrinterLink>(
    link: &mut L,
    name: &str,
    archive: &[u8],
    plate_index: u32,
    inputs: &GateInputs<'_>,
) -> Result<PrintTicket> {
    let ticket = gate::check(archive, plate_index, inputs)?;
    if gate::md5_upper(archive) != ticket.archive_md5() {
        return Err(BambuError::PrintError(
            "archive changed after the gate ran".into(),
        ));
    }
    link.upload(name, archive)?;
    match link.file_size(name)? {
        Some(n) if n == archive.len() as u64 => {}
        other => {
            return Err(BambuError::PrintError(format!(
                "uploaded {} bytes but printer reports {other:?}; not starting",
                archive.len()
            )))
        }
    }
    link.start_print(name, &ticket)?;
    Ok(ticket)
}

/// One recorded call on a [`FakePrinter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCall {
    /// `status()`
    Status,
    /// `camera_jpeg()`
    Camera,
    /// `upload(name, len)`
    Upload(String, usize),
    /// `file_size(name)`
    Size(String),
    /// `delete(name)`
    Delete(String),
    /// `start_print(name, plate)`
    StartPrint(String, u32),
}

/// In-memory printer for tests.
#[derive(Debug, Default)]
pub struct FakePrinter {
    /// Status returned by `status()`.
    pub status: PrinterStatus,
    /// Frame returned by `camera_jpeg()`; `None` = camera unreachable.
    pub jpeg: Option<Vec<u8>>,
    /// SD card contents.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Every call, in order.
    pub calls: Vec<FakeCall>,
    /// When set, uploads store this many bytes fewer (a truncated transfer).
    pub truncate_uploads: usize,
    /// When set, `delete` fails.
    pub fail_delete: bool,
    /// When set, MQTT is unreachable (`status`/`start_print` fail).
    pub offline: bool,
}

impl FakePrinter {
    /// True if any print was ever started.
    pub fn printed(&self) -> bool {
        self.calls
            .iter()
            .any(|c| matches!(c, FakeCall::StartPrint(..)))
    }
    /// True if anything was ever uploaded.
    pub fn uploaded(&self) -> bool {
        self.calls.iter().any(|c| matches!(c, FakeCall::Upload(..)))
    }
}

impl PrinterLink for FakePrinter {
    fn status(&mut self) -> Result<PrinterStatus> {
        self.calls.push(FakeCall::Status);
        if self.offline {
            return Err(BambuError::Timeout("fake printer offline".into()));
        }
        Ok(self.status.clone())
    }
    fn camera_jpeg(&mut self) -> Result<Vec<u8>> {
        self.calls.push(FakeCall::Camera);
        self.jpeg
            .clone()
            .ok_or_else(|| BambuError::ConnectionFailed("fake camera off".into()))
    }
    fn upload(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.calls.push(FakeCall::Upload(name.into(), data.len()));
        let keep = data.len().saturating_sub(self.truncate_uploads);
        self.files.insert(name.into(), data[..keep].to_vec());
        Ok(())
    }
    fn file_size(&mut self, name: &str) -> Result<Option<u64>> {
        self.calls.push(FakeCall::Size(name.into()));
        Ok(self.files.get(name).map(|f| f.len() as u64))
    }
    fn delete(&mut self, name: &str) -> Result<()> {
        self.calls.push(FakeCall::Delete(name.into()));
        if self.fail_delete {
            return Err(BambuError::PrintError("fake delete failed".into()));
        }
        self.files.remove(name);
        Ok(())
    }
    fn start_print(&mut self, name: &str, ticket: &PrintTicket) -> Result<()> {
        self.calls
            .push(FakeCall::StartPrint(name.into(), ticket.plate_index()));
        if self.offline {
            return Err(BambuError::Timeout("fake printer offline".into()));
        }
        let f = self
            .files
            .get(name)
            .ok_or_else(|| BambuError::PrintError(format!("{name} not on SD card")))?;
        if gate::md5_upper(f) != ticket.archive_md5() {
            return Err(BambuError::PrintError("ticket/file md5 mismatch".into()));
        }
        Ok(())
    }
}

/// A real printer on the LAN.
#[cfg(feature = "network")]
pub struct LanPrinter {
    entry: crate::config::PrinterEntry,
    rt: tokio::runtime::Runtime,
    timeout: std::time::Duration,
}

#[cfg(feature = "network")]
impl LanPrinter {
    /// Wrap a configured printer. Nothing is contacted until a method runs.
    pub fn new(entry: crate::config::PrinterEntry) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            entry,
            rt,
            timeout: std::time::Duration::from_secs(10),
        })
    }

    fn connect(&self) -> Result<crate::mqtt::BambuPrinter> {
        let e = &self.entry;
        self.rt.block_on(crate::mqtt::BambuPrinter::connect(
            e.ip,
            &e.serial,
            &e.access_code,
        ))
    }
}

#[cfg(feature = "network")]
impl PrinterLink for LanPrinter {
    fn status(&mut self) -> Result<PrinterStatus> {
        let p = self.connect()?;
        self.rt.block_on(p.status())
    }
    fn camera_jpeg(&mut self) -> Result<Vec<u8>> {
        crate::camera::snapshot(self.entry.ip, &self.entry.access_code, self.timeout)
    }
    fn upload(&mut self, name: &str, data: &[u8]) -> Result<()> {
        crate::ftp::upload_blocking(self.entry.ip, &self.entry.access_code, name, data)
    }
    fn file_size(&mut self, name: &str) -> Result<Option<u64>> {
        crate::ftp::size_blocking(self.entry.ip, &self.entry.access_code, name)
    }
    fn delete(&mut self, name: &str) -> Result<()> {
        crate::ftp::delete_blocking(self.entry.ip, &self.entry.access_code, name)
    }
    fn start_print(&mut self, name: &str, ticket: &PrintTicket) -> Result<()> {
        let p = self.connect()?;
        self.rt.block_on(p.mqtt_client().send_command(
            crate::commands::PrinterCommand::PrintStart {
                file: format!("/sdcard/{name}"),
                plate_index: ticket.plate_index(),
                use_ams: false,
                ams_mapping: vec![],
                bed_leveling: true,
                flow_calibration: true,
                vibration_calibration: true,
                skip_objects: vec![],
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::tests::{idle, sliced};

    fn inputs(st: &PrinterStatus) -> GateInputs<'_> {
        GateInputs {
            confirmed: true,
            printcheck_ok: Some(true),
            printer_model_id: Some("C12"),
            loaded_filament: Some("PLA"),
            status: st,
        }
    }

    #[test]
    fn upload_test_round_trips_and_cleans_up() {
        let mut p = FakePrinter::default();
        let r = upload_test(&mut p, "abc").unwrap();
        assert_eq!(r.name, "vcad-m0-test-abc.txt");
        assert_eq!(r.bytes, r.reported);
        assert!(p.files.is_empty(), "test file left behind");
        assert!(!p.printed());
    }

    #[test]
    fn upload_test_catches_truncation_and_still_deletes() {
        let mut p = FakePrinter {
            truncate_uploads: 3,
            ..Default::default()
        };
        let e = upload_test(&mut p, "t").unwrap_err().to_string();
        assert!(e.contains("printer reports"), "{e}");
        assert!(p.files.is_empty());
        assert!(p
            .calls
            .contains(&FakeCall::Delete("vcad-m0-test-t.txt".into())));
    }

    #[test]
    fn upload_test_reports_failed_delete() {
        let mut p = FakePrinter {
            fail_delete: true,
            ..Default::default()
        };
        assert!(upload_test(&mut p, "d").is_err());
    }

    #[test]
    fn gated_print_happy_path() {
        let st = idle();
        let f = sliced(Some("C12"), "PLA");
        let mut p = FakePrinter::default();
        print_gated(&mut p, "cube.gcode.3mf", &f, 0, &inputs(&st)).unwrap();
        assert_eq!(
            p.calls,
            vec![
                FakeCall::Upload("cube.gcode.3mf".into(), f.len()),
                FakeCall::Size("cube.gcode.3mf".into()),
                FakeCall::StartPrint("cube.gcode.3mf".into(), 0),
            ]
        );
    }

    #[test]
    fn refused_gate_never_touches_the_printer() {
        let st = idle();
        let f = sliced(Some("C12"), "PLA");
        let mut p = FakePrinter::default();
        let no = GateInputs {
            confirmed: false,
            ..inputs(&st)
        };
        assert!(print_gated(&mut p, "cube.gcode.3mf", &f, 0, &no).is_err());
        assert!(p.calls.is_empty(), "{:?}", p.calls);
    }

    #[test]
    fn truncated_upload_never_starts() {
        let st = idle();
        let f = sliced(Some("C12"), "PLA");
        let mut p = FakePrinter {
            truncate_uploads: 1,
            ..Default::default()
        };
        assert!(print_gated(&mut p, "cube.gcode.3mf", &f, 0, &inputs(&st)).is_err());
        assert!(p.uploaded());
        assert!(!p.printed());
    }
}
